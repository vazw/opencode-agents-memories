//! Timeline view (W4.2; net-new domain, R10): the project-scoped ordered read
//! over episodic sessions.
//!
//! Three modes share this module's vocabulary:
//!
//! - **chronological** — the plain statement (`created_at DESC, id DESC`);
//! - **filtered** — the same statement with AND-only optional predicates;
//! - **semantic** — the query embedding plus the exact
//!   `vector_distance_cos` ranking (see [`crate::store::EpisodicStore::semantic_timeline`]).
//!
//! Filter values are validated before any SQL runs and are never coerced
//! ([`crate::validation::validate_timeline_filter`]). The `touched_files`
//! predicate is evaluated in Rust against the decoded payload — there is no
//! join table (D-14), so no second normalized copy of the metadata lists can
//! drift.

use mem_store::db::Value;
use mem_store::serde::{Deserialize, Serialize};

use crate::record::AgentSession;

/// Default page size of a timeline listing.
pub const DEFAULT_TIMELINE_LIMIT: usize = 50;
/// Hard upper bound of `TimelineFilter::last_n_sessions` (and of the returned
/// page).
pub const MAX_TIMELINE_LIMIT: usize = 500;

/// The AND-only optional predicate set of a timeline read (D-14).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimelineFilter {
    /// Inclusive lower bound on `created_at` (unix millis).
    pub since: Option<i64>,
    /// Inclusive upper bound on `created_at` (unix millis).
    pub until: Option<i64>,
    /// Page size; `None` means [`DEFAULT_TIMELINE_LIMIT`], `Some(0)` means an
    /// empty page.
    pub last_n_sessions: Option<usize>,
    /// ANY-match (non-empty intersection) over `metadata.files_touched`,
    /// case-sensitive exact string equality.
    pub touched_files: Option<Vec<String>>,
    /// Exact `agent_id` match.
    pub agent_id: Option<String>,
    /// Exact `session_id` match.
    pub session_id: Option<String>,
}

/// The listing shape of one episodic session.
///
/// The full [`AgentSession`] is returned only on demand
/// ([`crate::store::EpisodicStore::session_detail`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub struct SessionSummary {
    /// Row id.
    pub id: String,
    /// Project scope.
    pub project_id: String,
    /// Session scope.
    pub session_id: String,
    /// Agent scope.
    pub agent_id: String,
    /// Display name from the payload.
    pub agent_name: String,
    /// Human-readable summary.
    pub summary: String,
    /// Optional parent row id.
    pub parent_session_id: Option<String>,
    /// Whether the parent link was severed by a delete (D-12).
    pub orphaned: bool,
    /// Files the session touched (decoded from the payload).
    pub files_touched: Vec<String>,
    /// Creation time (unix millis).
    pub created_at: i64,
    /// Last-update time (unix millis).
    pub updated_at: i64,
    /// Cosine similarity, present only on the semantic path.
    ///
    /// D-17: `default` + `skip_serializing_if` are a pair — without the
    /// `default`, a round trip through the tool result would fail on the
    /// omitted key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
}

impl SessionSummary {
    /// Projects a persisted record onto the listing shape.
    pub fn from_session(session: &AgentSession) -> Self {
        Self {
            id: session.id.clone(),
            project_id: session.project_id.clone(),
            session_id: session.session_id.clone(),
            agent_id: session.agent_id.clone(),
            agent_name: session.metadata.agent_name.clone(),
            summary: session.summary.clone(),
            parent_session_id: session.parent_session_id.clone(),
            orphaned: session.orphaned,
            files_touched: session.metadata.files_touched.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            similarity: None,
        }
    }
}

/// The page size a filter asks for (`None` ⇒ [`DEFAULT_TIMELINE_LIMIT`]).
pub fn timeline_limit(filter: &TimelineFilter) -> usize {
    filter.last_n_sessions.unwrap_or(DEFAULT_TIMELINE_LIMIT)
}

/// The Rust-side `touched_files` predicate (D-14: ANY-match, evaluated after
/// the SQL candidate read because `last_n_sessions` must apply last).
pub fn summary_matches(filter: &TimelineFilter, summary: &SessionSummary) -> bool {
    match &filter.touched_files {
        Some(files) => files
            .iter()
            .any(|file| summary.files_touched.contains(file)),
        None => true,
    }
}

/// Which optional predicates the statement carries.
pub fn timeline_predicates(filter: &TimelineFilter) -> mem_store::episodic_sql::TimelinePredicates {
    mem_store::episodic_sql::TimelinePredicates {
        since: filter.since.is_some(),
        until: filter.until.is_some(),
        agent_id: filter.agent_id.is_some(),
        session_id: filter.session_id.is_some(),
    }
}

/// The predicate bind values in statement order: `[since], [until],
/// [agent_id], [session_id]`.
pub fn timeline_predicate_params(filter: &TimelineFilter) -> Vec<Value> {
    let mut params = Vec::new();
    if let Some(since) = filter.since {
        params.push(Value::Integer(since));
    }
    if let Some(until) = filter.until {
        params.push(Value::Integer(until));
    }
    if let Some(agent_id) = &filter.agent_id {
        params.push(Value::Text(agent_id.clone()));
    }
    if let Some(session_id) = &filter.session_id {
        params.push(Value::Text(session_id.clone()));
    }
    params
}
