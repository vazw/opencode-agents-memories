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

    // Restore through the real ENOENT re-init path. The capture above exercised
    // only the layer's swallow branch (`EISDIR`); this is a separate facade
    // call. The facade and `SinkLayer` now share one rotation-aware
    // `write_line` path (ADR-0015): delete the sink's PARENT directory so the
    // append fails `NotFound`, the path clears `initialized`, re-creates the
    // directory, rewrites the banner and appends the triggering line.
    let sink_dir = path
        .parent()
        .expect("the sink path has a parent directory")
        .to_path_buf();
    std::fs::remove_dir_all(&sink_dir).expect("remove the sink's parent directory");
    assert!(
        !path.exists(),
        "the sink file must be gone before the re-init probe"
    );
    mem_core::logging::log("restore the sink after the unwritable case", None);
    let restored = std::fs::read_to_string(&path).expect("the log file is recreated");
    assert!(
        restored.contains("--- Session started: "),
        "recovery rewrites the banner, which append() alone cannot: {restored}"
    );
    assert!(
        restored.contains("] restore the sink after the unwritable case"),
        "the triggering line is appended after re-init: {restored}"
    );
}

// ---------------------------------------------------------------------------
// Host transport wave — every capture failure carries a NAME
// ---------------------------------------------------------------------------

/// Params carrying a transcript, so the summarizer leg is reached (a payload
/// with no context at all is a `skip`, not a failure).
fn with_transcript(capture_id: &str) -> CaptureParams {
    let mut params = params(capture_id);
    params.transcript = Some("user: fix the retry storm in the login handler".to_string());
    params
}

/// A summarizer that always fails and reports a FIXED provider failure.
struct FailingProviderSummarizer {
    failure: mem_bridge::capture_prompt::SummarizerFailure,
}

impl mem_bridge::capture_prompt::CaptureSummarizer for FailingProviderSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> mem_embed::embedder::BoxFuture<
        'a,
        Result<mem_bridge::capture::CaptureSummaryOutput, mem_episodic::error::EpisodicError>,
    > {
        Box::pin(async move {
            Err(mem_episodic::error::EpisodicError::Io {
                message: "provider leg failed".to_string(),
            })
        })
    }

    fn last_failure(&self) -> Option<mem_bridge::capture_prompt::SummarizerFailure> {
        Some(self.failure.clone())
    }
}

fn service_with_summarizer(
    dir: &TestDir,
    writer: Arc<dyn Tier1Writer>,
    summarizer: Arc<dyn mem_bridge::capture_prompt::CaptureSummarizer>,
) -> CaptureService {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store),
        writer,
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(summarizer),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
}

/// The line of the sink that names `capture_id` and contains `needle`.
fn line_with(capture_id: &str, needle: &str) -> String {
    std::fs::read_to_string(sink().path())
        .expect("the per-binary sink file exists")
        .lines()
        .rfind(|line| line.contains(capture_id) && line.contains(needle))
        .unwrap_or_default()
        .to_string()
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_retry_exhaustion_names_the_capture_provider_reason() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-no-provider-reason");
    // No summarizer at all + a transcript: the degraded composition path.
    let capture = service(&dir, Arc::new(ScriptedWriter::default()));
    let capture_id = "019a0000-0000-7f00-8000-00000000b101";
    capture
        .enqueue(&with_transcript(capture_id))
        .await
        .expect("enqueue");
    for _ in 0..5 {
        let report = capture.drain_once().await.expect("drain");
        assert_eq!(report.retryable + report.dead_lettered, 1, "{report:?}");
    }

    let line = line_with(capture_id, "Auto-capture final error");
    assert!(
        line.contains("\"reason\":\"capture_provider_unavailable\""),
        "the dead-letter names the missing summarizer: {line}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_tier1_failure_names_tier1_write_failed() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-tier1-reason");
    let capture = service(&dir, Arc::new(RecorderTier1Writer::new(usize::MAX)));
    let capture_id = "019a0000-0000-7f00-8000-00000000b102";
    capture
        .enqueue(&with_summary(capture_id))
        .await
        .expect("enqueue");
    for _ in 0..5 {
        let report = capture.drain_once().await.expect("drain");
        assert_eq!(report.retryable + report.dead_lettered, 1, "{report:?}");
    }

    let line = line_with(capture_id, "Auto-capture final error");
    assert!(
        line.contains("\"reason\":\"tier1_write_failed\""),
        "a tier-1 write failure has its own reason: {line}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_provider_failure_logs_error_kind_and_status() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-provider-kind");
    let summarizer = Arc::new(FailingProviderSummarizer {
        failure: mem_bridge::capture_prompt::SummarizerFailure {
            reason: "host_auth_rejected",
            error_kind: "auth",
            status: Some(401),
            retryable: true,
            retry_after_ms: None,
            detail: None,
        },
    });
    let capture = service_with_summarizer(&dir, Arc::new(ScriptedWriter::default()), summarizer);
    let capture_id = "019a0000-0000-7f00-8000-00000000b103";
    capture
        .enqueue(&with_transcript(capture_id))
        .await
        .expect("enqueue");
    for _ in 0..5 {
        let _ = capture.drain_once().await.expect("drain");
    }

    let failed = line_with(capture_id, "Auto-capture provider failed");
    assert!(
        failed.contains("\"reason\":\"host_auth_rejected\""),
        "{failed}"
    );
    assert!(failed.contains("\"errorKind\":\"auth\""), "{failed}");
    assert!(failed.contains("\"status\":401"), "{failed}");

    let final_error = line_with(capture_id, "Auto-capture final error");
    assert!(
        final_error.contains("\"reason\":\"host_auth_rejected\""),
        "{final_error}"
    );
    assert!(
        final_error.contains("\"errorKind\":\"auth\""),
        "{final_error}"
    );
    assert!(final_error.contains("\"status\":401"), "{final_error}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_host_config_reason_is_carried_into_the_dead_letter() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-host-config-reason");
    let summarizer = Arc::new(FailingProviderSummarizer {
        failure: mem_bridge::capture_prompt::SummarizerFailure {
            reason: "service_file_missing",
            error_kind: "config",
            status: None,
            retryable: true,
            retry_after_ms: None,
            detail: None,
        },
    });
    let capture = service_with_summarizer(&dir, Arc::new(ScriptedWriter::default()), summarizer);
    let capture_id = "019a0000-0000-7f00-8000-00000000b104";
    capture
        .enqueue(&with_transcript(capture_id))
        .await
        .expect("enqueue");
    for _ in 0..5 {
        let _ = capture.drain_once().await.expect("drain");
    }

    let line = line_with(capture_id, "Auto-capture final error");
    assert!(
        line.contains("\"reason\":\"service_file_missing\""),
        "the host reason survives into the dead-letter: {line}"
    );
    assert!(line.contains("\"errorKind\":\"config\""), "{line}");
    assert!(
        !line.contains("\"status\""),
        "a resolution failure has no HTTP status and must not invent one: {line}"
    );
}

/// A scripted provider: `Err(Network)` on the first call, then a body that is
/// not a capture JSON on every later call.
struct ScriptedProvider {
    calls: std::sync::atomic::AtomicUsize,
}

impl mem_ai::provider::AiProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn complete(
        &self,
        _request: mem_ai::provider::CompletionRequest,
    ) -> mem_ai::provider::CompletionStream {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            Box::pin(mem_episodic::futures::stream::iter(vec![Err(
                mem_ai::provider::ProviderError::network("transport down".to_string())
                    .with_provider("scripted"),
            )]))
        } else {
            Box::pin(mem_episodic::futures::stream::iter(vec![
                Ok(mem_store::serde_json::json!({ "text": "not json" })),
                Ok(mem_store::serde_json::json!({ "done": true })),
            ]))
        }
    }

    fn embed<'life0, 'async_trait>(
        &'life0 self,
        _request: mem_ai::provider::EmbedRequest,
    ) -> mem_ai::provider::BoxFuture<
        'async_trait,
        Result<Vec<Vec<f32>>, mem_ai::provider::ProviderError>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn list_models<'life0, 'async_trait>(
        &'life0 self,
    ) -> mem_ai::provider::BoxFuture<
        'async_trait,
        Result<Vec<mem_ai::provider::ModelInfo>, mem_ai::provider::ProviderError>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn count_tokens<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _model: &'life1 str,
        _text: &'life2 str,
    ) -> mem_ai::provider::BoxFuture<'async_trait, Result<u32, mem_ai::provider::ProviderError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(0) })
    }
}

/// The capture seam's half of the same script: `Err(network)` on the first
/// call, then a value that is not a capture object. The summarizer holds only
/// this handle now (ADR-0029), so the double must script it too.
impl mem_bridge::capture_provider::StructuredCapture for ScriptedProvider {
    fn complete_structured<'a>(
        &'a self,
        _request: mem_ai::provider::CompletionRequest,
        _schema: mem_store::serde_json::Value,
    ) -> mem_ai::provider::BoxFuture<
        'a,
        Result<mem_store::serde_json::Value, mem_ai::provider::ProviderError>,
    > {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if call == 0 {
                Err(
                    mem_ai::provider::ProviderError::network("transport down".to_string())
                        .with_provider("scripted"),
                )
            } else {
                Ok(mem_store::serde_json::json!("not json"))
            }
        })
    }
}

/// R25/B2 — the REAL summarizer clears its record on entry, so a stale
/// `network` reason can never name a later parse failure.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_stale_failure_cannot_name_a_later_attempt() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-stale-failure");
    let provider = Arc::new(ScriptedProvider {
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let summarizer = Arc::new(mem_bridge::capture_prompt::ProviderCaptureSummarizer::new(
        provider,
        "beam-free",
        131_072,
    ));
    let capture = service_with_summarizer(&dir, Arc::new(ScriptedWriter::default()), summarizer);
    let capture_id = "019a0000-0000-7f00-8000-00000000b105";
    capture
        .enqueue(&with_transcript(capture_id))
        .await
        .expect("enqueue");
    for _ in 0..5 {
        let _ = capture.drain_once().await.expect("drain");
    }

    let line = line_with(capture_id, "Auto-capture final error");
    assert!(
        line.contains("\"reason\":\"summarizer_failed\""),
        "attempts 2-5 failed on a parse, not on the provider: {line}"
    );
    assert!(
        !line.contains("\"errorKind\":\"network\""),
        "the attempt-1 network reason must not name a later parse failure: {line}"
    );
}

// --- host resolution lines -------------------------------------------------

struct FixedProbe {
    alive: bool,
}

impl mem_store::platform::ProcessProbe for FixedProbe {
    fn is_alive(&self, _pid: u32) -> bool {
        self.alive
    }
}

struct NoClock;

impl mem_bridge::host_endpoint::ProcessClock for NoClock {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// Never dialled by the resolution lines.
struct NoTransport;

impl mem_ai::provider::HostTransport for NoTransport {
    fn send(
        &self,
        _request: mem_ai::provider::HostRequest,
    ) -> mem_ai::provider::BoxFuture<
        'static,
        Result<mem_ai::provider::HostResponse, mem_ai::provider::ProviderError>,
    > {
        Box::pin(std::future::ready(Ok(mem_ai::provider::HostResponse {
            status: 404,
            body: String::new(),
        })))
    }
}

fn lazy_over(state: &std::path::Path, alive: bool) -> mem_bridge::host_provider::LazyHostProvider {
    let env: std::collections::BTreeMap<String, String> =
        [("XDG_STATE_HOME".to_string(), state.display().to_string())]
            .into_iter()
            .collect();
    mem_bridge::host_provider::LazyHostProvider::new(mem_bridge::host_provider::HostResolverDeps {
        override_url: None,
        provider_id: Some("opencode-go".to_string()),
        model_id: Some("beam-free".to_string()),
        variant: None,
        env: Arc::new(env) as Arc<dyn mem_core::env::EnvSource + Send + Sync>,
        probe: Arc::new(FixedProbe { alive }),
        clock: Arc::new(NoClock),
        transport: Arc::new(NoTransport),
    })
}

/// R9 — the resolution lines carry the API base AND the host version, so the
/// 2.2.5 mismatch is visible rather than inferred.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_resolution_logs_the_host_version_and_api_base() {
    let _guard = SERIAL.lock().await;
    let _ = sink();

    let dir = TestDir::new("log-resolution");
    std::fs::create_dir_all(dir.path().join("opencode")).expect("state dir");
    std::fs::write(
        dir.path().join("opencode").join("service.json"),
        r#"{"url":"http://127.0.0.1:49374","pid":4242,"password":"s3cret","version":"2.2.5"}"#,
    )
    .expect("service.json");

    // A dead pid: the reason AND the version it belongs to.
    let dead = lazy_over(dir.path(), false);
    assert_eq!(dead.warm(), Some("host_pid_dead"));
    let unavailable = line_with("host_pid_dead", "Auto-capture provider unavailable");
    assert!(
        unavailable.contains("\"reason\":\"host_pid_dead\""),
        "{unavailable}"
    );
    assert!(
        unavailable.contains("\"hostVersion\":\"2.2.5\""),
        "the failing generation is named: {unavailable}"
    );

    // The live host: the resolved base and the version.
    let alive = lazy_over(dir.path(), true);
    assert_eq!(alive.warm(), None);
    let ready = line_with("http://127.0.0.1:49374/api", "Auto-capture provider ready");
    assert!(
        ready.contains("\"baseUrl\":\"http://127.0.0.1:49374/api\""),
        "{ready}"
    );
    assert!(ready.contains("\"hostVersion\":\"2.2.5\""), "{ready}");
    assert!(ready.contains("\"providerId\":\"opencode-go\""), "{ready}");
}
