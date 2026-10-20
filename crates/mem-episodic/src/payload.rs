//! `SessionMetadata` — the structured payload stored in the `metadata`
//! column (W4.1; D-3: scope-binding columns + JSON payload).
//!
//! Wire shape is camelCase (matching the port's JSON conventions). Unknown
//! top-level keys are captured into [`SessionMetadata::extra`] instead of
//! being rejected, so a record written by a newer peer survives a round trip
//! (forward compatibility, fail-closed only on malformed JSON).
//!
//! `mem-episodic` declares no serde edge this wave (blueprint
//! \[DEPENDENCIES\]), so the derives are pointed at mem-store's re-export with
//! `#[serde(crate = "mem_store::serde")]`.

use std::collections::BTreeMap;

use mem_store::serde::{Deserialize, Serialize};
use mem_store::serde_json;

use crate::error::EpisodicError;
use crate::record::{Decision, WorkItem};

/// Structured session payload (the `metadata` TEXT column).
///
/// Normalization/dimension fields:
/// - `timestamp` — record time (unix millis, ≥ 0; D-8 validation);
/// - `agent_name` — display name; empty is defaulted to `agent_id` before
///   validation (D-8), whitespace-only is rejected;
/// - `git_branch` / `git_commit` — optional git context;
/// - `files_touched`, `completed_work`, `decisions`, `open_threads` — list
///   payload kept as TEXT-JSON (D-3: SQLite cannot index them usefully and a
///   second normalized copy would drift);
/// - `extra` — unknown/future top-level keys, preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub struct SessionMetadata {
    /// Record timestamp (unix milliseconds, ≥ 0).
    pub timestamp: i64,
    /// Display name for the writing agent (defaults to `agent_id`).
    #[serde(default)]
    pub agent_name: String,
    /// Git branch, when the session ran inside a repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    /// Git commit (7–64 hex characters), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    /// Files the session touched.
    #[serde(default)]
    pub files_touched: Vec<String>,
    /// Work items completed during the session.
    #[serde(default)]
    pub completed_work: Vec<WorkItem>,
    /// Decisions taken during the session.
    #[serde(default)]
    pub decisions: Vec<Decision>,
    /// Threads left open for a later session.
    #[serde(default)]
    pub open_threads: Vec<String>,
    /// Unknown/future keys, preserved verbatim on round trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl SessionMetadata {
    /// Serializes to the stored JSON text.
    pub fn to_json_string(&self) -> Result<String, EpisodicError> {
        serde_json::to_string(self).map_err(|error| EpisodicError::Json {
            message: error.to_string(),
        })
    }

    /// Parses stored JSON text (fail-closed: malformed text is an error, never
    /// a silently coerced default).
    pub fn from_json_str(json: &str) -> Result<Self, EpisodicError> {
        serde_json::from_str(json).map_err(|error| EpisodicError::Json {
            message: error.to_string(),
        })
    }
}
