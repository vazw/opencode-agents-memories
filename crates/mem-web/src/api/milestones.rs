//! `GET /api/projects/{project_id}/milestones` — the W6.6 agent-contribution
//! projection.
//!
//! A **product** view, not an upstream parity row: upstream has no agents
//! screen and no milestone endpoint. The route answers the project's
//! milestones newest-first — the episodic tier is the staging owner of agent
//! attribution (`contributors` is the only durable per-agent record) — and
//! each row carries the durable facts it promoted from `facts.db`.
//!
//! `project_id` is **data, not authority or a path**: it is bound into every
//! store query and never becomes a filesystem path.
//!
//! The list is bounded by the caller's `?limit` (default
//! [`MILESTONE_DEFAULT_LIMIT`], max [`MILESTONE_MAX_LIMIT`]) and a value
//! outside `1..=MILESTONE_MAX_LIMIT` is a fixed `400`, never a clamp. A full
//! page logs a WARN: the wire stays a bare array, so truncation is documented
//! in the log rather than signalled on the wire.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;

use super::envelope;
use super::port_failure;
use super::ports::PortError;
use crate::server::AppState;

/// The page size when `?limit` is absent.
pub const MILESTONE_DEFAULT_LIMIT: usize = 200;

/// The largest accepted `?limit`; anything above is a `400`.
pub const MILESTONE_MAX_LIMIT: usize = 500;

/// `GET /api/projects/{project_id}/milestones` — the project's milestone
/// roster.
pub async fn list_milestones(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Query(query): super::QueryPairs,
) -> Response {
    let limit = match milestone_limit(&query) {
        Ok(limit) => limit,
        Err(response) => return response,
    };
    match state
        .api
        .milestones
        .list_milestones(&project_id, limit)
        .await
    {
        Ok(rows) => {
            if rows.len() == limit {
                tracing::warn!(
                    project_id,
                    limit,
                    milestones = rows.len(),
                    "milestone list reached the limit bound; the roster may be truncated"
                );
            }
            envelope::ok(&rows)
        }
        Err(error) => milestone_failure("GET /api/projects/{project_id}/milestones", error),
    }
}

/// Maps a milestone port failure.
///
/// `Invalid` (a malformed `project_id`) is the `400` with the port's
/// field-and-reason message — the offending value never travels; `NotFound`
/// (an empty segment) is the fixed `404` with no existence leak; everything
/// else takes the shared mapping (`Unavailable` → 501, the D-9 fixed 500).
fn milestone_failure(endpoint: &'static str, error: PortError) -> Response {
    match error {
        PortError::Invalid(message) => envelope::bad_request(&message),
        PortError::NotFound => envelope::json(
            axum::http::StatusCode::NOT_FOUND,
            envelope::error("Project not found"),
        ),
        other => port_failure(endpoint, other),
    }
}

/// Parses `?limit` under the strict rule: **ASCII digits only**, then
/// `1..=500`.
///
/// Absent or empty is the default; a sign, whitespace, a fraction or an
/// out-of-range value is the fixed `400`. `usize::from_str` alone would accept
/// `"+5"` and a `trim()`-based parse would accept `" 5"` — both are refused
/// here, and the refusal never clamps.
fn milestone_limit(query: &[(String, String)]) -> Result<usize, Response> {
    let Some(raw) = super::query_first(query, "limit") else {
        return Ok(MILESTONE_DEFAULT_LIMIT);
    };
    if raw.is_empty() {
        return Ok(MILESTONE_DEFAULT_LIMIT);
    }
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_limit());
    }
    let parsed: usize = raw.parse().map_err(|_| invalid_limit())?;
    if (1..=MILESTONE_MAX_LIMIT).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(invalid_limit())
    }
}

/// The fixed `400` of a malformed / out-of-range `?limit`.
fn invalid_limit() -> Response {
    envelope::bad_request("Invalid limit parameter")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(raw: Option<&str>) -> Vec<(String, String)> {
        raw.map(|value| vec![("limit".to_string(), value.to_string())])
            .unwrap_or_default()
    }

    /// The parser's contract: absent/empty default, digits accepted, and
    /// every non-digit or out-of-range form refused (`"+5"` kills a bare
    /// `parse::<usize>()`; `" 5"`/`"5 "` kill a `trim()`-based parse; `"501"`
    /// kills a clamp).
    #[test]
    fn test_milestone_limit_defaults_and_refuses_out_of_range() {
        assert_eq!(
            milestone_limit(&query(None)).expect("absent is the default"),
            MILESTONE_DEFAULT_LIMIT
        );
        assert_eq!(
            milestone_limit(&query(Some(""))).expect("empty is the default"),
            MILESTONE_DEFAULT_LIMIT
        );
        assert_eq!(milestone_limit(&query(Some("5"))).expect("5"), 5);
        for raw in ["+5", " 5", "5 ", "0", "501", "5.0", "-1"] {
            assert!(
                milestone_limit(&query(Some(raw))).is_err(),
                "{raw:?} is refused"
            );
        }
    }
}
