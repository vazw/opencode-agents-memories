//! Global tracing install end-to-end: `init_logging` + `SinkLayer` capture
//! and level filtering. Runs as its own test binary because the global
//! subscriber is once-per-process.

use std::fs;
use std::sync::{Mutex, OnceLock};

use mem_core::logging::{LogSink, init_logging, log as core_log};
use tracing_subscriber::filter::LevelFilter;

/// Ensures the global install happened exactly once regardless of test
/// execution order within this binary.
fn ensure_initialized() -> LogSink {
    static ONCE: OnceLock<Result<LogSink, String>> = OnceLock::new();
    let held = ONCE.get_or_init(|| {
        let dir = common::unique_temp_dir("log-init-global");
        let sink = LogSink::new(dir.join("opencode-mem.log"));
        match init_logging(sink.clone(), LevelFilter::INFO) {
            Ok(()) => Ok(sink),
            Err(e) => Err(e.to_string()),
        }
    });
    held.clone()
        .expect("global logging must install in this fresh process")
}

/// Lock serializing log calls around reads (message ordering stays stable).
static SEQUENCER: Mutex<()> = Mutex::new(());

mod common;

#[test]
fn test_global_init_writes_info_events_and_filters_debug() {
    let sink = ensure_initialized();
    let path = sink.path().to_path_buf();

    let _guard = SEQUENCER.lock().unwrap_or_else(|p| p.into_inner());
    core_log("hello", Some(&serde_json::json!({ "a": 1 })));
    // A debug event must NOT be captured by the INFO filter.
    tracing::debug!("debug line must be filtered");
    drop(_guard);

    let content = fs::read_to_string(&path).expect("log file readable");
    assert!(content.contains("--- Session started: "), "{content}");
    assert!(
        content.contains("] hello: {\"a\":1}\n"),
        "info event with data_json must render upstream-style line: {content}"
    );
    let info_line = content
        .lines()
        .find(|line| line.contains("] hello: "))
        .expect("info line present");
    let ts = info_line
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .map(|(ts, _)| ts)
        .expect("timestamp prefix");
    assert!(
        ts.len() == 29 && ts.matches('.').count() == 1 && ts.contains('T'),
        "line must not carry level info; timestamp: {ts}"
    );
    assert!(
        !content.contains("DEBUG"),
        "no level suffix on lines: {content}"
    );
}

#[test]
fn test_global_init_twice_fails() {
    let _ = ensure_initialized();
    let dir = common::unique_temp_dir("log-init-global-second");
    let sink = LogSink::new(dir.join("opencode-mem.log"));
    let error = init_logging(sink, LevelFilter::INFO)
        .expect_err("second global install must fail with AlreadyInstalled");
    match &error {
        mem_core::error::LoggingError::AlreadyInstalled => {}
        other => panic!("expected AlreadyInstalled, got {other:?}"),
    }
    assert!(
        error.to_string().contains("already installed"),
        "display: {error}"
    );
}
