//! Episodic record types (W4.1): [`AgentSession`] (persisted) and
//! [`NewAgentSession`] (validated input), plus the payload leaves
//! [`WorkKind`], [`WorkItem`] and [`Decision`].
//!
//! Wire names are pinned by the acceptance tests (R10 net-new domain):
//! `WorkKind` serializes as `"feature"` / `"bugFix"` / `"command"` /
//! `"other"`.
//!
//! `NewAgentSession::into_session(now)` fills the documented defaults and
//! stamps `created_at == updated_at == now`; the row id is minted by the
//! store at insert time (D-1), so the produced session carries an empty `id`
//! and an empty `embedding`.

use mem_store::serde::{Deserialize, Serialize};

use crate::error::EpisodicError;
use crate::payload::SessionMetadata;

/// The kind of work a completed work item represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub enum WorkKind {
    /// New capability.
    Feature,
    /// Defect fix.
    BugFix,
    /// Command/tooling work.
    Command,
    /// Anything else.
    Other,
}

impl WorkKind {
    /// The pinned wire name.
    pub fn as_wire_name(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::BugFix => "bugFix",
            Self::Command => "command",
            Self::Other => "other",
        }
    }
}

/// One completed work item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub struct WorkItem {
    /// What kind of work this was.
    pub kind: WorkKind,
    /// What was done.
    pub description: String,
}

/// One decision taken during the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub struct Decision {
    /// What was decided.
    pub description: String,
}

/// A persisted episodic record bound to the non-optional scope triple
/// `(project_id, session_id, agent_id)`.
///
/// Wire shape is camelCase; the `embedding` never travels the wire
/// (`#[serde(skip)]` — the tool result carries summaries and JSON cannot
/// represent a non-finite float).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub struct AgentSession {
    /// Row id (UUIDv7, minted by the store at insert time).
    pub id: String,
    /// Project scope.
    pub project_id: String,
    /// Session scope.
    pub session_id: String,
    /// Agent scope.
    pub agent_id: String,
    /// Optional parent session (same project; FK `ON DELETE SET NULL`).
    pub parent_session_id: Option<String>,
    /// Human-readable summary (stored verbatim in W4.1; privacy is W5.3).
    pub summary: String,
    /// Structured payload (the `metadata` column).
    pub metadata: SessionMetadata,
    /// Embedding of the composed document (populated at insert time; never
    /// serialized).
    #[serde(skip)]
    pub embedding: Vec<f32>,
    /// Creation time (unix millis).
    pub created_at: i64,
    /// Last-update time (unix millis; equal to `created_at` in W4.1).
    pub updated_at: i64,
    /// Soft-delete stamp (unix millis; `None` = live, D-11).
    pub deleted_at: Option<i64>,
    /// True when this record's parent link was severed by a parent delete
    /// (D-12 write-time flag; never computed from the link at read time).
    pub orphaned: bool,
}

impl AgentSession {
    /// Serializes this record's metadata payload for the `metadata` column.
    pub fn to_metadata_json(&self) -> Result<String, EpisodicError> {
        self.metadata.to_json_string()
    }
}

/// Validated input shape accepted by
/// [`EpisodicStore::insert_session`](crate::store::EpisodicStore::insert_session).
///
/// Defaults live in [`NewAgentSession::into_session`]:
/// - empty `agent_name` → `agent_id` (whitespace-only is preserved so
///   validation can reject it, D-8);
/// - `timestamp` defaults to `now`;
/// - `created_at == updated_at == now`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewAgentSession {
    /// Project scope.
    pub project_id: String,
    /// Session scope.
    pub session_id: String,
    /// Agent scope.
    pub agent_id: String,
    /// Optional parent session id.
    pub parent_session_id: Option<String>,
    /// Human-readable summary.
    pub summary: String,
    /// Display name; empty means "use `agent_id`".
    pub agent_name: String,
    /// Git branch, when known.
    pub git_branch: Option<String>,
    /// Git commit, when known.
    pub git_commit: Option<String>,
    /// Files touched.
    pub files_touched: Vec<String>,
    /// Completed work items.
    pub completed_work: Vec<WorkItem>,
    /// Decisions taken.
    pub decisions: Vec<Decision>,
    /// Open threads.
    pub open_threads: Vec<String>,
    /// Forward-compatible extra keys.
    pub extra: std::collections::BTreeMap<String, mem_store::serde_json::Value>,
    /// Explicit metadata timestamp; `None` means "use `now`".
    pub timestamp: Option<i64>,
}

impl NewAgentSession {
    /// Fills defaults and stamps timestamps; the id and embedding stay empty
    /// until the store persists the record.
    pub fn into_session(self, now: i64) -> AgentSession {
        // D-8: only a truly empty `agent_name` defaults to `agent_id`;
        // whitespace-only is preserved so validation rejects it.
        let agent_name = if self.agent_name.is_empty() {
            self.agent_id.clone()
        } else {
            self.agent_name.clone()
        };
        let metadata = SessionMetadata {
            timestamp: self.timestamp.unwrap_or(now),
            agent_name,
            git_branch: self.git_branch,
            git_commit: self.git_commit,
            files_touched: self.files_touched,
            completed_work: self.completed_work,
            decisions: self.decisions,
            open_threads: self.open_threads,
            extra: self.extra,
        };
        AgentSession {
            id: String::new(),
            project_id: self.project_id,
            session_id: self.session_id,
            agent_id: self.agent_id,
            parent_session_id: self.parent_session_id,
            summary: self.summary,
            metadata,
            embedding: Vec::new(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            orphaned: false,
        }
    }
}
