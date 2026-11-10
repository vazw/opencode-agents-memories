//! Migration service (blueprint W1.3 step 7 — sB2 owns this file).
//!
//! Port of upstream `src/services/migration-service.ts`: the dims
//! preflight (`DimensionMismatch`), the two strategies (`fresh-start`
//! archives mismatched shards; `re-embed` rebuilds each shard into a
//! staged database, verifies the staged count, then performs the
//! backup/swap rename sequence), and the operation-lock + `is_running`
//! guards around them. Progress callbacks are N-A this wave (recorded;
//! the API wave owns them).
//!
//! `mem-store` stays embed-free: the concrete adapter (mem-embed) lands in
//! a later wave — enforced by gate `! cargo tree -p mem-store | grep -q mem-embed`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;

use crate::config::StoreConfig;
use crate::connection::ConnectionManager;
use crate::db::{BatchMode, BoxFuture, DbHandle, Statement, Value, query_one};
use crate::error::StoreError;
use crate::operation_lock::acquire_turso_operation_lock;
use crate::ready::ReadyGate;
use crate::scope::Scope;
use crate::shard_manager::ShardManager;
use crate::sql;
use crate::sqlite_files::{
    FILE_LOCK_RETRY_DELAYS_MS, RetryContext, remove_sqlite_database, rename_sqlite_database,
    with_sqlite_file_lock_retry,
};
use crate::vector::{MemoryInsert, count_all_vectors, insert_vector_statement};
use crate::vector_utils::format_tags_for_embedding;

/// Upstream `embeddingService` seam (injected): warmup, cache clear, embed.
/// Object-safe (`BoxFuture` returns) so `Arc<dyn ReEmbedder>` works.
pub trait ReEmbedder: Send + Sync {
    /// Upstream `embeddingService.warmup()`.
    fn warmup(&self) -> BoxFuture<'_, Result<(), StoreError>>;
    /// Upstream `embeddingService.clearCache()` — default no-op.
    fn clear_cache(&self) {}
    /// Upstream `embeddingService.embedWithTimeout(input, { task })`.
    fn embed(&self, input: &str) -> BoxFuture<'_, Result<Vec<f32>, StoreError>>;
}

/// Upstream `DimensionMismatch` interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionMismatch {
    pub needs_migration: bool,
    pub config_dimensions: u32,
    pub config_model: String,
    pub shard_mismatches: Vec<ShardMismatch>,
}

/// Upstream `DimensionMismatch.shardMismatches` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardMismatch {
    pub shard_id: i64,
    pub db_path: PathBuf,
    pub stored_dimensions: i64,
    pub stored_model: String,
    pub vector_count: u64,
}

/// A shard whose stored embedding identity does not match the configured one
/// (W2: model-only changes with equal dimensions are detectable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardEmbeddingMismatch {
    pub shard_id: i64,
    pub db_path: PathBuf,
    pub scope: Scope,
    pub scope_hash: String,
    pub stored_fingerprint: Option<String>,
    pub stored_model: Option<String>,
}

/// Upstream `"fresh-start" | "re-embed"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStrategy {
    FreshStart,
    ReEmbed,
}

impl MigrationStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            MigrationStrategy::FreshStart => "fresh-start",
            MigrationStrategy::ReEmbed => "re-embed",
        }
    }
}

/// Upstream `MigrationResult` (`duration` renames to `duration_ms`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationResult {
    pub success: bool,
    pub strategy: String,
    pub deleted_shards: u64,
    pub re_embedded_memories: u64,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// The on-disk re-embed swap journal (camelCase, byte-shaped like the
/// TS-written `JSON.stringify({ dbPath, stagedPath, backupPath })`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SwapState<'a> {
    db_path: &'a str,
    staged_path: &'a str,
    backup_path: &'a str,
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn internal(message: impl Into<String>) -> StoreError {
    StoreError::Internal {
        message: message.into(),
    }
}

/// Upstream `MigrationService` singleton (instantiated per wiring here;
/// `getStatus` is N-A this wave — recorded).
pub struct MigrationService {
    config: StoreConfig,
    ready: Arc<ReadyGate>,
    shards: Arc<ShardManager>,
    connections: Arc<ConnectionManager>,
    re_embedder: Arc<dyn ReEmbedder>,
    is_running: AtomicBool,
}

impl MigrationService {
    pub fn new(
        config: StoreConfig,
        ready: Arc<ReadyGate>,
        shards: Arc<ShardManager>,
        connections: Arc<ConnectionManager>,
        re_embedder: Arc<dyn ReEmbedder>,
    ) -> Self {
        Self {
            config,
            ready,
            shards,
            connections,
            re_embedder,
            is_running: AtomicBool::new(false),
        }
    }

    /// Upstream `detectDimensionMismatch`: ready gate, user+project shard
    /// sweep, per-shard dims/model probe; per-shard errors warn and skip.
    pub async fn detect_dimension_mismatch(&self) -> Result<DimensionMismatch, StoreError> {
        self.ready.ensure_ready().await?;
        let mut shards = self.shards.get_all_shards(Scope::User, None).await?;
        shards.extend(self.shards.get_all_shards(Scope::Project, None).await?);

        let mut shard_mismatches = Vec::new();
        for shard in &shards {
            match self.shard_mismatch(shard).await {
                Ok(Some(mismatch)) => shard_mismatches.push(mismatch),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        shard_id = shard.id,
                        error = %error.message(),
                        "Migration: error checking shard"
                    );
                }
            }
        }

        Ok(DimensionMismatch {
            needs_migration: !shard_mismatches.is_empty(),
            config_dimensions: self.config.embedding_dimensions,
            config_model: self.config.embedding_model.clone(),
            shard_mismatches,
        })
    }

    /// Read one `shard_metadata` value (missing row → `None`; missing
    /// table → `Err`, which upstream catches per shard).
    async fn metadata_value(&self, db: &DbHandle, key: &str) -> Result<Option<String>, StoreError> {
        let row = query_one(
            &**db,
            sql::GET_SHARD_METADATA_VALUE,
            vec![Value::Text(key.to_string())],
        )
        .await?;
        Ok(row.and_then(|row| row.text("value")))
    }

    /// Upstream per-shard probe: dims/model defaults `0`/`unknown`; the
    /// mismatch record carries the current vector count.
    async fn shard_mismatch(
        &self,
        shard: &crate::shard_manager::ShardInfo,
    ) -> Result<Option<ShardMismatch>, StoreError> {
        let db_path = &shard.db_path;
        let db = self.connections.get_connection(db_path).await?;
        let stored_dimensions = self
            .metadata_value(&db, "embedding_dimensions")
            .await?
            .unwrap_or_default()
            .parse::<i64>()
            .unwrap_or(0);
        let stored_model = self
            .metadata_value(&db, "embedding_model")
            .await?
            .unwrap_or_else(|| "unknown".to_string());
        if stored_dimensions == i64::from(self.config.embedding_dimensions) {
            return Ok(None);
        }
        let vector_count = count_all_vectors(&*db).await;
        Ok(Some(ShardMismatch {
            shard_id: shard.id,
            db_path: db_path.clone(),
            stored_dimensions,
            stored_model,
            vector_count,
        }))
    }

    /// Detection of a **model-only** change (equal dimensions): the three-case
    /// rule of ADR-0018.
    ///
    /// - a present fingerprint mismatches iff it differs from the configured
    ///   one;
    /// - an absent fingerprint with a different stored `embedding_model` is a
    ///   mismatch (a pre-W2 shard that named another model);
    /// - an absent fingerprint with an absent or equal model is **not** a
    ///   mismatch (legacy, assumed matching; model equality is not proof —
    ///   residual R-3).
    ///
    /// Callers must not invoke this on a manager without an embedding
    /// identity: it returns an error by design. `migrate_dispatch` unions it
    /// only when `ShardManager::embedding_identity()` is `Some`; the explicit
    /// refusal (AC14) belongs to `reembed_scope`.
    pub async fn detect_embedding_mismatch(
        &self,
    ) -> Result<Vec<ShardEmbeddingMismatch>, StoreError> {
        self.ready.ensure_ready().await?;
        let identity = self.shards.embedding_identity().ok_or_else(|| {
            internal("Embedding identity is not configured; cannot detect a model change")
        })?;
        let configured = identity.fingerprint().map_err(|error| StoreError::Json {
            message: error.to_string(),
        })?;
        let configured_model = identity.model.as_str();

        let mut shards = self.shards.get_all_shards(Scope::User, None).await?;
        shards.extend(self.shards.get_all_shards(Scope::Project, None).await?);

        let mut mismatches = Vec::new();
        for shard in &shards {
            match self
                .shard_embedding_mismatch(shard, &configured, configured_model)
                .await
            {
                Ok(Some(mismatch)) => mismatches.push(mismatch),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        shard_id = shard.id,
                        error = %error.message(),
                        "Migration: error checking shard embedding identity"
                    );
                }
            }
        }
        Ok(mismatches)
    }

    /// One shard's identity probe (a per-shard failure is the caller's to
    /// swallow, like `detect_dimension_mismatch`).
    async fn shard_embedding_mismatch(
        &self,
        shard: &crate::shard_manager::ShardInfo,
        configured_fingerprint: &str,
        configured_model: &str,
    ) -> Result<Option<ShardEmbeddingMismatch>, StoreError> {
        let db = self.connections.get_connection(&shard.db_path).await?;
        let stored_fingerprint = self.metadata_value(&db, "embedding_fingerprint").await?;
        let stored_model = self.metadata_value(&db, "embedding_model").await?;
        let mismatch = match &stored_fingerprint {
            Some(fingerprint) => fingerprint != configured_fingerprint,
            None => stored_model
                .as_deref()
                .is_some_and(|model| model != configured_model),
        };
        if !mismatch {
            return Ok(None);
        }
        Ok(Some(ShardEmbeddingMismatch {
            shard_id: shard.id,
            db_path: shard.db_path.clone(),
            scope: shard.scope,
            scope_hash: shard.scope_hash.clone(),
            stored_fingerprint,
            stored_model,
        }))
    }

    /// One scope's supported way out of a model change (ADR-0018 R-12).
    ///
    /// Reuses the guard chain of `migrate_to_new_model` — `is_running` swap,
    /// the operation lock, the per-shard write lock, the staged rebuild /
    /// verify / swap — and returns the same typed-outcome channel: a refusal
    /// is `success: false` + `error`.
    ///
    /// A scope whose shards ALL carry the configured fingerprint short-circuits
    /// **before** staging: `success: true`, zero re-embedded memories, no
    /// embed call, no `.reembed-*.tmp`. The predicate is "fingerprint present
    /// and equal", never `detect_embedding_mismatch().is_empty()` — a
    /// populated pre-W2 shard has no fingerprint and an equal model, so
    /// identity detection selects nothing, and it must still be rewritten.
    pub async fn reembed_scope(
        &self,
        scope: Scope,
        scope_hash: &str,
    ) -> Result<MigrationResult, StoreError> {
        let started = Instant::now();
        if self.is_running.swap(true, Ordering::SeqCst) {
            return Ok(MigrationResult {
                success: false,
                strategy: "re-embed".to_string(),
                deleted_shards: 0,
                re_embedded_memories: 0,
                duration_ms: 0,
                error: Some("Migration already running".to_string()),
            });
        }
        let outcome = self
            .reembed_scope_with_lock(scope, scope_hash, started)
            .await;
        self.is_running.store(false, Ordering::SeqCst);
        Ok(match outcome {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(error = %error.message(), "Migration: scope re-embed failed");
                MigrationResult {
                    success: false,
                    strategy: "re-embed".to_string(),
                    deleted_shards: 0,
                    re_embedded_memories: 0,
                    duration_ms: elapsed_ms(started),
                    error: Some(error.message()),
                }
            }
        })
    }

    /// The lock guard of [`reembed_scope`] (released whether the body throws
    /// or not, like `migrate_with_lock`).
    async fn reembed_scope_with_lock(
        &self,
        scope: Scope,
        scope_hash: &str,
        started: Instant,
    ) -> Result<MigrationResult, StoreError> {
        let identity = self
            .shards
            .embedding_identity()
            .ok_or_else(|| internal("Re-embed requires a configured embedding identity"))?;
        let configured = identity.fingerprint().map_err(|error| StoreError::Json {
            message: error.to_string(),
        })?;
        let operation = format!("re-embed-{}-{scope_hash}", scope.as_str());
        let operation_lock = acquire_turso_operation_lock(
            &self.config.storage_path,
            self.shards.process_probe(),
            &operation,
        )?;
        let outcome = self
            .reembed_scope_body(scope, scope_hash, &configured, started)
            .await;
        operation_lock.release();
        outcome
    }

    async fn reembed_scope_body(
        &self,
        scope: Scope,
        scope_hash: &str,
        configured_fingerprint: &str,
        started: Instant,
    ) -> Result<MigrationResult, StoreError> {
        self.ready.ensure_ready().await?;
        let shards = self.shards.get_all_shards(scope, Some(scope_hash)).await?;
        let mut work = Vec::new();
        for shard in &shards {
            let db = self.connections.get_connection(&shard.db_path).await?;
            let fingerprint = self.metadata_value(&db, "embedding_fingerprint").await?;
            if fingerprint.as_deref() != Some(configured_fingerprint) {
                work.push(shard.id);
            }
        }
        let re_embedded_memories = if work.is_empty() {
            0
        } else {
            self.reembed_shards(&work).await?
        };
        Ok(MigrationResult {
            success: true,
            strategy: "re-embed".to_string(),
            deleted_shards: 0,
            re_embedded_memories,
            duration_ms: elapsed_ms(started),
            error: None,
        })
    }

    /// The per-shard rebuild loop shared by `migrate_to_new_model` and
    /// `reembed_scope`: warm up once, then stage/verify/swap each shard under
    /// its scope write lock.
    async fn reembed_shards(&self, shard_ids: &[i64]) -> Result<u64, StoreError> {
        self.re_embedder.warmup().await?;
        self.re_embedder.clear_cache();

        let mut re_embedded_memories = 0u64;
        for shard_id in shard_ids {
            let shard = self
                .shards
                .get_shard_by_id(*shard_id)
                .await?
                .ok_or_else(|| {
                    internal(format!(
                        "Migration source shard {shard_id} no longer exists"
                    ))
                })?;
            let id = *shard_id;
            let migrated = {
                let config = self.config.clone();
                let ready = Arc::clone(&self.ready);
                let shards = Arc::clone(&self.shards);
                let connections = Arc::clone(&self.connections);
                let re_embedder = Arc::clone(&self.re_embedder);
                let scope = shard.scope;
                let scope_hash = shard.scope_hash.clone();
                self.shards
                    .with_scope_write_lock(scope, &scope_hash, move || {
                        Box::pin(Self::rebuild_shard_safely(
                            config,
                            ready,
                            shards,
                            connections,
                            re_embedder,
                            id,
                        ))
                    })
                    .await?
            };
            re_embedded_memories += migrated;
        }
        Ok(re_embedded_memories)
    }

    /// Upstream `migrateToNewModel`: the `isRunning` re-entrancy guard
    /// throws; every error inside the try becomes a `success: false`
    /// result; the operation lock and flag always release.
    pub async fn migrate_to_new_model(
        &self,
        strategy: MigrationStrategy,
    ) -> Result<MigrationResult, StoreError> {
        if self.is_running.swap(true, Ordering::SeqCst) {
            return Err(internal("Migration already running"));
        }
        let result = self.migrate_catch_all(strategy).await;
        self.is_running.store(false, Ordering::SeqCst);
        result
    }

    async fn migrate_catch_all(
        &self,
        strategy: MigrationStrategy,
    ) -> Result<MigrationResult, StoreError> {
        let started = Instant::now();
        let outcome = self.migrate_with_lock(strategy, started).await;
        match outcome {
            Ok(result) => Ok(result),
            Err(error) => {
                tracing::warn!(error = %error.message(), "Migration: failed");
                Ok(MigrationResult {
                    success: false,
                    strategy: strategy.as_str().to_string(),
                    deleted_shards: 0,
                    re_embedded_memories: 0,
                    duration_ms: elapsed_ms(started),
                    error: Some(error.message()),
                })
            }
        }
    }

    async fn migrate_with_lock(
        &self,
        strategy: MigrationStrategy,
        started: Instant,
    ) -> Result<MigrationResult, StoreError> {
        let operation = format!("dimension-{}", strategy.as_str());
        let operation_lock = acquire_turso_operation_lock(
            &self.config.storage_path,
            self.shards.process_probe(),
            &operation,
        )?;
        let outcome = self.migrate_dispatch(strategy, started).await;
        // Upstream finally: release runs whether or not the body threw.
        operation_lock.release();
        outcome
    }

    async fn migrate_dispatch(
        &self,
        strategy: MigrationStrategy,
        started: Instant,
    ) -> Result<MigrationResult, StoreError> {
        let mismatch = self.detect_dimension_mismatch().await?;
        // `ReEmbed` unions identity mismatches (a model change with equal
        // dimensions); `FreshStart` stays dimension-only, so a model rename
        // can never silently archive a scope (ADR-0018 D-11). The union is
        // gated on a configured identity: a manager without one keeps the
        // legal pre-W2 path (re-embed a dimension mismatch), and
        // `detect_embedding_mismatch` is never invoked on it.
        let identity_mismatches = match strategy {
            MigrationStrategy::ReEmbed if self.shards.embedding_identity().is_some() => {
                self.detect_embedding_mismatch().await?
            }
            _ => Vec::new(),
        };
        if !mismatch.needs_migration && identity_mismatches.is_empty() {
            return Ok(MigrationResult {
                success: true,
                strategy: strategy.as_str().to_string(),
                deleted_shards: 0,
                re_embedded_memories: 0,
                duration_ms: elapsed_ms(started),
                error: None,
            });
        }
        match strategy {
            MigrationStrategy::FreshStart => self.fresh_start_migration(&mismatch, started).await,
            MigrationStrategy::ReEmbed => {
                let mut ids: Vec<i64> = mismatch
                    .shard_mismatches
                    .iter()
                    .map(|mismatch| mismatch.shard_id)
                    .collect();
                for identity_mismatch in &identity_mismatches {
                    if !ids.contains(&identity_mismatch.shard_id) {
                        ids.push(identity_mismatch.shard_id);
                    }
                }
                self.re_embed_migration(&ids, started).await
            }
        }
    }

    /// Upstream `freshStartMigration`: archive each mismatched shard.
    async fn fresh_start_migration(
        &self,
        mismatch: &DimensionMismatch,
        started: Instant,
    ) -> Result<MigrationResult, StoreError> {
        let mut deleted_shards = 0u64;
        for shard_info in &mismatch.shard_mismatches {
            let archive_path = self
                .shards
                .archive_shard(shard_info.shard_id, "fresh-start", None)
                .await?;
            if archive_path.is_none() {
                return Err(internal(format!(
                    "Migration source shard {} no longer exists",
                    shard_info.shard_id
                )));
            }
            tracing::info!(
                shard_id = shard_info.shard_id,
                archive_path = %archive_path.unwrap_or_default().display(),
                "Migration: archived shard for fresh start"
            );
            deleted_shards += 1;
        }
        Ok(MigrationResult {
            success: true,
            strategy: "fresh-start".to_string(),
            deleted_shards,
            re_embedded_memories: 0,
            duration_ms: elapsed_ms(started),
            error: None,
        })
    }

    /// Upstream `reEmbedMigration`: rebuild every selected shard under its
    /// scope write lock (the loop itself lives in `reembed_shards`).
    async fn re_embed_migration(
        &self,
        shard_ids: &[i64],
        started: Instant,
    ) -> Result<MigrationResult, StoreError> {
        let re_embedded_memories = self.reembed_shards(shard_ids).await?;
        Ok(MigrationResult {
            success: true,
            strategy: "re-embed".to_string(),
            deleted_shards: 0,
            re_embedded_memories,
            duration_ms: elapsed_ms(started),
            error: None,
        })
    }

    /// Upstream `rebuildShardSafely`: stage every memory into a sibling
    /// database, verify the count, then journal + rename-swap. On ANY
    /// error the staged file and journal are cleaned up and the source is
    /// left untouched; on a failed staged→db rename the backup is restored
    /// before the error rethrows.
    #[allow(clippy::too_many_arguments)]
    async fn rebuild_shard_safely(
        config: StoreConfig,
        _ready: Arc<ReadyGate>,
        shards: Arc<ShardManager>,
        connections: Arc<ConnectionManager>,
        re_embedder: Arc<dyn ReEmbedder>,
        shard_id: i64,
    ) -> Result<u64, StoreError> {
        let shard = shards.get_shard_by_id(shard_id).await?.ok_or_else(|| {
            internal(format!(
                "Migration source shard {shard_id} no longer exists"
            ))
        })?;
        let source = connections.get_connection(&shard.db_path).await?;
        let memories = source.query(sql::SELECT_REEMBED_SOURCE, Vec::new()).await?;
        drop(source);

        let nonce = format!("{}-{}", std::process::id(), now_millis());
        let db_path = shard.db_path.clone();
        let staged_path = PathBuf::from(format!("{}.reembed-{nonce}.tmp", db_path.display()));
        let backup_path = PathBuf::from(format!("{}.pre-reembed-{nonce}.bak", db_path.display()));
        let swap_state_path = PathBuf::from(format!("{}.reembed-swap.json", db_path.display()));

        let outcome = Self::stage_and_swap(
            &config,
            &shards,
            &connections,
            re_embedder.as_ref(),
            shard_id,
            &db_path,
            &staged_path,
            &backup_path,
            &swap_state_path,
            &memories,
        )
        .await;

        match outcome {
            Ok(count) => Ok(count),
            Err(error) => {
                // Upstream catch: close + remove the staged shard, drop the
                // journal only while the (untouched) source still exists.
                connections.close_connection(&staged_path).await?;
                if staged_path.exists() {
                    let Some(proof) = connections.exclusive_root() else {
                        return Err(StoreError::NoExclusiveRoot {
                            db_path: staged_path.clone(),
                        });
                    };
                    let claim = proof.claim(&staged_path)?;
                    let ctx = RetryContext::system();
                    with_sqlite_file_lock_retry(
                        || remove_sqlite_database(claim),
                        FILE_LOCK_RETRY_DELAYS_MS.len(),
                        &ctx,
                    )?;
                }
                if swap_state_path.exists() && db_path.exists() {
                    std::fs::remove_file(&swap_state_path)?;
                }
                Err(error)
            }
        }
    }

    /// The staging + verified swap body (upstream try block).
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    async fn stage_and_swap(
        config: &StoreConfig,
        shards: &Arc<ShardManager>,
        connections: &Arc<ConnectionManager>,
        re_embedder: &dyn ReEmbedder,
        shard_id: i64,
        db_path: &Path,
        staged_path: &Path,
        backup_path: &Path,
        swap_state_path: &Path,
        memories: &[crate::db::Row],
    ) -> Result<u64, StoreError> {
        let expected = memories.len();
        let staged = connections.get_connection(staged_path).await?;
        shards
            .init_shard_db(
                &*staged,
                config.embedding_dimensions,
                &config.embedding_model,
            )
            .await?;

        // Upstream batches of 50: statement-list grouping only — the port
        // commits one transactional batch (recorded divergence, same as the
        // legacy migrator).
        //
        // The vector batch carries the shard facts (ADR-0018 D-4): the claim
        // is FIRST (the emptiness clause must still see an empty `memories`
        // table inside the transaction) and the absolute count is LAST, so a
        // crash can never leave new vectors under an old identity or a count
        // that disagrees with them.
        let mut statements = Vec::with_capacity(memories.len() * 2 + 2);
        if let Some(identity) = shards.embedding_identity() {
            match identity.fingerprint() {
                Ok(fingerprint) => {
                    statements.push(crate::shard_facts::claim_statement(&fingerprint));
                }
                Err(error) => {
                    tracing::error!(error = %error, "embedding fingerprint encoding failed");
                }
            }
        }
        for memory in memories {
            let id = memory.text("id").unwrap_or_default();
            let content = memory.text("content").unwrap_or_default();
            let tags: Vec<String> = memory
                .text("tags")
                .map(|tags| {
                    tags.split(',')
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let embedding_input = if tags.is_empty() {
                content.clone()
            } else {
                format!("{content}\nTags: {}", tags.join(", "))
            };
            let vector = re_embedder.embed(&embedding_input).await?;
            let tags_vector = if tags.is_empty() {
                None
            } else {
                Some(re_embedder.embed(&format_tags_for_embedding(&tags)).await?)
            };
            let record = MemoryInsert {
                id: id.clone(),
                content,
                vector,
                tags_vector,
                container_tag: memory.text("container_tag").unwrap_or_default(),
                tags: memory.text("tags").filter(|tags| !tags.is_empty()),
                memory_type: memory.text("type").filter(|kind| !kind.is_empty()),
                created_at: memory.i64("created_at").unwrap_or(0),
                updated_at: memory.i64("updated_at").unwrap_or(0),
                metadata: memory.text("metadata").filter(|meta| !meta.is_empty()),
                session_id: memory.text("session_id").filter(|id| !id.is_empty()),
                display_name: memory.text("display_name").filter(|name| !name.is_empty()),
                user_name: memory.text("user_name").filter(|name| !name.is_empty()),
                user_email: memory.text("user_email").filter(|email| !email.is_empty()),
                project_path: memory.text("project_path").filter(|path| !path.is_empty()),
                project_name: memory.text("project_name").filter(|name| !name.is_empty()),
                git_repo_url: memory.text("git_repo_url").filter(|url| !url.is_empty()),
                source_prompt_created_at: None,
            };
            statements.push(insert_vector_statement(&record)?);
            if memory.i64("is_pinned").unwrap_or(0) == 1 {
                statements.push(Statement::new(sql::PIN_MEMORY, vec![Value::Text(id)]));
            }
        }
        // The sync statement measures `COUNT(*)` inside the same transaction,
        // so it sees exactly the rows this batch inserted; the post-batch
        // verification independently proves the selection/insertion contract.
        statements.push(Statement::new(
            crate::shard_facts::SYNC_COUNT_SQL,
            Vec::new(),
        ));
        staged.batch(statements, BatchMode::Write).await?;

        let staged_count = count_all_vectors(&*staged).await;
        if staged_count != expected as u64 {
            return Err(internal(format!(
                "Migration staged count mismatch for shard {shard_id}: expected {expected}, got {staged_count}"
            )));
        }

        // Verified swap: journal first, source out, replacement in.
        connections.close_connection(staged_path).await?;
        // Issue 43 W6a: every open is the legacy mode now, so this process's own
        // handle owns the staged file's `F_SETLK` lock. The Arc must be released
        // before the swap's exclusivity probe runs — otherwise the probe answers
        // `Held` against our own descriptor (OFD locks are per open file
        // description) and the mutation is refused.
        drop(staged);
        let journal = SwapState {
            db_path: &db_path.to_string_lossy(),
            staged_path: &staged_path.to_string_lossy(),
            backup_path: &backup_path.to_string_lossy(),
        };
        let body = serde_json::to_vec(&journal).map_err(|error| StoreError::Json {
            message: error.to_string(),
        })?;
        std::fs::write(swap_state_path, body)?;
        // The close invalidates the identity-facts cache (R-13), so the next
        // read through a fresh handle sees the swapped file.
        connections.close_connection(db_path).await?;

        let ctx = RetryContext::system();
        let Some(proof) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot {
                db_path: db_path.to_path_buf(),
            });
        };
        let db_claim = proof.claim(db_path)?;
        let backup_claim = proof.claim(backup_path)?;
        let staged_claim = proof.claim(staged_path)?;
        with_sqlite_file_lock_retry(
            || rename_sqlite_database(db_claim, backup_claim),
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &ctx,
        )?;
        if let Err(error) = with_sqlite_file_lock_retry(
            || rename_sqlite_database(staged_claim, db_claim),
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &ctx,
        ) {
            // Restore the source before rethrowing (upstream rethrows the
            // ORIGINAL rename error, not the restore outcome).
            with_sqlite_file_lock_retry(
                || rename_sqlite_database(backup_claim, db_claim),
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &ctx,
            )?;
            return Err(error);
        }

        shards.set_vector_count(shard_id, staged_count).await?;
        std::fs::remove_file(swap_state_path)?;
        tracing::info!(
            shard_id,
            memories = staged_count,
            backup_path = %backup_path.display(),
            "Migration: safely replaced re-embedded shard"
        );
        Ok(staged_count)
    }
}
