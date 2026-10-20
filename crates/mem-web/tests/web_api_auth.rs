//! Port of upstream `example/opencode-mem/tests/web-api-auth.test.ts` ("web
//! api auth") onto the Rust `mem_web::api_auth` seam, plus the local-token
//! file cases of upstream `src/services/auth-token.ts`.
//!
//! Upstream cases traced:
//! - "detects loopback hosts" → `test_loopback_host_detects_loopback_only`
//! - "requires api token for non-loopback hosts" →
//!   `test_network_auth_rejects_non_loopback_without_token_or_basic`,
//!   `test_network_auth_allows_non_loopback_with_api_token_or_basic`,
//!   `test_network_auth_allows_loopback_without_auth`
//! - "authorizes bearer and custom header tokens" →
//!   `test_api_token_accepts_bearer_and_custom_header`,
//!   `test_api_token_absent_passes_every_request`,
//!   `test_api_token_rejects_missing_credentials`
//! - upstream `auth-token.ts` `getOrCreateAuthToken` →
//!   `test_local_token_file_created_owner_only_when_missing`,
//!   `test_local_token_file_reused_when_present`,
//!   `test_local_token_file_blank_content_is_regenerated`
//! - The HTTP-level API-gate cases land in Step 6 (they need the request
//!   gate): `test_api_auth_rejects_api_request_without_token_with_json_401`,
//!   `test_api_auth_rejects_unauthenticated_request_to_unknown_api_path_before_routing`,
//!   `test_api_auth_accepts_local_token_header_when_no_api_token_configured`.

mod common;

use std::path::PathBuf;

use mem_web::WebError;
use mem_web::api_auth::{
    FileLocalToken, LocalTokenSource, assert_network_auth, authorize_api_request, is_loopback_host,
};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// A dist directory that does not exist — the API cases never touch it.
const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

fn router_with(api_token: Option<&str>, local_token: Option<&str>) -> axum::Router {
    build_router(std::sync::Arc::new(common::app_state(
        WebAuth::new(None, None, "osuser"),
        api_token,
        local_token,
        PathBuf::from(MISSING_DIST),
    )))
}

#[test]
fn test_loopback_host_detects_loopback_only() {
    assert!(is_loopback_host("127.0.0.1"));
    assert!(is_loopback_host("localhost"));
    assert!(is_loopback_host("::1"));
    assert!(is_loopback_host("[::1]"));
    assert!(is_loopback_host("0:0:0:0:0:0:0:1"));
    assert!(is_loopback_host("LOCALHOST"));
    assert!(is_loopback_host("  127.0.0.1  "));
    assert!(!is_loopback_host("0.0.0.0"));
    assert!(!is_loopback_host("192.168.1.10"));
    assert!(!is_loopback_host("example.com"));
}

#[test]
fn test_network_auth_rejects_non_loopback_without_token_or_basic() {
    let error = assert_network_auth("0.0.0.0", None, false)
        .expect_err("a network bind without any credential must be refused");
    let message = error.to_string();
    match &error {
        WebError::NetworkAuthRequired { host } => assert_eq!(host, "0.0.0.0"),
        other => panic!("expected NetworkAuthRequired, got {other:?}"),
    }
    assert!(
        message.contains("webServerApiToken"),
        "the message must name the setting, got {message}"
    );
    assert_eq!(
        message,
        "webServerHost \"0.0.0.0\" exposes the API on the network. Set webServerApiToken in opencode-mem.jsonc, or bind to 127.0.0.1."
    );
}

#[test]
fn test_network_auth_allows_non_loopback_with_api_token_or_basic() {
    assert!(assert_network_auth("0.0.0.0", Some("secret"), false).is_ok());
    assert!(assert_network_auth("0.0.0.0", None, true).is_ok());
    assert!(assert_network_auth("0.0.0.0", Some("secret"), true).is_ok());
}

#[test]
fn test_network_auth_allows_loopback_without_auth() {
    assert!(assert_network_auth("127.0.0.1", None, false).is_ok());
    assert!(assert_network_auth("localhost", None, false).is_ok());
}

#[test]
fn test_api_token_accepts_bearer_and_custom_header() {
    let token = "test-token";
    assert!(authorize_api_request(
        Some("Bearer test-token"),
        None,
        Some(token)
    ));
    assert!(authorize_api_request(None, Some("test-token"), Some(token)));
    assert!(!authorize_api_request(None, None, Some(token)));
    assert!(!authorize_api_request(
        Some("Bearer wrong"),
        None,
        Some(token)
    ));
    assert!(!authorize_api_request(None, Some("wrong"), Some(token)));
}

#[test]
fn test_api_token_absent_passes_every_request() {
    assert!(authorize_api_request(None, None, None));
    assert!(authorize_api_request(Some("Bearer anything"), None, None));
    assert!(authorize_api_request(None, Some("anything"), None));
    // An empty configured token is "not configured" (upstream truthiness).
    assert!(authorize_api_request(None, None, Some("")));
}

#[test]
fn test_api_token_rejects_missing_credentials() {
    assert!(!authorize_api_request(None, None, Some("tok")));
}

#[test]
fn test_local_token_file_created_owner_only_when_missing() {
    let dir = common::TempDir::new("local-token-create");
    let path: PathBuf = dir.path().join("nested").join(".auth-token");
    let source = FileLocalToken::new(path.clone());

    let token = source
        .token()
        .expect("the local token must be created when the file is missing");
    assert_eq!(token.len(), 64, "32 random bytes hex-encoded");
    assert!(
        token
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "the token must be lowercase hex, got {token}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), token);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the token file must be owner-only");
    }
}

#[test]
fn test_local_token_file_reused_when_present() {
    let dir = common::TempDir::new("local-token-reuse");
    let path: PathBuf = dir.path().join(".auth-token");
    std::fs::write(&path, "  known-token\n").unwrap();

    let source = FileLocalToken::new(path.clone());
    let token = source.token().expect("the existing token must be reused");
    assert_eq!(token, "known-token");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "  known-token\n",
        "a non-empty file is never rewritten"
    );
    assert_eq!(source.token().as_deref(), Some("known-token"));
}

#[test]
fn test_local_token_file_blank_content_is_regenerated() {
    let dir = common::TempDir::new("local-token-blank");
    let path: PathBuf = dir.path().join(".auth-token");
    std::fs::write(&path, "   \n\t").unwrap();

    let source = FileLocalToken::new(path.clone());
    let token = source
        .token()
        .expect("a blank token file must be regenerated");
    assert_eq!(token.len(), 64);
    assert_ne!(token, "   \n\t");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), token);
}

#[tokio::test]
async fn test_api_auth_rejects_api_request_without_token_with_json_401() {
    let response = common::router_request(
        router_with(Some("api-token"), None),
        "GET",
        "/api/stats",
        &[],
    )
    .await;

    assert_eq!(response.status, 401);
    assert_eq!(response.header("Content-Type"), Some("application/json"));
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Unauthorized"}"#
    );
}

#[tokio::test]
async fn test_api_auth_rejects_unauthenticated_request_to_unknown_api_path_before_routing() {
    // No token is configured at all, so the local token is the only way in.
    let response = common::router_request(router_with(None, None), "GET", "/api/nope", &[]).await;

    assert_eq!(
        response.status, 401,
        "the API gate must answer before the router can answer 404"
    );
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Unauthorized"}"#
    );
}

#[tokio::test]
async fn test_api_auth_accepts_local_token_header_when_no_api_token_configured() {
    let router = router_with(None, Some("local-tok"));

    let with_token = common::router_request(
        router.clone(),
        "GET",
        "/api/nope",
        &[("x-opencode-mem-token", "local-tok")],
    )
    .await;
    assert_eq!(
        with_token.status, 404,
        "the local token must let the request through the gate"
    );
    assert_eq!(with_token.body_string(), "Not Found");

    let without_token = common::router_request(router, "GET", "/api/nope", &[]).await;
    assert_eq!(without_token.status, 401);
    assert_eq!(
        without_token.body_string(),
        r#"{"success":false,"error":"Unauthorized"}"#
    );
}
