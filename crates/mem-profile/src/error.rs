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
    /// The profile erase failed, so the prompt was deliberately NOT deleted:
    /// the source must survive so a retry still has an event that re-triggers
    /// erasure (47.1-B2′ / E2 — fail toward privacy).
    #[error("profile erasure failed for prompt {prompt_id}; the prompt was kept: {message}")]
    ErasureFailed { prompt_id: String, message: String },
    /// The profile is erased, then the prompt DELETE failed. Privacy is met; the
    /// caller must know the source row survives.
    #[error(
        "profile erased ({profiles_erased} profile(s)) but deleting prompt {prompt_id} failed: {message}"
    )]
    PromptDeleteAfterErasure {
        prompt_id: String,
        profiles_erased: u64,
        message: String,
    },
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

/// The closed set of `profile.learning_round_failed` reason tokens.
///
/// One token per [`ProfileError`] variant: a classification an operator or a
/// log consumer may branch on. Never a message, a path, a key, a prompt
/// fragment or parser text. `ALL` is the set a table test pins; it is a slice,
/// not a sized array, so the set has no length to drift.
pub mod failure_reason {
    pub const STORE: &str = "store";
    pub const IO: &str = "io";
    pub const JSON: &str = "json";
    pub const COLD_BUFFER: &str = "cold-buffer";
    pub const LOCK_UNTRUSTED: &str = "lock-untrusted";
    pub const PRIVATE_CONTENT: &str = "private-content";
    pub const SAVE: &str = "save";
    pub const ANALYSIS: &str = "analysis";
    pub const ERASURE_FAILED: &str = "erasure-failed";
    pub const PROMPT_DELETE_AFTER_ERASURE: &str = "prompt-delete-after-erasure";
    pub const LOCK_POISONED: &str = "lock-poisoned";

    /// Every token, in declaration order.
    pub const ALL: &[&str] = &[
        STORE,
        IO,
        JSON,
        COLD_BUFFER,
        LOCK_UNTRUSTED,
        PRIVATE_CONTENT,
        SAVE,
        ANALYSIS,
        ERASURE_FAILED,
        PROMPT_DELETE_AFTER_ERASURE,
        LOCK_POISONED,
    ];
}

impl ProfileError {
    /// The closed-set token for a round failure.
    ///
    /// The match has NO wildcard arm: `#[non_exhaustive]` binds only external
    /// crates, so a new variant is a compile error here until it gets a token.
    pub fn failure_reason(&self) -> &'static str {
        match self {
            Self::Store(_) => failure_reason::STORE,
            Self::Io { .. } => failure_reason::IO,
            Self::Json { .. } => failure_reason::JSON,
            Self::ColdBuffer(_) => failure_reason::COLD_BUFFER,
            Self::LockUntrusted => failure_reason::LOCK_UNTRUSTED,
            Self::PrivateContentBlocked => failure_reason::PRIVATE_CONTENT,
            Self::Save(_) => failure_reason::SAVE,
            Self::Analysis(_) => failure_reason::ANALYSIS,
            Self::ErasureFailed { .. } => failure_reason::ERASURE_FAILED,
            Self::PromptDeleteAfterErasure { .. } => failure_reason::PROMPT_DELETE_AFTER_ERASURE,
            Self::LockPoisoned { .. } => failure_reason::LOCK_POISONED,
        }
    }
}
