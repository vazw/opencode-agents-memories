//! D-8 — the Host allowlist at the request gate (router seam).
//!
//! The upstream project has no counterpart for this file: the allowlist is a
//! DNS-rebinding hardening measure the port adds on top of upstream's CORS
//! gate. Per `docs/agents/testing.md` a file with no upstream oracle gets no
//! registry row — this wave's test plan names it.

mod common;

use std::path::PathBuf;

use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// A dist directory that does not exist (D-7).
const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

/// The local token the shell would carry — it must never reach a response
/// that is served for a foreign `Host`.
const LOCAL_TOKEN: &str = "local-tok-secret";

fn router() -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(None, None, "osuser"),
        None,
        Some(LOCAL_TOKEN),
        PathBuf::from(MISSING_DIST),
    )))
}

#[tokio::test]
async fn test_foreign_host_header_is_refused_with_403_and_no_token_in_body() {
    let response = common::router_request(
        router(),
        "GET",
        "/api/health",
        &[("Host", "evil.example:4747")],
    )
    .await;

    assert_eq!(response.status, 403);
    assert_eq!(response.header("Content-Type"), Some("application/json"));
    assert_eq!(response.header("Cache-Control"), Some("no-store"));
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Host header is not allowed."}"#
    );
    assert!(!response.body_string().contains("__OPENCODE_MEM_TOKEN__"));
    assert!(!response.body_string().contains(LOCAL_TOKEN));
    assert!(!response.body_string().contains("evil.example"));
}

#[tokio::test]
async fn test_host_gate_runs_before_static_shell_token_injection() {
    let refused =
        common::router_request(router(), "GET", "/", &[("Host", "evil.example:4747")]).await;
    assert_eq!(refused.status, 403);
    assert!(!refused.body_string().contains("__OPENCODE_MEM_TOKEN__"));
    assert!(!refused.body_string().contains(LOCAL_TOKEN));

    // The same request with an allowed Host reaches the shell path; the
    // bundle does not exist yet, so the shell answers 404 (D-7).
    let allowed = common::router_request(router(), "GET", "/", &[]).await;
    assert_eq!(allowed.status, 404);
    assert_eq!(allowed.body_string(), "File not found");
    assert!(!allowed.body_string().contains("__OPENCODE_MEM_TOKEN__"));
}

#[tokio::test]
async fn test_loopback_host_headers_are_accepted_with_and_without_port() {
    for host in [
        "localhost",
        "localhost:4747",
        "127.0.0.1",
        "127.0.0.1:4747",
        "[::1]",
        "[::1]:4747",
    ] {
        let response =
            common::router_request(router(), "GET", "/api/health", &[("Host", host)]).await;
        assert_eq!(response.status, 200, "the host {host} must be accepted");
    }
}

#[tokio::test]
async fn test_configured_host_header_is_accepted() {
    for host in ["127.0.0.1", "127.0.0.1:4747"] {
        let response =
            common::router_request(router(), "GET", "/api/health", &[("Host", host)]).await;
        assert_eq!(response.status, 200, "the host {host} must be accepted");
    }
}

#[tokio::test]
async fn test_missing_or_empty_host_header_is_refused_with_403() {
    // The request is built DIRECTLY: the router helper injects a default
    // `Host`, so using it here would assert the helper's default path
    // instead of the missing-Host case.
    let without_host = axum::http::Request::builder()
        .method("GET")
        .uri("/api/health")
        .body(axum::body::Body::empty())
        .expect("valid request");
    let response = common::oneshot_response(router(), without_host).await;
    assert_eq!(response.status, 403);
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Host header is not allowed."}"#
    );

    let with_empty_host = axum::http::Request::builder()
        .method("GET")
        .uri("/api/health")
        .header("host", "")
        .body(axum::body::Body::empty())
        .expect("valid request");
    let response = common::oneshot_response(router(), with_empty_host).await;
    assert_eq!(response.status, 403);
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Host header is not allowed."}"#
    );
}

#[tokio::test]
async fn test_host_check_applies_to_api_and_static_paths_alike() {
    for path in ["/api/health", "/", "/api/nope"] {
        let response =
            common::router_request(router(), "GET", path, &[("Host", "evil.example:4747")]).await;
        assert_eq!(response.status, 403, "the path {path} must be refused");
        assert_eq!(
            response.body_string(),
            r#"{"success":false,"error":"Host header is not allowed."}"#
        );
    }
}
