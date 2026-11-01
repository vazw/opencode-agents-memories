//! W5.3 — capture plumbing. Step 6: the session-linked tier-1
//! capture-summary writer (`write_capture_summary`) and its deterministic
//! row id. The two-tier drain (`drain_once`) lands on top of this writer.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mem_core::logging::log;
use mem_core::logging::signatures::{
    AUTO_CAPTURE_EPISODIC_FAILED, AUTO_CAPTURE_FINAL_ERROR, AUTO_CAPTURE_MEMORY_PERSISTED,
    AUTO_CAPTURE_PROVIDER_FAILED, AUTO_CAPTURE_SKIPPED,
};
use mem_core::scope::Scope;
use mem_core::sha256;

use mem_embed::embedder::BoxFuture;
use mem_episodic::error::EpisodicError;
use mem_episodic::inbox::{CaptureInbox, DRAIN_BATCH_LIMIT, InboxEntry, InboxNewEntry};
use mem_episodic::raw_context::MAX_RAW_CONTEXT_BYTES;
use mem_episodic::record::NewAgentSession;
use mem_episodic::record::{Decision, WorkItem};

use mem_store::db::{BatchMode, Value};
use mem_store::serde_json::Value as Json;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{MemoryInsert, insert_vector_statement};

use crate::project_scope::ResolvedProjectScope;
use crate::vault_feeder::{VaultFeeder, VaultFeederJob};

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
/// `Deferred` is CRITICAL for the inbox contract: a live lock deferral (own OR
/// foreign pid) **or a scope shard that cannot be read** must NOT consume a
/// bounded inbox attempt — the caller distinguishes it structurally (A1).
#[derive(Debug, Clone)]
pub enum CaptureWriterError {
    /// A live operation lock is held by ANOTHER process, or a shard of the
    /// scope could not be read; retry later without consuming an attempt.
    Deferred(mem_store::error::StoreError),
    /// A real failure that consumed the attempt.
    Failed(mem_store::error::StoreError),
}

impl std::fmt::Display for CaptureWriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deferred(error) => write!(f, "tier-1 write deferred: {error}"),
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
/// The row id is deterministic; an existing id means REPLAY (the id is looked
/// up across EVERY shard of the scope — active and retired — and the row it
/// resolves to must belong to this scope/session before it is accepted as a
/// replay; a mismatch is a collision and must NOT be overwritten). A failed
/// scope-wide lookup is a DEFER, never a "no existing row".
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
                    // Announce the first truncated input this process sees.
                    let _ = crate::truncation_report::REPORTER
                        .report_first(deps.embedder.as_ref(), "capture");

                    // Existence check ACROSS every shard of the scope: the
                    // deterministic id may live in a retired shard, so a
                    // shard-local lookup is exactly the duplicate bug. An
                    // error is NOT "no existing row" — every `Err` defers the
                    // write (no attempt consumed), because capture has no
                    // unique-violation backstop behind this check.
                    match deps
                        .shards
                        .find_memory_in_scope(Scope::Project, &scope_hash, &row_id)
                        .await
                    {
                        Ok(Some(scoped)) => {
                            let same_scope =
                                scoped.container_tag.as_deref() == Some(&container_tag);
                            let same_session =
                                scoped.session_id.as_deref() == Some(input.session_id.as_str());
                            return if same_scope && same_session {
                                Ok(Tier1WriteOutcome::Replayed)
                            } else {
                                // Collision: never overwrite; structured failure.
                                Err(CaptureWriterError::Failed(failed_store_with(
                                    "deterministic capture id collision (different scope/session)",
                                )))
                            };
                        }
                        Ok(None) => {}
                        // Variant-blind on purpose: a registry-read failure
                        // and an unreadable shard are the same "not now"
                        // class, and `Failed` would consume a bounded attempt
                        // for an environmental fault.
                        Err(error) => return Err(CaptureWriterError::Deferred(error)),
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
                    let statement =
                        insert_vector_statement(&insert).map_err(CaptureWriterError::Failed)?;
                    // W3: the insert and the statement-measured ABSOLUTE count
                    // share one transaction; the registry then adopts the
                    // shard's own value (never a `±1`).
                    let statements =
                        vec![statement, mem_store::shard_facts::sync_count_statement()];
                    db.batch(statements, BatchMode::Write)
                        .await
                        .map_err(CaptureWriterError::Failed)?;
                    deps.shards
                        .adopt_stored_count(db.as_ref(), info.id)
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

/// The ONE operational record of a tier-1 deferral.
///
/// `reason` is a fixed literal from the closed set below — never the
/// `StoreError` Display, which carries the scope hash and a filesystem path.
/// The label is the only place that may look at the error variant; the
/// deferred/failed mapping above is variant-blind by design. No test asserts
/// this line (it is the wave's named, deliberate observation gap).
fn log_tier1_deferred(entry: &InboxEntry, reason: &'static str) {
    crate::engine::structured_stderr(
        "capture.tier1_deferred",
        "warn",
        "capture",
        serde_json::json!({
            "capture_id": entry.capture_id,
            "project_id": entry.project_id,
            "reason": reason,
        }),
    );
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
    /// W7.1 additive: the session's raw context. Persisted (privacy-stripped)
    /// only when `featureFlags.soundMemory` is on; a V1 payload without it
    /// keeps deserializing and behaving exactly as before.
    #[serde(default)]
    pub raw_context: Option<String>,
    /// W7.1 additive: the ADR-0009 agent role. Persisted with one targeted
    /// `UPDATE` on the session row; an unknown value dead-letters the entry
    /// with a fixed message that never echoes the value.
    #[serde(default)]
    pub agent_role: Option<String>,
    /// W8a additive: when true, the whole retained raw context is promoted
    /// verbatim into the vault tier by the session-end feeder.
    #[serde(default)]
    pub vault: bool,
}

/// The `featureFlags.soundMemory` + `soundMemory.rawContextRequired` slice
/// the capture service reads (W7.1).
///
/// Default OFF: with these defaults the capture path writes NO retention
/// rows (no raw context, no segments) — the flag-off proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SoundMemorySettings {
    /// `featureFlags.soundMemory`.
    pub enabled: bool,
    /// `soundMemory.rawContextRequired`: a payload with no raw text is
    /// dead-lettered instead of falling back to the transcript.
    pub raw_context_required: bool,
}

/// The fixed dead-letter reason when `rawContextRequired` is on and the
/// payload carries no raw text.
pub const RAW_CONTEXT_REQUIRED_MESSAGE: &str =
    "capture payload has no raw context (soundMemory.rawContextRequired)";

/// The fixed dead-letter reason for an invalid raw context (NUL/oversize).
pub const RAW_CONTEXT_INVALID_MESSAGE: &str = "capture payload has an invalid raw context";

/// The fixed dead-letter reason for an unknown agent role. It never echoes
/// the rejected value.
pub const AGENT_ROLE_INVALID_MESSAGE: &str =
    "agent role must be one of: main, explorer, reviewer, worker";

/// Parses the payload's agent role: absent is `None`, a known role is
/// accepted case-insensitively, anything else is the fixed message.
pub fn parse_capture_agent_role(
    value: Option<&str>,
) -> Result<Option<mem_core::agent::AgentRole>, &'static str> {
    match value {
        None => Ok(None),
        Some(value) => mem_core::agent::AgentRole::parse(value)
            .map(Some)
            .map_err(|_| AGENT_ROLE_INVALID_MESSAGE),
    }
}

/// The resolved retention payload for one capture entry.
struct RetentionWrite {
    /// The privacy-stripped raw context, when `soundMemory` is on and one
    /// was resolvable.
    raw_context: Option<String>,
    /// The validated agent role, when the payload supplied one.
    agent_role: Option<mem_core::agent::AgentRole>,
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
    /// Non-technical / incomplete / no context: writes NEITHER tier,
    /// completes the item. The reason is carried so "injected-only" is never
    /// confused with "blank" (D-52-5).
    Skip(SkipReason),
    Capture {
        summary: String,
        memory_type: String,
        tags: Vec<String>,
        /// `filesTouched` / `completedWork` / `decisions` / `openThreads`.
        extension: CaptureExtension,
    },
}

/// Why a capture produced neither tier (D-52-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// No prompt and no context to attach to it.
    NoContext,
    /// The prompt consisted entirely of injected boilerplate.
    InjectedOnly,
    /// The provider answered `type: "skip"`.
    ProviderSkipped,
}

impl SkipReason {
    /// The log-safe label (`AUTO_CAPTURE_SKIPPED`'s `reason` field).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoContext => "no_context",
            Self::InjectedOnly => "injected_only",
            Self::ProviderSkipped => "provider_skip",
        }
    }
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
    /// A live lock (own OR foreign) **or a scope shard that cannot be read** —
    /// NO attempt was consumed.
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
    /// W7.1: the sound-memory flags (default OFF).
    sound_memory: SoundMemorySettings,
    /// W8a: optional session-end vault feeder.
    vault_feeder: Option<Arc<VaultFeeder>>,
    /// D-52-3: the injection-filter configuration this service applies at
    /// its five store-path sites. Defaults to `PrivacyConfig::default()`
    /// (filter ON) — fail-closed: a missing `.with_privacy` weakens config
    /// fidelity (custom markers, the flag), never the filter itself.
    privacy: mem_ai::privacy::PrivacyConfig,
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
            sound_memory: SoundMemorySettings::default(),
            vault_feeder: None,
            privacy: mem_ai::privacy::PrivacyConfig::default(),
        }
    }

    /// W7.1 — sets the sound-memory flags. A builder setter, deliberately:
    /// the seventeen existing `CaptureService::new` call sites keep their exact
    /// argument list, and the default remains OFF.
    pub fn with_sound_memory(mut self, settings: SoundMemorySettings) -> Self {
        self.sound_memory = settings;
        self
    }

    /// W8a — attach a vault feeder. Builder setter, default None, mirroring
    /// `with_sound_memory`.
    pub fn with_vault_feeder(mut self, feeder: Option<Arc<VaultFeeder>>) -> Self {
        self.vault_feeder = feeder;
        self
    }

    /// D-52-3 — the injection-filter configuration. Builder setter, default
    /// `PrivacyConfig::default()` (filter ON), mirroring
    /// `with_sound_memory`/`with_vault_feeder`.
    pub fn with_privacy(mut self, privacy: mem_ai::privacy::PrivacyConfig) -> Self {
        self.privacy = privacy;
        self
    }

    /// The privacy configuration in force (composition diagnostics/tests).
    pub fn privacy(&self) -> &mem_ai::privacy::PrivacyConfig {
        &self.privacy
    }

    /// The sound-memory settings in force (composition diagnostics/tests).
    pub fn sound_memory_settings(&self) -> SoundMemorySettings {
        self.sound_memory
    }

    /// The attached vault feeder, if any (composition diagnostics/tests).
    pub fn vault_feeder(&self) -> Option<Arc<VaultFeeder>> {
        self.vault_feeder.clone()
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
            Err(_) => {
                return self
                    .retryable_or_dead(entry, Failure::of("scope_unresolved"))
                    .await;
            }
        };
        let Some((summary, tags)) = summary_with_extracted_tags(&params) else {
            return self
                .retryable_or_dead(entry, Failure::of("summary_missing"))
                .await;
        };
        // A caller-supplied summary carries no upstream `type` and no
        // provider extension: the documented default type and an empty
        // extension (the provider path supplies both via `assemble_summary`).
        let memory_type = "memory".to_string();
        let extension = CaptureExtension::default();
        // D-52-3 site 3: the flush path's transcript, stripped and filtered at
        // the call site. The strip is load-bearing — filtering first would
        // delete a `<private>` opener that meets a marker region; the
        // pre-existing strip inside `resolve_retention` stays untouched and is
        // a no-op on the already-stripped value.
        let filtered_transcript = params
            .transcript
            .as_deref()
            .map(|text| self.privacy.strip_then_filter_text(text));
        let retention = match self.resolve_retention(&params, filtered_transcript.as_deref()) {
            Ok(retention) => retention,
            Err(reason) => return self.dead_letter_now(entry, reason).await,
        };
        if let Err(error) = self
            .commit_episodic(&params, &summary, &tags, &extension, &retention)
            .await
        {
            log_episodic_write_failed(entry, &error);
            return self
                .retryable_or_dead(entry, Failure::of("episodic_write_failed"))
                .await;
        }

        // W8a: enqueue the vault feeder (fire-and-forget) after a successful
        // episodic commit when a raw context was retained.
        if let (Some(feeder), Some(raw)) = (&self.vault_feeder, &retention.raw_context) {
            feeder.enqueue(VaultFeederJob {
                scope: scope.clone(),
                raw_context: raw.clone(),
                params: params.clone(),
            });
        }
        let tier_input = capture_summary_input(entry, &summary, &memory_type, &tags, &params);
        match self.writer.write(&scope, &tier_input).await {
            Ok(Tier1WriteOutcome::Written) => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                log(
                    AUTO_CAPTURE_MEMORY_PERSISTED,
                    Some(&serde_json::json!({
                        "captureId": &entry.capture_id,
                        "sessionId": &params.session_id,
                        "projectId": &params.project_id,
                        "memoryType": &memory_type,
                    })),
                );
                EntryOutcome::Done
            }
            // A replay means the deterministic row already existed: emitting
            // here would double-count a retry after a lost `mark_done`.
            Ok(Tier1WriteOutcome::Replayed) => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                EntryOutcome::Done
            }
            Err(CaptureWriterError::Deferred(error)) => {
                // The label may look at the variant; the mapping may not.
                let reason = if matches!(
                    error,
                    mem_store::error::StoreError::ScopeShardUnreadable { .. }
                ) {
                    "scope_shard_unreadable"
                } else {
                    "store_error"
                };
                log_tier1_deferred(entry, reason);
                EntryOutcome::Deferred
            }
            Err(CaptureWriterError::Failed(_)) => {
                self.retryable_or_dead(entry, Failure::of("tier1_write_failed"))
                    .await
            }
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
            Err(_) => {
                return self
                    .retryable_or_dead(entry, Failure::of("scope_unresolved"))
                    .await;
            }
        };

        // 2. Private content is stripped from every free text BEFORE any
        // durable write. D-52-3 site 1: the injection filter wraps that
        // stripped value (strip -> filter). The stripper's statement is left
        // byte-identical and the filter is a second binding, which is what
        // 52.8's mechanical gate diffs.
        let transcript = params
            .transcript
            .as_deref()
            .map(mem_ai::privacy::strip_private_content);
        let transcript = transcript.map(|text| self.privacy.filter_injected_text(&text));

        // 2a. D-52-3 site 2: the refusal reads the RAW effective prompt, and
        // it runs before `resolve_retention` so a payload that is going to be
        // skipped never reaches the retention rules. `&params`: `params` is
        // owned here and `effective_prompt` takes `&CaptureParams`; the
        // transcript leg comes from `params`, NEVER from the local
        // `transcript` binding site 1 overwrote.
        let raw_prompt = effective_prompt(&params, params.transcript.as_deref());
        if raw_prompt.is_some_and(|prompt| self.privacy.prompt_is_fully_injected(prompt)) {
            let _ = self.inbox.mark_done(&entry.capture_id).await;
            log(
                AUTO_CAPTURE_SKIPPED,
                Some(&serde_json::json!({
                    "captureId": &entry.capture_id,
                    "sessionId": &params.session_id,
                    "type": "skip",
                    "reason": SkipReason::InjectedOnly.as_str(),
                })),
            );
            return EntryOutcome::Done;
        }

        // 2b. W7.1 — validate the retention payload BEFORE the summarizer:
        // a rejected raw context or agent role dead-letters the entry and
        // writes neither tier (no session row).
        let retention = match self.resolve_retention(&params, transcript.as_deref()) {
            Ok(retention) => retention,
            Err(reason) => return self.dead_letter_now(entry, reason).await,
        };

        // 3. Context + summary construction — OFF the serial DB lane.
        let output = match self.assemble_summary(&params, transcript.as_deref()).await {
            Ok(output) => output,
            Err(_) => {
                let failure = self.summarizer_failure();
                log_provider_failed(entry, failure);
                return self.retryable_or_dead(entry, failure).await;
            }
        };
        let (summary, memory_type, tags, extension) = match output {
            CaptureSummaryOutput::Skip(reason) => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                log(
                    AUTO_CAPTURE_SKIPPED,
                    Some(&serde_json::json!({
                        "captureId": &entry.capture_id,
                        "sessionId": &params.session_id,
                        "type": "skip",
                        "reason": reason.as_str(),
                    })),
                );
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
        if let Err(error) = self
            .commit_episodic(&params, &summary, &tags, &extension, &retention)
            .await
        {
            log_episodic_write_failed(entry, &error);
            return self
                .retryable_or_dead(entry, Failure::of("episodic_write_failed"))
                .await;
        }

        // W8a: enqueue the vault feeder (fire-and-forget) after a successful
        // episodic commit when a raw context was retained.
        if let (Some(feeder), Some(raw)) = (&self.vault_feeder, &retention.raw_context) {
            feeder.enqueue(VaultFeederJob {
                scope: scope.clone(),
                raw_context: raw.clone(),
                params: params.clone(),
            });
        }

        // 5. Commit 2: the tier-1 capture summary. A superseded staging
        // row still gets its own deterministic tier-1 row per capture_id.
        let tier_input = capture_summary_input(entry, &summary, &memory_type, &tags, &params);
        match self.writer.write(&scope, &tier_input).await {
            Ok(Tier1WriteOutcome::Written) => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                log(
                    AUTO_CAPTURE_MEMORY_PERSISTED,
                    Some(&serde_json::json!({
                        "captureId": &entry.capture_id,
                        "sessionId": &params.session_id,
                        "projectId": &params.project_id,
                        "memoryType": &memory_type,
                    })),
                );
                EntryOutcome::Done
            }
            // A replay means the deterministic row already existed: emitting
            // here would double-count a retry after a lost `mark_done`.
            Ok(Tier1WriteOutcome::Replayed) => {
                let _ = self.inbox.mark_done(&entry.capture_id).await;
                EntryOutcome::Done
            }
            Err(CaptureWriterError::Deferred(error)) => {
                // The label may look at the variant; the mapping may not.
                let reason = if matches!(
                    error,
                    mem_store::error::StoreError::ScopeShardUnreadable { .. }
                ) {
                    "scope_shard_unreadable"
                } else {
                    "store_error"
                };
                log_tier1_deferred(entry, reason);
                EntryOutcome::Deferred
            }
            Err(CaptureWriterError::Failed(_)) => {
                self.retryable_or_dead(entry, Failure::of("tier1_write_failed"))
                    .await
            }
        }
    }

    /// The NAMED cause of the failure about to be recorded.
    ///
    /// Every retry site names itself; only the summarizer leg consults the
    /// summarizer's own record, because only it can classify a provider error.
    fn summarizer_failure(&self) -> Failure {
        let Some(summarizer) = self.summarizer.as_ref() else {
            return Failure::of("capture_provider_unavailable");
        };
        match summarizer.last_failure() {
            Some(failure) => Failure {
                reason: failure.reason,
                error_kind: Some(failure.error_kind),
                status: failure.status,
            },
            // A JSON/parse failure: no provider classification belongs to it.
            None => Failure::of("summarizer_failed"),
        }
    }

    /// A consumed retryable failure; the fifth consumed attempt
    /// dead-letters the entry to the episodic-only terminal state.
    ///
    /// `failure` names the cause; it is carried into the dead-letter payload so
    /// no capture ever dies anonymously.
    async fn retryable_or_dead(
        &self,
        entry: &mem_episodic::inbox::InboxEntry,
        failure: Failure,
    ) -> EntryOutcome {
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
            let mut data = serde_json::json!({
                "captureId": &entry.capture_id,
                "projectId": &entry.project_id,
                "attempts": attempts_after,
                "outcome": "dead_lettered",
                "reason": failure.reason,
            });
            if let Some(error_kind) = failure.error_kind {
                data["errorKind"] = serde_json::json!(error_kind);
            }
            if let Some(status) = failure.status {
                data["status"] = serde_json::json!(status);
            }
            log(
                &format!("{AUTO_CAPTURE_FINAL_ERROR} after {attempts_after} attempts"),
                Some(&data),
            );
            EntryOutcome::Dead
        } else {
            EntryOutcome::Retryable
        }
    }

    /// Dead-letters an entry IMMEDIATELY for a payload the retry ladder could
    /// never fix (an invalid raw context or an unknown agent role).
    ///
    /// `reason` is one of the fixed message constants — never content, never
    /// the rejected value.
    async fn dead_letter_now(
        &self,
        entry: &mem_episodic::inbox::InboxEntry,
        reason: &'static str,
    ) -> EntryOutcome {
        let _ = self.inbox.dead_letter(&entry.capture_id).await;
        crate::engine::structured_stderr(
            "capture.dead_lettered",
            "warn",
            "capture",
            serde_json::json!({
                "capture_id": entry.capture_id,
                "project_id": entry.project_id,
                "reason": reason,
            }),
        );
        log(
            AUTO_CAPTURE_FINAL_ERROR,
            Some(&serde_json::json!({
                "captureId": &entry.capture_id,
                "projectId": &entry.project_id,
                "reason": reason,
            })),
        );
        EntryOutcome::Dead
    }

    /// Resolves the W7.1 retention payload BEFORE the summarizer runs.
    ///
    /// Rules:
    /// - the agent role (when supplied) is validated against the closed
    ///   ADR-0009 set and persisted regardless of `soundMemory` (the v6
    ///   column exists either way);
    /// - with `soundMemory` OFF nothing raw is retained;
    /// - with it ON, the supplied `raw_context` wins, else the transcript is
    ///   the fallback; a payload with no raw text is dead-lettered when
    ///   `rawContextRequired`, and skipped otherwise;
    /// - the bytes are privacy-stripped before they can reach the store.
    fn resolve_retention(
        &self,
        params: &CaptureParams,
        transcript: Option<&str>,
    ) -> Result<RetentionWrite, &'static str> {
        let agent_role = parse_capture_agent_role(params.agent_role.as_deref())?;
        if !self.sound_memory.enabled {
            return Ok(RetentionWrite {
                raw_context: None,
                agent_role,
            });
        }
        // D-52-3 site 4: strip and filter the SUPPLIED raw context only, before
        // its own blank guard. The strip is load-bearing — filtering first
        // would delete a `<private>` opener that meets a marker region; the
        // strip inside `resolve_retention` stays untouched and is a no-op on
        // the already-stripped value. The owned `String` is bound first so
        // `supplied` keeps `Option<&str>` (a one-step `.map(..).filter(..)`
        // would not compile against `supplied.or(transcript)`).
        let filtered_supplied = params
            .raw_context
            .as_deref()
            .map(|raw| self.privacy.strip_then_filter_text(raw));
        let supplied = filtered_supplied
            .as_deref()
            .filter(|raw| !raw.trim().is_empty());
        let candidate = supplied.or(transcript).filter(|raw| !raw.trim().is_empty());
        let Some(raw) = candidate else {
            if self.sound_memory.raw_context_required {
                return Err(RAW_CONTEXT_REQUIRED_MESSAGE);
            }
            return Ok(RetentionWrite {
                raw_context: None,
                agent_role,
            });
        };
        if raw.len() > MAX_RAW_CONTEXT_BYTES || raw.contains('\u{0}') {
            return Err(RAW_CONTEXT_INVALID_MESSAGE);
        }
        let stripped = mem_ai::privacy::strip_private_content(raw);
        if stripped.trim().is_empty() {
            if self.sound_memory.raw_context_required {
                return Err(RAW_CONTEXT_REQUIRED_MESSAGE);
            }
            return Ok(RetentionWrite {
                raw_context: None,
                agent_role,
            });
        }
        Ok(RetentionWrite {
            raw_context: Some(stripped),
            agent_role,
        })
    }

    /// Commit 1 of the two-tier drain: resolve the parent (WIRE session id →
    /// project-visible ROW id) and upsert the staging row, then write the
    /// W7.1 retention rows (raw context + segments) and the agent role.
    async fn commit_episodic(
        &self,
        params: &CaptureParams,
        summary: &str,
        tags: &[String],
        extension: &CaptureExtension,
        retention: &RetentionWrite,
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
            .await?;

        // Retention after the durable upsert: a failed upsert writes no
        // retention rows, and a retention failure consumes an attempt while
        // the retry's `AlreadyPresent` converges instead of duplicating.
        if let Some(raw_context) = &retention.raw_context {
            self.store
                .insert_raw_context(
                    &params.project_id,
                    &params.session_id,
                    &params.agent_id,
                    raw_context,
                )
                .await?;
        }
        if let Some(role) = retention.agent_role {
            self.store
                .set_session_agent_role(
                    &params.project_id,
                    &params.session_id,
                    &params.agent_id,
                    role.as_str(),
                )
                .await?;
        }
        Ok(())
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
            &self.privacy,
        ) {
            Some(context) => context,
            None => return Ok(CaptureSummaryOutput::Skip(SkipReason::NoContext)),
        };
        // D-52-3 site 5, assembled half: filter AFTER the builder's
        // pre-existing strip (which runs inside `auto_capture_context`), so a
        // marker region that meets a `<private>` opener cannot delete the
        // opener before the stripper sees it. Only the BUILT context is
        // filtered here: the transcript fallback was already filtered by site
        // 1 (`process_entry`), and filtering it again would leave test 8 green
        // with site 1 deleted.
        let context = if metadata_user_prompt(&params.metadata).is_some() {
            self.privacy.filter_injected_text(&context)
        } else {
            context
        };
        match self.summarizer.as_ref() {
            Some(summarizer) => summarizer.summarize(&context).await,
            None => Err(EpisodicError::Io {
                message: "no capture summarizer configured".to_string(),
            }),
        }
    }
}

/// The named cause of one consumed capture failure.
///
/// `reason` is one of the fixed capture reasons (or a host-resolution
/// reason carried through the summarizer); `error_kind`/`status` are present
/// only when a provider answered — never free text, never prompt content.
#[derive(Debug, Clone, Copy)]
struct Failure {
    reason: &'static str,
    error_kind: Option<&'static str>,
    status: Option<u16>,
}

impl Failure {
    const fn of(reason: &'static str) -> Self {
        Self {
            reason,
            error_kind: None,
            status: None,
        }
    }
}

/// One `Auto-capture provider failed` line per summarizer failure (every
/// attempt, not only the terminal one).
fn log_provider_failed(entry: &mem_episodic::inbox::InboxEntry, failure: Failure) {
    let mut data = serde_json::json!({
        "captureId": &entry.capture_id,
        "reason": failure.reason,
    });
    if let Some(error_kind) = failure.error_kind {
        data["errorKind"] = serde_json::json!(error_kind);
    }
    if let Some(status) = failure.status {
        data["status"] = serde_json::json!(status);
    }
    log(AUTO_CAPTURE_PROVIDER_FAILED, Some(&data));
}

/// The `detail` cap, in characters (never bytes: a byte slice panics on a
/// non-char boundary, and a panicking drain loop is worse than the defect).
const EPISODIC_DETAIL_CHARS: usize = 120;

fn truncate_detail(message: &str) -> String {
    message.chars().take(EPISODIC_DETAIL_CHARS).collect()
}

/// The closed `errorKind` map; `EpisodicError` is `#[non_exhaustive]`, so the
/// wildcard arm is mandatory.
fn error_kind_label(error: &EpisodicError) -> &'static str {
    match error {
        EpisodicError::Validation { .. } => "validation",
        EpisodicError::Store(_) => "store",
        EpisodicError::Embed(_) => "embed",
        EpisodicError::Conflict => "conflict",
        EpisodicError::NotFound { .. } => "not_found",
        EpisodicError::Json { .. } => "json",
        EpisodicError::Io { .. } => "io",
        EpisodicError::Summary { .. } => "summary",
        EpisodicError::CrossTier { .. } => "cross_tier",
        _ => "other",
    }
}

/// One `Auto-capture episodic write failed` line per failed `commit_episodic`
/// attempt. Only a validation rejection contributes `field` and `detail`: the
/// other variants' messages are not content-audited.
fn log_episodic_write_failed(entry: &mem_episodic::inbox::InboxEntry, error: &EpisodicError) {
    let mut data = serde_json::json!({
        "captureId": &entry.capture_id,
        "projectId": &entry.project_id,
        "sessionId": &entry.session_id,
        "agentId": &entry.agent_id,
        "errorKind": error_kind_label(error),
    });
    if let EpisodicError::Validation { field, message } = error {
        data["field"] = serde_json::json!(field);
        data["detail"] = serde_json::json!(truncate_detail(message));
    }
    log(AUTO_CAPTURE_EPISODIC_FAILED, Some(&data));
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
///
/// D-52-3 site 5: the prompt is **stripped and** filtered before
/// `build_markdown_context` (a region cut in half by the truncation would no
/// longer have a closer, and filtering before stripping would delete a
/// `<private>` opener that meets a marker region). The pre-existing strip
/// follows; `assemble_summary` filters the built context **after** this
/// function returns (`strip → filter`, covering `textResponses`/`toolCalls`,
/// which are host text too), and the transcript fallback is not filtered again
/// because site 1 already did. A present `userPrompt` wins even when blank: no
/// transcript fallback is introduced (r4 item 9).
fn auto_capture_context(
    metadata: &Option<Json>,
    transcript: Option<&str>,
    max_context_bytes: usize,
    _latest: LatestMemoryReader,
    privacy: &mem_ai::privacy::PrivacyConfig,
) -> Option<String> {
    let user_prompt = metadata_user_prompt(metadata);
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
        // Site 5's blank eligibility: after stripping and filtering, a present-
        // and-blank prompt with no responses and no tool calls has no context
        // at all. A non-blank transcript beside it is NOT a fallback — the
        // same branch, not a different one.
        //
        // The prompt filter runs BEFORE the builder so truncation cannot cut a
        // region in half; it strips first because filtering before the strip
        // would delete a `<private>` opener that meets a marker region.
        let prompt = privacy.strip_then_filter_text(prompt);
        if prompt.trim().is_empty() && text_responses.is_empty() && tool_calls.is_empty() {
            return None;
        }
        let latest_memory: Option<&str> = metadata
            .as_ref()
            .and_then(|m| m.get("autoCaptureContext"))
            .and_then(|ctx| ctx.get("latestMemory"))
            .and_then(Json::as_str);
        let context = crate::capture_context::build_markdown_context(
            &prompt,
            &text_responses,
            &tool_calls,
            latest_memory,
            max_context_bytes,
        );
        // The strip stays here, byte-identical (52.8): the assembled filter
        // runs in `assemble_summary` AFTER this returns (strip -> filter), so
        // a marker region that meets a `<private>` opener cannot delete the
        // opener before the stripper sees it.
        return Some(mem_ai::privacy::strip_private_content(&context));
    }

    // Transcript fallback: the caller already filtered it (site 1).
    transcript.map(|text| text.to_string())
}

/// The `metadata.autoCaptureContext.userPrompt` reader shared by the filter
/// sites, so the preference is single-sourced.
fn metadata_user_prompt(metadata: &Option<Json>) -> Option<&str> {
    metadata
        .as_ref()?
        .get("autoCaptureContext")?
        .get("userPrompt")?
        .as_str()
}

/// The prompt the summarizer will actually be handed: the metadata prompt
/// when present, else the transcript.
fn effective_prompt<'a>(params: &'a CaptureParams, transcript: Option<&'a str>) -> Option<&'a str> {
    metadata_user_prompt(&params.metadata).or(transcript)
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

#[cfg(test)]
mod tests {
    use super::truncate_detail;

    /// The 120th byte lands inside `é`: a byte slice panics, a char take does not.
    #[test]
    fn test_detail_truncation_never_splits_a_char() {
        let message = "a".repeat(119) + "é" + &"b".repeat(20);
        let truncated = truncate_detail(&message);
        assert_eq!(truncated.chars().count(), 120);
        assert!(truncated.ends_with('é'));
    }
}
