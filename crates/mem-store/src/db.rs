//! The `Db` seam — boxed-future abstraction over a turso connection
//! (blueprint W1.2), plus the production [`TursoConnectFactory`] /
//! [`TursoDb`] and the connect-option / encryption plumbing.
//!
//! `TursoDb` holds BOTH the owned [`turso::Database`] and its
//! [`turso::Connection`]: turso has no `close()` API — release happens when
//! the last handle is dropped (the issue's "deterministic via Rust Drop"
//! bullet is satisfied structurally). Manager close = remove + drop.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use crate::config::StoreConfig;
use crate::error::StoreError;
use crate::platform::{Platform, ProcessProbe};

/// `Box<dyn Future …>`, the trait-object future used across this crate's
/// async seams.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// SQLite value shapes carried across the seam.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Real(value) => Some(*value),
            _ => None,
        }
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

// impl From<u64> for Value is a false infallibility claim: From may not fail, so the trait is the defect (ADR-0013 D-3)
impl TryFrom<u64> for Value {
    type Error = StoreError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        i64::try_from(value)
            .map(Value::Integer)
            .map_err(|_| StoreError::internal("u64 value exceeds i64::MAX"))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Blob(value)
    }
}

impl From<Option<String>> for Value {
    fn from(value: Option<String>) -> Self {
        match value {
            Some(text) => Self::Text(text),
            None => Self::Null,
        }
    }
}

/// One query result row: shared column names + positional values.
#[derive(Debug, Clone)]
pub struct Row {
    columns: Arc<Vec<String>>,
    values: Vec<Value>,
}

impl Row {
    pub fn new(columns: Arc<Vec<String>>, values: Vec<Value>) -> Self {
        Self { columns, values }
    }

    pub fn column_names(&self) -> &[String] {
        &self.columns
    }

    pub fn get(&self, column: &str) -> Option<&Value> {
        self.columns
            .iter()
            .position(|name| name == column)
            .and_then(|index| self.values.get(index))
    }

    pub fn text(&self, column: &str) -> Option<String> {
        self.get(column)
            .and_then(|value| value.as_text().map(str::to_string))
    }

    pub fn i64(&self, column: &str) -> Option<i64> {
        match self.get(column) {
            Some(Value::Integer(value)) => Some(*value),
            Some(Value::Real(value)) => Some(*value as i64),
            _ => None,
        }
    }

    pub fn f64(&self, column: &str) -> Option<f64> {
        match self.get(column) {
            Some(Value::Real(value)) => Some(*value),
            Some(Value::Integer(value)) => Some(*value as f64),
            _ => None,
        }
    }

    pub fn is_null(&self, column: &str) -> bool {
        matches!(self.get(column), Some(Value::Null))
    }
}

/// One statement with positional bind parameters.
#[derive(Debug, Clone)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<Value>,
}

impl Statement {
    pub fn new(sql: impl Into<String>, params: Vec<Value>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

/// An execution result (upstream `run` / batch result info).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecResult {
    pub changes: u64,
    pub last_insert_rowid: i64,
}

/// Batch transaction mode: `Read` = DEFERRED, `Write` = IMMEDIATE.
///
/// Recorded divergence (reviewer note): turso's plain `Connection::batch` is
/// NON-transactional while upstream `"write"` batches are all-or-nothing —
/// this port maps `Write` to `transactional_batch(Immediate)` to preserve
/// the atomicity contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchMode {
    Read,
    Write,
}

/// A `Db` handle shared across consumers.
pub type DbHandle = Arc<dyn Db>;

/// The turso connection seam consumed by every W1.2 service.
pub trait Db: Send + Sync + 'static {
    fn execute(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> BoxFuture<'_, Result<ExecResult, StoreError>>;
    fn query(&self, sql: &str, params: Vec<Value>) -> BoxFuture<'_, Result<Vec<Row>, StoreError>>;
    fn batch(
        &self,
        statements: Vec<Statement>,
        mode: BatchMode,
    ) -> BoxFuture<'_, Result<Vec<ExecResult>, StoreError>>;
}

/// `SELECT … FROM …` single-row convenience (`db.get` upstream).
pub async fn query_one(
    db: &dyn Db,
    sql: &str,
    params: Vec<Value>,
) -> Result<Option<Row>, StoreError> {
    let rows = db.query(sql, params).await?;
    Ok(rows.into_iter().next())
}

// ---------------------------------------------------------------------------
// Connect options + encryption resolution
// ---------------------------------------------------------------------------

/// Encryption keys handed to turso (`EncryptionOpts { cipher, hexkey }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionOptions {
    pub cipher: String,
    pub hexkey: String,
}

/// The always-on experimental features applied to every turso open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentalFeature {
    Encryption,
    MultiprocessWal,
}

/// Per-open options handed to a [`ConnectFactory`].
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    pub experimental: Vec<ExperimentalFeature>,
    pub encryption: Option<EncryptionOptions>,
}

impl ConnectOptions {
    /// Upstream `buildConnectOptions` + `tursoExperimentalFeatures`: the
    /// encryption flag is always on, multiprocess WAL on non-Windows
    /// platforms only. The actual key material is resolved separately
    /// (`resolve_database_encryption`).
    pub fn for_config(config: &StoreConfig, platform: Platform) -> Self {
        let mut experimental = vec![ExperimentalFeature::Encryption];
        if platform != Platform::Windows {
            experimental.push(ExperimentalFeature::MultiprocessWal);
        }
        let _ = &config.database_encryption_cipher;
        Self {
            experimental,
            encryption: None,
        }
    }
}

/// Upstream `resolveDatabaseEncryption` (+ the missing-key guard): disabled
/// configuration yields `None`; enabled configuration resolves or provisions
/// a key (explicit hex, `env://VAR`, `file://path`, a bare path, or the
/// auto-created default key file — upstream
/// `resolveOrCreateDatabaseEncryptionKey`); the resolved cipher + key are
/// handed to turso. [`StoreError::EncryptionKeyMissing`] remains a defensive
/// guard for a resolver that yields `None` while encryption is enabled, which
/// the standard auto-provision path cannot reach.
pub fn resolve_database_encryption(
    config: &StoreConfig,
) -> Result<Option<EncryptionOptions>, StoreError> {
    let resolved = crate::encryption_key::resolve_or_create_database_encryption_key(config)?;
    match resolved {
        Some(hexkey) => Ok(Some(EncryptionOptions {
            cipher: config.database_encryption_cipher.clone(),
            hexkey,
        })),
        None if config.database_encryption_enabled => Err(StoreError::EncryptionKeyMissing {
            storage_path: config.storage_path.clone(),
        }),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Connect factory + production turso implementation
// ---------------------------------------------------------------------------

/// The open seam: builds (or fakes) a `Db` handle for a path.
pub trait ConnectFactory: Send + Sync {
    fn connect(
        &self,
        path: PathBuf,
        opts: ConnectOptions,
    ) -> BoxFuture<'static, Result<DbHandle, StoreError>>;
}

pub const TURSO_CONNECT_FACTORY: TursoConnectFactory = TursoConnectFactory;

/// A cheap cloneable production handle (every call site uses the same unit).
#[inline]
pub fn turso_connect_factory() -> TursoConnectFactory {
    TursoConnectFactory
}

/// The production [`ConnectFactory`] over `turso::Builder`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TursoConnectFactory;

impl ConnectFactory for TursoConnectFactory {
    fn connect(
        &self,
        path: PathBuf,
        opts: ConnectOptions,
    ) -> BoxFuture<'static, Result<DbHandle, StoreError>> {
        Box::pin(async move {
            let mut builder = turso::Builder::new_local(&path.to_string_lossy())
                .experimental_encryption(opts.encryption.is_some());
            let wants_multiprocess_wal = opts
                .experimental
                .iter()
                .any(|feature| matches!(feature, ExperimentalFeature::MultiprocessWal));
            if wants_multiprocess_wal {
                builder = builder.experimental_multiprocess_wal(true);
            }
            if let Some(encryption) = opts.encryption {
                builder = builder.with_encryption(turso::EncryptionOpts {
                    cipher: encryption.cipher,
                    hexkey: encryption.hexkey,
                });
            }
            let database = builder.build().await?;
            let connection = database.connect()?;
            let db: DbHandle = Arc::new(TursoDb {
                database,
                connection,
            });
            Ok(db)
        })
    }
}

/// The production [`Db`] over a real turso `Database` + `Connection`.
///
/// It holds the database itself so dropping the LAST handle releases the
/// file resources (a lone `Connection` recycles into the database pool);
/// there is no turso close API, and `Drop` is the release mechanism
/// (upstream `collectReleasedSqliteHandles` has no Rust analog).
pub struct TursoDb {
    /// Retained on purpose: dropping the LAST handle (connection dropped from
    /// the manager paths + this database) releases the sqlite resources up.
    /// There is no turso `close` — release is structural (Drop).
    #[allow(dead_code)]
    database: turso::Database,
    connection: turso::Connection,
}

fn value_into_turso(value: Value) -> turso::Value {
    match value {
        Value::Null => turso::Value::Null,
        Value::Integer(value) => turso::Value::Integer(value),
        Value::Real(value) => turso::Value::Real(value),
        Value::Text(text) => turso::Value::Text(text),
        Value::Blob(bytes) => turso::Value::Blob(bytes),
    }
}

fn value_from_turso(value: turso::Value) -> Value {
    match value {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(value) => Value::Integer(value),
        turso::Value::Real(value) => Value::Real(value),
        turso::Value::Text(text) => Value::Text(text),
        turso::Value::Blob(bytes) => Value::Blob(bytes),
    }
}

async fn rows_from_turso(mut rows: turso::Rows) -> Result<Vec<Row>, StoreError> {
    let columns = Arc::new(rows.column_names());
    let mut collected = Vec::new();
    while let Some(raw) = rows.next().await? {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            values.push(value_from_turso(raw.get_value(index)?));
        }
        collected.push(Row::new(columns.clone(), values));
    }
    Ok(collected)
}

/// Wraps an execute-shaped turso failure; a non-`Constraint` error text is
/// carried verbatim.
fn turso_error(error: turso::Error) -> StoreError {
    error.into()
}

impl Db for TursoDb {
    fn execute(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> BoxFuture<'_, Result<ExecResult, StoreError>> {
        let params: Vec<turso::Value> = params.into_iter().map(value_into_turso).collect();
        let sql = sql.to_string();
        Box::pin(async move {
            let changes = self
                .connection
                .execute(sql.as_str(), params)
                .await
                .map_err(turso_error)?;
            let last_insert_rowid = self.connection.last_insert_rowid();
            Ok(ExecResult {
                changes,
                last_insert_rowid,
            })
        })
    }

    fn query(&self, sql: &str, params: Vec<Value>) -> BoxFuture<'_, Result<Vec<Row>, StoreError>> {
        let params: Vec<turso::Value> = params.into_iter().map(value_into_turso).collect();
        let sql = sql.to_string();
        Box::pin(async move {
            let rows = self
                .connection
                .query(sql.as_str(), params)
                .await
                .map_err(turso_error)?;
            rows_from_turso(rows).await
        })
    }

    fn batch(
        &self,
        statements: Vec<Statement>,
        mode: BatchMode,
    ) -> BoxFuture<'_, Result<Vec<ExecResult>, StoreError>> {
        let behavior = match mode {
            // Documented divergence: `Connection::batch` is non-transactional
            // upstream ("write" batches are all-or-nothing) — map Write onto
            // the transactional Immediate form (Read stays Deferred).
            BatchMode::Write => turso::transaction::TransactionBehavior::Immediate,
            BatchMode::Read => turso::transaction::TransactionBehavior::Deferred,
        };
        let mut batched = Vec::with_capacity(statements.len());
        for statement in statements {
            batched.push((
                statement.sql,
                statement
                    .params
                    .into_iter()
                    .map(value_into_turso)
                    .collect::<Vec<turso::Value>>(),
            ));
        }
        Box::pin(async move {
            let results = self
                .connection
                .transactional_batch(batched, behavior)
                .await
                .map_err(turso_error)?;
            Ok(results
                .into_iter()
                .map(|result| ExecResult {
                    changes: result.rows_affected(),
                    last_insert_rowid: result.last_insert_rowid().unwrap_or(0),
                })
                .collect())
        })
    }
}

// ---------------------------------------------------------------------------
// Support plumbing used by the connection manager (next steps)
// ---------------------------------------------------------------------------

/// `assert_path_inside_storage` — strictly lexical (no `canonicalize`: the
/// target may not exist). Mirrors upstream `relative(storageRoot, resolve(dbPath))`
/// classification: escape iff the lexically computed relative path is `..`,
/// starts with `..{sep}`, or is absolute. A FILENAME merely containing `..`
/// is a single Normal component and stays allowed (upstream comment).
///
/// BOTH paths are `..`/`.` resolved FIRST (Node `path.relative` resolves
/// before comparing), so a MID-PATH traversal such as
/// `{root}/sub/../../outside.db` resolves outside the root and is rejected —
/// not only a leading `..` component.
pub fn assert_path_inside_storage(storage_root: &Path, db_path: &Path) -> Result<(), StoreError> {
    use std::path::Component;

    let root_is_absolute = storage_root.is_absolute();
    let target_is_absolute = db_path.is_absolute();
    if root_is_absolute != target_is_absolute {
        return Err(escape_error(&db_path.to_string_lossy()));
    }

    let Some(root_normalized) = normalize_lexical(storage_root) else {
        return Err(escape_error(&db_path.to_string_lossy()));
    };
    let Some(target_normalized) = normalize_lexical(db_path) else {
        return Err(escape_error(&db_path.to_string_lossy()));
    };

    let root_components: Vec<Component> = root_normalized.components().collect();
    let target_components: Vec<Component> = target_normalized.components().collect();
    if target_components.len() < root_components.len()
        || target_components[..root_components.len()] != root_components[..]
    {
        return Err(escape_error(&db_path.to_string_lossy()));
    }
    Ok(())
}

/// Lexical `..`/`.` resolution (no filesystem access). Pops the last real
/// component on `ParentDir`, keeps a `..` marker when a relative path has
/// nothing left to pop, and returns `None` when an ABSOLUTE path climbs above
/// its anchor (an escape the caller must reject). Mirrors Node's
/// `path.resolve` + `path.relative` semantics for the containment comparison.
fn normalize_lexical(path: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let mut output = PathBuf::new();
    let mut anchored = false;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                output.push(component.as_os_str());
                anchored = true;
            }
            Component::CurDir => {}
            Component::ParentDir => match output.components().next_back() {
                Some(Component::Normal(_)) => {
                    output.pop();
                }
                Some(Component::ParentDir) | None => {
                    if anchored {
                        return None; // climbs above the filesystem anchor
                    }
                    output.push(component.as_os_str());
                }
                _ => return None,
            },
            Component::Normal(_) => output.push(component.as_os_str()),
        }
    }
    Some(output)
}

fn escape_error(request: &str) -> StoreError {
    StoreError::PathOutsideStorage {
        path: PathBuf::from(request),
    }
}

/// Hint fragments pinned by the open-error tests — upstream text verbatim.
pub const OPEN_LOCK_HINT_WINDOWS: &str = "On Windows the Turso engine still allows only one process as database owner — close other OpenCode sessions.";
pub const OPEN_LOCK_HINT_UNIX: &str = "Another OpenCode session (or an older opencode-mem without multiprocess_wal) still holds the database — close it and retry.";

/// Upstream `LOCK_ERROR_RE` fragments: multi-process file-lock failures.
pub const MULTI_PROCESS_LOCK_FRAGMENTS: [&str; 4] = [
    "File is locked by another process",
    "already open with experimental multiprocess WAL",
    "already open without experimental multiprocess WAL",
    "Locking error",
];

/// Fragment check (upstream `isTursoMultiProcessLockError`).
pub fn is_multi_process_lock_error(message: &str) -> bool {
    MULTI_PROCESS_LOCK_FRAGMENTS
        .iter()
        .any(|fragment| message.contains(fragment))
}

/// Builds [`StoreError::Turso`] with the upstream open-failure text +
/// platform/encryption hints (`never log the encryption hexkey`).
pub fn wrap_turso_open_error(
    db_path: &Path,
    error_message: &str,
    platform: Platform,
    encryption_enabled: bool,
) -> StoreError {
    if is_multi_process_lock_error(error_message) {
        let hint = match platform {
            Platform::Windows => OPEN_LOCK_HINT_WINDOWS,
            Platform::Unix => OPEN_LOCK_HINT_UNIX,
        };
        return StoreError::Turso {
            message: format!(
                "Failed to open database {db_path}: {error_message}. {hint}",
                db_path = db_path.to_string_lossy()
            ),
            is_constraint: false,
        };
    }
    if encryption_enabled {
        return StoreError::Turso {
            message: format!(
                "Failed to open encrypted database {db_path}: {error_message}. \
                 Check databaseEncryptionKey / cipher, or remove encryption config for plaintext shards.",
                db_path = db_path.to_string_lossy(),
            ),
            is_constraint: false,
        };
    }
    StoreError::Turso {
        message: error_message.to_string(),
        is_constraint: false,
    }
}

/// `Send + Sync` assertions (upstream asserts the same for `Connection`).
#[allow(dead_code)]
fn assert_send_sync<T: ?Sized + Send + Sync>() {}

/// Unused-parameter silencer for the probe accessor kept for later steps.
#[allow(dead_code)]
fn probe_placeholder(_probe: &Arc<dyn ProcessProbe>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_try_from_u64_accepts_values_within_i64() {
        assert_eq!(
            Value::try_from(0_u64).expect("0 fits in i64"),
            Value::Integer(0)
        );
        assert_eq!(
            Value::try_from(42_u64).expect("42 fits in i64"),
            Value::Integer(42)
        );
        assert_eq!(
            Value::try_from(i64::MAX as u64).expect("i64::MAX fits"),
            Value::Integer(i64::MAX)
        );
    }

    #[test]
    fn test_value_try_from_u64_rejects_values_above_i64_max() {
        let error = Value::try_from(u64::MAX).expect_err("u64::MAX exceeds i64::MAX");
        assert!(matches!(error, StoreError::Internal { .. }));
        assert!(Value::try_from(i64::MAX as u64 + 1).is_err());
    }
}
