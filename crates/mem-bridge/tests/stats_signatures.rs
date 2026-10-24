//! The capture path's three production-reachable log signatures, asserted on
//! the RENDERED lines (never on internals).
//!
//! One process-global subscriber per binary (`init_process_logging`), one sink,
//! and a `tokio::sync::Mutex` serializing the sink-touching tests. A std guard
//! across `.await` would be an `await_holding_lock` finding, so the lock is
//! tokio's. Most assertions are line-scoped (`lines().filter(..)`) deltas from
//! the test's own baseline, so the shared sink is safe; the two payload checks
//! that name an exact capture id / reason constant search the whole file.

mod common;

use std::sync::{Arc, OnceLock};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_bridge::capture::{
    CaptureParams, CaptureService, CaptureSummaryInput, Tier1WriteOutcome, Tier1Writer,
};
use mem_bridge::project_scope::ResolvedProjectScope;
use mem_core::logging::LogSink;
use mem_embed::embedder::BoxFuture;
use mem_episodic::store::EpisodicStore;

/// Serializes the sink-touching tests (a tokio mutex: never a std guard across
/// an `.await`).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The process-global sink, installed exactly once.
fn sink() -> LogSink {
    static ONCE: OnceLock<LogSink> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = common::temp_dir("log-signatures");
        let sink = LogSink::new(dir.join("opencode-mem.log"));
        mem_core::logging::init_process_logging(sink.clone())
            .expect("the per-binary sink installs");
        sink
    })
    .clone()
}

/// The number of rendered lines containing `needle`.
fn count(needle: &str) -> usize {
    std::fs::read_to_string(sink().path())
        .expect("the per-binary sink file exists")
        .lines()
        .filter(|line| line.contains(needle))
        .count()
}

/// A scripted tier-1 writer: pops queued outcomes, defaulting to `Written`.
#[derive(Default)]
struct ScriptedWriter {
    outcomes: std::sync::Mutex<std::collections::VecDeque<Tier1WriteOutcome>>,
}

impl ScriptedWriter {
    fn queue(outcomes: &[Tier1WriteOutcome]) -> Self {
        Self {
            outcomes: std::sync::Mutex::new(outcomes.iter().copied().collect()),
        }
    }
}

impl Tier1Writer for ScriptedWriter {
    fn write<'a>(
        &'a self,
        _scope: &'a ResolvedProjectScope,
        _input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, mem_bridge::capture::CaptureWriterError>> {
        let outcome = self
            .outcomes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or(Tier1WriteOutcome::Written);
        Box::pin(async move { Ok(outcome) })
    }
}

/// A capture service over a fresh real episodic store.
fn service(dir: &TestDir, writer: Arc<dyn Tier1Writer>) -> CaptureService {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store),
        writer,
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
}

fn params(capture_id: &str) -> CaptureParams {
    CaptureParams {
        session_id: "sess-log".to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/log".to_string(),
        agent_id: "agent-1".to_string(),
        ..Default::default()
    }
}

/// A params value carrying a supplied summary (the drain's `Capture` arm).
fn with_summary(capture_id: &str) -> CaptureParams {
    let mut params = params(capture_id);
    params.summary = Some("Fixed the retry storm in the login handler.".to_string());
    params
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_written_emits_persisted_line_and_replayed_emits_none() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    // (a) `Written` on the drain path: exactly one persisted line.
    let dir = TestDir::new("log-written");
    let capture = service(
        &dir,
        Arc::new(ScriptedWriter::queue(&[Tier1WriteOutcome::Written])),
    );
    let before = count("] Auto-capture memory persisted: {\"captureId\":\"");
    capture
        .enqueue(&with_summary("019a0000-0000-7f00-8000-00000000b001"))
        .await
        .expect("enqueue");
    let report = capture.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "{report:?}");
    assert_eq!(
        count("] Auto-capture memory persisted: {\"captureId\":\""),
        before + 1,
        "a Written tier-1 result emits exactly one persisted line"
    );
    let content = std::fs::read_to_string(sink().path()).expect("the per-binary sink file exists");
    assert!(
        content.contains(
            "] Auto-capture memory persisted: {\"captureId\":\"019a0000-0000-7f00-8000-00000000b001\""
        ),
        "the payload carries the capture id: {content}"
    );

    // (b) `Replayed`: the deterministic row already existed — NO line.
    let dir = TestDir::new("log-replayed");
    let capture = service(
        &dir,
        Arc::new(ScriptedWriter::queue(&[Tier1WriteOutcome::Replayed])),
    );
    let before = count("] Auto-capture memory persisted: {\"captureId\":\"");
    capture
        .enqueue(&with_summary("019a0000-0000-7f00-8000-00000000b002"))
        .await
        .expect("enqueue");
    let report = capture.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "{report:?}");
    assert_eq!(
        count("] Auto-capture memory persisted: {\"captureId\":\""),
        before,
        "a Replayed result must not double-count the capture"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_supplied_summary_flush_emits_persisted_line() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-flush-supplied");
    let capture = service(
        &dir,
        Arc::new(ScriptedWriter::queue(&[Tier1WriteOutcome::Written])),
    );
    let before = count("] Auto-capture memory persisted: {\"captureId\":\"");
    capture
        .enqueue(&with_summary("019a0000-0000-7f00-8000-00000000b003"))
        .await
        .expect("enqueue");
    // The shutdown `db_flush` leg: supplied summaries only.
    let report = capture
        .flush_supplied_summary_queued()
        .await
        .expect("flush");
    assert_eq!(report.done, 1, "{report:?}");
    assert_eq!(
        count("] Auto-capture memory persisted: {\"captureId\":\""),
        before + 1,
        "the supplied-summary flush is a second production path and emits too"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_skipped_emits_one_skipped_line() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-skipped");
    let capture = service(&dir, Arc::new(ScriptedWriter::default()));
    let before = count("] Auto-capture skipped: {\"");
    // No summary, no metadata, no transcript → `CaptureSummaryOutput::Skip`.
    capture
        .enqueue(&params("019a0000-0000-7f00-8000-00000000b004"))
        .await
        .expect("enqueue");
    let report = capture.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "{report:?}");
    assert_eq!(
        count("] Auto-capture skipped: {\""),
        before + 1,
        "a skipped capture emits exactly one line"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_retry_exhaustion_emits_final_error_after_five_attempts() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-retry-exhaustion");
    // Every tier-1 write fails: five consumed attempts exhaust the budget.
    let capture = service(&dir, Arc::new(RecorderTier1Writer::new(usize::MAX)));
    let before = count("] Auto-capture final error after 5 attempts: {\"");
    capture
        .enqueue(&with_summary("019a0000-0000-7f00-8000-00000000b005"))
        .await
        .expect("enqueue");
    for _ in 0..5 {
        let report = capture.drain_once().await.expect("drain");
        assert_eq!(report.retryable + report.dead_lettered, 1, "{report:?}");
    }
    assert_eq!(
        count("] Auto-capture final error after 5 attempts: {\""),
        before + 1,
        "the fifth consumed attempt emits exactly one final-error line"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_immediate_dead_letter_emits_final_error() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-immediate-dead");
    let capture = service(&dir, Arc::new(ScriptedWriter::default()));
    let before = count("] Auto-capture final error: {\"");
    let mut dead = params("019a0000-0000-7f00-8000-00000000b006");
    dead.agent_role = Some("bogus".to_string());
    capture.enqueue(&dead).await.expect("enqueue");
    let report = capture.drain_once().await.expect("drain");
    assert_eq!(report.dead_lettered, 1, "{report:?}");
    assert_eq!(
        count("] Auto-capture final error: {\""),
        before + 1,
        "an immediate dead-letter emits exactly one final-error line"
    );
    let content = std::fs::read_to_string(sink().path()).expect("the per-binary sink file exists");
    assert!(
        content
            .contains("\"reason\":\"agent role must be one of: main, explorer, reviewer, worker\""),
        "the payload carries the FIXED reason constant, never the rejected value: {content}"
    );
    assert!(
        !content.contains("bogus"),
        "the rejected agent_role value must never reach the log: {content}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_still_completes_when_the_sink_becomes_unwritable() {
    let _guard = SERIAL.lock().await;
    let sink = sink();

    // Replace the log FILE with a DIRECTORY: `append` fails `EISDIR`, which the
    // layer must swallow (a logging failure never fails a capture).
    let path = sink.path().to_path_buf();
    std::fs::remove_file(&path).expect("remove the log file");
    std::fs::create_dir_all(&path).expect("replace the log file with a directory");

    let dir = TestDir::new("log-unwritable-sink");
    let capture = service(
        &dir,
        Arc::new(ScriptedWriter::queue(&[Tier1WriteOutcome::Written])),
    );
    capture
        .enqueue(&with_summary("019a0000-0000-7f00-8000-00000000b007"))
        .await
        .expect("enqueue");
    let report = capture.drain_once().await.expect("the drain still runs");
    assert_eq!(
        report.done, 1,
        "the capture completes regardless: {report:?}"
    );

    // Restore through the real ENOENT re-init path. The facade goes
    // `SinkLayer::on_event -> sink.append` (which opens with `create(true)`), so
    // it can never re-initialize; only `LogSink::log` does. Delete the sink's
    // PARENT directory: `append` then fails `NotFound`, `log` clears
    // `initialized`, and `init()` recreates the directory, rewrites the banner,
    // and appends the triggering line.
    let sink_dir = path
        .parent()
        .expect("the sink path has a parent directory")
        .to_path_buf();
    std::fs::remove_dir_all(&sink_dir).expect("remove the sink's parent directory");
    assert!(
        !path.exists(),
        "the sink file must be gone before the re-init probe"
    );
    sink.log("restore the sink after the unwritable case", None)
        .expect("log() re-initializes after the directory vanishes");
    let restored = std::fs::read_to_string(&path).expect("the log file is recreated");
    assert!(
        restored.contains("--- Session started: "),
        "init() rewrites the banner, which append() alone cannot: {restored}"
    );
    assert!(
        restored.contains("] restore the sink after the unwritable case"),
        "the triggering line is appended after re-init: {restored}"
    );
}
