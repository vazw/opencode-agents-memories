//! Turso shard manager — port of upstream
//! `src/services/turso/shard-manager.ts` (blueprint W1.2 step 8).
//!
//! Upstream behaviors preserved: the registry (`metadata.db`) is the SINGLE
//! writer of shard lifecycle; shard FILES are initialized before the
//! registry row insert (orphan-row guard comment preserved); unique-
//! violation dedup in `create_shard`; rotation = mark read-only + create
//! next; incompatible/corrupt shards REJECT writes with the original
//! database left untouched (`ShardIncompatible`).
//!
//! Divergences (reviewer notes): `withScopeWriteLock` is DEFERRED to W1.3
//! with its first caller; `CONFIG.storagePath` globals become the
//! `storage_path` field; `metadata: Mutex<Option<DbHandle>>` +
//! `OnceCell` init cell preserve upstream `ensureInitialized` (in-flight
//! dedup + revalidation: every call re-asks the connection manager; a
//! swapped handle is adopted and re-initialized) without holding std
//! guards across `.await`.

#![allow(dead_code)]

use std::collections::HashMap;
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

/// The manager.
pub struct ShardManager {
    storage_path: PathBuf,
    config: StoreConfig,
    connections: Arc<ConnectionManager>,
    probe: Arc<dyn ProcessProbe>,
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
    ) -> Self {
        Self {
            storage_path: storage_path.canonicalize().unwrap_or(storage_path),
            config,
            connections,
            probe,
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

    /// The process probe (live-lock checks in the migration paths).
    pub fn process_probe(&self) -> Arc<dyn ProcessProbe> {
        Arc::clone(&self.probe)
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

    /// Upstream `join(`${scope}s`, basename)` + `replace(/\\/g, "/")`.
    fn stored_path_string(&self, full_path: &Path, scope: Scope) -> String {
        let file_name = full_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        format!("{}/{}", scope.as_str(), file_name).replace('\\', "/")
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
                self.init_metadata_db(&current).await?;
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
        crate::schema_migrations::apply_schema_migrations(
            db.as_ref(),
            &crate::schema_migrations::metadata_db_migrations(),
            &crate::schema_migrations::MigrationOptions {
                db_path: Some(
                    self.storage_path
                        .join(crate::operation_lock::METADATA_DB_NAME),
                ),
                label: "metadata.db",
            },
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
        let stored = self.stored_path_string(&full_path, scope);
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
            Ok(result) => Ok(ShardInfo {
                id: result.last_insert_rowid,
                scope,
                scope_hash: scope_hash.to_string(),
                shard_index,
                db_path: full_path,
                vector_count: 0,
                is_active: true,
                created_at: now,
            }),
            Err(error) => {
                let unique = error.message().contains("UNIQUE constraint failed")
                    || error.message().contains("SQLITE_CONSTRAINT");
                if unique {
                    // Upstream dedup: the re-fetch IS the return value.
                    let existing = self
                        .find_shard_by_key(scope, scope_hash, shard_index)
                        .await?;
                    if let Some(existing) = existing {
                        return Ok(existing);
                    }
                }
                Err(error)
            }
        }
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
        let stored = self.stored_path_string(db_path, scope);
        let now = now_millis();
        db.execute(
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
        .await?;
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

    /// Upstream `initShardDb` — validated dims, ensure_shard_schema, then
    /// `INSERT OR REPLACE` dims/model metadata.
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
    pub async fn ensure_shard_schema(
        &self,
        db: &dyn Db,
        dimensions: u32,
    ) -> Result<i64, StoreError> {
        crate::schema_migrations::apply_schema_migrations(
            db,
            &crate::schema_migrations::memory_shard_migrations(dimensions),
            &crate::schema_migrations::MigrationOptions {
                db_path: None,
                label: "memory-shard",
            },
        )
        .await
    }

    fn row_to_shard_info(&self, row: Row) -> ShardInfo {
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

    /// Upstream `syncShardVectorCount`.
    async fn sync_shard_vector_count(&self, shard: ShardInfo) -> Result<ShardInfo, StoreError> {
        let db = self.connections.get_connection(&shard.db_path).await?;
        let row = query_one(&*db, crate::sql::COUNT_ALL_VECTORS, Vec::new()).await?;
        let count = row.and_then(|row| row.i64("count")).unwrap_or(0).max(0) as u64;
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
        // Upstream budget: 3 attempts; both terminal branches return, so the
        // third iteration is unreachable unless a future branch keeps looping.
        #[allow(clippy::never_loop)]
        for _attempt in 0..3 {
            match self.get_active_shard(scope, scope_hash).await? {
                None => {
                    return self.create_shard(scope, scope_hash, 0).await;
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
                    let shard = self.sync_shard_vector_count(shard).await?;
                    if shard.vector_count >= u64::from(self.config.max_vectors_per_shard) {
                        self.mark_shard_read_only_off(shard.id).await?;
                        return self
                            .create_shard(scope, scope_hash, shard.shard_index + 1)
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

    /// Upstream `incrementVectorCount`.
    pub async fn increment_vector_count(&self, shard_id: i64) -> Result<(), StoreError> {
        let db = self.ensure_initialized().await?;
        db.execute(
            crate::sql::INCREMENT_VECTOR_COUNT,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        Ok(())
    }

    /// Upstream `decrementVectorCount` (`AND vector_count > 0` in the SQL).
    pub async fn decrement_vector_count(&self, shard_id: i64) -> Result<(), StoreError> {
        let db = self.ensure_initialized().await?;
        db.execute(
            crate::sql::DECREMENT_VECTOR_COUNT,
            vec![Value::Integer(shard_id)],
        )
        .await?;
        Ok(())
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
            let from = full_path.clone();
            let to = archive_path.clone();
            with_sqlite_file_lock_retry(
                || rename_sqlite_database(&from, &to),
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
                if archive_path.exists() && !full_path.exists() {
                    let from = archive_path.clone();
                    let to = full_path.clone();
                    let _ = with_sqlite_file_lock_retry(
                        || rename_sqlite_database(&from, &to),
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
        let stored = self.stored_path_string_from(new_db_path, &scope_text);
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

    fn stored_path_string_from(&self, path: &Path, scope: &str) -> String {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        format!("{scope}s/{file_name}").replace('\\', "/")
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
