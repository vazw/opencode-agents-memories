//! Turso shard manager — port of upstream
//! `src/services/turso/shard-manager.ts` (blueprint W1.2 step 8).
//!
//! Upstream behaviors preserved: the registry (`metadata.db`) is the SINGLE
//! writer of shard lifecycle; shard FILES are initialized before the
//! registry row insert (orphan-row guard comment preserved); unique-
//! violation dedup in `create_shard`; incompatible/corrupt shards REJECT
//! writes with the original database left untouched (`ShardIncompatible`).
//!
//! W4 (ADR-0022) replaces the rotation: `rotate_shard` retires the previous
//! shard and inserts the next in ONE transaction (the shipped two-step could
//! leave ZERO actives and hand the dedup an inactive row), a zero-active
//! tuple reactivates its highest row through the normal write validation,
//! and a second activation is `ActiveShardConflict`.
//!
//! Divergences (reviewer notes): `CONFIG.storagePath` globals become the
//! `storage_path` field; `metadata: Mutex<Option<DbHandle>>` +
//! `OnceCell` init cell preserve upstream `ensureInitialized` (in-flight
//! dedup + revalidation: every call re-asks the connection manager; a
//! swapped handle is adopted and re-initialized) without holding std
//! guards across `.await`.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use mem_core::scope::{Scope, assert_safe_scope_hash};

use crate::config::StoreConfig;
use crate::connection::ConnectionManager;
use crate::db::{BatchMode, BoxFuture, Db, DbHandle, Row, Statement, Value, query_one};
use crate::error::StoreError;
use crate::operation_lock::assert_no_turso_migration_in_progress;
use crate::platform::ProcessProbe;
use crate::sqlite_files::{
    FILE_LOCK_RETRY_DELAYS_MS, rename_sqlite_database, with_sqlite_file_lock_retry,
};

/// Upstream `rowToShardInfo` target shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardInfo {
    pub id: i64,
    pub scope: Scope,
    pub scope_hash: String,
    pub shard_index: u32,
    pub db_path: PathBuf,
    pub vector_count: u64,
    pub is_active: bool,
    pub created_at: i64,
}

/// The outcome of a shard-level retention sweep pass (issue 39).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SweepOutcome {
    pub deleted_count: u64,
    pub user_count: u64,
    pub project_count: u64,
    pub protected_memories_skipped: u64,
    pub deleted_ids: Vec<String>,
}

/// Later-wave seam (W1.3 search fan-out, inventory, migrations).
pub trait ShardRegistry: Send + Sync {
    fn get_all_shards<'a>(
        &'a self,
        scope: Scope,
        scope_hash: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<ShardInfo>, StoreError>>;
    fn register_existing_shard<'a>(
        &'a self,
        scope: Scope,
        scope_hash: &'a str,
        shard_index: u32,
        db_path: &'a Path,
        vector_count: u64,
        is_active: bool,
    ) -> BoxFuture<'a, Result<ShardInfo, StoreError>>;
    fn archive_shard<'a>(
        &'a self,
        shard_id: i64,
        reason: &'a str,
        archive_path_override: Option<&'a Path>,
    ) -> BoxFuture<'a, Result<Option<PathBuf>, StoreError>>;
    fn reassign_shard_scope<'a>(
        &'a self,
        shard_id: i64,
        new_scope_hash: &'a str,
        new_db_path: &'a Path,
        vector_count: u64,
        is_active: bool,
    ) -> BoxFuture<'a, Result<ShardInfo, StoreError>>;
    fn delete_shard<'a>(&'a self, shard_id: i64) -> BoxFuture<'a, Result<(), StoreError>>;
    fn get_shard_by_id<'a>(
        &'a self,
        shard_id: i64,
    ) -> BoxFuture<'a, Result<Option<ShardInfo>, StoreError>>;
}

type InitCell = OnceCell<Result<(), StoreError>>;

/// Which registry writer is classifying a failed write (W4/ADR-0022).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingRow {
    /// `create_shard`: an existing row at the target key is the idempotent
    /// dedup result.
    Dedup,
    /// `register_existing_shard`: the target key row is the upsert's own row.
    Activate,
    /// `rotate_shard`: a rotation never treats an existing row as success.
    Occupied,
}

/// The typed refusal for a write that would leave a second active row.
fn active_shard_conflict(
    scope: Scope,
    scope_hash: &str,
    shard_index: u32,
    active: &ShardInfo,
) -> StoreError {
    StoreError::ActiveShardConflict {
        scope: scope.as_str(),
        scope_hash: scope_hash.to_string(),
        shard_index,
        active_shard_index: active.shard_index,
    }
}

/// The manager.
pub struct ShardManager {
    storage_path: PathBuf,
    config: StoreConfig,
    connections: Arc<ConnectionManager>,
    probe: Arc<dyn ProcessProbe>,
    /// The build's configured embedding identity (`None` = no identity: no
    /// fingerprint is ever claimed — today's legacy behaviour).
    embedding_identity: Option<mem_core::embedding::EmbeddingIdentity>,
    metadata: Arc<Mutex<Option<DbHandle>>>,
    init: Arc<Mutex<Option<Arc<InitCell>>>>,
    scope_write_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

impl Clone for ShardManager {
    fn clone(&self) -> Self {
        Self {
            storage_path: self.storage_path.clone(),
            config: self.config.clone(),
            connections: self.connections.clone(),
            probe: self.probe.clone(),
            embedding_identity: self.embedding_identity.clone(),
            // Clone::clone may not fail: metadata and init are Arc-shared, so a clone shares
            // the one authoritative cache and the one init slot instead of snapshotting them (ADR-0013 D-3 / F10)
            metadata: self.metadata.clone(),
            init: self.init.clone(),
            scope_write_locks: self.scope_write_locks.clone(),
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn validated_dims(dimensions: u32) -> Result<u32, StoreError> {
    if !(1..=65536).contains(&dimensions) {
        return Err(StoreError::InvalidEmbeddingDimensions {
            dimensions: i64::from(dimensions),
        });
    }
    Ok(dimensions)
}

/// Free seam used by tests/later waves (delegates to [`ShardManager::shard_path`]).
pub fn get_shard_path(
    storage_path: &Path,
    scope: Scope,
    scope_hash: &str,
    shard_index: u32,
) -> Result<PathBuf, StoreError> {
    ShardManager::shard_path(storage_path, scope, scope_hash, shard_index)
}

/// One memory row located somewhere in a scope — projected to identity only.
///
/// Deliberately does NOT carry `content`, `metadata`, `tags` or either vector:
/// the write-path callers need the shard it lives in and the two identity
/// columns, and nothing else may leave the store. `Row` is consumed inside
/// [`ShardManager::find_memory_in_scope`] and never escapes.
#[derive(Debug, Clone)]
pub struct ScopedMemory {
    pub shard: ShardInfo,
    pub container_tag: Option<String>,
    pub session_id: Option<String>,
}

impl ShardManager {
    pub fn new(
        storage_path: PathBuf,
        config: StoreConfig,
        connections: Arc<ConnectionManager>,
        probe: Arc<dyn ProcessProbe>,
        embedding_identity: Option<mem_core::embedding::EmbeddingIdentity>,
    ) -> Self {
        Self {
            storage_path: storage_path.canonicalize().unwrap_or(storage_path),
            config,
            connections,
            probe,
            embedding_identity,
            metadata: Arc::new(Mutex::new(None)),
            init: Arc::new(Mutex::new(None)),
            scope_write_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The store config (legacy migrator + migration service read
    /// dims/model without reaching into `Arc<ShardManager>` privates).
    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    /// The configured embedding identity, when this build has one.
    pub fn embedding_identity(&self) -> Option<&mem_core::embedding::EmbeddingIdentity> {
        self.embedding_identity.as_ref()
    }

    /// The process probe (live-lock checks in the migration paths).
    pub fn process_probe(&self) -> Arc<dyn ProcessProbe> {
        Arc::clone(&self.probe)
    }

    /// This process's exclusivity proof for the manager's storage root, when
    /// it holds the root's engine lock.
    ///
    /// Delegates to the connection manager, which owns the exact root the
    /// proof is matched against (never canonicalised, never by prefix).
    /// `None` is a fact, not an error: the reaper converts it into
    /// [`StoreError::NoExclusiveRoot`] before opening anything for deletion.
    pub(crate) fn exclusive_root(&self) -> Option<crate::delete_guard::ExclusiveRoot> {
        self.connections.exclusive_root()
    }

    /// The initialized metadata handle — the reaping module's alias of
    /// [`Self::ensure_initialized`], which stays module-private.
    pub(crate) async fn metadata_handle(&self) -> Result<DbHandle, StoreError> {
        self.ensure_initialized().await
    }

    /// Upstream `withScopeWriteLock`: per-`(scope, scope_hash)` async
    /// mutex — operations on the same scope serialize; distinct scopes run
    /// unblocked. The std guard is cloned out and DROPPED before `.await`
    /// (never held across a yield); the async guard is held for the whole
    /// `op().await`.
    pub async fn with_scope_write_lock<'a, T, F>(
        &self,
        scope: Scope,
        scope_hash: &str,
        op: F,
    ) -> Result<T, StoreError>
    where
        T: 'a,
        F: FnOnce() -> BoxFuture<'a, Result<T, StoreError>>,
    {
        let key = format!("{}:{}", scope.as_str(), scope_hash);
        let mutex = {
            // per-scope write locks: a lost entry mints a second AsyncMutex and lets two writers into one shard (ADR-0013 D-2 D2-a)
            let mut locks =
                self.scope_write_locks
                    .lock()
                    .map_err(|_| StoreError::LockPoisoned {
                        lock: "scope_write_locks",
                        operation: "with_scope_write_lock",
                    })?;
            Arc::clone(
                locks
                    .entry(key)
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let _guard = mutex.lock().await;
        op().await
    }

    /// Connection seam for tests and later waves: open (or join) the
    /// shard db through this manager's connection manager.
    pub async fn open_shard(&self, full_path: &Path) -> Result<DbHandle, StoreError> {
        self.connections.get_connection(full_path).await
    }

    /// The per-open-connection identity-facts cache (R-13).
    pub fn model_facts_cache(&self) -> &crate::shard_facts::ModelFactsCache {
        self.connections.model_facts_cache()
    }

    /// Invalidates a shard's cached identity facts. REQUIRED after every
    /// in-process write to `shard_metadata` (ADR-0018 D-18-13).
    pub fn invalidate_model_facts(&self, db_path: &Path) -> Result<(), StoreError> {
        self.connections.model_facts_cache().invalidate(db_path)
    }

    /// The authority's cached read through the open shard handle.
    pub async fn model_facts(
        &self,
        db_path: &Path,
    ) -> Result<crate::shard_facts::ModelFacts, StoreError> {
        let db = self.connections.get_connection(db_path).await?;
        crate::shard_facts::model_facts_cached(&db, self.connections.model_facts_cache(), db_path)
            .await
    }

    /// Upstream `reset()` (sync; consumed by the lifecycle step).
    pub fn reset(&self) -> Result<(), StoreError> {
        // metadata cache: reset must invalidate, or the next call reuses a stale handle (ADR-0013 D-2 D2-c)
        *self.metadata.lock().map_err(|_| StoreError::LockPoisoned {
            lock: "metadata",
            operation: "reset",
        })? = None;
        // init slot: reset must invalidate, or the next call joins a dead init cell (ADR-0013 D-2 D2-c)
        *self.init.lock().map_err(|_| StoreError::LockPoisoned {
            lock: "init",
            operation: "reset",
        })? = None;
        // the identity-facts cache: reset must not serve facts from a closed
        // handle's file (the open/close hooks are the primary guard)
        self.connections.model_facts_cache().clear()?;
        Ok(())
    }

    /// Upstream `getShardPath` — strict hash validation (mem-core; failure
    /// = `Core` passthrough), then containment -> `ShardPathEscape`.
    pub fn get_shard_path(
        &self,
        scope: Scope,
        scope_hash: &str,
        shard_index: u32,
    ) -> Result<PathBuf, StoreError> {
        Self::shard_path(&self.storage_path, scope, scope_hash, shard_index)
    }

    /// Static variant used by tests and the registry trait seam.
    pub fn shard_path(
        storage_path: &Path,
        scope: Scope,
        scope_hash: &str,
        shard_index: u32,
    ) -> Result<PathBuf, StoreError> {
        assert_safe_scope_hash(scope_hash)
            .map(|_| ())
            .map_err(|error| StoreError::Core {
                message: error.to_string(),
            })?;
        let scope_dir = storage_path.join(format!("{}s", scope.as_str()));
        let full = scope_dir.join(format!(
            "{}_{}_shard_{}.db",
            scope.as_str(),
            scope_hash,
            shard_index
        ));
        let normalized = normalize(&full);
        let normalized_root = normalize(storage_path);
        if crate::db::assert_path_inside_storage(&normalized_root, &normalized).is_ok() {
            Ok(normalized)
        } else {
            Err(StoreError::ShardPathEscape { path: normalized })
        }
    }

    /// Upstream `resolveStoredPath` — basename, re-anchored under the scope dir.
    pub fn resolve_stored_path(&self, stored_path: &str, scope: &str) -> PathBuf {
        let file_name = Path::new(stored_path)
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| stored_path.to_string().into());
        self.storage_path.join(format!("{scope}s")).join(file_name)
    }

    /// Upstream's stored `db_path` — one expression, spelled out at all three
    /// write sites (`createShard` shard-manager.ts L173,
    /// `registerExistingShard` L230, `reassignShardScope` L528): join the
    /// scope directory with the basename, then map `\` to `/`. Upstream
    /// repeats it instead of naming it; here it is ONE helper, because a
    /// second copy is exactly how the plural drifted out of the first.
    ///
    /// The port's four callers are `create_shard`, `register_existing_shard`
    /// and `reassign_shard_scope` — one per upstream site, in that order —
    /// plus `rotate_shard`, which is port-specific: upstream has no such
    /// method (ADR-0022 D-22-4 replaces its two-step create/retire) and it
    /// reuses this shape because it writes the same column.
    /// `reassign_shard_scope` passes the row's OWN scope (upstream L528 reads
    /// `row.scope`), so its scope text comes from the registry, not from its
    /// arguments. `tests/shard_stored_path.rs` pins the raw column of all
    /// four.
    ///
    /// The **plural** is the invariant (L441: "Stored db_path is always
    /// `<scope>s/<basename>`"). [`Self::resolve_stored_path`] and
    /// [`Self::get_shard_by_path`] both take the BASENAME, so a wrong prefix
    /// still resolves and only the registry row is wrong — which is why every
    /// reader masked the defect.
    fn stored_path_string(&self, path: &Path, scope: &str) -> String {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        format!("{scope}s/{file_name}").replace('\\', "/")
    }

    /// Upstream `ensureInitialized`: dedup in-flight init, then REVALIDATE
    /// (connection manager is authoritative — a swapped handle is adopted
    /// and re-initialized).
    async fn ensure_initialized(&self) -> Result<DbHandle, StoreError> {
        let metadata_path = self
            .storage_path
            .join(crate::operation_lock::METADATA_DB_NAME);
        // metadata: the cached handle gates init and revalidation (ADR-0013 D-2 D2-a)
        let cached = self
            .metadata
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "metadata",
                operation: "ensure_initialized",
            })?
            .clone();
        if let Some(cached) = cached {
            let current = self.connections.get_connection(&metadata_path).await?;
            if !Arc::ptr_eq(&current, &cached) {
                // metadata: adopt the connection manager's authoritative handle, then re-init (ADR-0013 D-2 D2-c)
                self.metadata
                    .lock()
                    .map_err(|_| StoreError::LockPoisoned {
                        lock: "metadata",
                        operation: "ensure_initialized",
                    })?
                    .replace(current.clone());
                if let Err(error) = self.init_metadata_db(&current).await {
                    // A failed adopt must not leave a half-trusted cell: clear
                    // BOTH before propagating, so the next call retries.
                    // (Ruling 2a: metadata never settles a restore.)
                    *self.init.lock().map_err(|_| StoreError::LockPoisoned {
                        lock: "init",
                        operation: "ensure_initialized",
                    })? = None;
                    *self.metadata.lock().map_err(|_| StoreError::LockPoisoned {
                        lock: "metadata",
                        operation: "ensure_initialized",
                    })? = None;
                    return Err(error);
                }
            }
            return Ok(cached);
        }
        let cell = {
            // init: claim-or-install the init cell — the shard analogue of pending (ADR-0013 D-2 D2-a)
            let mut slot = self.init.lock().map_err(|_| StoreError::LockPoisoned {
                lock: "init",
                operation: "ensure_initialized",
            })?;
            match slot.as_ref() {
                Some(cell) => Arc::clone(cell),
                None => {
                    let cell = Arc::new(OnceCell::new());
                    *slot = Some(cell.clone());
                    cell
                }
            }
        };
        let result: Result<(), StoreError> = cell
            .get_or_init(|| self.initialize_metadata_db(metadata_path))
            .await
            .clone();
        if result.is_err() {
            // upstream catch: clear both slots so the next call retries
            // init: cleared after a failed init so the next call retries (ADR-0013 D-2 D2-c)
            *self.init.lock().map_err(|_| StoreError::LockPoisoned {
                lock: "init",
                operation: "ensure_initialized",
            })? = None;
            // metadata: cleared after a failed init so the next call retries (ADR-0013 D-2 D2-c)
            *self.metadata.lock().map_err(|_| StoreError::LockPoisoned {
                lock: "metadata",
                operation: "ensure_initialized",
            })? = None;
        }
        result?;
        // metadata: the post-init read; a miss is an internal defect, not a panic (ADR-0013 D-2 D2-a)
        self.metadata
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "metadata",
                operation: "ensure_initialized",
            })?
            .clone()
            .ok_or_else(|| StoreError::Internal {
                message: "metadata db missing after init".into(),
            })
    }

    async fn initialize_metadata_db(&self, metadata_path: PathBuf) -> Result<(), StoreError> {
        let db = self.connections.get_connection(&metadata_path).await?;
        let outcome = self.init_metadata_db(&db).await;
        match outcome {
            Ok(()) => {
                // metadata: store the initialized handle — the flag every later call trusts (ADR-0013 D-2 D2-c)
                *self.metadata.lock().map_err(|_| StoreError::LockPoisoned {
                    lock: "metadata",
                    operation: "initialize_metadata_db",
                })? = Some(db);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn init_metadata_db(&self, db: &DbHandle) -> Result<(), StoreError> {
        // Metadata is a no-backup site (Ruling 2a, W4/ADR-0022 D-22-7): v2
        // deletes no row — the repair only clears a redundant `is_active`
        // flag — and the repair + unique index commit as ONE atomic batch.
        // Nothing recoverable is lost, so no backup is owed, and `db_path:
        // None` keeps this hot open path free of both settle sites: callers
        // without the engine lock (the ready gate's early phases,
        // `real_components`) would otherwise get `NoExclusiveRoot` before
        // the batch, and callers with it would become
        // `MigrationRestorePending` sites with no settle path.
        crate::schema_migrations::apply_schema_migrations(
            db.as_ref(),
            &crate::schema_migrations::metadata_db_migrations(),
            &crate::schema_migrations::MigrationOptions {
                db_path: None,
                label: "metadata.db",
                max_known: crate::schema_migrations::METADATA_MAX_KNOWN,
            },
            // No backup is taken without a `db_path`, so no proof is needed.
            None,
        )
        .await
        .map(|_| ())
    }

    /// Upstream `getActiveShard`.
    pub async fn get_active_shard(
        &self,
        scope: Scope,
        scope_hash: &str,
    ) -> Result<Option<ShardInfo>, StoreError> {
        let db = self.ensure_initialized().await?;
        let row = query_one(
            db.as_ref(),
            crate::sql::GET_ACTIVE_SHARD,
            vec![
                Value::Text(scope.as_str().to_string()),
                Value::Text(scope_hash.to_string()),
            ],
        )
        .await?;
        Ok(row.map(|row| self.row_to_shard_info(row)))
    }

    /// Upstream `getAllShards` (`None` hash = upstream `""` wildcard).
    pub async fn get_all_shards(
        &self,
        scope: Scope,
        scope_hash: Option<&str>,
    ) -> Result<Vec<ShardInfo>, StoreError> {
        let db = self.ensure_initialized().await?;
        let rows = match scope_hash {
            None | Some("") => {
                db.query(
                    crate::sql::GET_ALL_SHARDS_FOR_SCOPE,
                    vec![Value::Text(scope.as_str().to_string())],
                )
                .await?
            }
            Some(hash) => {
                db.query(
                    crate::sql::GET_ALL_SHARDS_FOR_SCOPE_HASH,
                    vec![
                        Value::Text(scope.as_str().to_string()),
                        Value::Text(hash.to_string()),
                    ],
                )
                .await?
            }
        };
        Ok(rows
            .into_iter()
            .map(|row| self.row_to_shard_info(row))
            .collect())
    }

    /// Find `id` in ANY shard of `(scope, scope_hash)` — active or retired.
    ///
    /// PRECONDITION (documented, not type-enforced): on the write path the
    /// caller holds [`with_scope_write_lock`](Self::with_scope_write_lock).
    ///
    /// `Ok(Some(_))` = the copy in the HIGHEST `shard_index`; `Ok(None)` = the
    /// id is in no shard of the scope; `Err(StoreError::ScopeShardUnreadable)`
    /// = some shard of the scope could not be stat'ed / opened / queried (an
    /// enumeration failure is NOT wrapped — `get_all_shards` errors propagate
    /// as their own variant, see [`StoreError::ScopeShardUnreadable`]).
    ///
    /// It never creates a shard file, never writes, never bumps a count.
    ///
    /// Loop invariant: before iteration *i*, the id is absent from every shard
    /// already visited, and every visited shard opened cleanly. Termination:
    /// the `get_all_shards` vector is finite and the loop consumes exactly one
    /// entry per iteration.
    pub async fn find_memory_in_scope(
        &self,
        scope: Scope,
        scope_hash: &str,
        id: &str,
    ) -> Result<Option<ScopedMemory>, StoreError> {
        let mut shards = self.get_all_shards(scope, Some(scope_hash)).await?;
        // DESC by shard_index: the highest index is the latest row the scope
        // has written — the latest truth (and the copy W1b will make
        // `find_memory` prefer). "Any hit" would make the verdict depend on
        // scan order; `UNIQUE(scope, scope_hash, shard_index)` makes DESC a
        // total order, so the answer is deterministic.
        shards.sort_by_key(|shard| std::cmp::Reverse(shard.shard_index));

        for info in shards {
            let unreadable = |message: String| StoreError::ScopeShardUnreadable {
                scope: scope.as_str(),
                scope_hash: scope_hash.to_string(),
                shard_index: u64::from(info.shard_index),
                db_path: info.db_path.clone(),
                id: id.to_string(),
                message: message.into(),
            };

            // Stat BEFORE opening: `ConnectionManager::open_database` CREATES
            // a missing database file, so opening first would resurrect a dead
            // shard and yield `Ok(None)` — the fail-open shape this seam
            // exists to prevent.
            if let Err(error) = std::fs::metadata(&info.db_path) {
                return Err(unreadable(format!(
                    "shard file is missing or unreadable ({error})"
                )));
            }

            let db = self
                .open_shard(&info.db_path)
                .await
                .map_err(|error| unreadable(error.message()))?;

            let row = crate::vector::get_memory_by_id(db.as_ref(), id)
                .await
                .map_err(|error| unreadable(error.message()))?;
            if let Some(row) = row {
                let container_tag = row.text("container_tag");
                let session_id = row.text("session_id");
                // `row` — content, metadata, tags, vectors — is dropped HERE:
                // nothing but identity leaves the store.
                return Ok(Some(ScopedMemory {
                    shard: info.clone(),
                    container_tag,
                    session_id,
                }));
            }
        }
        Ok(None)
    }

    /// Deletes every copy of `id` in the shards of `(scope, scope_hash)` —
    /// active and retired — and returns the number of shards that held a copy
    /// at lookup.
    ///
    /// CONTRACT
    ///
    /// `Ok(c)` implies exactly `c` shards of the tuple held `id` at their
    /// lookup and held no copy immediately after their batch; every shard from
    /// which a copy was deleted was migrated to v3 first
    /// ([`ensure_shard_schema`](Self::ensure_shard_schema)) and has
    /// `shard_stats` = `COUNT(*)` with the registry equal to that committed
    /// value; shards with no copy are untouched.
    ///
    /// The address space is the anchor's `(scope, scope_hash)`: it never
    /// crosses `scope_hash` (another project is another tenant) and never
    /// crosses `Scope` (project -> user). `scope_hash` is validated before the
    /// registry query, so `""` can never reach the wildcard branch.
    ///
    /// Best-effort under concurrent writers: no scope lock is taken, so a row
    /// inserted after a shard's batch is a new write and may recreate a copy.
    /// `Err(ScopeShardUnreadable)` names the first unreadable shard and stops;
    /// the visited prefix has been deleted/counted. A migration, delete or
    /// adoption failure keeps its own typed error (a newer-than-build shard is
    /// a refusal, not an unreadable shard), and deletes are never
    /// model-refused (ADR-0018 D-18-10).
    ///
    /// Loop invariant: before iteration *i*, every visited shard either held
    /// no copy of `id` or held one and has since deleted it and adopted its
    /// own committed count; the visited count equals `copies`.
    /// Termination: `get_all_shards` is finite and each iteration consumes
    /// exactly one entry.
    pub async fn delete_memory_in_scope(
        &self,
        scope: Scope,
        scope_hash: &str,
        id: &str,
    ) -> Result<usize, StoreError> {
        assert_safe_scope_hash(scope_hash)?;
        let ids = [id.to_string()];
        let mut shards = self.get_all_shards(scope, Some(scope_hash)).await?;
        // DESC by shard_index: deterministic, newest first (W1's precedence).
        // The scan is only an ordering choice — every shard is visited, so
        // the residue of a mid-scan failure is the older copies.
        shards.sort_by_key(|shard| std::cmp::Reverse(shard.shard_index));

        let mut copies = 0_usize;
        for info in shards {
            let unreadable = |message: String| StoreError::ScopeShardUnreadable {
                scope: scope.as_str(),
                scope_hash: scope_hash.to_string(),
                shard_index: u64::from(info.shard_index),
                db_path: info.db_path.clone(),
                id: id.to_string(),
                message: message.into(),
            };

            // Stat BEFORE opening: `open_shard` CREATES a missing database
            // file, so opening first would resurrect a dead shard and turn the
            // fail-closed path into a silent recreation.
            if let Err(error) = std::fs::metadata(&info.db_path) {
                return Err(unreadable(format!(
                    "shard file is missing or unreadable ({error})"
                )));
            }

            let db = self
                .open_shard(&info.db_path)
                .await
                .map_err(|error| unreadable(error.message()))?;

            let existing = crate::vector::get_memory_by_id(db.as_ref(), id)
                .await
                .map_err(|error| unreadable(error.message()))?;
            if existing.is_none() {
                continue;
            }

            // Migrate-on-touch (issue 39 line 80): `open_shard` does not
            // migrate and the counted batch needs `shard_stats`. A v3 shard is
            // a no-op; a v2-stamped shard gains the table here; a
            // newer-than-build shard propagates its own typed refusal.
            self.ensure_shard_schema(db.as_ref(), self.config.embedding_dimensions)
                .await?;
            // The batch owns the statement-measured `COUNT(*)`; the registry
            // adopts the committed value (no delta, no guessed number).
            crate::vector::delete_memories(db.as_ref(), &ids).await?;
            self.adopt_stored_count(db.as_ref(), info.id).await?;
            copies += 1;
        }
        Ok(copies)
    }

    /// Upstream `createShard`: init the shard FILE first (orphan-row guard):
    /// get_connection + init_shard_db, then INSERT; unique violation yields
    /// a re-fetch of the existing row (idempotent dedup).
    pub async fn create_shard(
        &self,
        scope: Scope,
        scope_hash: &str,
        shard_index: u32,
    ) -> Result<ShardInfo, StoreError> {
        let db = self.ensure_initialized().await?;
        let full_path = self.get_shard_path(scope, scope_hash, shard_index)?;
        let stored = self.stored_path_string(&full_path, scope.as_str());
        let now = now_millis();

        // Initialize the shard file BEFORE inserting the registry row. If
        // init throws (disk full, permissions), the registry stays free of
        // an orphan row that points at an uninitialized file — such a row
        // would later fail is_shard_valid on every get_write_shard and
        // brick all writes to this scope. init_shard_db is idempotent.
        let shard_db = self.connections.get_connection(&full_path).await?;
        self.init_shard_db(
            shard_db.as_ref(),
            self.config.embedding_dimensions,
            &self.config.embedding_model,
        )
        .await?;
        // `init_shard_db` writes the identity keys; a registered file can
        // already have a cache entry (the lost-registry-row branch).
        self.connections
            .model_facts_cache()
            .invalidate(&full_path)?;

        let insert = db
            .execute(
                crate::sql::CREATE_SHARD,
                vec![
                    Value::Text(scope.as_str().to_string()),
                    Value::Text(scope_hash.to_string()),
                    Value::Integer(i64::from(shard_index)),
                    Value::Text(stored),
                    Value::Integer(now),
                ],
            )
            .await;
        match insert {
            Ok(result) => {
                let shard = ShardInfo {
                    id: result.last_insert_rowid,
                    scope,
                    scope_hash: scope_hash.to_string(),
                    shard_index,
                    db_path: full_path,
                    vector_count: 0,
                    is_active: true,
                    created_at: now,
                };
                // A populated file whose registry row was lost reaches this
                // path: the observed count (not a false 0) becomes the row's
                // value, and the write-once identity keys survive (D-16).
                self.finish_created_shard(shard, shard_db.as_ref()).await
            }
            Err(error) => {
                self.classify_registry_write(
                    scope,
                    scope_hash,
                    shard_index,
                    error,
                    ExistingRow::Dedup,
                )
                .await
            }
        }
    }

    /// The tail shared by [`create_shard`](Self::create_shard) and
    /// [`rotate_shard`](Self::rotate_shard): claim the configured identity on
    /// the freshly-active shard and sync its committed count.
    ///
    /// R-13: a claim wrote `shard_metadata`, so the cached facts for the path
    /// are stale and are invalidated.
    async fn finish_created_shard(
        &self,
        shard: ShardInfo,
        shard_db: &dyn Db,
    ) -> Result<ShardInfo, StoreError> {
        let outcome = crate::shard_facts::prepare_active_shard(
            shard_db,
            self.config.embedding_dimensions,
            self.embedding_identity.as_ref(),
            &self.config.embedding_model,
        )
        .await?;
        if matches!(outcome.step, crate::shard_facts::FingerprintStep::Claimed) {
            // The claim wrote `shard_metadata`; the cached pre-claim facts (if
            // any) are stale.
            self.connections
                .model_facts_cache()
                .invalidate(&shard.db_path)?;
        }
        self.sync_shard_vector_count(shard, outcome.vector_count)
            .await
    }

    /// Rotates one scope from `retire_shard_id` to a new shard at
    /// `next_index` — **atomically** (W4/ADR-0022 D-22-4).
    ///
    /// CONTRACT
    ///
    /// The shard FILE is initialized first (the pre-existing orphan-row guard:
    /// a failed init leaves no registry row), then ONE `BatchMode::Write` batch
    /// retires the previous row and inserts the new one. A failed create thus
    /// rolls the retire back: the previous shard stays active, which the
    /// shipped two-step could not promise (it left ZERO actives and the dedup
    /// then returned the inactive row as the write shard).
    ///
    /// The caller holds the scope write lock
    /// ([`with_scope_write_lock`](Self::with_scope_write_lock)). A stale or
    /// foreign `retire_shard_id` retires nothing and yields a typed conflict;
    /// an occupied target index yields [`StoreError::ShardIndexOccupied`]. An
    /// orphan shard *file* may remain after a failure (W7 reaps it, and
    /// reconcile normally re-registers it).
    pub async fn rotate_shard(
        &self,
        scope: Scope,
        scope_hash: &str,
        retire_shard_id: i64,
        next_index: u32,
    ) -> Result<ShardInfo, StoreError> {
        assert_safe_scope_hash(scope_hash)?;
        let db = self.ensure_initialized().await?;
        let full_path = self.get_shard_path(scope, scope_hash, next_index)?;
        let stored = self.stored_path_string(&full_path, scope.as_str());
        let now = now_millis();

        // File init stays OUTSIDE the transaction (orphan-row guard).
        let shard_db = self.connections.get_connection(&full_path).await?;
        self.init_shard_db(
            shard_db.as_ref(),
            self.config.embedding_dimensions,
            &self.config.embedding_model,
        )
        .await?;
        self.connections
            .model_facts_cache()
            .invalidate(&full_path)?;

        let statements = vec![
            Statement::new(
                crate::shard_registry_sql::MARK_SHARD_INACTIVE_IN_SCOPE,
                vec![
                    Value::Integer(retire_shard_id),
                    Value::Text(scope.as_str().to_string()),
                    Value::Text(scope_hash.to_string()),
                ],
            ),
            Statement::new(
                crate::sql::CREATE_SHARD,
                vec![
                    Value::Text(scope.as_str().to_string()),
                    Value::Text(scope_hash.to_string()),
                    Value::Integer(i64::from(next_index)),
                    Value::Text(stored),
                    Value::Integer(now),
                ],
            ),
        ];
        let results = match db.batch(statements, BatchMode::Write).await {
            Ok(results) => results,
            Err(error) => {
                return self
                    .classify_registry_write(
                        scope,
                        scope_hash,
                        next_index,
                        error,
                        ExistingRow::Occupied,
                    )
                    .await;
            }
        };
        let Some(result) = results.last() else {
            return Err(StoreError::Internal {
                message: format!(
                    "Rotation for {}/{scope_hash}#{next_index} committed no insert result",
                    scope.as_str()
                ),
            });
        };
        let shard = ShardInfo {
            id: result.last_insert_rowid,
            scope,
            scope_hash: scope_hash.to_string(),
            shard_index: next_index,
            db_path: full_path,
            vector_count: 0,
            is_active: true,
            created_at: now,
        };
        self.finish_created_shard(shard, shard_db.as_ref()).await
    }

    async fn find_shard_by_key(
        &self,
        scope: Scope,
        scope_hash: &str,
        shard_index: u32,
    ) -> Result<Option<ShardInfo>, StoreError> {
        let db = self.ensure_initialized().await?;
        let row = query_one(
            db.as_ref(),
            crate::sql::FIND_SHARD_BY_KEY,
            vec![
                Value::Text(scope.as_str().to_string()),
                Value::Text(scope_hash.to_string()),
                Value::Integer(i64::from(shard_index)),
            ],
        )
        .await?;
        Ok(row.map(|row| self.row_to_shard_info(row)))
    }

    /// Classifies a failed registry write by RE-READING the registry (W4,
    /// decision 6). `is_unique_violation()` only decides that a probe is worth
    /// it — the message never becomes a verdict; and inside `BatchMode::Write`
    /// `is_constraint` is false, so it is never consulted.
    ///
    /// Probe order is load-bearing: the tuple's ACTIVE row is consulted BEFORE
    /// the target key row, and no success is returned while `A` is `Some` and
    /// `A.id != K.id`. A key-first classifier would answer `Ok(inactive row)`
    /// when the target already exists inactive and a *different* row is
    /// active — silently dropping the activation.
    async fn classify_registry_write(
        &self,
        scope: Scope,
        scope_hash: &str,
        shard_index: u32,
        error: StoreError,
        mode: ExistingRow,
    ) -> Result<ShardInfo, StoreError> {
        if !error.is_unique_violation() {
            return Err(error);
        }
        let key = self
            .find_shard_by_key(scope, scope_hash, shard_index)
            .await?;
        let active = self.get_active_shard(scope, scope_hash).await?;
        match (active, key) {
            (Some(active), Some(key)) if active.id == key.id => match mode {
                // Upstream dedup: the re-fetch IS the return value, and the
                // row is the active row the caller observed.
                ExistingRow::Dedup | ExistingRow::Activate => Ok(key),
                ExistingRow::Occupied => Err(StoreError::ShardIndexOccupied { shard_index }),
            },
            (Some(active), Some(_)) => match mode {
                // A rotation was inserting over an existing (inactive) key:
                // the index, not the activation, is the conflict.
                ExistingRow::Occupied => Err(StoreError::ShardIndexOccupied { shard_index }),
                ExistingRow::Dedup | ExistingRow::Activate => Err(active_shard_conflict(
                    scope,
                    scope_hash,
                    shard_index,
                    &active,
                )),
            },
            (Some(active), None) => Err(active_shard_conflict(
                scope,
                scope_hash,
                shard_index,
                &active,
            )),
            (None, Some(key)) => match mode {
                ExistingRow::Dedup | ExistingRow::Activate => Ok(key),
                ExistingRow::Occupied => Err(StoreError::ShardIndexOccupied { shard_index }),
            },
            // Neither unique constraint's row is present: defensive, and
            // unreachable by construction (`shards` has exactly two unique
            // constraints, both probed above).
            (None, None) => Err(error),
        }
    }

    /// Upstream `registerExistingShard` (upsert + re-fetch).
    pub async fn register_existing_shard(
        &self,
        scope: Scope,
        scope_hash: &str,
        shard_index: u32,
        db_path: &Path,
        vector_count: u64,
        is_active: bool,
    ) -> Result<ShardInfo, StoreError> {
        assert_safe_scope_hash(scope_hash)
            .map(|_| ())
            .map_err(|error| StoreError::Core {
                message: error.to_string(),
            })?;
        let db = self.ensure_initialized().await?;
        let stored = self.stored_path_string(db_path, scope.as_str());
        let now = now_millis();
        let write = db
            .execute(
                crate::sql::REGISTER_EXISTING_SHARD,
                vec![
                    Value::Text(scope.as_str().to_string()),
                    Value::Text(scope_hash.to_string()),
                    Value::Integer(i64::from(shard_index)),
                    Value::Text(stored),
                    Value::Integer(i64::try_from(vector_count).unwrap_or(i64::MAX)),
                    Value::Integer(i64::from(is_active)),
                    Value::Integer(now),
                ],
            )
            .await;
        if let Err(error) = write {
            // The upsert can be refused by the one-active index when it
            // promotes an existing inactive row while another row is active
            // (T6). Re-read the registry rather than parse the message.
            return self
                .classify_registry_write(
                    scope,
                    scope_hash,
                    shard_index,
                    error,
                    ExistingRow::Activate,
                )
                .await;
        }
        let row = query_one(
            db.as_ref(),
            crate::sql::FIND_SHARD_BY_KEY,
            vec![
                Value::Text(scope.as_str().to_string()),
                Value::Text(scope_hash.to_string()),
                Value::Integer(i64::from(shard_index)),
            ],
        )
        .await?;
        row.map(|row| self.row_to_shard_info(row))
            .ok_or_else(|| StoreError::Internal {
                message: format!(
                    "Failed to register shard {}/{scope_hash}#{shard_index}",
                    scope.as_str()
                ),
            })
    }

    /// Upstream `initShardDb` — validated dims, ensure_shard_schema, then the
    /// dims/model metadata batch.
    ///
    /// **Writer rule (R-13):** this writes `shard_metadata`, so every caller
    /// MUST invalidate the path in its `ModelFactsCache` after commit (the
    /// connection open/close hooks are the second line of defence).
    ///
    /// The two identity keys are **write-once** (`INSERT OR IGNORE`, ADR-0018
    /// D-16): a populated file whose registry row was lost is re-opened by
    /// `create_shard`, and the old `INSERT OR REPLACE` would overwrite its
    /// stored model with the configured one — blinding the very detector W2
    /// ships. A fresh file (including the staged rebuild) still gets both.
    pub async fn init_shard_db(
        &self,
        db: &dyn Db,
        dimensions: u32,
        embedding_model: &str,
    ) -> Result<(), StoreError> {
        let dims = validated_dims(dimensions)?;
        self.ensure_shard_schema(db, dims).await?;
        db.batch(
            vec![
                Statement::new(
                    crate::sql::SHARD_METADATA_INSERT_DIMENSIONS,
                    vec![Value::Text(dims.to_string())],
                ),
                Statement::new(
                    crate::sql::SHARD_METADATA_INSERT_MODEL,
                    vec![Value::Text(embedding_model.to_string())],
                ),
            ],
            BatchMode::Write,
        )
        .await?;
        Ok(())
    }

    /// Upstream `ensureShardSchema` (no shard_metadata rewrite).
    ///
    /// Delegates to [`crate::shard_facts::ensure_v3`] — the same idempotent
    /// v1..v3 runner PLUS the `sqlite_master` probe that repairs a shard whose
    /// `user_version` claims v3 while `shard_stats` is missing. Without the
    /// delegation this seam (the scope-wide delete at
    /// [`Self::delete_memory_in_scope`] and [`Self::init_shard_db`]) would
    /// still die in `sync_count` on exactly the shard the repair exists for.
    pub async fn ensure_shard_schema(
        &self,
        db: &dyn Db,
        dimensions: u32,
    ) -> Result<i64, StoreError> {
        crate::shard_facts::ensure_v3(db, dimensions).await
    }

    pub(crate) fn row_to_shard_info(&self, row: Row) -> ShardInfo {
        let id = row.i64("id").unwrap_or(0);
        let scope_text = row.text("scope").unwrap_or_else(|| "user".to_string());
        let scope = match scope_text.as_str() {
            "project" => Scope::Project,
            _ => Scope::User,
        };
        let stored = row.text("db_path").unwrap_or_default();
        let db_path = self.resolve_stored_path(&stored, &scope_text);
        ShardInfo {
            id,
            scope,
            scope_hash: row.text("scope_hash").unwrap_or_default(),
            shard_index: row.i64("shard_index").unwrap_or(0) as u32,
            db_path: normalize(&db_path),
            vector_count: row.i64("vector_count").unwrap_or(0).max(0) as u64,
            is_active: row.i64("is_active").unwrap_or(0) == 1,
            created_at: row.i64("created_at").unwrap_or(0),
        }
    }

    /// Upstream `hasMatchingEmbeddingDimensions`.
    async fn has_matching_embedding_dimensions(&self, db: &dyn Db, shard: &ShardInfo) -> bool {
        let row = match query_one(db, crate::sql::SHARD_METADATA_SELECT_DIMENSIONS, Vec::new())
            .await
        {
            Ok(row) => row,
            Err(error) => {
                tracing::error!(db_path = %shard.db_path.display(), shard_id = shard.id, error = %error.message(), "Error validating shard DB");
                return false;
            }
        };
        let stored = match row.and_then(|row| row.text("value")) {
            Some(stored) => stored,
            None => {
                tracing::warn!(db_path = %shard.db_path.display(), shard_id = shard.id, "Shard missing embedding_dimensions metadata");
                return false;
            }
        };
        let stored_dimensions: u32 = stored.parse().unwrap_or(0);
        if stored_dimensions != self.config.embedding_dimensions {
            tracing::warn!(
                db_path = %shard.db_path.display(),
                shard_id = shard.id,
                stored_dimensions,
                config_dimensions = self.config.embedding_dimensions,
                "Shard embedding dimensions mismatch"
            );
            return false;
        }
        true
    }

    /// Upstream `syncShardVectorCount`, fed the count the caller already
    /// measured (`prepare_active_shard`): the hot path pays no second
    /// `COUNT(*)` (D-7).
    async fn sync_shard_vector_count(
        &self,
        shard: ShardInfo,
        count: u64,
    ) -> Result<ShardInfo, StoreError> {
        if count == shard.vector_count {
            return Ok(shard);
        }
        let metadata = self.ensure_initialized().await?;
        metadata
            .execute(
                crate::sql::SET_VECTOR_COUNT,
                vec![
                    Value::Integer(i64::try_from(count).unwrap_or(i64::MAX)),
                    Value::Integer(shard.id),
                ],
            )
            .await?;
        Ok(ShardInfo {
            vector_count: count,
            ..shard
        })
    }

    /// Upstream `isShardValid` (file/table probes; errors -> false + log).
    async fn is_shard_valid(&self, shard: &ShardInfo) -> bool {
        if !shard.db_path.exists() {
            tracing::warn!(db_path = %shard.db_path.display(), shard_id = shard.id, "Shard DB file missing");
            return false;
        }
        let db = match self.connections.get_connection(&shard.db_path).await {
            Ok(db) => db,
            Err(error) => {
                tracing::error!(error = %error, db_path = %shard.db_path.display(), "Error validating shard DB");
                return false;
            }
        };
        let table = match query_one(&*db, crate::sql::SQLITE_MASTER_MEMORIES_TABLE, Vec::new())
            .await
        {
            Ok(table) => table,
            Err(error) => {
                tracing::error!(error = %error.message(), db_path = %shard.db_path.display(), "Error validating shard DB");
                return false;
            }
        };
        if table.is_none() {
            tracing::warn!(db_path = %shard.db_path.display(), shard_id = shard.id, "Shard DB missing 'memories' table");
            return false;
        }
        self.has_matching_embedding_dimensions(db.as_ref(), shard)
            .await
    }

    /// Upstream `getWriteShard`.
    pub async fn get_write_shard(
        &self,
        scope: Scope,
        scope_hash: &str,
    ) -> Result<ShardInfo, StoreError> {
        assert_no_turso_migration_in_progress(&self.storage_path, self.probe.as_ref())?;
        self.resolve_write_shard(scope, scope_hash).await
    }

    /// The write shard for `scope`/`scope_hash`, tolerating an operation
    /// lock held by THIS process.
    ///
    /// [`get_write_shard`](Self::get_write_shard) refuses to resolve a shard
    /// while ANY live operation lock exists — including one this very
    /// process holds. The W4.3 consolidation run holds that lock for its
    /// whole duration (ADR-0006 D-25), and its promotion writes land in a
    /// shard: without this seam the consolidation could never promote
    /// anything, because its own marker would block it.
    ///
    /// A lock held by ANOTHER process still blocks the write, and so does
    /// the legacy migration lock file (a different marker).
    pub async fn get_write_shard_allowing_own_lock(
        &self,
        scope: Scope,
        scope_hash: &str,
    ) -> Result<ShardInfo, StoreError> {
        if let Some(state) = crate::operation_lock::read_live_lock(
            &self.storage_path,
            crate::operation_lock::OPERATION_LOCK_FILE,
            self.probe.as_ref(),
        ) && state.pid != std::process::id()
        {
            return Err(StoreError::MigrationLocked {
                pid: state.pid,
                operation: state.operation,
            });
        }
        if let Some(state) = crate::operation_lock::read_live_lock(
            &self.storage_path,
            crate::operation_lock::LEGACY_MIGRATION_LOCK_FILE,
            self.probe.as_ref(),
        ) {
            return Err(StoreError::MigrationLocked {
                pid: state.pid,
                operation: state.operation,
            });
        }
        self.resolve_write_shard(scope, scope_hash).await
    }

    /// The write-shard resolution behind [`get_write_shard`](Self::get_write_shard).
    async fn resolve_write_shard(
        &self,
        scope: Scope,
        scope_hash: &str,
    ) -> Result<ShardInfo, StoreError> {
        // Upstream budget: 3 attempts. The loop is live: the zero-active arm
        // activates the highest registered row and `continue`s, so the
        // reactivated shard runs the SAME `Some` body below. Termination is
        // bounded: each iteration either returns or activates exactly once,
        // and a third `None` reaches the `Internal` tail.
        for _attempt in 0..3 {
            match self.get_active_shard(scope, scope_hash).await? {
                None => {
                    // W4/ADR-0022 D-22-5: no shortcut — the next iteration
                    // runs `is_shard_valid`, `prepare_write_shard` (W3's
                    // fingerprint refusal), the count sync and the cap check.
                    self.activate_latest_shard(scope, scope_hash).await?;
                    continue;
                }
                Some(shard) => {
                    if !self.is_shard_valid(&shard).await {
                        return Err(StoreError::ShardIncompatible {
                            scope: scope.as_str().to_string(),
                            scope_hash: shard.scope_hash.clone(),
                            shard_index: u64::from(shard.shard_index),
                            db_path: shard.db_path.clone(),
                        });
                    }
                    let shard_db = self.open_shard(&shard.db_path).await?;
                    let outcome = crate::shard_facts::prepare_write_shard(
                        &shard_db,
                        self.config.embedding_dimensions,
                        self.embedding_identity.as_ref(),
                        &self.config.embedding_model,
                        crate::shard_facts::ShardMismatchContext {
                            shard_id: shard.id,
                            container_tag_prefix: &self.config.container_tag_prefix,
                            scope: shard.scope,
                            scope_hash: &shard.scope_hash,
                        },
                        self.connections.model_facts_cache(),
                        &shard.db_path,
                    )
                    .await?;
                    let shard = self
                        .sync_shard_vector_count(shard, outcome.facts.vector_count)
                        .await?;
                    if shard.vector_count >= u64::from(self.config.max_vectors_per_shard) {
                        // W4: ONE transaction (retire + insert) — a failed
                        // create leaves the previous shard active, where the
                        // shipped two-step left ZERO actives.
                        return self
                            .rotate_shard(scope, scope_hash, shard.id, shard.shard_index + 1)
                            .await;
                    }
                    return Ok(shard);
                }
            }
        }
        Err(StoreError::Internal {
            message: format!(
                "Failed to resolve write shard for {}/{scope_hash}",
                scope.as_str()
            ),
        })
    }

    /// Activates the highest-index registered shard of a zero-active tuple
    /// (W4/ADR-0022 D-22-5).
    ///
    /// Returns `()` on purpose: the caller continues into the ordinary `Some`
    /// arm, so the reactivated shard is validated exactly like any other write
    /// shard — `is_shard_valid`, `prepare_write_shard`'s fingerprint refusal,
    /// the count sync and the cap check. A reduced tail here would be a second
    /// path to keep in sync, and a foreign-fingerprint or full shard could
    /// accept an over-cap write.
    ///
    /// With no registry row at all it creates index 0 (the resolver's
    /// first-write path); with a row already active it is a no-op. The update
    /// is re-read before returning: a vanished row is `Internal`, never a
    /// panic (ADR-0013).
    async fn activate_latest_shard(
        &self,
        scope: Scope,
        scope_hash: &str,
    ) -> Result<(), StoreError> {
        assert_safe_scope_hash(scope_hash)?;
        // `get_all_shards` is ASC by `shard_index`, so `next_back` is the
        // highest — the row the legacy migrator's map and the inventory
        // default already treat as latest.
        let shards = self.get_all_shards(scope, Some(scope_hash)).await?;
        let Some(latest) = shards.into_iter().next_back() else {
            self.create_shard(scope, scope_hash, 0).await?;
            return Ok(());
        };
        if latest.is_active {
            return Ok(());
        }
        let db = self.ensure_initialized().await?;
        db.execute(
            crate::shard_registry_sql::SET_SHARD_ACTIVE,
            vec![Value::Integer(latest.id)],
        )
        .await?;
        self.get_active_shard(scope, scope_hash)
            .await?
            .ok_or_else(|| StoreError::Internal {
                message: format!(
                    "Failed to activate shard {} for {}/{scope_hash}",
                    latest.id,
                    scope.as_str()
                ),
            })?;
        Ok(())
    }

    /// Upstream `markShardReadOnly`.
    async fn mark_shard_read_only_off(&self, shard_id: i64) -> Result<(), StoreError> {
        let db = self.ensure_initialized().await?;
        db.execute(
            crate::sql::MARK_SHARD_INACTIVE,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        Ok(())
    }

    pub async fn mark_shard_read_only(&self, shard_id: i64) -> Result<(), StoreError> {
        self.mark_shard_read_only_off(shard_id).await
    }

    /// Upstream `setVectorCount` (invalid counts type-enforced as `u64`;
    /// registry note).
    pub async fn set_vector_count(&self, shard_id: i64, count: u64) -> Result<(), StoreError> {
        let db = self.ensure_initialized().await?;
        db.execute(
            crate::sql::SET_VECTOR_COUNT,
            vec![
                Value::Integer(i64::try_from(count).unwrap_or(i64::MAX)),
                Value::Integer(shard_id),
            ],
        )
        .await?;
        Ok(())
    }

    /// W3: the shard's committed `shard_stats` count is the authority; this
    /// writes it into the registry CACHE.
    ///
    /// Reads the value the caller's batch already measured (`SYNC_COUNT_SQL`)
    /// — it never measures again and never accepts a caller-supplied number.
    pub async fn adopt_stored_count(&self, db: &dyn Db, shard_id: i64) -> Result<u64, StoreError> {
        let count = crate::shard_facts::read_stored_count(db).await?;
        self.set_vector_count(shard_id, count).await?;
        Ok(count)
    }

    /// Upstream `getShardById`.
    pub async fn get_shard_by_id(&self, shard_id: i64) -> Result<Option<ShardInfo>, StoreError> {
        let db = self.ensure_initialized().await?;
        let row = query_one(
            db.as_ref(),
            crate::sql::GET_SHARD_BY_ID,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        Ok(row.map(|row| self.row_to_shard_info(row)))
    }

    /// Upstream `getShardByPath` — basename + escape `\`/`%`/`_`, then
    /// `LIKE '%/' || ? ESCAPE '\'`.
    pub async fn get_shard_by_path(&self, db_path: &Path) -> Result<Option<ShardInfo>, StoreError> {
        let db = self.ensure_initialized().await?;
        let file_name = db_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let escaped = file_name
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let row = query_one(
            db.as_ref(),
            crate::sql::GET_SHARD_BY_PATH,
            vec![Value::Text(escaped)],
        )
        .await?;
        Ok(row.map(|row| self.row_to_shard_info(row)))
    }

    /// Upstream `deleteShard`: close connection, remove the file under the
    /// retry (failure logged only), DELETE the row.
    pub async fn delete_shard(&self, shard_id: i64) -> Result<(), StoreError> {
        let db = self.ensure_initialized().await?;
        let row = query_one(
            db.as_ref(),
            crate::sql::GET_SHARD_BY_ID,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        let Some(row) = row else {
            return Ok(()); // no row: no-op (upstream `if (!row) return`)
        };
        let scope = db_row_scope(&row);
        let stored = row.text("db_path").unwrap_or_default();
        let full_path = self.resolve_stored_path(&stored, &scope);
        self.connections.close_connection(&full_path).await?;
        if full_path.exists() {
            let path = full_path.clone();
            let removed = with_sqlite_file_lock_retry(
                || fs_remove(&path),
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &crate::sqlite_files::RetryContext::system(),
            );
            if let Err(error) = removed {
                tracing::warn!(error = %error, db_path = %full_path.display(), "Error deleting shard file");
            }
        }
        db.execute(crate::sql::DELETE_SHARD_ROW, vec![Value::Integer(shard_id)])
            .await?;
        Ok(())
    }

    /// Upstream `archiveShard`: close connection; default archive name
    /// `{full}.{reason}-{pid}-{millis}.bak`; containment -> `ArchivePathEscape`;
    /// rename; DELETE row — registry failure restores the archive then
    /// rethrows; return the path when the archive exists.
    pub async fn archive_shard(
        &self,
        shard_id: i64,
        reason: &str,
        archive_path_override: Option<&Path>,
    ) -> Result<Option<PathBuf>, StoreError> {
        let db = self.ensure_initialized().await?;
        let row = query_one(
            db.as_ref(),
            crate::sql::GET_SHARD_BY_ID,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        let Some(row) = row else {
            return Ok(None); // no row: None (upstream `if (!row) return null`)
        };
        let scope_text = db_row_scope(&row);
        let stored = row.text("db_path").unwrap_or_default();
        let full_path = self.resolve_stored_path(&stored, &scope_text);
        self.connections.close_connection(&full_path).await?;
        let archive_path = match archive_path_override {
            Some(override_path) => override_path.to_path_buf(),
            None => PathBuf::from(format!(
                "{}.{reason}-{}-{}.bak",
                full_path.display(),
                std::process::id(),
                now_millis()
            )),
        };
        // Upstream `relative(scopeDir, archive)`: the archive must sit
        // INSIDE the shard's scope dir (overrides elsewhere escape).
        let scope_dir = self.storage_path.join(format!("{scope_text}s"));
        if !crate::db::assert_path_inside_storage(&normalize(&scope_dir), &normalize(&archive_path))
            .is_ok()
        {
            return Err(StoreError::ArchivePathEscape { path: archive_path });
        }
        if full_path.exists() {
            let Some(proof) = self.connections.exclusive_root() else {
                return Err(StoreError::NoExclusiveRoot {
                    db_path: full_path.clone(),
                });
            };
            let from_claim = proof.claim(&full_path)?;
            let to_claim = proof.claim(&archive_path)?;
            with_sqlite_file_lock_retry(
                || rename_sqlite_database(from_claim, to_claim),
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &crate::sqlite_files::RetryContext::system(),
            )?;
        }
        match db
            .execute(crate::sql::DELETE_SHARD_ROW, vec![Value::Integer(shard_id)])
            .await
        {
            Ok(_) => {}
            Err(error) => {
                if archive_path.exists()
                    && !full_path.exists()
                    && let Some(proof) = self.connections.exclusive_root()
                    && let (Ok(from_claim), Ok(to_claim)) =
                        (proof.claim(&archive_path), proof.claim(&full_path))
                {
                    let _ = with_sqlite_file_lock_retry(
                        || rename_sqlite_database(from_claim, to_claim),
                        FILE_LOCK_RETRY_DELAYS_MS.len(),
                        &crate::sqlite_files::RetryContext::system(),
                    );
                }
                return Err(error);
            }
        }
        Ok(archive_path.exists().then_some(archive_path))
    }

    /// Upstream `reassignShardScope`.
    pub async fn reassign_shard_scope(
        &self,
        shard_id: i64,
        new_scope_hash: &str,
        new_db_path: &Path,
        vector_count: u64,
        is_active: bool,
    ) -> Result<ShardInfo, StoreError> {
        assert_safe_scope_hash(new_scope_hash)
            .map(|_| ())
            .map_err(|error| StoreError::Core {
                message: error.to_string(),
            })?;
        let db = self.ensure_initialized().await?;
        let row = query_one(
            db.as_ref(),
            crate::sql::GET_SHARD_BY_ID,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        let Some(row) = row else {
            return Err(StoreError::Internal {
                message: format!("Shard {shard_id} not found in metadata registry"),
            });
        };
        let scope_text = db_row_scope(&row);
        let stored = self.stored_path_string(new_db_path, &scope_text);
        db.execute(
            crate::sql::REASSIGN_SHARD_SCOPE,
            vec![
                Value::Text(new_scope_hash.to_string()),
                Value::Text(stored),
                Value::Integer(i64::try_from(vector_count).unwrap_or(i64::MAX)),
                Value::Integer(i64::from(is_active)),
                Value::Integer(shard_id),
            ],
        )
        .await?;
        let updated = query_one(
            db.as_ref(),
            crate::sql::GET_SHARD_BY_ID,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        updated
            .map(|row| self.row_to_shard_info(row))
            .ok_or_else(|| StoreError::Internal {
                message: format!("Failed to reassign shard {shard_id}"),
            })
    }

    /// Upstream `cleanupService` shard-level retention sweep (issue 39).
    ///
    /// Migrates each touched shard (v4 migrate-on-touch), selects candidates using
    /// [`RETENTION_CANDIDATES_SELECT`], filters in Rust (skips pinned, stamped grace,
    /// and NULL-stamp grace via `grace_ids`), and deletes survivors in one batch
    /// per shard before adopting the stored count into the registry.
    pub async fn run_retention_sweep(
        &self,
        cutoff: i64,
        retention_ms: i64,
        two_retention_ms: i64,
        grace_ids: &HashSet<String>,
    ) -> Result<SweepOutcome, StoreError> {
        let metadata = self.ensure_initialized().await?;
        let rows = metadata
            .query(
                crate::shard_retention_sql::LIST_ALL_SHARDS_FOR_RETENTION,
                Vec::new(),
            )
            .await?;
        let shards: Vec<ShardInfo> = rows
            .into_iter()
            .map(|row| self.row_to_shard_info(row))
            .collect();

        let mut outcome = SweepOutcome::default();
        let cutoff_sub_retention = cutoff.saturating_sub(retention_ms);

        for shard in shards {
            let db = self.open_shard(&shard.db_path).await?;
            self.ensure_shard_schema(db.as_ref(), self.config.embedding_dimensions)
                .await?;

            let candidates = db
                .query(
                    crate::shard_retention_sql::RETENTION_CANDIDATES_SELECT,
                    vec![Value::Integer(cutoff), Value::Integer(cutoff_sub_retention)],
                )
                .await?;

            let mut shard_to_delete: Vec<String> = Vec::new();
            for row in candidates {
                let Some(id) = row.text("id") else {
                    continue;
                };
                let container_tag = row.text("container_tag").unwrap_or_default();
                let is_pinned = row.i64("is_pinned").unwrap_or(0) == 1;
                let stamp = row.i64("source_prompt_created_at");

                if is_pinned {
                    outcome.protected_memories_skipped += 1;
                    continue;
                }

                if let Some(s) = stamp {
                    // Stamped grace: exempt ⇔ s + 2r > c
                    if s.saturating_add(two_retention_ms) > cutoff {
                        outcome.protected_memories_skipped += 1;
                        continue;
                    }
                } else {
                    // NULL-stamp grace: exempt ⇔ grace_ids.contains(&id)
                    if grace_ids.contains(&id) {
                        outcome.protected_memories_skipped += 1;
                        continue;
                    }
                }

                if container_tag.contains("_user_") {
                    outcome.user_count += 1;
                } else if container_tag.contains("_project_") {
                    outcome.project_count += 1;
                }

                shard_to_delete.push(id);
            }

            if !shard_to_delete.is_empty() {
                crate::vector::delete_memories(db.as_ref(), &shard_to_delete).await?;
                self.adopt_stored_count(db.as_ref(), shard.id).await?;
                outcome.deleted_count += shard_to_delete.len() as u64;
                outcome.deleted_ids.extend(shard_to_delete);
            }
        }

        Ok(outcome)
    }
}

fn db_row_scope(row: &Row) -> String {
    row.text("scope").unwrap_or_else(|| "user".to_string())
}

fn fs_remove(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

fn normalize(path: &Path) -> PathBuf {
    // lexical cleanup — same intent as `resolve` in JS with no symlink walk
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                output.pop();
            }
            std::path::Component::CurDir => {}
            other => output.push(other.as_os_str()),
        }
    }
    output
}

impl ShardRegistry for ShardManager {
    fn get_all_shards<'a>(
        &'a self,
        scope: Scope,
        scope_hash: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<ShardInfo>, StoreError>> {
        Box::pin(self.get_all_shards(scope, scope_hash))
    }

    fn register_existing_shard<'a>(
        &'a self,
        scope: Scope,
        scope_hash: &'a str,
        shard_index: u32,
        db_path: &'a Path,
        vector_count: u64,
        is_active: bool,
    ) -> BoxFuture<'a, Result<ShardInfo, StoreError>> {
        Box::pin(self.register_existing_shard(
            scope,
            scope_hash,
            shard_index,
            db_path,
            vector_count,
            is_active,
        ))
    }

    fn archive_shard<'a>(
        &'a self,
        shard_id: i64,
        reason: &'a str,
        archive_path_override: Option<&'a Path>,
    ) -> BoxFuture<'a, Result<Option<PathBuf>, StoreError>> {
        Box::pin(self.archive_shard(shard_id, reason, archive_path_override))
    }

    fn reassign_shard_scope<'a>(
        &'a self,
        shard_id: i64,
        new_scope_hash: &'a str,
        new_db_path: &'a Path,
        vector_count: u64,
        is_active: bool,
    ) -> BoxFuture<'a, Result<ShardInfo, StoreError>> {
        Box::pin(self.reassign_shard_scope(
            shard_id,
            new_scope_hash,
            new_db_path,
            vector_count,
            is_active,
        ))
    }

    fn delete_shard<'a>(&'a self, shard_id: i64) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(self.delete_shard(shard_id))
    }

    fn get_shard_by_id<'a>(
        &'a self,
        shard_id: i64,
    ) -> BoxFuture<'a, Result<Option<ShardInfo>, StoreError>> {
        Box::pin(self.get_shard_by_id(shard_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn test_manager() -> ShardManager {
        let storage_path = PathBuf::from("/nonexistent/opencode-mem-test");
        let config = StoreConfig {
            storage_path: storage_path.clone(),
            embedding_dimensions: 2,
            embedding_model: "test-model".to_string(),
            max_vectors_per_shard: 1000,
            container_tag_prefix: "opencode".to_string(),
            database_encryption_enabled: false,
            database_encryption_cipher: "aes256gcm".to_string(),
            database_encryption_key: None,
            allow_unguarded_open: false,
        };
        ShardManager::new(
            storage_path,
            config,
            Arc::new(ConnectionManager::new(
                PathBuf::from("/nonexistent/opencode-mem-test"),
                StoreConfig {
                    storage_path: PathBuf::from("/nonexistent/opencode-mem-test"),
                    embedding_dimensions: 2,
                    embedding_model: "test-model".to_string(),
                    max_vectors_per_shard: 1000,
                    container_tag_prefix: "opencode".to_string(),
                    database_encryption_enabled: false,
                    database_encryption_cipher: "aes256gcm".to_string(),
                    database_encryption_key: None,
                    allow_unguarded_open: false,
                },
                Platform::current(),
                Arc::new(crate::db::TursoConnectFactory),
            )),
            Arc::new(crate::platform::SystemProcessProbe),
            None,
        )
    }

    #[test]
    fn test_shard_manager_reset_propagates_poisoned_slot_lock() {
        let manager = test_manager();
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = manager.init.lock().expect("fresh lock");
            panic!("poison");
        }));
        assert!(manager.init.is_poisoned(), "init lock must be poisoned");

        let error = manager
            .reset()
            .expect_err("a poisoned init slot must refuse the reset");
        assert!(matches!(
            error,
            StoreError::LockPoisoned {
                lock: "init",
                operation: "reset",
            }
        ));
    }
}
