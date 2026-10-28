//! `StoreError` — the mem-store error surface (blueprint W1.2).
//!
//! The port carries upstream error text byte-for-byte wherever a test or
//! tool pins it (Display fragments below). Every error fan-out target —
//! concurrent open dedup, shared ready-gate awaits — must be able to clone
//! one rejection and hand the same value to every waiter (exact upstream
//! single-rejection semantics), so the enum derives `Clone`.
//!
//! Recorded divergence from ADR-0002 (reviewer note): source chains of IO /
//! turso errors are captured as `{ message, os_code / kind }` instead of
//! non-`Clone` `#[from]` fields — a clone of the richer error would lose the
//! chain anyway.

use std::path::{Path, PathBuf};

/// One shard whose stored embedding identity disagrees with the configured
/// one (W3's model authority).
///
/// Carries ids, model names and the expected model only: never content and
/// never a path (the refusal is answered on the HTTP surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardModelMismatchEntry {
    /// The registry row id of the mismatched shard.
    pub id: i64,
    /// The shard's stored `embedding_model`, when it has one.
    pub stored_model: Option<String>,
    /// The configured model the shard had to prove.
    pub expected: String,
}

/// The model-authority refusal: every mismatched shard of one scope, plus the
/// one operator remedy that re-embeds it.
///
/// `remedy` is built from [`crate::shard_facts::REEMBED_SCOPE_COMMAND`] by
/// [`crate::shard_facts::model_mismatch`], so the store error, the HTTP body
/// and the operator log cannot diverge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardModelMismatch {
    /// Every mismatched shard of the request's scope, collected before the
    /// refusal is raised.
    pub shards: Vec<ShardModelMismatchEntry>,
    /// The exact command an operator runs to re-embed the scope.
    pub remedy: String,
}

impl std::fmt::Display for ShardModelMismatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("embedding model mismatch: ")?;
        for (index, shard) in self.shards.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            match shard.stored_model.as_deref() {
                Some(model) => write!(
                    formatter,
                    "shard {} (stored model '{model}', expected '{}')",
                    shard.id, shard.expected
                )?,
                None => write!(
                    formatter,
                    "shard {} (stored model none, expected '{}')",
                    shard.id, shard.expected
                )?,
            }
        }
        write!(formatter, "; re-embed the scope: {}", self.remedy)
    }
}

/// The mem-store operation error surface.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// Filesystem/OS failure (`os_code` = `raw_os_error()`, `kind` =
    /// `io::ErrorKind`). Display is the IO message itself.
    #[error("{message}")]
    Io {
        message: String,
        os_code: Option<i32>,
        kind: Option<std::io::ErrorKind>,
    },
    /// Turso/libSQL engine error (`is_constraint` = `matches!(e, turso::Error::Constraint(_))`).
    #[error("{message}")]
    Turso {
        message: String,
        is_constraint: bool,
    },
    /// `mem_core` scope/config/parser pass-through.
    #[error("{message}")]
    Core { message: String },
    /// JSON (de)serialization failures.
    #[error("{message}")]
    Json { message: String },
    /// Storage-path containment violation at connection open.
    #[error("Refusing to open database outside storagePath: {path}")]
    PathOutsideStorage { path: PathBuf },
    /// Shard-path containment violation (`get_shard_path`).
    #[error("Shard path escapes storage directory: {path}")]
    ShardPathEscape { path: PathBuf },
    /// Archive-override containment violation (`archive_shard`).
    #[error("Archive path escapes storage directory: {path}")]
    ArchivePathEscape { path: PathBuf },
    /// Journal-path containment violation during swap recovery.
    #[error("Path-migrate recovery path escapes storage: {path}")]
    RecoveryPathEscape { path: PathBuf },
    /// `embeddingDimensions` outside 1..=65536 (upstream
    /// `getValidatedEmbeddingDimensions`).
    #[error("Invalid embeddingDimensions config: {dimensions}")]
    InvalidEmbeddingDimensions { dimensions: i64 },
    /// An incompatible/corrupt shard blocks writes; the original database is
    /// never moved or emptied (upstream `getWriteShard` guard).
    #[error(
        "Shard {scope}/{scope_hash}#{shard_index} is incompatible or corrupt. \
         The original database was left untouched at {db_path}; \
         run the migration or restore it before writing."
    )]
    ShardIncompatible {
        scope: String,
        scope_hash: String,
        shard_index: u64,
        db_path: PathBuf,
    },
    /// A live migration operation lock is held (upstream
    /// `assertNoTursoMigrationInProgress` text, byte-compatible: must contain
    /// `writes are temporarily blocked`).
    #[error(
        "Database migration is in progress{operation} \
         in process {pid}; writes are temporarily blocked",
        operation = .operation
            .as_deref()
            .map(|op| format!(" ({op})"))
            .unwrap_or_default(),
    )]
    MigrationLocked { pid: u32, operation: Option<String> },
    /// A sidecar mutation was refused because the database-file probe did not
    /// prove exclusivity (`Held` or `Unknown`). Raised **before** the first
    /// unlink, so nothing on disk has changed.
    #[error("Refusing to mutate database {db_path}: exclusivity is not proven ({reason})")]
    ExclusivityRefused {
        /// The database the mutation would have touched.
        db_path: PathBuf,
        /// Why the probe refused (never a holder identity: the probe has none).
        reason: String,
    },
    /// A destructive operation needed the storage root's exclusivity proof, and
    /// this process holds no engine lock for it. The database is untouched.
    #[error(
        "Refusing to mutate database {db_path}: this process holds no engine lock \
         (exclusivity proof) for its storage root"
    )]
    NoExclusiveRoot {
        /// The database the operation would have touched. Carried instead of the
        /// root because sites such as the schema runner only have `db_path` in
        /// hand (`ConnectionManager` owns the root).
        db_path: PathBuf,
    },
    /// A schema migration failed after taking a backup. The runner still borrows
    /// the database handle, so it must not unlink the file it has open: the
    /// caller drops its handle and settles the restore (issue 46.4).
    #[error("{0}")]
    MigrationRestorePending(Box<MigrationRestorePending>),
    /// Encryption is enabled but no key is resolvable. Upstream
    /// `resolveOrCreateDatabaseEncryptionKey` auto-provisions the default key
    /// file when encryption is enabled without an explicit key, so in practice
    /// this guard fires only if a resolver returns `None` while encryption is
    /// enabled (unreachable through the standard auto-provision path). An
    /// explicitly configured but unreadable/invalid key source instead fails
    /// as [`StoreError::Internal`]. The hexkey itself is never part of any
    /// error or log.
    #[error(
        "Database encryption is enabled but no databaseEncryptionKey is available \
         for storage {storage_path}. Configure databaseEncryptionKey to use encrypted shards."
    )]
    EncryptionKeyMissing { storage_path: PathBuf },
    /// Upstream plain-`new Error(...)` throw sites carried verbatim
    /// (registry failures, swap invariants, staged-count mismatch).
    #[error("{message}")]
    Internal { message: String },
    /// A `std` lock was poisoned on a value whose failure cannot be recovered
    /// (ADR-0013 D-2): the guard protects a cross-field protocol, key
    /// material, or a side-effect gate, so the call refuses instead of
    /// continuing on state the panicking thread left behind.
    ///
    /// The payload is two `&'static str` literals from the per-site closed
    /// set — never `PoisonError`'s `Box<dyn Any>`, never a path, never key
    /// material.
    #[error(
        "lock `{lock}` poisoned while {operation}; refusing to continue on possibly half-updated state"
    )]
    LockPoisoned {
        /// The guarded value's name (a closed set of literals).
        lock: &'static str,
        /// The operation that found the lock poisoned.
        operation: &'static str,
    },
    /// A database stamped by a newer build than this one knows how to read
    /// (ADR-0021). Checked against the file's current `user_version` before
    /// anything is applied; a newer file has no pending migrations, so a
    /// check inside the apply loop would never run.
    #[error(
        "Database {label} is at schema version {found}, but this build supports at most {supported}"
    )]
    SchemaNewerThanBuild {
        label: &'static str,
        found: i64,
        supported: i64,
    },
    /// A shard of the scope could not be read during a scope-wide memory-id
    /// lookup, so the deterministic write was deferred rather than risk a
    /// duplicate. Never a "not found".
    ///
    /// Field types (a recorded deviation from the wave blueprint's declared
    /// `String`s): `scope` is a closed literal set (`Scope::as_str` is
    /// `&'static str`, exactly like `LockPoisoned`'s fields) and `message` is
    /// an immutable diagnostic, so `Box<str>` serves it. The plain-`String`
    /// shape made `StoreError` 136 bytes, which trips
    /// `clippy::result_large_err` on every `Result<_, StoreError>` in the
    /// workspace under the commit gate's `-D warnings`; every field name,
    /// field order, the variant name and the exact Display text are unchanged.
    #[error(
        "Shard {scope}/{scope_hash}#{shard_index} could not be read at {db_path} while \
         resolving memory id {id} across the scope ({message}); the write was deferred"
    )]
    ScopeShardUnreadable {
        scope: &'static str,
        scope_hash: String,
        shard_index: u64,
        db_path: PathBuf,
        id: String,
        message: Box<str>,
    },
    /// The mixed-version open guard refused the open **before** turso touched the
    /// file (or, for the post-open re-probe, after it and with the handle dropped).
    ///
    /// Sized to fit the budget: `PathBuf + OpenDirection` = 48 bytes
    /// (`OpenDirection` is 24 — `PathBuf` 24 + the wider `Unknown` payload), far
    /// under the 112-byte high-water mark set by
    /// [`StoreError::ScopeShardUnreadable`]. No pid, no holder identity: an OFD
    /// lock carries none.
    #[error("Refusing to open database {db_path}: {direction}")]
    OpenBlocked {
        db_path: PathBuf,
        direction: OpenDirection,
    },
    /// A legacy (single-process) WAL option set was refused by construction
    /// (owner decision O3): this build opens only in multiprocess WAL mode. The
    /// trailing positional arguments are the constants from
    /// [`crate::open_guard`] — the spelling lives in one place.
    #[error(
        "Refusing to open database {db_path} in {mode} WAL mode: this build opens only in \
         multiprocess WAL mode (owner decision O3). Set {}=1 (or the config key {}) to bypass \
         the mixed-version guard",
        crate::open_guard::ALLOW_UNGUARDED_OPEN_ENV,
        crate::open_guard::allow_unguarded_open_key()
    )]
    UnsupportedOpenMode {
        db_path: PathBuf,
        mode: &'static str,
    },
    /// A shard's stored embedding identity disagrees with the configured one
    /// (W3): every vector-producing write is refused before it reaches the
    /// shard, never appended into a foreign latent space.
    ///
    /// Appended last and boxed: the mismatch list is unbounded and
    /// `StoreError`'s 112-byte high-water mark is a budget tripwire, not the
    /// enforcement.
    #[error("{0}")]
    ShardModelMismatch(Box<ShardModelMismatch>),
}

/// The value a failing schema migration hands its caller: the runner never
/// unlinks a database it has open. The caller closes its own handle, then calls
/// [`MigrationRestorePending::restore`].
///
/// It lives beside [`StoreError`] because it *is* an error payload; the restore
/// machinery itself lives in [`crate::schema_migrations`].
#[derive(Debug, Clone)]
pub struct MigrationRestorePending {
    db_path: PathBuf,
    backup_path: PathBuf,
    original: StoreError,
}

impl MigrationRestorePending {
    pub(crate) fn new(db_path: PathBuf, backup_path: PathBuf, original: StoreError) -> Self {
        Self {
            db_path,
            backup_path,
            original,
        }
    }

    /// The schema failure the caller should re-raise after a successful restore.
    pub fn original(&self) -> &StoreError {
        &self.original
    }

    /// The database that still exists (the runner did not unlink it).
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// The backup the caller's [`Self::restore`] moves back into place.
    pub fn backup_path(&self) -> &Path {
        &self.backup_path
    }
}

impl std::fmt::Display for MigrationRestorePending {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "schema migration failed for {} and the restore from {} awaits the caller: {}",
            self.db_path.display(),
            self.backup_path.display(),
            self.original
        )
    }
}

/// The direction the open guard saw, never the holder's identity: an OFD lock
/// carries no `l_pid` (`turso_core-0.8.3/io/unix.rs:125-180` sets `l_pid: 0`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenDirection {
    /// A live single-process (legacy) opener holds an exclusive lock on the file.
    LegacyAuthority,
    /// The probe or the marker could not answer. `errno` is `None` when there is
    /// no OS error to name (a platform without a probe, a defensive default).
    Unknown { errno: Option<i32> },
    /// A live opener recorded a different WAL mode for this storage root.
    ModeMarkerConflict { recorded: &'static str },
}

impl std::fmt::Display for OpenDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LegacyAuthority => f.write_str(
                "a live single-process (legacy) opener holds an exclusive lock on this database \
                 file; mixing WAL modes silently loses rows",
            ),
            Self::Unknown { errno } => write!(
                f,
                "the WAL-mode probe or the `{}` marker could not determine whether another \
                 opener holds this database (errno {errno:?}); the guard refuses rather than \
                 guess. Set {}=1 (or the config key {}) to bypass the mixed-version guard",
                crate::open_guard::WAL_MODE_MARKER_FILE,
                crate::open_guard::ALLOW_UNGUARDED_OPEN_ENV,
                crate::open_guard::allow_unguarded_open_key()
            ),
            Self::ModeMarkerConflict { recorded } => write!(
                f,
                "a live opener recorded `{recorded}` WAL mode for this storage root"
            ),
        }
    }
}

impl StoreError {
    /// `Io` without OS details (wrap an `io::Error` — [`StoreError::from`] —
    /// to capture `raw_os_error` + `kind`).
    pub fn io_message(message: impl Into<String>) -> Self {
        Self::Io {
            message: message.into(),
            os_code: None,
            kind: None,
        }
    }

    /// Non-constraint `Turso` message wrapper.
    pub fn turso_message(message: impl Into<String>) -> Self {
        Self::Turso {
            message: message.into(),
            is_constraint: false,
        }
    }

    /// Upstream plain-`new Error(...)` site.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    /// `MigrationLocked` constructor.
    pub fn migration_locked(pid: u32, operation: Option<String>) -> Self {
        Self::MigrationLocked { pid, operation }
    }

    /// Upstream `isUniqueConstraintError` (shard-manager.ts L27-30): the
    /// message mentions the unique violation directly.
    pub fn is_unique_violation(&self) -> bool {
        let message = self.to_string();
        message.contains("UNIQUE constraint failed") || message.contains("SQLITE_CONSTRAINT")
    }

    /// The human-readable kernel of the error (e.g. open-error wrapping).
    pub fn message(&self) -> String {
        match self {
            Self::Io { message, .. }
            | Self::Turso { message, .. }
            | Self::Core { message }
            | Self::Json { message }
            | Self::Internal { message } => message.clone(),
            other => other.to_string(),
        }
    }

    /// `true` for exactly the two open-guard refusals.
    ///
    /// `ConnectionManager::open_database` rewrites every factory error through
    /// `wrap_turso_open_error`; without this classifier the engine's paths would
    /// see a generic [`StoreError::Turso`] and issue 45's "typed refusal" contract
    /// would be false in production while green at the factory.
    #[must_use]
    pub fn is_open_guard_refusal(&self) -> bool {
        matches!(
            self,
            Self::OpenBlocked { .. } | Self::UnsupportedOpenMode { .. }
        )
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
            os_code: error.raw_os_error(),
            kind: Some(error.kind()),
        }
    }
}

impl From<turso::Error> for StoreError {
    fn from(error: turso::Error) -> Self {
        Self::Turso {
            message: error.to_string(),
            is_constraint: matches!(error, turso::Error::Constraint(_)),
        }
    }
}

impl From<mem_core::error::Error> for StoreError {
    fn from(error: mem_core::error::Error) -> Self {
        Self::Core {
            message: error.to_string(),
        }
    }
}

impl From<mem_core::error::ScopeError> for StoreError {
    fn from(error: mem_core::error::ScopeError) -> Self {
        Self::Core {
            message: error.to_string(),
        }
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json {
            message: error.to_string(),
        }
    }
}

impl From<StoreError> for std::io::Error {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Io {
                os_code: Some(code),
                ..
            } => std::io::Error::from_raw_os_error(code),
            StoreError::Io {
                kind: Some(kind), ..
            } => kind.into(),
            other => std::io::Error::other(other.message()),
        }
    }
}
