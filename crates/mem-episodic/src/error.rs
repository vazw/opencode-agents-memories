//! `EpisodicError` — the mem-episodic error surface (W4.1).
//!
//! Dependencies note: `mem-episodic` intentionally declares no serde/tokio/
//! thiserror edges this wave (blueprint \[DEPENDENCIES\]: no dependency
//! changes), so Display/source are hand-written instead of derived through
//! `mem_store`'s re-export. This keeps the crate's public surface explicit.
//!
//! Validation errors carry the field name so callers/tests can tell a
//! rejected scope-binding column (`project_id`, `session_id`, `agent_id`, …)
//! from an embedding-dimension error. A duplicate scope triple is surfaced as
//! the dedicated [`EpisodicError::Conflict`] so turso's unique-violation text
//! never leaks into caller-visible errors (D-6).

use std::fmt;

use mem_embed::error::EmbedError;
use mem_store::error::StoreError;

/// The mem-episodic operation error surface.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum EpisodicError {
    /// Storage-layer pass-through (`mem_store::StoreError`).
    Store(StoreError),
    /// Embedding-provider failure (no row is written).
    Embed(EmbedError),
    /// Rejected input; `field` names the offending column/field.
    Validation {
        /// Machine-readable field name (`project_id`, `embedding`, …).
        field: String,
        /// Human-readable reason.
        message: String,
    },
    /// The `(project_id, session_id, agent_id)` unique triple already exists.
    Conflict,
    /// The addressed row does not exist (or is invisible to the caller's
    /// scope/visibility rule); `entity` names it (`child_session`,
    /// `parent_session`).
    NotFound {
        /// Machine-readable entity name.
        entity: String,
    },
    /// Stored JSON is not a valid payload (fail-closed): the session
    /// metadata column or the milestone contributors column.
    Json {
        /// Parser message.
        message: String,
    },
    /// Filesystem failure outside the `StoreError` surface.
    Io {
        /// OS message.
        message: String,
    },
    /// The milestone summarizer failed (provider error, empty summary or an
    /// oversize summary). A per-agent skip: nothing is written for that
    /// agent.
    Summary {
        /// Human-readable reason.
        message: String,
    },
}

impl EpisodicError {
    /// Builds a validation error for `field`.
    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            message: message.into(),
        }
    }

    /// The Display message (never the enum).
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for EpisodicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "{error}"),
            Self::Embed(error) => write!(formatter, "{error}"),
            Self::Validation { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::Conflict => formatter.write_str(
                "agent session already exists for this (project_id, session_id, agent_id) scope triple",
            ),
            Self::NotFound { entity } => write!(formatter, "no such {entity}"),
            Self::Json { message } => {
                write!(formatter, "invalid JSON payload: {message}")
            }
            Self::Io { message } => formatter.write_str(message),
            Self::Summary { message } => {
                write!(formatter, "summarizer failed: {message}")
            }
        }
    }
}

impl std::error::Error for EpisodicError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Embed(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StoreError> for EpisodicError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<EmbedError> for EpisodicError {
    fn from(error: EmbedError) -> Self {
        Self::Embed(error)
    }
}

impl From<mem_store::serde_json::Error> for EpisodicError {
    fn from(error: mem_store::serde_json::Error) -> Self {
        Self::Json {
            message: error.to_string(),
        }
    }
}

impl From<std::io::Error> for EpisodicError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }
}
