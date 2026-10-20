//! Port of upstream `example/opencode-mem/tests/web-server-health.test.ts`
//! ("web server health check") onto the Rust `mem_web` health seam.
//!
//! Upstream cases traced:
//! - "does not treat a 2xx from an unrelated service as an opencode-mem
//!   owner" → `test_health_probe_rule_accepts_only_the_upstream_envelope`
//!   (the health-probe envelope rule of `checkServerAvailable`, L432-441).
//! - The takeover / fallback-port cases of upstream's `web-server-health` are
//!   **N-A** per D-1 (fail-fast on port conflict; no port hopping, no
//!   takeover loop) — they get a registry row instead of a test.
//! - The upstream "authenticates the stats request when an API token is
//!   configured" case is the API-token gate, ported in
//!   `tests/web_api_auth.rs` (Step 6).
//! - The HTTP-level envelope cases land in Step 6 (they need the request
//!   gate); they are written in Step 5 and stay RED until then.

mod common;

use std::path::PathBuf;

use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// A dist directory that does not exist — the health endpoint never touches
/// it (D-7).
const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

fn router_without_auth() -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(None, None, "osuser"),
        None,
        None,
        PathBuf::from(MISSING_DIST),
    )))
}

fn router_with_auth() -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(Some("secret"), Some("admin"), "osuser"),
        None,
        None,
        PathBuf::from(MISSING_DIST),
    )))
}

fn router_with_auth_and_token() -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(Some("secret"), Some("admin"), "osuser"),
        Some("api-token"),
        Some("local-token"),
        PathBuf::from(MISSING_DIST),
    )))
}

/// A router with an API token but no Basic password — the API gate is then
/// the only gate that can answer.
fn router_with_api_token() -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(None, None, "osuser"),
        Some("api-token"),
        Some("local-token"),
        PathBuf::from(MISSING_DIST),
    )))
}

#[tokio::test]
async fn test_health_returns_ok_envelope_when_auth_disabled() {
    let response = common::router_request(router_without_auth(), "GET", "/api/health", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Type"), Some("application/json"));
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"status":"ok","authEnabled":false}"#
    );
}

#[tokio::test]
async fn test_health_reports_auth_enabled_when_password_set() {
    let response = common::router_request(router_with_auth(), "GET", "/api/health", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"status":"ok","authEnabled":true}"#
    );
}

#[tokio::test]
async fn test_health_bypasses_basic_and_api_token_gates() {
    let response =
        common::router_request(router_with_auth_and_token(), "GET", "/api/health", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"status":"ok","authEnabled":true}"#
    );
}

#[tokio::test]
async fn test_health_rejects_non_get_method_with_404() {
    let response = common::router_request(router_without_auth(), "POST", "/api/health", &[]).await;

    assert_eq!(response.status, 404);
}

#[tokio::test]
async fn test_health_trailing_slash_is_not_exempt() {
    let response =
        common::router_request(router_with_api_token(), "GET", "/api/health/", &[]).await;

    assert_eq!(response.status, 401);
    assert_eq!(response.header("Content-Type"), Some("application/json"));
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Unauthorized"}"#
    );
}

#[test]
fn test_health_probe_rule_accepts_only_the_upstream_envelope() {
    use mem_web::{
        HEALTH_PROBE_EXPECTED_STATUS, HEALTH_PROBE_STATUS_FIELD, HEALTH_PROBE_SUCCESS_FIELD,
        is_valid_health_probe_envelope,
    };

    assert_eq!(HEALTH_PROBE_SUCCESS_FIELD, "success");
    assert_eq!(HEALTH_PROBE_STATUS_FIELD, "status");
    assert_eq!(HEALTH_PROBE_EXPECTED_STATUS, "ok");

    assert!(is_valid_health_probe_envelope(
        &serde_json::json!({"success": true, "status": "ok"})
    ));
    assert!(!is_valid_health_probe_envelope(
        &serde_json::json!({"success": true})
    ));
    assert!(!is_valid_health_probe_envelope(
        &serde_json::json!({"success": false, "status": "ok"})
    ));
    assert!(!is_valid_health_probe_envelope(
        &serde_json::json!({"status": "ok"})
    ));
    assert!(!is_valid_health_probe_envelope(
        &serde_json::json!({"success": true, "status": "degraded"})
    ));
}
