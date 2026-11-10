//! Promoted facts — the W4.3 selection rule and the permanent-memory sink.
//!
//! # The selection rule (D-28)
//!
//! [`select_promotable_facts`] is PURE (no I/O, no clock, no randomness) and
//! deterministic:
//!
//! 1. the candidate list is built in FIRST-APPEARANCE order — for each row
//!    (input order) and each decision of that row (decision order), the
//!    content is privacy-stripped and trimmed, a blank content is SKIPPED,
//!    and the candidate id is `"{row_id}#{decision_index}"`;
//! 2. the candidates are grouped by normalized key (lowercased, whitespace
//!    runs collapsed), preserving the FIRST-APPEARANCE order of the keys;
//! 3. **EVERY** group yields exactly one fact — a SINGLETON group (one
//!    member) is promoted directly, because `mem_core::dedup::dedup_groups`
//!    skips groups with fewer than two members (`dedup.rs:99`) and would
//!    therefore drop the normal case (a decision stated in one session);
//! 4. a MULTI-MEMBER group is handed to `dedup_groups` as
//!    `DedupCandidate { id, key, embedding: None }`: all members share one
//!    key and carry no embedding, so the call returns exactly one
//!    `DedupKind::Exact` group whose member ids are the group's member ids
//!    in first-appearance order;
//! 5. the fact's `content` is the FIRST member's content, its
//!    `source_row_ids` are the DISTINCT row ids parsed from the member ids,
//!    and its `id` is `"episodic-decision-"` plus the first 32 hex
//!    characters of `sha256(project_id + "\u{1f}" + key)` — deterministic, so
//!    promotion stays idempotent across runs.

use std::sync::Arc;

use mem_core::dedup::{DedupCandidate, DedupKind, DedupOptions, dedup_groups};
use mem_core::scope::is_valid_scope_hash;
use mem_store::error::StoreError;
use mem_store::scope::Scope;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{MemoryInsert, insert_vector_handle};

use crate::error::EpisodicError;
use crate::record::AgentSession;
use crate::summarizer::stripped_source_summary;

/// The prefix of a promoted fact's row id.
pub const PROMOTED_FACT_ID_PREFIX: &str = "episodic-decision-";

/// The `memory_type` of a promoted fact (a free-form TEXT column; this repo
/// has no registry or enum for it).
pub const PROMOTED_MEMORY_TYPE: &str = "decision";

/// One promotable fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotableFact {
    /// The deterministic row id (`episodic-decision-<32 hex>`).
    pub id: String,
    /// The fact text (privacy-stripped, trimmed).
    pub content: String,
    /// The normalized dedup key (lowercased, whitespace collapsed).
    pub key: String,
    /// The distinct source row ids, in first-appearance order.
    pub source_row_ids: Vec<String>,
}

/// One candidate decision inside the selection.
struct Candidate {
    id: String,
    content: String,
    key: String,
}

/// Normalizes a decision into its dedup key: lowercased with whitespace runs
/// collapsed to a single space.
///
/// W7.1: ONE definition (`mem_core::dedup::normalize_key`) serves promotion,
/// the facts tier and the cross-tier task — a local copy would let the tiers
/// disagree about identity.
fn normalize_key(content: &str) -> String {
    mem_core::dedup::normalize_key(content)
}

/// The deterministic id of a promoted fact.
///
/// `sha256(project_id + "\u{1f}" + key)`, truncated to 32 hex characters —
/// the fact id is stable across runs, so a re-run finds the existing row
/// instead of inserting a duplicate.
pub fn promoted_fact_id(project_id: &str, key: &str) -> String {
    let digest = mem_core::sha256::digest(format!("{project_id}\u{1f}{key}").as_bytes());
    let hex = mem_core::sha256::hex(&digest);
    format!("{PROMOTED_FACT_ID_PREFIX}{}", &hex[..32])
}

/// Extracts the source row id from a candidate id.
///
/// **ASSUMPTION:** the source row id contains NO `#`. The candidate id is
/// `"{row_id}#{decision_index}"`, so the row id is everything before the
/// FIRST `#`; a row id that itself contained a `#` would be truncated at
/// that first `#`. Episodic row ids are UUIDv7 values minted by the store,
/// so they never contain a `#`.
fn source_row_id(candidate_id: &str) -> &str {
    candidate_id
        .split_once('#')
        .map(|(row_id, _)| row_id)
        .unwrap_or(candidate_id)
}

/// Selects the facts to promote from `rows`.
///
/// `rows` are the CONSOLIDATED rows (the prefix the consolidator actually
/// merged) — never the whole claimed set.
pub fn select_promotable_facts(project_id: &str, rows: &[AgentSession]) -> Vec<PromotableFact> {
    // 1. The candidate list, in first-appearance order.
    let mut candidates: Vec<Candidate> = Vec::new();
    for row in rows {
        for (decision_index, decision) in row.metadata.decisions.iter().enumerate() {
            let content = stripped_source_summary(&decision.description);
            if content.is_empty() {
                continue;
            }
            candidates.push(Candidate {
                id: format!("{}#{decision_index}", row.id),
                key: normalize_key(&content),
                content,
            });
        }
    }

    // 2. Group by key, preserving the first-appearance order of the keys.
    let mut groups: Vec<(String, Vec<Candidate>)> = Vec::new();
    for candidate in candidates {
        match groups.iter_mut().find(|(key, _)| *key == candidate.key) {
            Some((_, members)) => members.push(candidate),
            None => groups.push((candidate.key.clone(), vec![candidate])),
        }
    }

    // 3-5. One fact per group, in group order.
    let mut facts = Vec::with_capacity(groups.len());
    for (key, members) in groups {
        let member_ids = group_member_ids(&members);
        let mut source_row_ids: Vec<String> = Vec::new();
        for member_id in &member_ids {
            let row_id = source_row_id(member_id);
            if !source_row_ids.iter().any(|existing| existing == row_id) {
                source_row_ids.push(row_id.to_string());
            }
        }
        facts.push(PromotableFact {
            id: promoted_fact_id(project_id, &key),
            content: members
                .first()
                .map(|member| member.content.clone())
                .unwrap_or_default(),
            key,
            source_row_ids,
        });
    }
    facts
}

/// The member ids of one group, in first-appearance order.
///
/// A single-member group is returned as-is. A multi-member group is handed
/// to [`dedup_groups`]: all members share one key and carry no embedding, so
/// the call returns exactly ONE `DedupKind::Exact` group whose member ids
/// are the group's member ids in first-appearance order. The fallback is
/// unreachable (the exact-group rule above guarantees the group) and exists
/// only to keep the function total.
fn group_member_ids(members: &[Candidate]) -> Vec<String> {
    if members.len() < 2 {
        return members.iter().map(|member| member.id.clone()).collect();
    }
    let candidates: Vec<DedupCandidate<'_>> = members
        .iter()
        .map(|member| DedupCandidate {
            id: &member.id,
            key: &member.key,
            embedding: None,
        })
        .collect();
    let groups = dedup_groups(&candidates, &DedupOptions::default());
    match groups
        .into_iter()
        .find(|group| group.kind == DedupKind::Exact)
    {
        Some(group) => group.ids,
        None => members.iter().map(|member| member.id.clone()).collect(),
    }
}

// ---------------------------------------------------------------------------
// The promotion sink (Step 6)
// ---------------------------------------------------------------------------

/// One record handed to the permanent-memory sink.
#[derive(Debug, Clone, PartialEq)]
pub struct PromotionRecord {
    /// The deterministic row id.
    pub id: String,
    /// The fact text.
    pub content: String,
    /// The embedding of the fact.
    pub vector: Vec<f32>,
    /// The caller's container tag (project scope).
    pub container_tag: String,
    /// The memory type (`decision`).
    pub memory_type: String,
    /// The creation stamp.
    pub created_at: i64,
    /// The source row ids the fact came from.
    pub source_row_ids: Vec<String>,
}

/// What the sink did with a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteOutcome {
    /// The row was inserted.
    Inserted,
    /// A row with the same id already existed (nothing was written).
    AlreadyPresent,
}

/// The permanent-memory sink port.
///
/// The scope (`scope_hash`) and the container tag come from the CALLER — the
/// consolidator never derives them from the episodic `project_id`.
pub trait PermanentMemorySink: Send + Sync {
    /// Promotes one record into the permanent memory tier.
    fn promote<'a>(
        &'a self,
        scope_hash: &'a str,
        record: &'a PromotionRecord,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<PromoteOutcome, EpisodicError>>;
}

/// The seam that makes the shard's ABSOLUTE count resync testable (W3).
pub trait VectorCountSync: Send + Sync {
    /// Re-measures the shard's rows and writes the absolute count into both
    /// authorities.
    fn resync<'a>(
        &'a self,
        shard_id: i64,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<(), StoreError>>;
}

/// The production resync: `shard_stats` measured by `COUNT(*)`, then the
/// registry cache adopted from the shard's own value.
pub struct ShardVectorCountSync {
    shards: Arc<ShardManager>,
}

impl ShardVectorCountSync {
    /// The resync over `shards`.
    pub fn new(shards: Arc<ShardManager>) -> Self {
        Self { shards }
    }
}

impl VectorCountSync for ShardVectorCountSync {
    fn resync<'a>(
        &'a self,
        shard_id: i64,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            let shard = self
                .shards
                .get_shard_by_id(shard_id)
                .await?
                .ok_or_else(|| {
                    StoreError::internal(format!("Shard {shard_id} no longer exists"))
                })?;
            let db = self.shards.open_shard(&shard.db_path).await?;
            let count = mem_store::shard_facts::sync_count(db.as_ref()).await?;
            self.shards.set_vector_count(shard_id, count).await
        })
    }
}

/// The production sink: a project-scoped memory shard.
pub struct ShardPermanentMemorySink {
    shards: Arc<ShardManager>,
    dimensions: u32,
    counts: Arc<dyn VectorCountSync>,
}

impl ShardPermanentMemorySink {
    /// The sink over `shards`, resyncing through the shard manager.
    pub fn new(shards: Arc<ShardManager>, dimensions: u32) -> Self {
        Self::with_vector_sync(
            shards.clone(),
            dimensions,
            Arc::new(ShardVectorCountSync::new(shards)),
        )
    }

    /// The sink over `shards` with an injected resync (the test seam).
    pub fn with_vector_sync(
        shards: Arc<ShardManager>,
        dimensions: u32,
        counts: Arc<dyn VectorCountSync>,
    ) -> Self {
        Self {
            shards,
            dimensions,
            counts,
        }
    }
}

impl PermanentMemorySink for ShardPermanentMemorySink {
    fn promote<'a>(
        &'a self,
        scope_hash: &'a str,
        record: &'a PromotionRecord,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<PromoteOutcome, EpisodicError>> {
        Box::pin(async move {
            // (1) The scope hash is validated before anything is touched (pure
            // and cheap, so it stays OUTSIDE the scope lock).
            if !is_valid_scope_hash(scope_hash) {
                return Err(EpisodicError::validation(
                    "scope_hash",
                    "must be 16 lowercase hex characters",
                ));
            }
            // (2) The embedding must match the store's dimensions (pure; also
            // outside the lock).
            if record.vector.len() != self.dimensions as usize {
                return Err(EpisodicError::validation(
                    "embedding",
                    format!(
                        "expected {} dimensions, got {}",
                        self.dimensions,
                        record.vector.len()
                    ),
                ));
            }

            // (3)-(7) under the SAME per-scope write lock capture takes.
            //
            // (3) is load-bearing: `get_write_shard` may mark the full shard
            // read-only (registry UPDATE), sync its count (`SET_VECTOR_COUNT`)
            // and create the next shard (new file + registry INSERT). With
            // resolution OUTSIDE the lock, another writer of the scope could
            // retire the resolved shard between resolution and insert, so the
            // row would land in a retired shard.
            self.shards
                .with_scope_write_lock(Scope::Project, scope_hash, move || {
                    Box::pin(async move {
                        let body = async move {
                            // (3) The write shard for the caller's scope.
                            //
                            // `get_write_shard` refuses to resolve a shard while a live
                            // operation lock exists. The consolidation run HOLDS that lock
                            // itself for its whole duration (ADR-0006 D-25), so the lock it
                            // sees is its own — the lock-tolerant path resolves the shard
                            // then. A lock held by ANOTHER process still fails the
                            // promotion (the run aborts).
                            let info = match self
                                .shards
                                .get_write_shard(Scope::Project, scope_hash)
                                .await
                            {
                                Ok(info) => info,
                                Err(error) if is_own_operation_lock(&error) => {
                                    self.shards
                                        .get_write_shard_allowing_own_lock(
                                            Scope::Project,
                                            scope_hash,
                                        )
                                        .await?
                                }
                                Err(error) => return Err(EpisodicError::Store(error)),
                            };
                            let db = self.shards.open_shard(&info.db_path).await?;

                            // (4) Existence check across EVERY shard of the scope
                            // (active and retired): an existing row is never
                            // re-inserted and never bumps the count. The `?` is the
                            // fail-closed mapping for EVERY variant —
                            // `From<StoreError> for EpisodicError` makes it
                            // `EpisodicError::Store`, which aborts the run; the
                            // scheduler re-ticks (`engine.rs:1307-1320`), so the abort
                            // is genuinely retried. Promotion is step (e) and the
                            // episodic write batch is step (f), so an abort here has
                            // written nothing; `select_promotable_facts` is
                            // deterministic (`project_id`, normalized key), so the
                            // retry re-derives the same fact.
                            if self
                                .shards
                                .find_memory_in_scope(Scope::Project, scope_hash, &record.id)
                                .await?
                                .is_some()
                            {
                                return Ok(PromoteOutcome::AlreadyPresent);
                            }

                            // (5) The insert.
                            let metadata = promotion_metadata(record);
                            let insert = MemoryInsert {
                                id: record.id.clone(),
                                content: record.content.clone(),
                                vector: record.vector.clone(),
                                tags_vector: None,
                                container_tag: record.container_tag.clone(),
                                tags: None,
                                memory_type: Some(record.memory_type.clone()),
                                created_at: record.created_at,
                                updated_at: record.created_at,
                                metadata: Some(metadata),
                                session_id: None,
                                display_name: None,
                                user_name: None,
                                user_email: None,
                                project_path: None,
                                project_name: None,
                                git_repo_url: None,
                                source_prompt_created_at: None,
                            };
                            // The unique-violation fallback below is the second
                            // line of defence behind the seam, and it works ONLY
                            // because this insert is a single `db.execute`
                            // (`vector.rs:142-146`), not a `BatchMode::Write`
                            // batch: a constraint raised inside a batch surfaces
                            // as `BatchStatementFailed` with `is_constraint ==
                            // false` (W0 AC7), which this classifier cannot see.
                            // W2 must not move this insert into a batch without
                            // re-checking the classifier. (Capture has no such
                            // fallback — its only duplicate defence is the seam.)
                            match insert_vector_handle(&db, &insert).await {
                                Ok(()) => {}
                                Err(error) if is_unique_violation(&error) => {
                                    // A concurrent writer inserted the same deterministic id.
                                    return Ok(PromoteOutcome::AlreadyPresent);
                                }
                                Err(error) => return Err(EpisodicError::Store(error)),
                            }

                            // (6) The absolute count resync — exactly once, on the
                            // insert path only. The insert above stays a single
                            // `execute` (the W0 classifier depends on it).
                            self.counts.resync(info.id).await?;

                            // (7) Inserted.
                            Ok::<PromoteOutcome, EpisodicError>(PromoteOutcome::Inserted)
                        };
                        Ok::<_, StoreError>(body.await)
                    })
                })
                .await
                .map_err(EpisodicError::Store)?
        })
    }
}

/// The metadata JSON of a promoted row.
fn promotion_metadata(record: &PromotionRecord) -> String {
    let source_row_ids = mem_store::serde_json::to_string(&record.source_row_ids)
        .unwrap_or_else(|_| "[]".to_string());
    format!("{{\"source\":\"episodic-consolidation\",\"sourceRowIds\":{source_row_ids}}}")
}

/// Whether the error is an operation lock held by THIS process.
fn is_own_operation_lock(error: &StoreError) -> bool {
    let StoreError::MigrationLocked { pid, .. } = error else {
        return false;
    };
    *pid == std::process::id()
}

/// Structured-first constraint classification: `StoreError::Turso
/// { is_constraint: true, .. }` gates the branch, and only then does the
/// lowercased message select the *kind* (`"unique"`).
fn is_unique_violation(error: &StoreError) -> bool {
    let mem_store::error::StoreError::Turso {
        is_constraint: true,
        ..
    } = error
    else {
        return false;
    };
    error.message().to_ascii_lowercase().contains("unique")
}

// ---------------------------------------------------------------------------
// Crate-internal tests (private items only — `docs/agents/testing.md`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The own-lock bypass has TWO independent guards: this crate's
    /// [`is_own_operation_lock`] and mem-store's
    /// `get_write_shard_allowing_own_lock` (which re-reads the live lock and
    /// refuses a foreign pid). Either guard alone is sufficient, so either
    /// one can be broken without the other noticing — this test pins THIS
    /// crate's decision point, the mem-store one is pinned by
    /// `crates/mem-store/tests/turso_shard_rotation.rs`.
    #[test]
    fn test_is_own_operation_lock_distinguishes_our_pid_from_a_foreign_pid() {
        let ours = StoreError::MigrationLocked {
            pid: std::process::id(),
            operation: Some("episodic-consolidation".to_string()),
        };
        let foreign = StoreError::MigrationLocked {
            pid: 4242,
            operation: Some("foreign-migration".to_string()),
        };
        assert!(is_own_operation_lock(&ours), "our own lock is ours");
        assert!(
            !is_own_operation_lock(&foreign),
            "a lock held by another process is never ours"
        );
        assert!(
            !is_own_operation_lock(&StoreError::internal("nope")),
            "only a MigrationLocked error can be an operation lock"
        );
    }
}
