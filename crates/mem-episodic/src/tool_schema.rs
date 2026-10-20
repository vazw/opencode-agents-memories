//! `recall_project_sessions` — the typed tool schema of the query half of
//! US 1.4 (W4.2; net-new, R10).
//!
//! **Schema only; registration is W5.4.** No tool registration, runtime or
//! handler lands here — this module owns the argument contract, its JSON
//! schema (mirroring upstream `tool-schema.ts`'s `{ type, properties,
//! required }` parameter shape) and the result shape.
//!
//! Divergence from the forward-compatible `SessionMetadata.extra` capture
//! (D-16): args are an *input* contract, so unknown fields are denied.

use mem_store::serde::{Deserialize, Serialize};
use mem_store::serde_json::{Value, json};

use crate::error::EpisodicError;
use crate::record::AgentSession;
use crate::timeline::{MAX_TIMELINE_LIMIT, SessionSummary, TimelineFilter};
use crate::validation::{MAX_ID_LEN, check_id, validate_timeline_filter};

/// The tool name pinned by the acceptance criteria.
pub const RECALL_PROJECT_SESSIONS_TOOL_NAME: &str = "recall_project_sessions";

/// The tool description handed to a model (registration is W5.4).
pub const RECALL_PROJECT_SESSIONS_DESCRIPTION: &str = "Recall past agent sessions for a project as a chronological, filtered or semantically ranked timeline.";

/// The three modes of a timeline read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub enum RecallMode {
    /// Newest-first, no ranking (the default).
    #[default]
    Chronological,
    /// Query embedding + exact cosine ranking.
    Semantic,
    /// AND-only optional predicates, chronological order.
    Filtered,
}

/// The validated argument shape of `recall_project_sessions`.
///
/// D-17: every `skip_serializing_if` is paired with `#[serde(default)]`, so
/// the struct's own serialized output always deserializes back.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(
    crate = "mem_store::serde",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct RecallProjectSessionsArgs {
    /// Project scope (the only required field).
    pub project_id: String,
    /// Explicit mode; resolved from `query` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RecallMode>,
    /// The semantic query (semantic mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Inclusive lower bound on `created_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Inclusive upper bound on `created_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    /// Page size (bounded by [`MAX_TIMELINE_LIMIT`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_n_sessions: Option<u32>,
    /// ANY-match over `metadata.files_touched`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touched_files: Option<Vec<String>>,
    /// Exact `agent_id` match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Exact `session_id` match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Return full [`AgentSession`] records instead of [`SessionSummary`]s.
    #[serde(default)]
    pub include_full_sessions: bool,
}

impl RecallProjectSessionsArgs {
    /// Resolves the effective mode.
    ///
    /// An explicit `mode` wins, but `Semantic` without `query` and any
    /// non-semantic mode *with* `query` are `Validation` errors; an absent
    /// mode follows the query's presence.
    pub fn resolved_mode(&self) -> Result<RecallMode, EpisodicError> {
        match self.mode {
            Some(RecallMode::Semantic) => {
                if self.query.is_none() {
                    return Err(EpisodicError::validation(
                        "query",
                        "semantic mode requires a query",
                    ));
                }
                Ok(RecallMode::Semantic)
            }
            Some(mode) => {
                if self.query.is_some() {
                    return Err(EpisodicError::validation(
                        "query",
                        "query is only valid in semantic mode",
                    ));
                }
                Ok(mode)
            }
            None => Ok(if self.query.is_some() {
                RecallMode::Semantic
            } else {
                RecallMode::Chronological
            }),
        }
    }

    /// Validates the resolved mode and the mapped timeline filter.
    pub fn validate(&self) -> Result<(), EpisodicError> {
        check_id("project_id", &self.project_id, MAX_ID_LEN)?;
        self.resolved_mode()?;
        validate_timeline_filter(&self.to_timeline_filter())
    }

    /// The AND-only filter this call maps onto.
    pub fn to_timeline_filter(&self) -> TimelineFilter {
        TimelineFilter {
            since: self.since,
            until: self.until,
            last_n_sessions: self.last_n_sessions.map(|count| count as usize),
            touched_files: self.touched_files.clone(),
            agent_id: self.agent_id.clone(),
            session_id: self.session_id.clone(),
        }
    }

    /// The tool's JSON-schema `parameters` object.
    ///
    /// Content contract (D-18): compared as a parsed [`Value`] by every test,
    /// never as a serialized string — `serde_json` is pinned without
    /// `preserve_order`, so object keys serialize alphabetically.
    pub fn to_json_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "projectId": {"type": "string"},
                "mode": {"type": "string", "enum": ["chronological", "semantic", "filtered"]},
                "query": {"type": "string"},
                "since": {"type": "integer"},
                "until": {"type": "integer"},
                "lastNSessions": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_TIMELINE_LIMIT
                },
                "touchedFiles": {"type": "array", "items": {"type": "string"}},
                "agentId": {"type": "string"},
                "sessionId": {"type": "string"},
                "includeFullSessions": {"type": "boolean"}
            },
            "required": ["projectId"],
            "additionalProperties": false
        })
    }
}

/// The result shape: summaries for listings, full sessions on demand.
///
/// Externally tagged — serde cannot internally tag a newtype sequence
/// variant (the wire names are `summaries` / `fullSessions`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub enum RecallProjectSessionsResult {
    /// The listing shape (chronological/filtered/semantic timelines).
    Summaries(Vec<SessionSummary>),
    /// Full records, when `include_full_sessions` asked for them.
    FullSessions(Vec<AgentSession>),
}
