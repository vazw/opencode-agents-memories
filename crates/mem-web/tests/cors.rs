//! Port of upstream `example/opencode-mem/tests/web-server-cors.test.ts`
//! ("web server CORS policy") onto the Rust `mem_web::cors` seam.
//!
//! Upstream cases traced:
//! - "allows non-browser requests without an Origin header" →
//!   `test_cors_allows_missing_origin`
//! - "allows loopback browser origins" →
//!   `test_cors_allows_loopback_origins_with_port`
//! - "rejects non-loopback browser origins when auth is disabled" →
//!   `test_cors_rejects_non_loopback_origin_when_auth_disabled`
//! - "allows any http(s) non-loopback origin when httpAuthEnabled is true" →
//!   `test_cors_allows_any_http_origin_when_basic_auth_enabled`
//! - "still rejects non-http(s) origins even when httpAuthEnabled is true" →
//!   `test_cors_rejects_non_http_scheme_even_when_auth_enabled`
//! - "treats malformed origins as rejected" →
//!   `test_cors_rejects_malformed_origin`
//! - "returns a loopback-bound preflight response" →
//!   `test_preflight_returns_204_with_allow_headers_for_loopback`
//! - "accepts non-loopback origins once httpAuthEnabled is true" →
//!   `test_preflight_accepts_remote_origin_when_auth_enabled`
//! - "does not expose CORS headers on rejected origins" →
//!   `test_disallowed_response_has_no_cors_headers`
//! - HTTP-level preflight cases land in Step 6 (they need the request gate):
//!   `test_http_preflight_from_disallowed_origin_returns_403`,
//!   `test_http_preflight_without_origin_returns_204_without_cors_headers`.

mod common;

use std::path::PathBuf;

use mem_web::cors::{disallowed_response, is_allowed_browser_origin, preflight_response};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// A dist directory that does not exist — the CORS cases never touch it.
const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

fn router_without_auth() -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(None, None, "osuser"),
        None,
        None,
        PathBuf::from(MISSING_DIST),
    )))
}

#[test]
fn test_cors_allows_missing_origin() {
    assert!(is_allowed_browser_origin(None, false));
    assert!(is_allowed_browser_origin(None, true));
}

#[test]
fn test_cors_allows_loopback_origins_with_port() {
    assert!(is_allowed_browser_origin(
        Some("http://127.0.0.1:4747"),
        false
    ));
    assert!(is_allowed_browser_origin(
        Some("http://localhost:4747"),
        false
    ));
    assert!(is_allowed_browser_origin(Some("http://[::1]:4747"), false));
}

#[test]
fn test_cors_rejects_non_loopback_origin_when_auth_disabled() {
    assert!(!is_allowed_browser_origin(
        Some("https://example.com"),
        false
    ));
    assert!(!is_allowed_browser_origin(
        Some("http://192.168.1.50:4747"),
        false
    ));
    assert!(!is_allowed_browser_origin(Some("null"), false));
}

#[test]
fn test_cors_allows_any_http_origin_when_basic_auth_enabled() {
    assert!(is_allowed_browser_origin(Some("https://example.com"), true));
    assert!(is_allowed_browser_origin(
        Some("http://192.168.1.50:4747"),
        true
    ));
}

#[test]
fn test_cors_rejects_non_http_scheme_even_when_auth_enabled() {
    assert!(!is_allowed_browser_origin(Some("file:///etc/passwd"), true));
    assert!(!is_allowed_browser_origin(Some("null"), true));
}

#[test]
fn test_cors_rejects_malformed_origin() {
    assert!(!is_allowed_browser_origin(Some("not-a-url"), true));
    assert!(!is_allowed_browser_origin(Some("not-a-url"), false));
}

#[tokio::test]
async fn test_preflight_returns_204_with_allow_headers_for_loopback() {
    let response =
        common::response_to_raw(preflight_response(Some("http://localhost:4747"), false)).await;

    assert_eq!(response.status, 204);
    assert_eq!(
        response.header("Access-Control-Allow-Origin"),
        Some("http://localhost:4747")
    );
    let methods = response
        .header("Access-Control-Allow-Methods")
        .unwrap_or_default();
    assert!(methods.contains("POST"), "methods were {methods}");
    assert_eq!(response.header("Vary"), Some("Origin"));
    assert_eq!(response.header("Access-Control-Max-Age"), Some("600"));
}

#[tokio::test]
async fn test_preflight_accepts_remote_origin_when_auth_enabled() {
    let response =
        common::response_to_raw(preflight_response(Some("https://example.com"), true)).await;

    assert_eq!(response.status, 204);
    assert_eq!(
        response.header("Access-Control-Allow-Origin"),
        Some("https://example.com")
    );
}

#[tokio::test]
async fn test_disallowed_response_has_no_cors_headers() {
    let response = common::response_to_raw(disallowed_response()).await;

    assert_eq!(response.status, 403);
    assert_eq!(response.header("Access-Control-Allow-Origin"), None);
    assert_eq!(response.header("Access-Control-Allow-Methods"), None);
    assert_eq!(response.header("Access-Control-Allow-Headers"), None);
    assert_eq!(response.header("Content-Type"), Some("application/json"));
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Cross-origin requests are restricted to loopback origins."}"#
    );
}

#[tokio::test]
async fn test_http_preflight_from_disallowed_origin_returns_403() {
    let response = common::router_request(
        router_without_auth(),
        "OPTIONS",
        "/api/memories",
        &[("Origin", "https://evil.example")],
    )
    .await;

    assert_eq!(response.status, 403);
    assert_eq!(response.header("Access-Control-Allow-Origin"), None);
    assert_eq!(response.header("Access-Control-Allow-Methods"), None);
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Cross-origin requests are restricted to loopback origins."}"#
    );
}

#[tokio::test]
async fn test_http_preflight_without_origin_returns_204_without_cors_headers() {
    let response =
        common::router_request(router_without_auth(), "OPTIONS", "/api/memories", &[]).await;

    assert_eq!(response.status, 204);
    assert_eq!(response.header("Access-Control-Allow-Origin"), None);
    assert_eq!(response.header("Access-Control-Max-Age"), Some("600"));
}
