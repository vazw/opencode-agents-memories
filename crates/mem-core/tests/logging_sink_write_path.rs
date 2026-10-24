//! The single rotation-aware write path shared by the `log` facade and
//! `SinkLayer` (ADR-0015).
//!
//! Three contracts, all asserted through public surfaces only:
//!
//! 1. A deleted log directory is recovered by the next facade write, with the
//!    session banner and the triggering line (AC1).
//! 2. The 5 MiB cap is enforced on the write path and bounds the LIVE file
//!    (AC2, added in Step 3).
//! 3. A failing sink never re-enters `tracing` and never writes stderr (AC4) —
//!    proved in a child process so the scoped subscriber stays hermetic, with
//!    the child asserting the write actually failed before it records the
//!    observed event count.
//!
//! No global subscriber is installed here: every case uses a thread-scoped
//! `with_default`, so test order stays free.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mem_core::logging::{LogSink, MAX_LOG_SIZE, SinkLayer, log};
use tracing_subscriber::layer::SubscriberExt as _;

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

/// Every path in `dir`, sorted for deterministic assertions.
fn files_in(dir: &PathBuf) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .expect("the log directory is readable")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

/// Counts every event that reaches the subscriber (the recursion oracle).
#[derive(Clone, Default)]
struct CountingLayer(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountingLayer {
    fn on_event(
        &self,
        _event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn test_sink_layer_recovers_after_the_log_directory_is_deleted() {
    let dir = common::unique_temp_dir("log-write-path-recovery");
    let sink = LogSink::new(dir.join("opencode-mem.log"));
    let subscriber = tracing_subscriber::registry().with(SinkLayer::new(sink.clone()));

    tracing::subscriber::with_default(subscriber, || {
        log("before removal", None);
        assert!(
            sink.path().exists(),
            "fixture wiring: the first facade write creates the file at {}",
            sink.path().display()
        );

        fs::remove_dir_all(&dir).expect("simulate the external directory removal");

        log("after removal", Some(&serde_json::json!({ "probe": true })));
    });

    let content = fs::read_to_string(sink.path())
        .expect("the next facade write must recreate the deleted file");
    assert!(
        content.starts_with("\n--- Session started: "),
        "recovery writes the session banner first: {content:?}"
    );
    assert_eq!(
        content.matches("--- Session started: ").count(),
        1,
        "recovery writes exactly one banner: {content}"
    );
    assert!(
        content.contains("] after removal"),
        "recovery appends the triggering line: {content}"
    );
    assert!(
        !content.contains("before removal"),
        "the pre-deletion file is gone, so its lines cannot reappear: {content}"
    );
}

#[test]
fn test_facade_enforces_the_size_cap_and_bounds_the_live_file() {
    let dir = common::unique_temp_dir("log-write-path-cap");
    let sink = LogSink::new(dir.join("opencode-mem.log"));
    let subscriber = tracing_subscriber::registry().with(SinkLayer::new(sink.clone()));

    tracing::subscriber::with_default(subscriber, || {
        log("pre-fill", None);
        assert!(
            sink.path().exists(),
            "fixture wiring: the first facade write creates the file at {}",
            sink.path().display()
        );
        // A SPARSE file at the production threshold: the production size branch
        // is exercised without writing 5 MiB of user data.
        let file = fs::OpenOptions::new()
            .write(true)
            .open(sink.path())
            .expect("the live log file is writable");
        file.set_len(MAX_LOG_SIZE)
            .expect("the live log file is extended to the production cap");

        log("after the cap", None);
    });

    let live_len = fs::metadata(sink.path())
        .expect("the live log file exists after rotation")
        .len();
    assert!(
        live_len < MAX_LOG_SIZE,
        "rotation must bound the live file below the cap: {live_len}"
    );

    let entries = files_in(&dir);
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
        "size rotation produces exactly one dated archive: {entries:?}"
    );
    assert_eq!(
        fs::metadata(archives[0]).expect("archive metadata").len(),
        MAX_LOG_SIZE,
        "the archive holds the pre-rotation file byte-for-byte"
    );
    assert!(
        !dir.join("opencode-mem.log.old").exists(),
        "a same-date archive did not exist, so no .old fallback is used: {entries:?}"
    );

    let archived = fs::read_to_string(archives[0]).expect("the archive is readable");
    assert!(
        archived.contains("--- Session started: "),
        "the archive keeps the session banner: {archives:?}"
    );
    assert!(
        archived.contains("] pre-fill"),
        "the archive holds the pre-rotation line: {archives:?}"
    );
    let live = fs::read_to_string(sink.path()).expect("the live log is readable");
    assert!(
        live.contains("] after the cap"),
        "the live file holds the triggering line: {live}"
    );
}

#[test]
fn test_failing_sink_does_not_reenter_tracing_or_write_stderr() {
    let dir = common::unique_temp_dir("log-write-path-failing-sink");
    let out = dir.join("failing-sink");
    // The sink path's parent is a REGULAR FILE, so nothing can ever be created
    // below it: the child proves the write failed by asserting the sink path
    // does not exist before it records the event count.
    let blocker = PathBuf::from(format!("{}.blocker", out.display()));
    fs::write(&blocker, b"not a directory").expect("the blocker file is writable");
    assert!(
        blocker.is_file(),
        "fixture wiring: the blocker is a regular file at {}",
        blocker.display()
    );

    let status = common::run_fixture_child(
        "failing-sink-reentry",
        &out,
        // libtest captures stdout/stderr by default (and propagates the capture
        // to spawned threads), which would hide a production `eprintln!` from
        // the parent's stderr file. `RUST_TEST_NOCAPTURE` keeps the child's
        // stderr on the real file descriptor, so the guard is non-vacuous.
        &[("RUST_TEST_NOCAPTURE", "1")],
        Duration::from_secs(30),
    )
    .expect("the child fixture runs to completion within its deadline");
    assert!(
        status.success(),
        "the child must exit successfully: {status}"
    );

    let evidence = fs::read_to_string(&out).expect("the child writes its evidence file");
    assert_eq!(
        evidence, "1",
        "one facade call must reach the subscriber exactly once (no re-entry)"
    );
    let stderr =
        fs::read_to_string(out.with_extension("stderr")).expect("the child stderr file exists");
    assert!(
        stderr.is_empty(),
        "a failing sink write must never write stderr (NDJSON corruption): {stderr:?}"
    );
}

/// Child entry point (exact name required by `common::run_fixture_child`):
/// harmless no-op unless the parent selected a mode.
#[test]
#[ignore = "child entry: re-executed by run_fixture_child; no-op without MEM_FIXTURE_CHILD"]
fn fixture_child_entry() {
    let Ok(mode) = std::env::var(common::FIXTURE_CHILD_ENV) else {
        return;
    };
    if mode == "failing-sink-reentry" {
        failing_sink_reentry_child();
    }
}

/// Child mode `failing-sink-reentry`: install a scoped counting layer plus the
/// real `SinkLayer` over an uncreatable sink path, make one facade call, prove
/// the write failed, then record how many events reached the subscriber.
fn failing_sink_reentry_child() {
    let out = PathBuf::from(
        std::env::var(common::FIXTURE_OUT_ENV).expect("MEM_FIXTURE_OUT is set by the parent"),
    );
    let blocker = PathBuf::from(format!("{}.blocker", out.display()));
    assert!(
        blocker.is_file(),
        "the parent must create the blocker as a regular file: {}",
        blocker.display()
    );
    let sink = LogSink::new(blocker.join("opencode-mem.log"));
    let events = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry()
        .with(CountingLayer(events.clone()))
        .with(SinkLayer::new(sink.clone()));

    tracing::subscriber::with_default(subscriber, || {
        log("must not re-enter", None);
    });

    assert!(
        !sink.path().exists(),
        "the sink write must have failed before the count is recorded: {}",
        sink.path().display()
    );
    fs::write(&out, events.load(Ordering::SeqCst).to_string())
        .expect("the evidence file is writable");
}
