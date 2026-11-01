//! The wrapper's own idle-capture payload, consumed by the engine.
//!
//! `plugin/tests/fixtures/idle-capture-payload.json` is regenerated from the
//! live producer by `plugin/tests/capture-payload.test.mjs`
//! (`test_idle_capture_fixture_matches_the_live_payload`); this file feeds that
//! exact byte string through the real `CaptureService` drain. The summarizer
//! always succeeds, so identity validation is the ONLY possible failure and no
//! RED here can be a summarizer artefact.

mod common;

use std::sync::{Arc, OnceLock};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_bridge::capture::{CaptureParams, CaptureService, CaptureSummaryOutput, DrainReport};
use mem_bridge::capture_prompt::CaptureSummarizer;
use mem_core::logging::LogSink;
use mem_core::logging::signatures;
use mem_embed::embedder::BoxFuture;
use mem_episodic::error::EpisodicError;
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::{self, Value};

/// The wrapper's payload, read at compile time: a deleted fixture is a build
/// error, never a silently skipped test.
const WRAPPER_PAYLOAD: &str =
    include_str!("../../../plugin/tests/fixtures/idle-capture-payload.json");

/// Serializes the sink-touching rows (a tokio mutex: never a std guard across
/// an `.await` - the `await_holding_lock` allowance in the gate exists for this).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The process-global sink, installed exactly once per test binary.
fn sink() -> LogSink {
    static ONCE: OnceLock<LogSink> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = common::temp_dir("capture-wrapper-payload");
        let sink = LogSink::new(dir.join("opencode-mem.log"));
        mem_core::logging::init_process_logging(sink.clone())
            .expect("the per-binary sink installs");
        sink
    })
    .clone()
}

struct FixedSummarizer;

impl CaptureSummarizer for FixedSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, EpisodicError>> {
        Box::pin(async {
            Ok(CaptureSummaryOutput::Capture {
                summary: "the recorded summary".to_string(),
                memory_type: "memory".to_string(),
                tags: Vec::new(),
                extension: Default::default(),
            })
        })
    }
}

async fn service(dir: &TestDir) -> (CaptureService, Arc<EpisodicStore>) {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let service = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(Arc::new(FixedSummarizer) as Arc<dyn CaptureSummarizer>),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );
    (service, store)
}

/// The fixture with `key` deleted (the pre-fix producer shape), as `CaptureParams`.
fn payload_without(key: &str) -> CaptureParams {
    let mut value: Value = serde_json::from_str(WRAPPER_PAYLOAD).expect("fixture is JSON");
    value
        .as_object_mut()
        .expect("fixture is an object")
        .remove(key)
        .expect("the fixture carries the key");
    serde_json::from_value(value).expect("params parse")
}

/// No attempt-based backoff exists (the batch gates on `status = 'queued'`
/// alone), so five consecutive drains re-pick the same entry until it dies.
async fn drain_five(service: &CaptureService) -> DrainReport {
    let mut total = DrainReport::default();
    for _ in 0..5 {
        let report = service.drain_once().await.expect("drain");
        total.done += report.done;
        total.retryable += report.retryable;
        total.dead_lettered += report.dead_lettered;
        total.deferred += report.deferred;
    }
    total
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_wrapper_payload_persists_through_the_drain() {
    let dir = TestDir::new("cwp-persist");
    let (service, store) = service(&dir).await;
    let params: CaptureParams = serde_json::from_str(WRAPPER_PAYLOAD).expect("params parse");
    assert_eq!(params.agent_id, "build");

    service.enqueue(&params).await.expect("enqueue");
    let total = drain_five(&service).await;

    assert_eq!(total.done, 1, "the wrapper payload persists: {total:?}");
    assert_eq!(total.dead_lettered, 0, "{total:?}");
    // Keyed by the parsed params, never by re-typed literals.
    let key = (&params.project_id, &params.session_id, &params.agent_id);
    let rows = store.list_sessions(key.0, 20).await.expect("list sessions");
    let row = rows
        .iter()
        .find(|row| &row.session_id == key.1 && &row.agent_id == key.2)
        .expect("the (project, session, agent) row exists");
    assert_eq!(row.agent_id, "build");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_empty_agent_id_still_dead_letters() {
    // Guards the rejected design (engine defaults the field): if a future wave
    // makes the engine default `agent_id`, this fails - which is the point.
    let _serial = SERIAL.lock().await;
    let _ = sink();
    let dir = TestDir::new("cwp-no-agent");
    let (service, _store) = service(&dir).await;
    let params = payload_without("agent_id");
    assert_eq!(params.agent_id, "");

    service.enqueue(&params).await.expect("enqueue");
    let total = drain_five(&service).await;

    assert_eq!(total.dead_lettered, 1, "{total:?}");
    assert_eq!(total.done, 0, "{total:?}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_empty_project_id_still_dead_letters() {
    // Mutant: a future engine default fills `project_id`.
    let _serial = SERIAL.lock().await;
    let _ = sink();
    let dir = TestDir::new("cwp-no-project");
    let (service, _store) = service(&dir).await;
    let params = payload_without("project_id");
    assert_eq!(params.project_id, "");

    service.enqueue(&params).await.expect("enqueue");
    let total = drain_five(&service).await;

    assert_eq!(total.dead_lettered, 1, "{total:?}");
    assert_eq!(total.done, 0, "{total:?}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_episodic_write_failure_names_the_field() {
    let _serial = SERIAL.lock().await;
    let sink = sink();
    let dir = TestDir::new("cwp-log");
    let (service, _store) = service(&dir).await;
    let mut params: CaptureParams = serde_json::from_str(WRAPPER_PAYLOAD).expect("params parse");
    // A row-scoped id, so the sibling pin (same fixture id, five dead-letters)
    // can never be counted here; and the pre-fix producer shape.
    params.capture_id = "cap-log-0001".to_owned();
    params.agent_id = String::new();

    service.enqueue(&params).await.expect("enqueue");
    service.drain_once().await.expect("drain");

    let text = std::fs::read_to_string(sink.path()).expect("the sink file exists");
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("\"captureId\":\"cap-log-0001\""))
        .filter(|line| line.contains(signatures::AUTO_CAPTURE_EPISODIC_FAILED))
        .collect();
    assert_eq!(lines.len(), 1, "one attempt = one line: {lines:?}");
    let line = lines[0];
    for needle in [
        "\"field\":\"agent_id\"",
        "\"errorKind\":\"validation\"",
        "\"agentId\":\"\"",
        "\"projectId\":\"/work/proj\"",
        "\"sessionId\":\"ses-A\"",
    ] {
        assert!(line.contains(needle), "{needle} missing from {line}");
    }
}
