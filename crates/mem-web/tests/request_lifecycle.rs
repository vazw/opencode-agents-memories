//! Port of upstream `example/opencode-mem/tests/web-server-request-lifecycle.test.ts`
//! ("Node web request lifecycle") onto the Rust `mem_web::lifecycle` seam.
//!
//! Upstream cases traced:
//! - "keeps the HTTP timeout above every cleanup deadline" →
//!   `test_idle_timeout_exceeds_every_cleanup_deadline`
//! - "does not disconnect when a fully consumed request body emits close",
//!   "disconnects once when the client aborts before the response ends" and
//!   "ignores response close after a completed response" → the disconnect
//!   bookkeeping is hyper's in the port; the observable contract (a client
//!   abort mid-request never stops the server) is
//!   `test_client_abort_mid_request_does_not_stop_server`.
//! - The panic envelope (D-3) → `test_panic_response_is_json_500_without_internal_message`.

mod common;

use std::net::Shutdown;
use std::path::PathBuf;
use std::time::Duration;

use mem_web::lifecycle::{
    EXTERNAL_PROFILE_CLEANUP_TIMEOUT, HTTP_IDLE_TIMEOUT, OPENCODE_PROFILE_CLEANUP_TIMEOUT,
    panic_response,
};
use mem_web::web_auth::WebAuth;

const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

#[test]
fn test_idle_timeout_exceeds_every_cleanup_deadline() {
    assert!(HTTP_IDLE_TIMEOUT > EXTERNAL_PROFILE_CLEANUP_TIMEOUT);
    assert!(HTTP_IDLE_TIMEOUT > OPENCODE_PROFILE_CLEANUP_TIMEOUT);
    assert_eq!(HTTP_IDLE_TIMEOUT, Duration::from_secs(330));
    assert_eq!(EXTERNAL_PROFILE_CLEANUP_TIMEOUT, Duration::from_secs(60));
    assert_eq!(OPENCODE_PROFILE_CLEANUP_TIMEOUT, Duration::from_secs(300));
}

#[tokio::test]
async fn test_panic_response_is_json_500_without_internal_message() {
    let response = common::response_to_raw(panic_response()).await;

    assert_eq!(response.status, 500);
    assert_eq!(response.header("Content-Type"), Some("application/json"));
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Internal Server Error"}"#
    );
    assert!(!response.body_string().contains("panic"));
}

#[tokio::test]
async fn test_client_abort_mid_request_does_not_stop_server() {
    let settings = mem_web::WebServerSettings {
        enabled: true,
        host: "127.0.0.1".to_owned(),
        port: 0,
        auth_password: None,
        auth_username: None,
        api_token: None,
    };
    let state = common::app_state(
        WebAuth::new(None, None, "osuser"),
        None,
        Some("local-token"),
        PathBuf::from(MISSING_DIST),
    );
    let handle = common::spawn_server(&settings, state).await;
    let addr = handle.local_addr();

    // A half-written request: the client aborts before the head is complete.
    {
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let partial = format!(
            "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n",
            addr.port()
        );
        use std::io::Write;
        let _ = stream.write_all(partial.as_bytes());
        let _ = stream.shutdown(Shutdown::Write);
    }

    // The server must still answer a well-formed request.
    let raw = format!(
        "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        addr.port()
    );
    let response = tokio::task::spawn_blocking(move || common::raw_request(addr, &raw))
        .await
        .expect("the socket helper must not panic");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"status":"ok","authEnabled":false}"#
    );
}

#[tokio::test]
async fn test_unknown_api_route_returns_404_after_gate_passes() {
    let settings = mem_web::WebServerSettings {
        enabled: true,
        host: "127.0.0.1".to_owned(),
        port: 0,
        auth_password: None,
        auth_username: None,
        api_token: None,
    };
    let state = common::app_state(
        WebAuth::new(None, None, "osuser"),
        None,
        Some("local-token"),
        PathBuf::from(MISSING_DIST),
    );
    let handle = common::spawn_server(&settings, state).await;
    let addr = handle.local_addr();

    // The local token is mandatory on every `/api/*` path except
    // `/api/health` (upstream `isAuthorizedApiRequest`), so the header must
    // be present for the request to reach the routing layer at all.
    let raw = format!(
        "GET /api/nope HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nx-opencode-mem-token: local-token\r\n\r\n",
        addr.port()
    );
    let response = tokio::task::spawn_blocking(move || common::raw_request(addr, &raw))
        .await
        .expect("the socket helper must not panic");
    assert_eq!(response.status, 404);
    assert_eq!(response.body_string(), "Not Found");
}
