//! Rotation stays visible to the frozen stats scanner (ADR-0015).
//!
//! The writer enforces the 5 MiB cap on every write. This file proves the two
//! halves that only meet at the reader's door:
//!
//! - the rotated archive uses exactly the name `list_log_files` recognises
//!   (`opencode-mem-<YYYY-MM-DD>.log`, no `.old` fallback here), and
//! - `GET /api/stats` still counts the pre-rotation lines (they live in the
//!   archive) plus the post-rotation line (it lives in the fresh live file).
//!
//! The reader itself is frozen and untouched: this test consumes the public
//! `LogSink::log` renderer and the public router seam only.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use common::TempDir;
use common::fakes::fake_services;
use common::{api_request, app_state_with_api};
use mem_core::logging::signatures::{AUTO_CAPTURE_MEMORY_PERSISTED, AUTO_CAPTURE_SKIPPED};
use mem_core::logging::{LogSink, MAX_LOG_SIZE};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// The seeded user identity the stats handler reports.
const USER_ID: &str = "probe@example.com";

/// `opencode-mem-YYYY-MM-DD.log` — the frozen reader's dated-archive shape.
fn is_dated_archive(name: &str) -> bool {
    let Some(stem) = name
        .strip_prefix("opencode-mem-")
        .and_then(|rest| rest.strip_suffix(".log"))
    else {
        return false;
    };
    let bytes = stem.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| (index == 4 || index == 7) || byte.is_ascii_digit())
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

#[tokio::test]
async fn test_size_rotation_keeps_the_archive_scanner_visible_and_pre_rotation_lines_counted() {
    let dir = TempDir::new("rotation-contract");
    let sink = LogSink::new(dir.path().join("opencode-mem.log"));

    // Five counted lines through the public renderer, then a SPARSE extension
    // to the production threshold, then the write that must rotate.
    for _ in 0..5 {
        sink.log(
            AUTO_CAPTURE_MEMORY_PERSISTED,
            Some(&serde_json::json!({ "probe": true })),
        )
        .expect("the scratch sink is writable");
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .open(sink.path())
        .expect("the live log file is writable");
    file.set_len(MAX_LOG_SIZE)
        .expect("the live log file is extended to the production cap");
    sink.log(
        AUTO_CAPTURE_SKIPPED,
        Some(&serde_json::json!({ "reason": "probe" })),
    )
    .expect("the triggering write rotates and appends");

    // --- writer-side shape -------------------------------------------------
    let entries: Vec<PathBuf> = fs::read_dir(dir.path())
        .expect("the log directory is readable")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let archives: Vec<&PathBuf> = entries
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| is_dated_archive(&name.to_string_lossy()))
        })
        .collect();
    assert_eq!(
        archives.len(),
        1,
        "the rotated archive must use the scanner-visible name: {entries:?}"
    );
    assert_eq!(
        fs::metadata(archives[0]).expect("archive metadata").len(),
        MAX_LOG_SIZE,
        "the archive holds the pre-rotation file byte-for-byte"
    );
    assert!(
        !dir.path().join("opencode-mem.log.old").exists(),
        "a same-date archive did not exist, so no .old fallback is used: {entries:?}"
    );
    let live_len = fs::metadata(sink.path())
        .expect("the live log file exists after rotation")
        .len();
    assert!(
        live_len < MAX_LOG_SIZE,
        "rotation must bound the live file below the cap: {live_len}"
    );

    // --- reader-side counts ------------------------------------------------
    let series = daily(dir.path().to_path_buf()).await;
    assert_eq!(series.len(), 14, "the series is a 14-day window");
    let row = series.last().expect("the 14-day series is non-empty");
    assert_eq!(
        row["captures"].as_u64(),
        Some(5),
        "the five pre-rotation lines live in the archive and are still counted: {row}"
    );
    assert_eq!(
        row["captureSkipped"].as_u64(),
        Some(1),
        "the post-rotation line lives in the live file and is counted: {row}"
    );
    assert_eq!(
        row["profileInjections"].as_u64(),
        Some(0),
        "no other cell moved: {row}"
    );
    assert_eq!(
        row["compactionInjections"].as_u64(),
        Some(0),
        "no other cell moved: {row}"
    );
    assert_eq!(
        row["errors"],
        serde_json::json!({}),
        "no error bucket is created: {row}"
    );
}
