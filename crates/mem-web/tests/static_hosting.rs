//! Static hosting of the SPA bundle — the port of upstream `web-server.ts`
//! `serveStaticFile` (L838-895) plus the SPA fallback of `handleRequest`.
//!
//! Upstream has no dedicated static-hosting test file (the upstream suite
//! never built the `web` bundle), so this file has no upstream oracle and
//! gets no registry row — the wave test plan names it.

mod common;

use std::path::PathBuf;

use mem_web::server::build_router;
use mem_web::static_hosting::{cache_control_for, inject_token_script};
use mem_web::web_auth::WebAuth;

/// The marker the fixture shell carries.
const SHELL_MARKER: &str = "MEM-WEB-SHELL-MARKER";

/// The local token the shell would carry.
const LOCAL_TOKEN: &str = "shell-token";

/// Builds a dist fixture: `index.html`, `assets/app.js`, `favicon.ico` and
/// `font.woff2`.
fn dist_fixture(label: &str) -> (common::TempDir, PathBuf) {
    let dir = common::TempDir::new(label);
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(dist.join("assets")).expect("create the assets dir");
    std::fs::write(
        dist.join("index.html"),
        format!("<html><head><title>mem</title></head><body>{SHELL_MARKER}</body></html>"),
    )
    .expect("write index.html");
    std::fs::write(dist.join("assets/app.js"), "console.log('app');").expect("write app.js");
    std::fs::write(dist.join("favicon.ico"), "ICON-BYTES").expect("write favicon.ico");
    std::fs::write(dist.join("font.woff2"), "FONT-BYTES").expect("write font.woff2");
    (dir, dist)
}

fn router(dist_dir: PathBuf, local_token: Option<&str>, basic_auth: WebAuth) -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        basic_auth,
        None,
        local_token,
        dist_dir,
    )))
}

fn plain_router(dist_dir: PathBuf) -> axum::Router {
    router(
        dist_dir,
        Some(LOCAL_TOKEN),
        WebAuth::new(None, None, "osuser"),
    )
}

#[tokio::test]
async fn test_root_serves_index_shell_as_html() {
    let (_dir, dist) = dist_fixture("static-root");
    let response = common::router_request(plain_router(dist), "GET", "/", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Type"), Some("text/html"));
    assert!(response.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_index_html_path_serves_same_shell() {
    let (_dir, dist) = dist_fixture("static-index");
    let response = common::router_request(plain_router(dist), "GET", "/index.html", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Type"), Some("text/html"));
    assert!(response.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_client_route_falls_back_to_index_html() {
    let (_dir, dist) = dist_fixture("static-client-route");
    let response = common::router_request(plain_router(dist), "GET", "/profile", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Type"), Some("text/html"));
    assert!(response.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_nested_client_route_falls_back_to_index_html() {
    let (_dir, dist) = dist_fixture("static-nested-route");
    let response = common::router_request(plain_router(dist), "GET", "/settings/tags", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Type"), Some("text/html"));
    assert!(response.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_missing_asset_with_extension_returns_404_not_shell() {
    let (_dir, dist) = dist_fixture("static-missing-asset");
    let response = common::router_request(plain_router(dist), "GET", "/missing.js", &[]).await;

    assert_eq!(response.status, 404);
    assert!(!response.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_existing_asset_served_with_immutable_cache_control() {
    let (_dir, dist) = dist_fixture("static-asset");
    let response = common::router_request(plain_router(dist), "GET", "/assets/app.js", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("Cache-Control"),
        Some("public, max-age=31536000, immutable")
    );
    assert!(response.body_string().contains("console.log"));
}

#[tokio::test]
async fn test_index_shell_is_served_no_cache() {
    let (_dir, dist) = dist_fixture("static-no-cache");
    let response = common::router_request(plain_router(dist), "GET", "/", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("Cache-Control"), Some("no-cache"));
}

#[tokio::test]
async fn test_favicon_served_with_one_day_cache_control() {
    let (_dir, dist) = dist_fixture("static-favicon");
    let response = common::router_request(plain_router(dist), "GET", "/favicon.ico", &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("Cache-Control"),
        Some("public, max-age=86400")
    );
}

#[tokio::test]
async fn test_index_shell_injects_local_token_script_before_head_close() {
    let (_dir, dist) = dist_fixture("static-token");
    let response = common::router_request(plain_router(dist), "GET", "/", &[]).await;

    assert_eq!(response.status, 200);
    assert!(
        response
            .body_string()
            .contains("<script>window.__OPENCODE_MEM_TOKEN__=\"shell-token\"</script></head>"),
        "the injected script must sit right before </head>, body was {}",
        response.body_string()
    );
}

#[tokio::test]
async fn test_index_token_injection_escapes_script_terminator() {
    let injected = inject_token_script("<head></head>", Some("</script><b>"));
    assert!(
        !injected.contains("</script><b>"),
        "the token must not break out of the script element: {injected}"
    );
    assert!(injected.contains("\\u003c/script>"));

    let (_dir, dist) = dist_fixture("static-token-escape");
    let response = common::router_request(
        router(
            dist,
            Some("</script><b>"),
            WebAuth::new(None, None, "osuser"),
        ),
        "GET",
        "/",
        &[],
    )
    .await;
    assert_eq!(response.status, 200);
    assert!(!response.body_string().contains("</script><b>"));
    assert!(response.body_string().contains("\\u003c/script>"));
}

#[test]
fn test_index_without_head_is_served_unmodified() {
    assert_eq!(inject_token_script("<p>x</p>", Some("t")), "<p>x</p>");
    assert_eq!(inject_token_script("<p>x</p>", None), "<p>x</p>");
}

#[tokio::test]
async fn test_no_token_source_serves_shell_without_script() {
    let (_dir, dist) = dist_fixture("static-no-token");
    let response = common::router_request(
        router(dist, None, WebAuth::new(None, None, "osuser")),
        "GET",
        "/",
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert!(response.body_string().contains(SHELL_MARKER));
    assert!(!response.body_string().contains("__OPENCODE_MEM_TOKEN__"));
}

#[tokio::test]
async fn test_missing_dist_dir_returns_404_and_api_still_healthy() {
    let missing = PathBuf::from("/nonexistent-mem-web-dist");
    let response = common::router_request(plain_router(missing.clone()), "GET", "/", &[]).await;
    assert_eq!(response.status, 404);
    assert_eq!(response.body_string(), "File not found");

    let health = common::router_request(plain_router(missing), "GET", "/api/health", &[]).await;
    assert_eq!(health.status, 200);
}

#[tokio::test]
async fn test_path_traversal_outside_dist_is_rejected() {
    let (dir, dist) = dist_fixture("static-traversal");
    std::fs::write(dir.path().join("secret.txt"), "TOP-SECRET-CONTENT").expect("write the secret");

    let response = common::router_request(plain_router(dist), "GET", "/../secret.txt", &[]).await;

    assert_ne!(response.status, 200);
    assert!(!response.body_string().contains("TOP-SECRET-CONTENT"));
}

#[tokio::test]
async fn test_spa_fallback_never_applies_to_api_paths() {
    let (_dir, dist) = dist_fixture("static-api-path");
    let response = common::router_request(
        plain_router(dist),
        "GET",
        "/api/unknown",
        &[("x-opencode-mem-token", LOCAL_TOKEN)],
    )
    .await;

    assert_eq!(response.status, 404);
    assert_eq!(response.body_string(), "Not Found");
    assert!(!response.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_non_get_unknown_path_returns_404() {
    let (_dir, dist) = dist_fixture("static-non-get");
    let response = common::router_request(plain_router(dist), "POST", "/profile", &[]).await;

    assert_eq!(response.status, 404);
}

#[tokio::test]
async fn test_head_request_returns_404_parity() {
    let (_dir, dist) = dist_fixture("static-head");
    let response = common::router_request(plain_router(dist), "HEAD", "/", &[]).await;

    assert_eq!(
        response.status, 404,
        "upstream answers HEAD with 404, not 200"
    );
}

#[tokio::test]
async fn test_static_shell_is_gated_by_basic_auth_when_password_set() {
    let (_dir, dist) = dist_fixture("static-basic");
    let guarded = router(
        dist,
        Some(LOCAL_TOKEN),
        WebAuth::new(Some("secret"), Some("admin"), "osuser"),
    );

    let anonymous = common::router_request(guarded.clone(), "GET", "/", &[]).await;
    assert_eq!(anonymous.status, 401);
    assert_eq!(
        anonymous.header("WWW-Authenticate"),
        Some("Basic realm=\"OpenCode Memory Explorer\", charset=\"UTF-8\"")
    );
    assert_eq!(anonymous.header("Cache-Control"), Some("no-store"));

    let credentials =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "admin:secret");
    let authorized = common::router_request(
        guarded,
        "GET",
        "/",
        &[("Authorization", &format!("Basic {credentials}"))],
    )
    .await;
    assert_eq!(authorized.status, 200);
    assert!(authorized.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_static_shell_is_not_gated_by_api_token() {
    let (_dir, dist) = dist_fixture("static-api-token");
    let guarded = build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(None, None, "osuser"),
        Some("api-token"),
        Some(LOCAL_TOKEN),
        dist,
    )));

    let response = common::router_request(guarded, "GET", "/", &[]).await;
    assert_eq!(response.status, 200);
    assert!(response.body_string().contains(SHELL_MARKER));
}

#[tokio::test]
async fn test_cors_gate_rejects_foreign_origin_before_static() {
    let (_dir, dist) = dist_fixture("static-cors");
    let response = common::router_request(
        plain_router(dist),
        "GET",
        "/",
        &[("Origin", "https://evil.example")],
    )
    .await;

    assert_eq!(response.status, 403);
    assert_eq!(response.header("Access-Control-Allow-Origin"), None);
}

#[test]
fn test_cache_control_for_classifies_the_relative_path() {
    assert_eq!(
        cache_control_for("assets/app.js"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        cache_control_for("font.woff2"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(cache_control_for("favicon.ico"), "public, max-age=86400");
    assert_eq!(cache_control_for("logo.svg"), "public, max-age=86400");
    assert_eq!(cache_control_for("index.html"), "no-cache");
    assert_eq!(cache_control_for("profile"), "no-cache");
}
