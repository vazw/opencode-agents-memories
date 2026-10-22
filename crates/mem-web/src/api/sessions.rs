//! The W6.3 session-history stub routes.
//!
//! `GET /api/projects/{project_id}/sessions` and
//! `DELETE /api/projects/{project_id}/sessions/{session_id}` are **new** in the
//! port (upstream has no episodic REST surface). W6.2 lands the route shapes,
//! the auth seam and the [`SessionPort`] seam; the shipped port is the
//! unavailable one, so both answer the documented `501` until W6.3 supplies the
//! implementation (D-13).
//!
//! **Traversal safety:** `project_id` / `session_id` are **data, not
//! authority** — they select rows through the port and are carried as bound
//! values; they never become a filesystem path or an authority. A
//! percent-encoded separator therefore reaches the port as part of the id
//! (the port's own validation owns its shape), it never escapes the store.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Response;

use super::dto::SessionListDto;
use super::envelope;
use super::port_failure;
use super::ports::PortError;
use crate::server::AppState;

/// The documented W6.3 stub message (D-13).
pub const W63_STUB_MESSAGE: &str = "Session history endpoints land with W6.3";

/// Maps a session port failure. The unavailable port is the shipped posture
/// (D-13); the remaining arms are the seam W6.3 will exercise.
fn session_failure(endpoint: &'static str, error: PortError) -> Response {
    match error {
        PortError::Unavailable => envelope::not_implemented(W63_STUB_MESSAGE),
        PortError::Invalid(message) => envelope::bad_request(&message),
        PortError::NotFound => envelope::json(
            axum::http::StatusCode::NOT_FOUND,
            envelope::error("Session not found"),
        ),
        other => port_failure(endpoint, other),
    }
}

/// `GET /api/projects/{project_id}/sessions` — the W6.3 list endpoint.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
) -> Response {
    match state.api.sessions.list_sessions(&project_id).await {
        Ok(sessions) => {
            let data: SessionListDto = sessions;
            envelope::ok(&data)
        }
        Err(error) => session_failure("GET /api/projects/{project_id}/sessions", error),
    }
}

/// `DELETE /api/projects/{project_id}/sessions/{session_id}` — the W6.3
/// delete endpoint.
pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path((project_id, session_id)): Path<(String, String)>,
) -> Response {
    match state
        .api
        .sessions
        .delete_session(&project_id, &session_id)
        .await
    {
        Ok(()) => envelope::ok_empty(),
        Err(error) => session_failure(
            "DELETE /api/projects/{project_id}/sessions/{session_id}",
            error,
        ),
    }
}
