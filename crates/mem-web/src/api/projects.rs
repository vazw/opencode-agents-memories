//! `GET /api/projects` — the project roster (issue 116, D-6).
//!
//! A **product** route, not an upstream parity row: upstream's Agents screen
//! assumes one project and lists its own contributors. This port's `/agents`
//! screen needs the project list itself, so the route answers the union of live
//! `agent_sessions` and `episodic_milestones` — the store's one statement
//! (`sql::PROJECT_ROSTER`), projected through [`SessionPort`](super::ports::SessionPort).
//!
//! `project_id` is **data**: it is returned verbatim (a path-shaped id stays one
//! string) and is bound into every store query, never turned into a filesystem
//! path here.
//!
//! The list is bounded by the caller's `?limit` (default
//! [`PROJECT_DEFAULT_LIMIT`], max [`PROJECT_MAX_LIMIT`]) through the shared
//! strict parser: a value outside `1..=PROJECT_MAX_LIMIT` is the fixed `400`,
//! never a clamp. A full page logs a WARN — the wire stays a bare array, so
//! truncation is documented in the log rather than signalled on the wire.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Response;

use super::envelope;
use super::port_failure;
use crate::server::AppState;

/// The page size when `?limit` is absent.
pub const PROJECT_DEFAULT_LIMIT: usize = 200;

/// The largest accepted `?limit`; anything above is a `400`.
pub const PROJECT_MAX_LIMIT: usize = 500;

/// `GET /api/projects` — the project roster, newest activity first.
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    Query(query): super::QueryPairs,
) -> Response {
    let limit = match super::parse_limit(&query, PROJECT_DEFAULT_LIMIT, PROJECT_MAX_LIMIT) {
        Ok(limit) => limit,
        Err(response) => return response,
    };
    match state.api.sessions.projects(limit).await {
        Ok(rows) => {
            if rows.len() == limit {
                tracing::warn!(
                    limit,
                    projects = rows.len(),
                    "project roster reached the limit bound; the list may be truncated"
                );
            }
            envelope::ok(&rows)
        }
        Err(error) => port_failure("GET /api/projects", error),
    }
}
