//! Fix B2 — mapped-500 diagnosability: a mapped `PortError` that answers 5xx
//! must leave a visible, structured line on the engine's stderr.
//!
//! Root cause (debug: `.tmp/progress/engine-silent-500.debug-1.md`): the
//! mapped-failure log (`port_failure`'s `tracing::error!(… "api handler failed")`)
//! emits under the default module target (`mem_web::api`), while the
//! production file sink accepts ONLY the exact target `mem_core::logging`
//! (`crates/mem-core/src/logging.rs:384`) — every mapped 500 was therefore
//! silent by construction, and the "zero log lines" observation could not
//! discriminate panic from mapped failure. The fix routes the diagnostic
//! through the existing `structured_stderr` NDJSON seam (hoisted to mem-core
//! so mem-web can call it; the sink and `LOG_TARGET` stay untouched —
//! ADR-0014/ADR-0015, decision ADR-0031). The emitted event token is
//! `api.handler_failed` — the wrapper's message-slot grammar (`plugin.js`
//! `engineStderrLogLine`) admits `[A-Za-z0-9_.-]` only, and the house event
//! style is dot-separated (`capture.refused`, `profile.decay_failed`).
//!
//! Payload posture (ADR-0015 D-1 / issue-129 precedent): status + route +
//! error-variant TOKEN only. The store detail string (`PortError::Store(..)`)
//! is free text built from engine faults (paths, IO messages) and is NEVER
//! emitted — the emitted token names the variant, never the contents.
//!
//! RED contract: with the emitter call deleted from `port_failure` (or before
//! it exists), the diagnostic line is absent and this suite fails; the
//! response contract (`api_port_failure_class.rs`) is untouched by the fix, so
//! the two suites are orthogonal by construction.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::Arc;

use common::{TempDir, api_request, app_state_with_api, run_captured, throwaway_log_dir};
use mem_web::api::ports::{ApiServices, PortError};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// The configured API token.
const API_TOKEN: &str = "test-token";
/// A non-existent absolute path: the API cases never read the UI dist.
const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

/// The shipped router over an unavailable port bundle: `GET /api/memories`
/// reaches `port_failure` with `PortError::Unavailable` (the mapped 501 class)
/// through the production auth gate.
fn unavailable_router() -> axum::Router {
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from(MISSING_DIST),
        throwaway_log_dir(),
        ApiServices::unavailable(),
        "probe-user",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

/// The child half: drive the mapped-500 class through the shipped router. Its
/// stderr is the surface under test (`run_captured` puts it on a file, because
/// the emitter writes to the raw descriptor libtest never intercepts).
#[tokio::test]
#[ignore = "runs only as the child of test_mapped_failure_emits_a_structured_stderr_line"]
async fn mapped_failure_child() {
    let response = api_request(
        unavailable_router(),
        "GET",
        "/api/memories",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        501,
        "the child must actually reach the mapped-failure path: {}",
        response.body_string()
    );
}

/// THE pin: a mapped port failure emits one `api handler failed` NDJSON line on
/// the process's stderr, carrying the endpoint and an error-variant token and
/// NEVER the fault's free text.
#[test]
fn test_mapped_failure_emits_a_structured_stderr_line() {
    let dir = TempDir::new("b2-visibility");
    let text = run_captured("mapped_failure_child", &dir);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("\"event\":\"api.handler_failed\""))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one diagnostic line for the one mapped failure; child stderr:\n{text}"
    );
    let line = lines[0];
    assert!(
        line.contains("\"component\":\"api\""),
        "the line names its component: {line}"
    );
    assert!(
        line.contains("\"endpoint\":\"GET /api/memories\""),
        "the line names the endpoint: {line}"
    );
    assert!(
        line.contains("\"status\":501"),
        "the line carries the answered status: {line}"
    );
    assert!(
        line.contains("\"variant\":\"Unavailable\""),
        "the line carries the error-variant TOKEN: {line}"
    );
}

/// The privacy pin: the fault's own free text must never ride the stderr line.
/// `PortError::Store` wraps arbitrary engine-fault text (paths, IO errors);
/// the token posture names the variant only.
#[test]
fn test_store_detail_never_reaches_the_stderr_line() {
    let dir = TempDir::new("b2-privacy");
    let text = run_captured("store_failure_child", &dir);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("\"event\":\"api.handler_failed\""))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one diagnostic line for the one store failure; child stderr:\n{text}"
    );
    let line = lines[0];
    assert!(
        line.contains("\"variant\":\"Store\""),
        "the line carries the variant token: {line}"
    );
    assert!(
        line.contains("\"status\":500"),
        "the store class answers 500: {line}"
    );
    assert!(
        !line.contains("shard-7"),
        "no path fragment from the store detail may be emitted: {line}"
    );
    assert!(
        !line.contains("connection refused"),
        "the store detail string must never be emitted: {line}"
    );
}

/// The child half for the store class: the helper answers the fixed 500 for a
/// `PortError::Store` carrying realistic engine-fault text.
#[tokio::test]
#[ignore = "runs only as the child of test_store_detail_never_reaches_the_stderr_line"]
async fn store_failure_child() {
    let detail = "connection refused: /home/probe/shard-7.db";
    let response = common::response_to_raw(mem_web::api::port_failure(
        "GET /api/tags",
        PortError::Store(detail.to_string()),
    ))
    .await;
    assert_eq!(
        response.status,
        500,
        "the child must actually reach the store class: {}",
        response.body_string()
    );
}
