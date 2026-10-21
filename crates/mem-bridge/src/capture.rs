//! W5.3 — capture plumbing. Step 6: the session-linked tier-1
//! capture-summary writer (`write_capture_summary`) and its deterministic
//! row id. The two-tier drain (`drain_once`) lands on top of this writer.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mem_core::scope::Scope;
use mem_core::sha256;

use mem_embed::embedder::BoxFuture;
use mem_episodic::error::EpisodicError;
use mem_episodic::inbox::{CaptureInbox, DRAIN_BATCH_LIMIT, InboxNewEntry};
use mem_episodic::record::NewAgentSession;
use mem_episodic::record::{Decision, WorkItem};

use mem_store::db::{BatchMode, Value, query_one};
use mem_store::serde_json::Value as Json;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{MemoryInsert, insert_vector_statement};

use crate::project_scope::ResolvedProjectScope;

/// The deterministic capture-summary row id prefix.
pub const CAPTURE_SUMMARY_ID_PREFIX: &str = "auto-capture-";

/// The domain separator pinning the id derivation (upstream generates a
/// fresh id per write; the port's id is a deterministic function of the
/// (domain, container tag, capture id) tuple so retries never duplicate).
pub const CAPTURE_SUMMARY_ID_DOMAIN: &str = "capture-summary-v1";

/// The tier-1 source literal (upstream `source: "auto-capture"`).
pub const AUTO_CAPTURE_SOURCE: &str = "auto-capture";

/// The metadata key both the writer and the injector read.
pub const CAPTURE_SUMMARY_SOURCE_METADATA_KEY: &str = "source";

/// The promotion-row source literal (promotion.rs owns the literal; the
/// injector's classification uses the SUBSTRING rule below).
pub const PROMOTED_FACT_SOURCE: &str = "episodic-consolidation";

/// `formatTagsForEmbedding` (upstream `turso/vector-utils.ts:69-71`).
pub fn format_tags_for_embedding(tags: &[String]) -> String {
    format!("Topics: {}", tags.join(", "))
}

/// The deterministic tier-1 row id:
/// `auto-capture-<64 lowercase hex>` over the domain-separated tuple
/// `("capture-summary-v1", container_tag, capture_id)`.
pub fn capture_summary_row_id(container_tag: &str, capture_id: &str) -> String {
    let digest = sha256::digest(
        format!("{CAPTURE_SUMMARY_ID_DOMAIN}\u{1}{container_tag}\u{1}{capture_id}").as_bytes(),
    );
    format!("{CAPTURE_SUMMARY_ID_PREFIX}{}", sha256::hex(&digest))
}

/// Everything the writer needs besides the resolved scope.
pub struct CaptureWriterDeps {
    pub shards: Arc<ShardManager>,
    pub embedder: Arc<dyn mem_embed::embedder::Embedder>,
}

/// The capture summary content handed to the tier-1 writer (upstream's
/// summary-result + tags + scope extras; the producing session's id).
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureSummaryInput {
    pub capture_id: String,
    /// The producing session's id (Q13: used for BOTH linkages).
    pub session_id: String,
    /// Summary text WITH the upstream `Tags: …` suffix already applied when
    /// tags exist (upstream composes `summaryWithTags` before `addMemory`).
    pub summary: String,
    /// The memory `type` literal (upstream's structured-output type).
    pub memory_type: String,
    /// Normalized (lowercase/trimmed) tags.
    pub tags: Vec<String>,
    /// The eligible user-message id (tier-1 metadata only).
    pub prompt_id: Option<String>,
}

/// What one tier-1 write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier1WriteOutcome {
    /// The row was inserted and the shard count bumped once.
    Written,
    /// A row with the same deterministic id already existed (no write, no
    /// count bump).
    Replayed,
}

/// Why a tier-1 write could not complete.
///
/// `Deferred` is CRITICAL for the inbox contract: a lock deferral (own OR
/// foreign pid) must NOT consume a bounded inbox attempt — the caller
/// distinguishes it structurally (A1).
#[derive(Debug, Clone)]
pub enum CaptureWriterError {
    /// A live operation lock is held by ANOTHER process; retry later
    /// without consuming an attempt.
    Deferred(mem_store::error::StoreError),
    /// A real failure that consumed the attempt.
    Failed(mem_store::error::StoreError),
}

impl std::fmt::Display for CaptureWriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deferred(error) => write!(f, "tier-1 write deferred (live lock): {error}"),
            Self::Failed(error) => write!(f, "tier-1 write failed: {error}"),
        }
    }
}

impl std::error::Error for CaptureWriterError {}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// The tier-1 write under the per-scope write lock.
///
/// Resolution order (A1): `get_write_shard` refuses ANY live lock; on
/// `MigrationLocked` whose pid is OUR OWN process' pid — the engine's own
/// `Consolidator::run` holding `.turso-operation.lock` — retry with
/// `get_write_shard_allowing_own_lock` (own tolerated, foreign refused).
/// A foreign live pid is a DEFER: no write, no attempt consumed.
///
/// The row id is deterministic; an existing id means REPLAY (verify the
/// row actually belongs to this scope/session before treating it as a
/// replay — a mismatch is a collision and must NOT be overwritten).
pub async fn write_capture_summary(
    deps: &CaptureWriterDeps,
    scope: &ResolvedProjectScope,
    input: &CaptureSummaryInput,
) -> Result<Tier1WriteOutcome, CaptureWriterError> {
    let row_id = capture_summary_row_id(&scope.container_tag, &input.capture_id);
    let scope_hash = scope.scope_hash.clone();
    let container_tag = scope.container_tag.clone();

    let inner: Result<Tier1WriteOutcome, CaptureWriterError> = deps
        .shards
        .with_scope_write_lock(Scope::Project, &scope_hash.clone(), move || {
            Box::pin(async move {
                let body = async move {
                    let info = match deps
                        .shards
                        .get_write_shard(Scope::Project, &scope_hash)
                        .await
                    {
                        Ok(info) => info,
                        Err(mem_store::error::StoreError::MigrationLocked { pid, .. })
                            if pid == std::process::id() =>
                        {
                            // Our own consolidation's lock: the own-tolerant path.
                            deps.shards
                                .get_write_shard_allowing_own_lock(Scope::Project, &scope_hash)
                                .await
                                .map_err(capture_writer_error)
                                .map_err(|error| match error {
                                    CaptureWriterError::Failed(inner) => republish(inner),
                                    other => other,
                                })?
                        }
                        Err(error @ mem_store::error::StoreError::MigrationLocked { .. }) => {
                            return Err(CaptureWriterError::Deferred(error));
                        }
                        Err(error) => return Err(CaptureWriterError::Failed(error)),
                    };
                    let db = deps
                        .shards
                        .open_shard(&info.db_path)
                        .await
                        .map_err(CaptureWriterError::Failed)?;

                    // Embeddings FIRST: nothing is written unless both succeed
                    // (document vector; optional tags vector).
                    let vector = deps
                        .embedder
                        .embed(&content_for_embedding(&input.summary, &input.tags))
                        .await
                        .map_err(|_embed_error| {
                            CaptureWriterError::Failed(failed_store_with(
                                "capture summary embedding failed",
                            ))
                        })?;
                    let tags_vector = if input.tags.is_empty() {
                        None
                    } else {
                        Some(
                            deps.embedder
                                .embed(&format_tags_for_embedding(&input.tags))
                                .await
                                .map_err(|_embed_error| {
                                    CaptureWriterError::Failed(failed_store_with(
                                        "capture tags embedding failed",
                                    ))
                                })?,
                        )
                    };

                    // Existence check: existing id = replay candidate; verify
                    // scope/session identity before accepting it.
                    if let Some(row) = query_one(
                        db.as_ref(),
                        "SELECT id, container_tag, session_id FROM memories WHERE id = ?",
                        vec![Value::Text(row_id.clone())],
                    )
                    .await
                    .map_err(CaptureWriterError::Failed)?
                    {
                        let same_scope =
                            row.text("container_tag").as_deref() == Some(&container_tag);
                        let same_session =
                            row.text("session_id").as_deref() == Some(input.session_id.as_str());
                        return if same_scope && same_session {
                            Ok(Tier1WriteOutcome::Replayed)
                        } else {
                            // Collision: never overwrite; structured failure.
                            Err(CaptureWriterError::Failed(failed_store_with(
                                "deterministic capture id collision (different scope/session)",
                            )))
                        };
                    }

                    // The write: `insert_vector_statement` in ONE
                    // `BatchMode::Write` transaction.
                    let created_at = now_millis();
                    let metadata = capture_summary_metadata(input);
                    let insert = MemoryInsert {
                        id: row_id.clone(),
                        content: input.summary.clone(),
                        vector,
                        tags_vector,
                        container_tag: container_tag.clone(),
                        tags: if input.tags.is_empty() {
                            None
                        } else {
                            Some(input.tags.join(","))
                        },
                        memory_type: Some(input.memory_type.clone()),
                        created_at,
                        updated_at: created_at,
                        metadata: Some(metadata),
                        session_id: Some(input.session_id.clone()),
                        display_name: None,
                        user_name: scope.user_name.clone(),
                        user_email: scope.user_email.clone(),
                        project_path: scope.project_path.as_ref().map(|p| p.display().to_string()),
                        project_name: scope.project_name.clone(),
                        git_repo_url: scope.git_repo_url.clone(),
                    };
                    let statement = insert_vector_statement(&insert);
                    let statements = vec![
                        statement,
                        // SQLite allows DML batches; the count bump is
                        // intentionally NOT here (the shard count reconciles
                        // from the actual row count on rotation, and the bump
                        // is manager-owned below the transaction).
                    ];
                    db.batch(statements, BatchMode::Write)
                        .await
                        .map_err(CaptureWriterError::Failed)?;
                    deps.shards
                        .increment_vector_count(info.id)
                        .await
                        .map_err(CaptureWriterError::Failed)?;

                    Ok(Tier1WriteOutcome::Written)
                };
                let outcome: Result<Tier1WriteOutcome, CaptureWriterError> = body.await;
                Ok::<_, mem_store::error::StoreError>(outcome)
            })
        })
        .await
        .map_err(CaptureWriterError::Failed)?;
    inner
}

/// The embedding input content (upstream: content, or content + tags line
/// when tags exist).
fn content_for_embedding(summary: &str, tags: &[String]) -> String {
    if tags.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\nTags: {}", tags.join(", "))
    }
}

/// The tier-1 metadata document: `source`, `sessionID`, `promptId`,
/// `captureTimestamp` ride in metadata; `type`/tags/display fields have
/// dedicated columns (blueprint Step 6).
///
/// SECURITY (mandate 3): built by serializing a TYPED struct. `session_id`
/// and `prompt_id` are wire-controlled and validated only for presence and
/// length, so string interpolation would let a quote in either value forge
/// duplicate keys — serde keeps the LAST duplicate, which would rewrite
/// `source` and re-classify the row as a promoted fact.
fn capture_summary_metadata(input: &CaptureSummaryInput) -> String {
    #[derive(mem_store::serde::Serialize)]
    #[serde(crate = "mem_store::serde")]
    struct CaptureSummaryMetadata<'a> {
        #[serde(rename = "source")]
        source: &'a str,
        #[serde(rename = "sessionID")]
        session_id: &'a str,
        #[serde(rename = "captureTimestamp")]
        capture_timestamp: i64,
        #[serde(rename = "promptId", skip_serializing_if = "Option::is_none")]
        prompt_id: Option<&'a str>,
    }

    let metadata = CaptureSummaryMetadata {
        source: AUTO_CAPTURE_SOURCE,
        session_id: &input.session_id,
        capture_timestamp: now_millis(),
        prompt_id: input.prompt_id.as_deref(),
    };
    mem_store::serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string())
}

fn failed_store_with(message: &str) -> mem_store::error::StoreError {
    mem_store::error::StoreError::Internal {
        message: message.to_string(),
    }
}

/// Classifies a store error on the write leg: a live lock (any pid) is a
/// deferral; everything else failed and consumed the attempt.
fn capture_writer_error(error: mem_store::error::StoreError) -> CaptureWriterError {
    match error {
        mem_store::error::StoreError::MigrationLocked { .. } => {
            CaptureWriterError::Deferred(mem_store::error::StoreError::MigrationLocked {
                pid: 0,
                operation: None,
            })
        }
        other => CaptureWriterError::Failed(other),
    }
}

fn republish(error: mem_store::error::StoreError) -> CaptureWriterError {
    match error {
        mem_store::error::StoreError::MigrationLocked { pid, operation } => {
            CaptureWriterError::Deferred(mem_store::error::StoreError::MigrationLocked {
                pid,
                operation,
            })
        }
        other => CaptureWriterError::Failed(other),
    }
}

// ---------------------------------------------------------------------------
// The two-tier drain (Step 7)
// ---------------------------------------------------------------------------

/// The parsed `capture` request params (bridge-schema.md §3).
#[derive(
    Debug, Clone, Default, PartialEq, mem_store::serde::Deserialize, mem_store::serde::Serialize,
)]
#[serde(crate = "mem_store::serde", default)]
pub struct CaptureParams {
    pub session_id: String,
    pub capture_id: String,
    pub project_id: String,
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub parent_session_id: Option<String>,
    pub transcript: Option<String>,
    pub summary: Option<String>,
    /// The incoming raw `metadata` object (includes `promptId` and — per
    /// Step 8 — `autoCaptureContext`).
    pub metadata: Option<Json>,
}

/// The summarizer's episodic extension (blueprint Step 7): the four fields
/// the frozen schema advertises beyond upstream's base `{summary, type, tags}`.
/// Carried into `NewAgentSession`; every entry is sanitized and bounded at
/// parse time because `mem-episodic`'s validation REJECTS (never coerces)
/// empty, NUL-bearing or oversized entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureExtension {
    pub files_touched: Vec<String>,
    pub completed_work: Vec<WorkItem>,
    pub decisions: Vec<Decision>,
    pub open_threads: Vec<String>,
}

/// The summarizer's structured result: upstream's base `{summary, type,
/// tags}` with `type == "skip"` (normalized tags), plus the port's episodic
/// extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSummaryOutput {
    /// Non-technical / incomplete: writes NEITHER tier, completes the item.
    Skip,
    Capture {
        summary: String,
        memory_type: String,
        tags: Vec<String>,
        /// `filesTouched` / `completedWork` / `decisions` / `openThreads`.
        extension: CaptureExtension,
    },
}

/// One entry's drain outcome (every dequeued entry ends in one of these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryOutcome {
    /// Both tiers committed (or provider `skip`): the inbox row is done.
    Done,
    /// Consumed a retryable failure.
    Retryable,
    /// The bounded retry budget was consumed: dead-lettered to an
    /// episodic-only row.
    Dead,
    /// A lock deferral (own OR foreign) — NO attempt was consumed.
    Deferred,
}

/// What one `drain_once` pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub done: u64,
    pub retryable: u64,
    pub dead_lettered: u64,
    pub deferred: u64,
}

/// The tier-1 write seam (the production implementation wraps
/// [`write_capture_summary`]; tests inject failures/record calls).
pub trait Tier1Writer: Send + Sync {
    fn write<'a>(
        &'a self,
        scope: &'a ResolvedProjectScope,
        input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, CaptureWriterError>>;
}

/// The production tier-1 writer.
pub struct DefaultTier1Writer {
    deps: CaptureWriterDeps,
}

impl DefaultTier1Writer {
    pub fn new(deps: CaptureWriterDeps) -> Self {
        Self { deps }
    }
}

impl Tier1Writer for DefaultTier1Writer {
    fn write<'a>(
        &'a self,
        scope: &'a ResolvedProjectScope,
        input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, CaptureWriterError>> {
        Box::pin(write_capture_summary(&self.deps, scope, input))
    }
}

/// The two-tier capture service.
pub struct CaptureService {
    store: Arc<mem_episodic::store::EpisodicStore>,
    inbox: CaptureInbox,
    writer: Arc<dyn Tier1Writer>,
    embedder: Arc<dyn mem_embed::embedder::Embedder>,
    summarizer: Option<Arc<dyn crate::capture_prompt::CaptureSummarizer>>,
    scope_resolver: Arc<dyn crate::scope_resolver::ProjectScopeResolver>,
    max_context_bytes: usize,
    /// Serializes drain passes (Step 10): enqueue stays lock-free, drains
    /// hold this mutex; the LLM work happens OFF the serial lane.
    drain_lock: Arc<tokio::sync::Mutex<()>>,
    drains: std::sync::atomic::AtomicU64,
}

impl CaptureService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<mem_episodic::store::EpisodicStore>,
        inbox: CaptureInbox,
        writer: Arc<dyn Tier1Writer>,
        embedder: Arc<dyn mem_embed::embedder::Embedder>,
        summarizer: Option<Arc<dyn crate::capture_prompt::CaptureSummarizer>>,
        scope_resolver: Arc<dyn crate::scope_resolver::ProjectScopeResolver>,
        max_context_bytes: usize,
    ) -> Self {
        Self {
            store,
            inbox,
            writer,
            embedder,
            summarizer,
            scope_resolver,
            max_context_bytes,
            drain_lock: Arc::new(tokio::sync::Mutex::new(())),
            drains: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The store root (the scheduler and diagnostics reuse it).
    pub fn storage_path(&self) -> &std::path::Path {
        self.store.storage_path()
    }

    /// Durably enqueues one parsed capture (idempotent on `capture_id`).
    pub async fn enqueue(&self, params: &CaptureParams) -> Result<(), ObjError> {
        let payload = params_payload(params);
        self.inbox
            .enqueue(InboxNewEntry {
                capture_id: params.capture_id.clone(),
                project_id: params.project_id.clone(),
                session_id: params.session_id.clone(),
                agent_id: params.agent_id.clone(),
                payload: payload.clone(),
            })
            .await
            .map_err(ObjError::from)
    }

    /// The inbox (durable-replay diagnostics + tests).
    pub fn inbox(&self) -> &CaptureInbox {
        &self.inbox
    }

    /// Drains one bounded FIFO batch; serialized behind the drain mutex so
    /// two passes never interleave one entry's writes.
    ///
    /// **Loop invariant:** before entry i is processed, every entry before
    /// i has an explicit Done/Retryable/Dead/Deferred outcome and entries
    /// `i..end` are untouched; the index advances once per finite batch
    /// item, so the loop terminates.
    pub async fn drain_once(&self) -> Result<DrainReport, EpisodicError> {
        let _gate = self.drain_lock.lock().await;
        self.drains.fetch_add(1, Ordering::Relaxed);
        let batch = self.inbox.dequeue(DRAIN_BATCH_LIMIT).await?;
        let mut report = DrainReport::default();
        for entry in &batch {
            match self.process_entry(entry).await {
                EntryOutcome::Done => report.done += 1,
                EntryOutcome::Retryable => report.retryable += 1,
                EntryOutcome::Dead => report.dead_lettered += 1,
                EntryOutcome::Deferred => report.deferred += 1,
            }
        }
        Ok(report)
    }

    /// The bounded drain behind the shutdown `db_flush` leg: flushes only
    /// queued entries whose summary was ALREADY supplied — an LLM is never
    /// invoked on the shutdown path; other entries stay queued for the
    /// next start's durable replay.
    pub async fn flush_supplied_summary_queued(&self) -> Result<DrainReport, EpisodicError> {
        let _gate = self.drain_lock.lock().await;
        let batch = self.inbox.dequeue(DRAIN_BATCH_LIMIT).await?;
        let mut report = DrainReport::default();
        for entry in &batch {
            let has_summary =
                mem_store::serde_json::from_value::<CaptureParams>(entry.payload.clone())
                    .ok()
                    .and_then(|params| params.summary.as_deref().map(|opt| !opt.trim().is_empty()))
                    .unwrap_or(false);
            if !has_summary {
                continue; // stays queued
            }
            match self.flush_entry_with_supplied_summary(entry).await {
                EntryOutcome::Done => report.done += 1,
                EntryOutcome::Retryable => report.retryable += 1,
                EntryOutcome::Dead => report.dead_lettered += 1,
                EntryOutcome::Deferred => report.deferred += 1,
            }
        }
        Ok(report)
    }

    /// The summary-supplied variant of `process_entry` (identical commit
    /// order, provider never called).
    async fn flush_entry_with_supplied_summary(
        &self,
        entry: &mem_episodic::inbox::InboxEntry,
    ) -> EntryOutcome {
        let params = mem_store::serde_json::from_value::<CaptureParams>(entry.payload.clone())
            .unwrap_or_default();
        let scope = match self.scope_resolver.resolve(&params.project_id) {
            Ok(scope) => scope,
            Err(_) => return self.retryable_or_dead(entry).await,
        };
        let Some((summary, tags)) = summary_with_extracted_tags(&params) else {
            return self.retryable_or_dead(entry).await;
        };
        // A caller-supplied summary carries no upstream `type` and no
        // provider extension: the documented default type and an empty
        // extension (the provider path supplies both via `assemble_summary`).
        let memory_type = "memory".to_string();
        let extension = CaptureExtension::default();
        if self
            .commit_episodic(&params, &summary, &tags, &extension)
            .await
            .is_err()
        {
            return self.retryable_or_dead(entry).await;
        }
        let tier_input = capture_summary_input(entry, &summary, &memory_type, &tags, &params);
        match self.writer.write(&scope, &tier_input).await {
            Ok(Tier1WriteOutcome::Written | Tier1WriteOutcome::Replayed) => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                EntryOutcome::Done
            }
            Err(CaptureWriterError::Deferred(_)) => EntryOutcome::Deferred,
            Err(CaptureWriterError::Failed(_)) => self.retryable_or_dead(entry).await,
        }
    }

    /// Processes ONE dequeued entry to its terminal-or-deferral state.
    ///
    /// Commit order (D-W5.3-5, two LOCAL commits, episodic first): (1) the
    /// episodic upsert, (2) the tier-1 capture summary, (3) the inbox
    /// `mark_done` purge. `skip` writes neither tier. An episodic failure
    /// never reaches tier-1.
    async fn process_entry(&self, entry: &mem_episodic::inbox::InboxEntry) -> EntryOutcome {
        let params = mem_store::serde_json::from_value::<CaptureParams>(entry.payload.clone())
            .unwrap_or_default();

        // 1. The scope (an engine-prevented blank arriving here is a
        // corrupt payload: fail closed as a consumed attempt).
        let scope = match self.scope_resolver.resolve(&params.project_id) {
            Ok(scope) => scope,
            Err(_) => return self.retryable_or_dead(entry).await,
        };

        // 2. Private content is stripped from every free text BEFORE any
        // durable write.
        let transcript = params
            .transcript
            .as_deref()
            .map(mem_ai::privacy::strip_private_content);

        // 3. Context + summary construction — OFF the serial DB lane.
        let output = match self.assemble_summary(&params, transcript.as_deref()).await {
            Ok(output) => output,
            Err(_) => return self.retryable_or_dead(entry).await,
        };
        let (summary, memory_type, tags, extension) = match output {
            CaptureSummaryOutput::Skip => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                return EntryOutcome::Done;
            }
            CaptureSummaryOutput::Capture {
                summary,
                memory_type,
                tags,
                extension,
            } => (summary, memory_type, tags, extension),
        };

        // 4. Commit 1: the episodic staging row (capture_id-aware upsert).
        if self
            .commit_episodic(&params, &summary, &tags, &extension)
            .await
            .is_err()
        {
            return self.retryable_or_dead(entry).await;
        }

        // 5. Commit 2: the tier-1 capture summary. A superseded staging
        // row still gets its own deterministic tier-1 row per capture_id.
        let tier_input = capture_summary_input(entry, &summary, &memory_type, &tags, &params);
        match self.writer.write(&scope, &tier_input).await {
            Ok(Tier1WriteOutcome::Written | Tier1WriteOutcome::Replayed) => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                EntryOutcome::Done
            }
            Err(CaptureWriterError::Deferred(_)) => EntryOutcome::Deferred,
            Err(CaptureWriterError::Failed(_)) => self.retryable_or_dead(entry).await,
        }
    }

    /// A consumed retryable failure; the fifth consumed attempt
    /// dead-letters the entry to the episodic-only terminal state.
    async fn retryable_or_dead(&self, entry: &mem_episodic::inbox::InboxEntry) -> EntryOutcome {
        let attempts_after = self
            .inbox
            .bump_attempts(&entry.capture_id)
            .await
            .unwrap_or(u32::MAX);
        if attempts_after >= 5 {
            let _ = self.inbox.dead_letter(&entry.capture_id).await;
            crate::engine::structured_stderr(
                "capture.dead_lettered",
                "warn",
                "capture",
                serde_json::json!({ "capture_id": entry.capture_id, "project_id": entry.project_id }),
            );
            EntryOutcome::Dead
        } else {
            EntryOutcome::Retryable
        }
    }

    /// Commit 1 of the two-tier drain: resolve the parent (WIRE session id →
    /// project-visible ROW id) and upsert the staging row.
    async fn commit_episodic(
        &self,
        params: &CaptureParams,
        summary: &str,
        tags: &[String],
        extension: &CaptureExtension,
    ) -> Result<(), EpisodicError> {
        let parent_row_id = match params.parent_session_id.as_deref() {
            Some(wire) if !wire.trim().is_empty() => {
                Some(self.resolve_parent_row_id(params, wire).await?)
            }
            _ => None,
        };
        self.store
            .upsert_session(
                episodic_record(params, summary, tags, extension, parent_row_id),
                self.embedder.as_ref(),
            )
            .await
            .map(|_| ())
    }

    /// Resolves a WIRE parent session id to the project-visible ROW id (the
    /// `agent_sessions.parent_session_id` FK target). Several agents may
    /// share one wire session id: prefer the row whose `agent_id` matches
    /// this capture's agent, else the newest visible row. Absent /
    /// cross-project / deleted is a typed `NotFound`, so a child that arrives
    /// ahead of its parent stays queued for the bounded retry/dead-letter.
    async fn resolve_parent_row_id(
        &self,
        params: &CaptureParams,
        parent_wire_id: &str,
    ) -> Result<String, EpisodicError> {
        let db = self.store.ready().await?;
        let rows = db
            .query(
                "SELECT id, agent_id FROM agent_sessions \
                 WHERE project_id = ? AND session_id = ? AND deleted_at IS NULL \
                 ORDER BY created_at DESC, id DESC",
                vec![
                    Value::Text(params.project_id.clone()),
                    Value::Text(parent_wire_id.to_string()),
                ],
            )
            .await
            .map_err(EpisodicError::Store)?;
        let Some(first) = rows.first() else {
            return Err(EpisodicError::NotFound {
                entity: "parent_session".to_string(),
            });
        };
        if let Some(matching) = rows
            .iter()
            .find(|row| row.text("agent_id").as_deref() == Some(params.agent_id.as_str()))
        {
            return Ok(matching.text("id").unwrap_or_default());
        }
        Ok(first.text("id").unwrap_or_default())
    }

    /// The capture-time summary assembly: a supplied summary wins; else
    /// the bounded auto-capture context (metadata `autoCaptureContext`
    /// when the wrapper sent one, else the raw transcript) goes to the
    /// injected summarizer.
    async fn assemble_summary(
        &self,
        params: &CaptureParams,
        transcript: Option<&str>,
    ) -> Result<CaptureSummaryOutput, EpisodicError> {
        if let Some((summary, tags)) = summary_with_extracted_tags(params) {
            let _ = &params;
            return Ok(CaptureSummaryOutput::Capture {
                summary,
                memory_type: "memory".to_string(),
                tags,
                extension: CaptureExtension::default(),
            });
        }

        let context = match auto_capture_context(
            &params.metadata,
            transcript,
            self.max_context_bytes,
            LatestMemoryReader::None,
        ) {
            Some(context) => context,
            None => return Ok(CaptureSummaryOutput::Skip),
        };
        match self.summarizer.as_ref() {
            Some(summarizer) => summarizer.summarize(&context).await,
            None => Err(EpisodicError::Io {
                message: "no capture summarizer configured".to_string(),
            }),
        }
    }
}

/// Envelope plumbing of enqueue errors for the engine's `-32002` mapping.
#[derive(Debug, Clone)]
pub enum ObjError {
    Full,
    Store(EpisodicError),
}

impl From<mem_episodic::inbox::InboxError> for ObjError {
    fn from(error: mem_episodic::inbox::InboxError) -> Self {
        match error {
            mem_episodic::inbox::InboxError::Full => Self::Full,
            mem_episodic::inbox::InboxError::Store(store) => Self::Store(store),
        }
    }
}

/// The supplied-summary path extracts the upstream `Tags: …` suffix into
/// the tags vector (normalized), leaving the summary clean for the
/// episodic row's own `Tags:` rendering at the tier-1 stage.
fn summary_with_extracted_tags(params: &CaptureParams) -> Option<(String, Vec<String>)> {
    let summary = params.summary.as_deref().filter(|s| !s.trim().is_empty())?;
    let stripped = mem_ai::privacy::strip_private_content(summary);
    let (head, tags) = split_tags_suffix(&stripped);
    Some((head, tags))
}

/// Splits a trailing `\n\nTags: a, b` suffix (upstream composes
/// `summaryWithTags` this way before `addMemory`).
fn split_tags_suffix(summary: &str) -> (String, Vec<String>) {
    if let Some((head, tail)) = summary.rsplit_once("\n\nTags: ") {
        let tags: Vec<String> = tail
            .split(',')
            .map(|pair| pair.trim().to_lowercase())
            .filter(|pair| !pair.is_empty())
            .collect();
        if !tags.is_empty() {
            return (head.trim_end().to_string(), tags);
        }
    }
    (summary.trim_end().to_string(), Vec::new())
}

/// The bounded capture-time markdown context: `metadata.autoCaptureContext`
/// (`userPrompt`, `textResponses`, `toolCalls`, optional `latestMemory`
/// —the wrapper is allowed to send the block, and the Rust builder may obtain
/// it through the same scope) or the raw transcript.
fn auto_capture_context(
    metadata: &Option<Json>,
    transcript: Option<&str>,
    max_context_bytes: usize,
    _latest: LatestMemoryReader,
) -> Option<String> {
    let user_prompt = metadata
        .as_ref()
        .and_then(|m| m.get("autoCaptureContext"))
        .and_then(|ctx| ctx.get("userPrompt"))
        .and_then(Json::as_str);
    let text_responses: Vec<String> = metadata
        .as_ref()
        .and_then(|m| m.get("autoCaptureContext"))
        .and_then(|ctx| ctx.get("textResponses"))
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let tool_calls: Vec<crate::capture_context::ToolCallInfo> = metadata
        .as_ref()
        .and_then(|m| m.get("autoCaptureContext"))
        .and_then(|ctx| ctx.get("toolCalls"))
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(crate::capture_context::ToolCallInfo {
                        name: item.get("name")?.as_str()?.to_string(),
                        input: item
                            .get("input")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if let Some(prompt) = user_prompt {
        let latest_memory: Option<&str> = metadata
            .as_ref()
            .and_then(|m| m.get("autoCaptureContext"))
            .and_then(|ctx| ctx.get("latestMemory"))
            .and_then(Json::as_str);
        let context = crate::capture_context::build_markdown_context(
            prompt,
            &text_responses,
            &tool_calls,
            latest_memory,
            max_context_bytes,
        );
        return Some(mem_ai::privacy::strip_private_content(&context));
    }

    // Transcript fallback (strip first).
    transcript.map(|text| text.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatestMemoryReader {
    None,
}

/// The tier-1 content: summary WITH the `Tags: …` suffix (upstream
/// composes `summaryWithTags` then `addMemory`).
fn summary_with_tags_line(summary: &str, tags: &[String]) -> String {
    if tags.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\n\nTags: {}", tags.join(", "))
    }
}

fn prompt_id_of(params: &CaptureParams) -> Option<String> {
    params
        .metadata
        .as_ref()
        .and_then(|m| m.get("promptId"))
        .and_then(Json::as_str)
        .map(str::to_string)
}

fn capture_summary_input(
    entry: &mem_episodic::inbox::InboxEntry,
    summary: &str,
    memory_type: &str,
    tags: &[String],
    params: &CaptureParams,
) -> CaptureSummaryInput {
    CaptureSummaryInput {
        capture_id: entry.capture_id.clone(),
        session_id: entry.session_id.clone(),
        summary: summary_with_tags_line(summary, tags),
        // Step 6: the provider's parsed `type` maps to the dedicated
        // `MemoryInsert.memory_type` column; the supplied-summary path
        // supplies upstream's `"memory"` default at its own call site.
        memory_type: memory_type.to_string(),
        tags: tags.to_vec(),
        prompt_id: prompt_id_of(params),
    }
}

fn params_payload(params: &CaptureParams) -> Json {
    mem_store::serde_json::to_value(params).unwrap_or(Json::Null)
}

/// The episodic staging record (project/session/agent scope triple, the
/// summary text WITHOUT the tags suffix, `capture_id`, and the extra
/// metadata minus `autoCaptureContext`).
fn episodic_record(
    params: &CaptureParams,
    summary: &str,
    _tags: &[String],
    extension: &CaptureExtension,
    parent_row_id: Option<String>,
) -> NewAgentSession {
    let mut extra: std::collections::BTreeMap<String, Json> = Default::default();
    if let Some(metadata) = params.metadata.as_ref().and_then(Json::as_object) {
        for (key, value) in metadata {
            // The context envelope is removed from persisted metadata; the
            // extension's camelCase wire keys are now TYPED fields, so their
            // raw copies must not be duplicated into `extra`.
            if key == "autoCaptureContext"
                || matches!(
                    key.as_str(),
                    "filesTouched" | "completedWork" | "decisions" | "openThreads"
                )
            {
                continue;
            }
            extra.insert(key.clone(), value.clone());
        }
    }
    NewAgentSession {
        project_id: params.project_id.clone(),
        session_id: params.session_id.clone(),
        agent_id: params.agent_id.clone(),
        agent_name: params.agent_name.clone().unwrap_or_default(),
        summary: summary.to_string(),
        files_touched: extension.files_touched.clone(),
        completed_work: extension.completed_work.clone(),
        decisions: extension.decisions.clone(),
        open_threads: extension.open_threads.clone(),
        extra,
        capture_id: Some(params.capture_id.clone()),
        parent_session_id: parent_row_id,
        ..Default::default()
    }
}
