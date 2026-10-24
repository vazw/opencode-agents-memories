//! Target scoping of the file sink + the composition-facing install helper.
//!
//! The file sink is a contract, not a debugging dump: after this wave ONLY the
//! `mem_core::logging::log` facade (tracing target `mem_core::logging`) reaches
//! the log file. The 90 pre-existing `tracing::*!` diagnostics stay out, which
//! is what makes the reader's generic `x: error` rule safe.
//!
//! Runs as its own test binary because the global subscriber is
//! once-per-process. The pre-install no-op check runs first (deterministic
//! ordering); a scoped `with_default` subscriber then proves the facade call
//! site is live without touching the global default.

mod common;

use std::fs;
use std::sync::OnceLock;

use mem_core::error::LoggingError;
use mem_core::logging::{LogSink, SinkLayer, init_process_logging, log};
use tracing_subscriber::layer::SubscriberExt as _;

/// Installs the global subscriber exactly once regardless of test order.
fn ensure_initialized() -> LogSink {
    static ONCE: OnceLock<Result<LogSink, String>> = OnceLock::new();
    let held = ONCE.get_or_init(|| {
        let dir = common::unique_temp_dir("log-sink-scope");
        let sink = LogSink::new(dir.join("opencode-mem.log"));
        match init_process_logging(sink.clone()) {
            Ok(()) => Ok(sink),
            Err(error) => Err(error.to_string()),
        }
    });
    held.clone()
        .expect("the global logging install must succeed in a fresh process")
}

#[test]
fn test_sink_layer_scopes_to_facade_target_and_is_a_no_op_without_subscriber() {
    // (1) Before ANY global subscriber exists, the infallible facade is a no-op
    // and must not panic (tracing drops the event). No sink is attached to this
    // path yet, so this assert witnesses only "no panic / no eager file
    // creation" — it cannot prove routing.
    let early_dir = common::unique_temp_dir("log-sink-scope-early");
    let early_path = early_dir.join("opencode-mem.log");
    log(
        "before install",
        Some(&serde_json::json!({ "probe": true })),
    );
    assert!(
        !early_path.exists(),
        "a pre-install log() must not create the sink file"
    );

    // (1b) The real oracle: a scoped, thread-local subscriber over a fresh sink
    // proves the facade call site is live and routes through `SinkLayer`. A
    // stubbed or non-routing `log()` leaves the file absent and fails here.
    // `with_default` installs nothing globally and reverts when the closure ends.
    let scoped_sink = LogSink::new(early_path.clone());
    let scoped = tracing_subscriber::registry().with(SinkLayer::new(scoped_sink));
    tracing::subscriber::with_default(scoped, || {
        log(
            "inside a scoped subscriber",
            Some(&serde_json::json!({ "probe": true })),
        );
    });
    let scoped_content =
        fs::read_to_string(&early_path).expect("the scoped subscriber must create its sink file");
    assert!(
        scoped_content.contains(r#"] inside a scoped subscriber: {"probe":true}"#),
        "the facade must route through SinkLayer: {scoped_content}"
    );

    // (2) Install through the composition-facing helper (INFO pinned here, so
    // the root crate never names `LevelFilter`).
    let sink = ensure_initialized();
    let path = sink.path().to_path_buf();

    // (3) A raw `tracing` event from THIS test's own target must stay out of
    // the file; the facade event must land.
    tracing::info!("outside the facade target");
    log(
        "inside the facade target",
        Some(&serde_json::json!({ "probe": true })),
    );

    let content = fs::read_to_string(&path).expect("the log file is readable");
    assert!(
        content.contains("--- Session started: "),
        "the sink is primed with the banner: {content}"
    );
    assert!(
        content.contains(r#"] inside the facade target: {"probe":true}"#),
        "the facade event renders upstream-style: {content}"
    );
    assert!(
        !content.contains("outside the facade target"),
        "a non-facade tracing event must never reach the file: {content}"
    );
}

#[test]
fn test_init_process_logging_reports_unwritable_path() {
    // A path whose PARENT is a regular file: `create_dir_all` fails with a
    // typed io error before any global install is attempted.
    let dir = common::unique_temp_dir("log-sink-scope-unwritable");
    let blocker = dir.join("blocker");
    fs::write(&blocker, b"not a directory").expect("the blocker file is writable");
    let sink = LogSink::new(blocker.join("opencode-mem.log"));

    match init_process_logging(sink) {
        Err(LoggingError::Io { .. }) => {}
        other => panic!("an unwritable sink path must be LoggingError::Io, got {other:?}"),
    }
}
