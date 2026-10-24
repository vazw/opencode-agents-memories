//! `ProfileError` — the mem-profile error surface (blueprint W3.1).
//!
//! Display strings carry the upstream sentences byte-for-byte wherever an
//! upstream test pins them (`Private content blocked`, `profile cold buffer:
//! …`, `write stalled after {N} bytes`). The enum derives `Clone` so a
//! rejection can be observed without consuming it (the mem-store
//! `StoreError` precedent), and carries the `os_code`/`kind` shape for IO
//! faults instead of a non-`Clone` `#[from]` chain.

/// The mem-profile operation error surface.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ProfileError {
    /// Storage-layer pass-through (`mem_store::StoreError`).
    #[error("{0}")]
    Store(#[from] mem_store::error::StoreError),
    /// Filesystem/OS failure (cold-buffer reads/saves).
    #[error("{message}")]
    Io {
        message: String,
        os_code: Option<i32>,
        kind: Option<std::io::ErrorKind>,
    },
    /// JSON (de)serialization failure.
    #[error("{message}")]
    Json {
        message: String,
        line: usize,
        column: usize,
    },
    /// Cold-buffer snapshot fault: strict load / fail-closed refusal / atomic
    /// save. The message is the upstream sentence (already path-qualified).
    #[error("{0}")]
    ColdBuffer(String),
    /// A coordination record exists but cannot be trusted (malformed row,
    /// unparseable identity). Never treated as a free lock.
    #[error("profile-learning lock: coordination record untrusted")]
    LockUntrusted,
    /// Upstream `Private content blocked` (D-V6 prompt write gate).
    #[error("Private content blocked")]
    PrivateContentBlocked,
    /// Prompt sweep / project-path update failure carrying its own sentence.
    #[error("{0}")]
    Save(String),
    /// An unrecoverable provider-analysis failure: the failing provider's
    /// original message, verbatim (upstream's propagated `Error`). Distinct
    /// from [`ProfileError::Save`] so callers can tell an analysis fault from
    /// a persistence fault, and from `NotReady`, which means "the readiness
    /// gate said no".
    #[error("{0}")]
    Analysis(String),
    /// The daily decay stamp was poisoned (ADR-0013 D-2 `D2-c`): the stamp
    /// **gates** a side effect over on-disk profiles, so a poisoned gate must
    /// refuse rather than continue on a value its owner left behind.
    ///
    /// The payload is two `&'static str` literals — never `PoisonError`'s
    /// `Box<dyn Any>`.
    #[error("lock `{lock}` poisoned while {operation}; refusing to continue")]
    LockPoisoned {
        /// The guarded value's name (a closed set of literals).
        lock: &'static str,
        /// The operation that found the lock poisoned.
        operation: &'static str,
    },
}

impl ProfileError {
    /// Builds an [`ProfileError::Io`] from a real IO error.
    pub fn io(error: &std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
            os_code: error.raw_os_error(),
            kind: Some(error.kind()),
        }
    }

    /// Builds an [`ProfileError::Io`] with an explicit message.
    pub fn io_message(message: impl Into<String>, error: &std::io::Error) -> Self {
        Self::Io {
            message: message.into(),
            os_code: error.raw_os_error(),
            kind: Some(error.kind()),
        }
    }

    /// Builds an [`ProfileError::Json`] from a serde failure.
    pub fn json(error: &serde_json::Error) -> Self {
        Self::Json {
            message: error.to_string(),
            line: error.line(),
            column: error.column(),
        }
    }

    /// The Display message, for call sites that need the sentence (never the
    /// enum) — the mem-store `StoreError::message` precedent.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl From<std::io::Error> for ProfileError {
    fn from(error: std::io::Error) -> Self {
        Self::io(&error)
    }
}

impl From<serde_json::Error> for ProfileError {
    fn from(error: serde_json::Error) -> Self {
        Self::json(&error)
    }
}
