//! Drift guard: the nine scraped log signatures live in TWO spellings — the
//! constants in `mem_core::logging::signatures` (the writer side) and the
//! literals inside `stats.rs`'s frozen classifier (the reader side). This test
//! is the mandatory bridge: it writes each constant through the real
//! `LogSink::log` renderer, drives `GET /api/stats` through `build_router`,
//! and asserts the expected cell moved while every other cell of today's row
//! stayed zero.
//!
//! The parser and its in-module tests are frozen; this file only consumes the
//! public writer + the public router seam. Temp dirs live under
//! `std::env::temp_dir()` (TMPDIR-redirected into `.tmp/scratch`); the process
//! environment is never mutated.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::TempDir;
use common::fakes::fake_services;
use common::{api_request, app_state_with_api};
use mem_core::logging::LogSink;
use mem_core::logging::signatures::{
    AUTO_CAPTURE_FINAL_ERROR, AUTO_CAPTURE_MEMORY_PERSISTED, AUTO_CAPTURE_SKIPPED,
    COMPACTION_HANDLER_ERROR, COMPACTION_MEMORY_INJECTED, FAILED_TO_LOAD_SKILLS,
    IDLE_PROCESSING_ERROR, PROFILE_INJECT, TURSO_READY_GATE_FAILED,
};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// The seeded user identity the stats handler reports.
const USER_ID: &str = "probe@example.com";

/// The intended `GET /api/stats` cell of one signature.
#[derive(Debug, Clone, Copy)]
enum Cell {
    Captures,
    CaptureSkipped,
    ProfileInjections,
    CompactionInjections,
    Error(&'static str),
}

/// The nine constants and the cell each one must classify to.
fn constants() -> [(&'static str, Cell); 9] {
    [
        (AUTO_CAPTURE_MEMORY_PERSISTED, Cell::Captures),
        (AUTO_CAPTURE_SKIPPED, Cell::CaptureSkipped),
        (PROFILE_INJECT, Cell::ProfileInjections),
        (COMPACTION_MEMORY_INJECTED, Cell::CompactionInjections),
        (
            TURSO_READY_GATE_FAILED,
            Cell::Error(TURSO_READY_GATE_FAILED),
        ),
        (FAILED_TO_LOAD_SKILLS, Cell::Error(FAILED_TO_LOAD_SKILLS)),
        (
            AUTO_CAPTURE_FINAL_ERROR,
            Cell::Error(AUTO_CAPTURE_FINAL_ERROR),
        ),
        (
            COMPACTION_HANDLER_ERROR,
            Cell::Error(COMPACTION_HANDLER_ERROR),
        ),
        (IDLE_PROCESSING_ERROR, Cell::Error(IDLE_PROCESSING_ERROR)),
    ]
}

/// Writes one line through the production renderer into a fresh scratch log.
fn write_line(dir: &Path, message: &str) -> PathBuf {
    let sink = LogSink::new(dir.join("opencode-mem.log"));
    sink.log(message, Some(&serde_json::json!({ "probe": true })))
        .expect("the scratch sink is writable");
    sink.path().to_path_buf()
}

/// The stats router over `log_dir` with the canonical fake port bundle.
fn stats_router(log_dir: PathBuf) -> axum::Router {
    let (_backend, services) = fake_services();
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        log_dir,
        services,
        USER_ID,
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

/// `GET /api/stats` and its daily series (oldest first, ending today).
async fn daily(log_dir: PathBuf) -> Vec<Value> {
    let response = api_request(
        stats_router(log_dir),
        "GET",
        "/api/stats",
        Some(API_TOKEN),
        None,
    )
    .await;
    let body = response.body_string();
    assert_eq!(response.status, 200, "{body}");
    let parsed: Value = serde_json::from_str(&body).expect("the stats answer is JSON");
    parsed["data"]["daily"]
        .as_array()
        .cloned()
        .expect("the stats answer carries the daily series")
}

/// Today's row is the last element (the series is oldest-first).
fn today(series: &[Value]) -> &Value {
    series.last().expect("the 14-day series is non-empty")
}

/// Every cell of one day, as a comparable tuple.
fn cells(day: &Value) -> (u64, u64, u64, u64, Value) {
    (
        day["captures"].as_u64().expect("captures"),
        day["captureSkipped"].as_u64().expect("captureSkipped"),
        day["profileInjections"]
            .as_u64()
            .expect("profileInjections"),
        day["compactionInjections"]
            .as_u64()
            .expect("compactionInjections"),
        day["errors"].clone(),
    )
}

#[tokio::test]
async fn test_signature_constants_classify_to_their_expected_stats_cell() {
    for (constant, cell) in constants() {
        let dir = TempDir::new("signature-cell");
        write_line(dir.path(), constant);
        let series = daily(dir.path().to_path_buf()).await;
        assert_eq!(series.len(), 14, "the series is a 14-day window");
        let row = today(&series);

        let (captures, skipped, profile, compaction, errors) = cells(row);
        let (want_captures, want_skipped, want_profile, want_compaction, want_error) = match cell {
            Cell::Captures => (1, 0, 0, 0, None),
            Cell::CaptureSkipped => (0, 1, 0, 0, None),
            Cell::ProfileInjections => (0, 0, 1, 0, None),
            Cell::CompactionInjections => (0, 0, 0, 1, None),
            Cell::Error(bucket) => (0, 0, 0, 0, Some(bucket)),
        };
        assert_eq!(captures, want_captures, "captures for {constant:?}: {row}");
        assert_eq!(
            skipped, want_skipped,
            "captureSkipped for {constant:?}: {row}"
        );
        assert_eq!(
            profile, want_profile,
            "profileInjections for {constant:?}: {row}"
        );
        assert_eq!(
            compaction, want_compaction,
            "compactionInjections for {constant:?}: {row}"
        );
        match want_error {
            None => assert_eq!(
                errors,
                serde_json::json!({}),
                "no error bucket for {constant:?}: {row}"
            ),
            Some(bucket) => {
                assert_eq!(
                    errors,
                    serde_json::json!({ bucket: 1 }),
                    "exactly the {bucket:?} bucket for {constant:?}: {row}"
                );
            }
        }
    }
}

#[tokio::test]
async fn test_signature_constants_are_prefix_safe_for_the_parser() {
    let constants = constants();
    for (index, (constant, _)) in constants.iter().enumerate() {
        // The frozen parser splits the message at the FIRST `: {` and the
        // generic rule keys on a trailing `: error`; a constant carrying
        // either delimiter would be truncated or re-bucketed.
        assert!(
            !constant.contains(": {"),
            "the payload delimiter must never appear in a signature: {constant:?}"
        );
        assert!(
            !constant.ends_with(':'),
            "a trailing colon would be swallowed by the parser: {constant:?}"
        );
        // `classify_log_message` matches by prefix, so one constant being a
        // strict prefix of another makes the answer order-dependent.
        for (other, _) in constants.iter().skip(index + 1) {
            assert!(
                !other.starts_with(constant) && !constant.starts_with(other),
                "prefix collision between {constant:?} and {other:?}"
            );
        }

        // The rendered line's message half is the constant, byte-exact.
        let dir = TempDir::new("signature-prefix");
        let path = write_line(dir.path(), constant);
        let content = std::fs::read_to_string(&path).expect("the scratch log is readable");
        let line = content
            .lines()
            .find(|line| line.starts_with('[') && line.contains(&format!("] {constant}")))
            .unwrap_or_else(|| panic!("no rendered line for {constant:?}: {content}"));
        let rendered = line
            .split_once("] ")
            .map(|(_stamp, rest)| rest)
            .expect("the rendered line carries the stamp delimiter");
        let message = rendered
            .split_once(": {")
            .map(|(message, _payload)| message)
            .unwrap_or(rendered);
        assert_eq!(
            message, *constant,
            "the message half must be the constant byte-exact"
        );
    }
}
