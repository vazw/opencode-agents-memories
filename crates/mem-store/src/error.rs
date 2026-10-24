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

use std::path::PathBuf;

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
