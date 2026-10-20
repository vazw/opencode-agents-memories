//! Port of upstream `example/opencode-mem/tests/web-auth.test.ts` ("WebAuth")
//! onto the Rust `mem_web::web_auth` seam.
//!
//! Upstream cases traced:
//! - "is disabled when password is empty" / "…whitespace only" / "…omitted" →
//!   `test_basic_auth_disabled_when_password_empty_or_whitespace`
//! - "is enabled when password is set" → same test (the positive half)
//! - "defaults username to os.userInfo().username when not provided" and
//!   "treats whitespace-only username as missing and falls back to OS user" →
//!   `test_basic_auth_username_defaults_to_os_user_when_absent_or_blank`
//! - "uses explicit username when provided" →
//!   `test_basic_auth_explicit_username_wins_over_os_user`
//! - "passes through when auth is disabled" →
//!   `test_basic_auth_passes_everything_when_disabled`
//! - "always allows the health endpoint through" →
//!   `test_basic_auth_always_allows_health_path`
//! - "rejects requests without an Authorization header" →
//!   `test_basic_auth_rejects_missing_header_with_challenge`
//! - "rejects requests with the wrong credentials" →
//!   `test_basic_auth_rejects_wrong_password`
//! - "rejects requests with a non-Basic Authorization scheme" →
//!   `test_basic_auth_rejects_non_basic_scheme`
//! - "rejects requests with malformed Basic payload (no colon)" →
//!   `test_basic_auth_rejects_payload_without_colon`
//! - "accepts requests with the correct credentials" →
//!   `test_basic_auth_accepts_correct_credentials`
//! - "treats username case-sensitively" →
//!   `test_basic_auth_username_is_case_sensitive`
//! - "accepts credentials whose password contains a colon" →
//!   `test_basic_auth_password_may_contain_colon`
//! - "uses the configured realm in WWW-Authenticate" / "marks the challenge as
//!   no-store" → `test_basic_auth_rejects_missing_header_with_challenge`
//! - D-4 (recorded divergence: upstream's base64 decoding is lenient, the
//!   port rejects an undecodable / non-UTF-8 payload) →
//!   `test_basic_auth_rejects_undecodable_base64`
//! - the `Basic` scheme is matched case-insensitively (upstream lowercases
//!   the header before the prefix test) →
//!   `test_basic_auth_scheme_is_case_insensitive`

mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mem_web::web_auth::{WEB_AUTH_REALM, WebAuth};

fn basic_header(user: &str, pass: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{pass}")))
}

#[test]
fn test_basic_auth_disabled_when_password_empty_or_whitespace() {
    assert!(!WebAuth::new(Some(""), None, "osuser").is_enabled());
    assert!(!WebAuth::new(Some("   "), None, "osuser").is_enabled());
    assert!(!WebAuth::new(None, None, "osuser").is_enabled());
    assert!(WebAuth::new(Some("secret"), None, "osuser").is_enabled());
}

#[test]
fn test_basic_auth_username_defaults_to_os_user_when_absent_or_blank() {
    assert_eq!(
        WebAuth::new(Some("secret"), None, "osuser").username(),
        "osuser"
    );
    assert_eq!(
        WebAuth::new(Some("secret"), Some("  "), "osuser").username(),
        "osuser"
    );
}

#[test]
fn test_basic_auth_explicit_username_wins_over_os_user() {
    assert_eq!(
        WebAuth::new(Some("secret"), Some("custom-admin"), "osuser").username(),
        "custom-admin"
    );
}

#[test]
fn test_basic_auth_passes_everything_when_disabled() {
    let auth = WebAuth::new(None, None, "osuser");
    assert!(auth.check(None, "/api/memories"));
}

#[test]
fn test_basic_auth_always_allows_health_path() {
    let auth = WebAuth::new(Some("secret"), Some("admin"), "osuser");
    assert!(auth.check(None, "/api/health"));
}

#[tokio::test]
async fn test_basic_auth_rejects_missing_header_with_challenge() {
    let auth = WebAuth::new(Some("secret"), Some("admin"), "osuser");
    assert!(!auth.check(None, "/api/memories"));

    let response = common::response_to_raw(auth.challenge()).await;
    assert_eq!(response.status, 401);
    assert_eq!(response.body_string(), "Authentication required");
    assert_eq!(
        response.header("WWW-Authenticate"),
        Some("Basic realm=\"OpenCode Memory Explorer\", charset=\"UTF-8\"")
    );
    assert_eq!(response.header("Cache-Control"), Some("no-store"));
    assert_eq!(
        response.header("Content-Type"),
        Some("text/plain; charset=utf-8")
    );
    assert!(
        response
            .header("WWW-Authenticate")
            .unwrap_or_default()
            .contains(&format!("realm=\"{WEB_AUTH_REALM}\""))
    );
}

#[test]
fn test_basic_auth_rejects_wrong_password() {
    let auth = WebAuth::new(Some("right"), Some("admin"), "osuser");
    assert!(!auth.check(Some(&basic_header("admin", "wrong")), "/api/memories"));
}

#[test]
fn test_basic_auth_rejects_non_basic_scheme() {
    let auth = WebAuth::new(Some("secret"), Some("admin"), "osuser");
    assert!(!auth.check(Some("Bearer not-a-basic-token"), "/api/memories"));
}

#[test]
fn test_basic_auth_rejects_payload_without_colon() {
    let auth = WebAuth::new(Some("secret"), Some("admin"), "osuser");
    let encoded = STANDARD.encode("no-colon-here");
    assert!(!auth.check(Some(&format!("Basic {encoded}")), "/api/memories"));
}

#[test]
fn test_basic_auth_rejects_undecodable_base64() {
    let auth = WebAuth::new(Some("secret"), Some("admin"), "osuser");
    // Not valid base64 at all.
    assert!(!auth.check(Some("Basic !!!not-base64!!!"), "/api/memories"));
    // Valid base64, but the decoded payload is not UTF-8 (D-4).
    let non_utf8 = STANDARD.encode([0xff_u8, 0xfe, 0xfd]);
    assert!(!auth.check(Some(&format!("Basic {non_utf8}")), "/api/memories"));
}

#[test]
fn test_basic_auth_accepts_correct_credentials() {
    let auth = WebAuth::new(Some("secret"), Some("admin"), "osuser");
    assert!(auth.check(Some(&basic_header("admin", "secret")), "/api/memories"));
}

#[test]
fn test_basic_auth_username_is_case_sensitive() {
    let auth = WebAuth::new(Some("secret"), Some("Admin"), "osuser");
    assert!(!auth.check(Some(&basic_header("admin", "secret")), "/api/memories"));
    assert!(auth.check(Some(&basic_header("Admin", "secret")), "/api/memories"));
}

#[test]
fn test_basic_auth_password_may_contain_colon() {
    let auth = WebAuth::new(Some("pa:ss:word"), Some("admin"), "osuser");
    assert!(auth.check(Some(&basic_header("admin", "pa:ss:word")), "/api/memories"));
}

#[test]
fn test_basic_auth_scheme_is_case_insensitive() {
    let auth = WebAuth::new(Some("secret"), Some("admin"), "osuser");
    let encoded = STANDARD.encode("admin:secret");
    assert!(auth.check(Some(&format!("basic {encoded}")), "/api/memories"));
    assert!(auth.check(Some(&format!("BASIC {encoded}")), "/api/memories"));
}
