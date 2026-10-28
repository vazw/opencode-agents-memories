//! Shard facts (W2): the per-shard `shard_stats` row and the embedding
//! identity fingerprint stored in `shard_metadata`.
//!
//! W2 **records**; W3 verifies. Nothing here is consulted by a read path.

use crate::db::{Db, Statement, Value, query_one};
use crate::error::{ShardModelMismatch, ShardModelMismatchEntry, StoreError};
use crate::schema_migrations::{MEMORY_SHARD_MAX_KNOWN, MigrationOptions, apply_schema_migrations};
use crate::sql;
use mem_core::embedding::EmbeddingIdentity;
use mem_core::scope::Scope;

/// The one conditional claim statement (AC1).
///
/// Three clauses, each load-bearing:
/// - `INSERT OR IGNORE` turns a lost race (two writers that both passed the
///   `NOT EXISTS` read) into a no-op instead of a failed user write — the
///   primary key is `shard_metadata.key`;
/// - `NOT EXISTS (… key = 'embedding_fingerprint')` makes a present key
///   immutable: a second claim with a different value changes nothing;
/// - `NOT EXISTS (SELECT 1 FROM memories)` restricts the claim to an empty
///   shard, the only state where the vectors' provenance is provable.
///
/// Non-vacuity: a bare `INSERT OR IGNORE … VALUES` fails arm (c) of
/// `test_conditional_claim_inserts_once_and_only_on_an_empty_shard` — it
/// writes the row on a populated shard.
pub const CLAIM_IDENTITY_SQL: &str = "INSERT OR IGNORE INTO shard_metadata (key, value) SELECT 'embedding_fingerprint', ?1 WHERE NOT EXISTS (SELECT 1 FROM shard_metadata WHERE key = 'embedding_fingerprint') AND NOT EXISTS (SELECT 1 FROM memories)";

/// The `shard_stats` table: one row, `id` pinned to 1, an absolute count.
///
/// No row is written by the v3 migration. A `0` written before the count is
/// known would be a W7 reaping trigger, so the invariant is: the row exists
/// only with a value measured by `COUNT(*)` in the statement that writes it.
pub const SHARD_STATS_DDL: &str = "CREATE TABLE IF NOT EXISTS shard_stats (id INTEGER PRIMARY KEY CHECK (id = 1), vector_count INTEGER NOT NULL CHECK (vector_count >= 0))";

/// Absolute upsert (the shape of `REGISTER_EXISTING_SHARD`). Every writer
/// writes `COUNT(*)` **measured by the statement itself**, so a lost or
/// skipped write is repaired by the next one and no caller can pass a count
/// the table contradicts (the re-embed batch includes this statement after
/// its inserts).
pub const SYNC_COUNT_SQL: &str = "INSERT INTO shard_stats (id, vector_count) SELECT 1, COUNT(*) FROM memories ON CONFLICT(id) DO UPDATE SET vector_count = excluded.vector_count";

/// The count read-back (after [`SYNC_COUNT_SQL`]).
pub const SELECT_VECTOR_COUNT: &str = "SELECT vector_count FROM shard_stats WHERE id = 1";

/// The absolute count as a batch statement (no binds — the statement measures
/// `COUNT(*)` itself, so no caller can pass a count the table contradicts).
pub fn sync_count_statement() -> Statement {
    Statement::new(SYNC_COUNT_SQL, Vec::new())
}

/// Reads the committed `shard_stats` value (`0` when the row is absent).
///
/// A read-back only, never a fallback authority: the caller has already
/// measured `COUNT(*)` in the statement that wrote the row.
pub async fn read_stored_count(db: &dyn Db) -> Result<u64, StoreError> {
    let row = query_one(db, SELECT_VECTOR_COUNT, Vec::new()).await?;
    let count = row
        .and_then(|row| row.i64("vector_count"))
        .unwrap_or(0)
        .max(0);
    Ok(u64::try_from(count).unwrap_or(u64::MAX))
}

/// The fingerprint read (before any claim attempt).
pub const SELECT_FINGERPRINT: &str =
    "SELECT value FROM shard_metadata WHERE key = 'embedding_fingerprint'";

/// What [`prepare_active_shard`] decided about the fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintStep {
    /// This call inserted the fingerprint.
    Claimed,
    /// A fingerprint was already present (including a lost race: the claim's
    /// `changes = 0`).
    AlreadyPresent,
    /// No identity was configured on the manager.
    WithheldNoIdentity,
    /// The identity disagrees with the configured dimensions/model.
    WithheldInconsistent,
    /// The fingerprint could not be serialized; nothing was written.
    WithheldEncoding,
    /// The shard is non-empty and has no fingerprint: the build cannot prove
    /// provenance, so it records nothing.
    WithheldNonEmpty { rows: u64 },
}

/// The result of the write-path shard-facts seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardFactsOutcome {
    pub step: FingerprintStep,
    pub vector_count: u64,
}

/// Applies the idempotent v1..v3 shard schema (no backup: the v3 step is
/// additive and convergent, ADR-0018 D-14). No `db_path` is supplied, so no
/// backup — and therefore no exclusivity proof — can arise (issue 46).
pub async fn ensure_v3(db: &dyn Db, dimensions: u32) -> Result<i64, StoreError> {
    apply_schema_migrations(
        db,
        &crate::schema_migrations::memory_shard_migrations(dimensions),
        &MigrationOptions {
            db_path: None,
            label: "memory-shard",
            max_known: MEMORY_SHARD_MAX_KNOWN,
        },
        None,
    )
    .await
}

/// `COUNT(*)` as `u64` (clamped like the existing registry sync).
pub async fn read_count(db: &dyn Db) -> Result<u64, StoreError> {
    let row = query_one(db, sql::COUNT_ALL_VECTORS, Vec::new()).await?;
    let count = row.and_then(|row| row.i64("count")).unwrap_or(0).max(0);
    Ok(u64::try_from(count).unwrap_or(u64::MAX))
}

/// Writes the statement-measured `COUNT(*)` into `shard_stats` and returns
/// the value read back from the table (never a caller-supplied number).
pub async fn sync_count(db: &dyn Db) -> Result<u64, StoreError> {
    db.execute(SYNC_COUNT_SQL, Vec::new()).await?;
    read_stored_count(db).await
}

/// Reads the stored fingerprint (`None` when the key is absent).
pub async fn read_fingerprint(db: &dyn Db) -> Result<Option<String>, StoreError> {
    let row = query_one(db, SELECT_FINGERPRINT, Vec::new()).await?;
    Ok(row.and_then(|row| row.text("value")))
}

/// The claim as a batch statement (the re-embed vector batch's first entry).
pub fn claim_statement(fingerprint: &str) -> Statement {
    Statement::new(
        CLAIM_IDENTITY_SQL,
        vec![Value::Text(fingerprint.to_string())],
    )
}

/// Executes the claim and reports what the statement did.
///
/// `changes = 0` after a clean read is only a lost race: another writer
/// claimed between the read and the insert.
pub async fn claim_identity(db: &dyn Db, fingerprint: &str) -> Result<FingerprintStep, StoreError> {
    let result = db
        .execute(
            CLAIM_IDENTITY_SQL,
            vec![Value::Text(fingerprint.to_string())],
        )
        .await?;
    if result.changes == 0 {
        Ok(FingerprintStep::AlreadyPresent)
    } else {
        Ok(FingerprintStep::Claimed)
    }
}

/// The pre-write fingerprint decision: the configured identity, the shard's
/// dimensions/model, and the fallible encoding.
///
/// `WithheldInconsistent` when the identity disagrees with the configured
/// dimensions/model — the guard that stops a hand-mutated config from writing
/// a fingerprint naming a model the config never had. `WithheldEncoding` when
/// `fingerprint()` returns `Err`: no empty string is ever written (an empty
/// fingerprint is unequal forever and would force a re-embed on every open).
pub fn claimable_fingerprint(
    identity: Option<&mem_core::embedding::EmbeddingIdentity>,
    dimensions: u32,
    model: &str,
) -> Result<String, FingerprintStep> {
    let Some(identity) = identity else {
        return Err(FingerprintStep::WithheldNoIdentity);
    };
    if identity.dimensions != dimensions || identity.model != model {
        return Err(FingerprintStep::WithheldInconsistent);
    }
    match identity.fingerprint() {
        Ok(fingerprint) => Ok(fingerprint),
        Err(error) => {
            tracing::error!(error = %error, "embedding fingerprint encoding failed");
            Err(FingerprintStep::WithheldEncoding)
        }
    }
}

/// The registry context a write refusal names. The facts live in the shard
/// DB; the registry id and the remedy's scope triple do not.
#[derive(Debug, Clone, Copy)]
pub struct ShardMismatchContext<'a> {
    /// The registry row id of the shard being written.
    pub shard_id: i64,
    /// The configured container-tag prefix (`config.container_tag_prefix`).
    pub container_tag_prefix: &'a str,
    /// The scope the shard belongs to.
    pub scope: Scope,
    /// The scope hash the shard belongs to.
    pub scope_hash: &'a str,
}

/// The outcome of the write-path seam: the judgement plus W2's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardWriteFacts {
    /// What the stored identity proved (or failed to prove).
    pub authority: ModelAuthority,
    /// The W2 fingerprint/count outcome.
    pub facts: ShardFactsOutcome,
}

/// The composed write-path shard-facts seam.
///
/// Order is load-bearing: the idempotent v1..v3 migration first (`ensure_v3`),
/// then ONE identity-facts read, then the judgement — a positive mismatch
/// refuses **before** the count body runs, so the shard is never mutated by a
/// refused write. W2's claim/count body then runs with the facts already read
/// (no second identity statement).
///
/// Concurrent writers cannot straddle the pair: the read is one statement and
/// the claim is conditional (`INSERT OR IGNORE`), so a present key always
/// wins.
pub async fn prepare_write_shard(
    db: &dyn Db,
    dimensions: u32,
    identity: Option<&EmbeddingIdentity>,
    model: &str,
    mismatch: ShardMismatchContext<'_>,
) -> Result<ShardWriteFacts, StoreError> {
    prepare_shard_facts(db, dimensions, identity, model, Some(mismatch)).await
}

/// The single inner body shared by [`prepare_active_shard`] and
/// [`prepare_write_shard`].
async fn prepare_shard_facts(
    db: &dyn Db,
    dimensions: u32,
    identity: Option<&EmbeddingIdentity>,
    model: &str,
    mismatch: Option<ShardMismatchContext<'_>>,
) -> Result<ShardWriteFacts, StoreError> {
    ensure_v3(db, dimensions).await?;
    let facts = read_model_facts(db).await?;
    let authority = judge_model_facts(&facts, identity, model);
    if let (ModelAuthority::Mismatch { stored_model }, Some(context)) = (&authority, mismatch) {
        return Err(model_mismatch_error(
            vec![ShardModelMismatchEntry {
                id: context.shard_id,
                stored_model: stored_model.clone(),
                expected: model.to_string(),
            }],
            context.container_tag_prefix,
            context.scope,
            context.scope_hash,
        ));
    }
    let vector_count = sync_count(db).await?;
    let step = if facts.fingerprint.is_some() {
        FingerprintStep::AlreadyPresent
    } else if vector_count > 0 {
        tracing::info!(
            rows = vector_count,
            dimensions,
            "shard facts: embedding fingerprint withheld on a non-empty shard"
        );
        FingerprintStep::WithheldNonEmpty { rows: vector_count }
    } else {
        match claimable_fingerprint(identity, dimensions, model) {
            Ok(fingerprint) => claim_identity(db, &fingerprint).await?,
            Err(step) => step,
        }
    };
    Ok(ShardWriteFacts {
        authority,
        facts: ShardFactsOutcome { step, vector_count },
    })
}

/// The W2 low-level seam: judges the facts but never refuses (it has no
/// registry id to name), so the claim/count behaviour W2 pinned is unchanged.
///
/// The write refusal lives in [`prepare_write_shard`].
pub async fn prepare_active_shard(
    db: &dyn Db,
    dimensions: u32,
    identity: Option<&EmbeddingIdentity>,
    model: &str,
) -> Result<ShardFactsOutcome, StoreError> {
    prepare_shard_facts(db, dimensions, identity, model, None)
        .await
        .map(|write| write.facts)
}

// ---------------------------------------------------------------------------
// W3 — the model authority
// ---------------------------------------------------------------------------

/// The one identity-facts read (W3): both stored identity keys in a single
/// statement, so a concurrent writer cannot straddle the pair.
pub const SELECT_MODEL_FACTS: &str = "SELECT key, value FROM shard_metadata WHERE key IN ('embedding_fingerprint','embedding_model')";

/// The operator command that re-embeds one scope. The single source of the
/// remedy: the store error, the HTTP body and the operator log all read it.
pub const REEMBED_SCOPE_COMMAND: &str = "opencode-agents-memory --mode=reembed-scope --scope";

/// The two stored identity facts, read together.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelFacts {
    /// The stored `embedding_fingerprint`, when the shard proves its identity.
    pub fingerprint: Option<String>,
    /// The stored `embedding_model`, when the shard names one.
    pub stored_model: Option<String>,
}

/// The pure judgement over [`ModelFacts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelAuthority {
    /// The stored fingerprint equals the configured identity's fingerprint:
    /// the strongest proof a shard can carry.
    Fingerprint,
    /// No fingerprint, no configured identity, and the stored model is absent
    /// or equal to the configured one — a warning, not proof (R-1).
    ModelEqualityNoIdentity { stored_model: Option<String> },
    /// No fingerprint, and the stored model is absent or equal to the
    /// configured one — legacy, warned about, allowed (R-1).
    ModelEquality { stored_model: Option<String> },
    /// Positive evidence that the shard belongs to another model.
    Mismatch { stored_model: Option<String> },
}

/// Reads both identity facts in ONE statement (never two round trips: the pair
/// could straddle a concurrent write).
pub async fn read_model_facts(db: &dyn Db) -> Result<ModelFacts, StoreError> {
    let rows = db.query(SELECT_MODEL_FACTS, Vec::new()).await?;
    let mut facts = ModelFacts::default();
    for row in rows {
        let Some(key) = row.text("key") else {
            continue;
        };
        let value = row.text("value");
        match key.as_str() {
            "embedding_fingerprint" => facts.fingerprint = value,
            "embedding_model" => facts.stored_model = value,
            _ => {}
        }
    }
    Ok(facts)
}

/// The pure, total judgement of a shard's stored identity.
///
/// The fingerprint wins when one is present and the build carries an
/// identity; a build without one still compares the MODEL (it cannot check a
/// fingerprint, but it is not blind to a named foreign model). An absent
/// fingerprint falls back to model equality — a warning, never proof (R-1).
pub fn judge_model_facts(
    facts: &ModelFacts,
    identity: Option<&EmbeddingIdentity>,
    configured_model: &str,
) -> ModelAuthority {
    if let Some(stored_fingerprint) = facts.fingerprint.as_deref() {
        if let Some(identity) = identity {
            let matches = identity
                .fingerprint()
                .is_ok_and(|configured| configured == stored_fingerprint);
            return if matches {
                ModelAuthority::Fingerprint
            } else {
                ModelAuthority::Mismatch {
                    stored_model: facts.stored_model.clone(),
                }
            };
        }
        return match facts.stored_model.as_deref() {
            Some(stored) if stored != configured_model => ModelAuthority::Mismatch {
                stored_model: facts.stored_model.clone(),
            },
            _ => ModelAuthority::ModelEqualityNoIdentity {
                stored_model: facts.stored_model.clone(),
            },
        };
    }
    match facts.stored_model.as_deref() {
        Some(stored) if stored != configured_model => ModelAuthority::Mismatch {
            stored_model: facts.stored_model.clone(),
        },
        stored => ModelAuthority::ModelEquality {
            stored_model: stored.map(str::to_string),
        },
    }
}

/// The operator remedy for one scope: the exact command that re-embeds it.
pub fn reembed_remedy(container_tag_prefix: &str, scope: Scope, scope_hash: &str) -> String {
    format!(
        "{REEMBED_SCOPE_COMMAND} {container_tag_prefix}_{}_{scope_hash}",
        scope.as_str()
    )
}

/// The refusal payload naming every mismatched shard of one scope.
pub fn model_mismatch(
    entries: Vec<ShardModelMismatchEntry>,
    container_tag_prefix: &str,
    scope: Scope,
    scope_hash: &str,
) -> ShardModelMismatch {
    ShardModelMismatch {
        shards: entries,
        remedy: reembed_remedy(container_tag_prefix, scope, scope_hash),
    }
}

/// The store-layer refusal for the write path.
pub fn model_mismatch_error(
    entries: Vec<ShardModelMismatchEntry>,
    container_tag_prefix: &str,
    scope: Scope,
    scope_hash: &str,
) -> StoreError {
    StoreError::ShardModelMismatch(Box::new(model_mismatch(
        entries,
        container_tag_prefix,
        scope,
        scope_hash,
    )))
}
