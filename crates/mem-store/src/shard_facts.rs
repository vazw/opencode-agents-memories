//! Shard facts (W2): the per-shard `shard_stats` row and the embedding
//! identity fingerprint stored in `shard_metadata`.
//!
//! W2 **records**; W3 verifies. Nothing here is consulted by a read path.

use crate::db::{Db, Statement, Value, query_one};
use crate::error::StoreError;
use crate::schema_migrations::{MEMORY_SHARD_MAX_KNOWN, MigrationOptions, apply_schema_migrations};
use crate::sql;

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
    let row = query_one(db, SELECT_VECTOR_COUNT, Vec::new()).await?;
    let count = row
        .and_then(|row| row.i64("vector_count"))
        .unwrap_or(0)
        .max(0);
    Ok(u64::try_from(count).unwrap_or(u64::MAX))
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

/// The composed write-path shard-facts seam.
///
/// Order is load-bearing: the idempotent v1..v3 migration first (`ensure_v3`),
/// then the fingerprint READ (before any claim attempt, so a present key on a
/// non-empty shard is `AlreadyPresent`, not `WithheldNonEmpty`), then the
/// measured count, then the precedence:
///
/// 1. key present -> `AlreadyPresent`, even on a non-empty shard;
/// 2. key absent + rows > 0 -> `WithheldNonEmpty { rows }` (provenance is
///    unprovable; a value written here would be a lie);
/// 3. key absent + rows = 0 -> claim; the claim's `changes = 0` is only a lost
///    race and is reported as `AlreadyPresent`.
///
/// The measured count is always returned, whatever the fingerprint decision
/// was.
pub async fn prepare_active_shard(
    db: &dyn Db,
    dimensions: u32,
    identity: Option<&mem_core::embedding::EmbeddingIdentity>,
    model: &str,
) -> Result<ShardFactsOutcome, StoreError> {
    ensure_v3(db, dimensions).await?;
    let existing = read_fingerprint(db).await?;
    let vector_count = sync_count(db).await?;
    let step = if existing.is_some() {
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
    Ok(ShardFactsOutcome { step, vector_count })
}
