//! The health endpoint and the D-1 health-probe rule.
//!
//! Upstream `web-server.ts` `checkServerAvailable` (L432-441).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;

use crate::server::AppState;

/// The JSON field carrying the success flag of a health envelope.
pub const HEALTH_PROBE_SUCCESS_FIELD: &str = "success";

/// The JSON field carrying the status of a health envelope.
pub const HEALTH_PROBE_STATUS_FIELD: &str = "status";

/// The status value a health envelope must carry.
pub const HEALTH_PROBE_EXPECTED_STATUS: &str = "ok";

/// Whether the payload is a health envelope produced by this server.
///
/// A 2xx from an unrelated local service must never be mistaken for an
/// opencode-mem owner, so the envelope must carry `success: true` *and*
/// `status: "ok"`.
pub fn is_valid_health_probe_envelope(value: &serde_json::Value) -> bool {
    value.get(HEALTH_PROBE_SUCCESS_FIELD) == Some(&serde_json::Value::Bool(true))
        && value
            .get(HEALTH_PROBE_STATUS_FIELD)
            .and_then(serde_json::Value::as_str)
            == Some(HEALTH_PROBE_EXPECTED_STATUS)
}

/// The health envelope body.
///
/// The key order is fixed (`success`, `status`, `authEnabled`) to match the
/// upstream envelope byte for byte; `serde_json::Map` sorts its keys, so the
/// body is assembled directly.
pub(crate) fn health_envelope(auth_enabled: bool) -> String {
    format!(r#"{{"success":true,"status":"ok","authEnabled":{auth_enabled}}}"#)
}

/// `GET /api/health` — the liveness envelope.
pub(crate) async fn health_handler(State(state): State<Arc<AppState>>) -> Response {
    let auth_enabled = state.basic_auth.is_enabled();
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(health_envelope(auth_enabled)))
        .expect("the health response is always a valid HTTP response")
}
