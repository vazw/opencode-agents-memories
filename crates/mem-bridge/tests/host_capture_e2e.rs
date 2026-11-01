//! End-to-end: the LIVE 2.2.5 controller shape against the real lazy provider.
//!
//! The host is 2.2.5 and answers `POST /api/session` with
//! `{"data": Session.Info}`. The port's transport looks for a top-level `id`,
//! so the capture CANNOT persist — and that is this wave's acceptance: it must
//! fail with a NAME (`summarizer_failed` + `errorKind: "protocol"`), never as an
//! anonymous dead-letter. Restoring capture is the wire wave's job.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_ai::provider::{BoxFuture, HostRequest, HostResponse, HostTransport, ProviderError};
use mem_bridge::capture::{CaptureParams, CaptureService};
use mem_bridge::capture_prompt::{CaptureSummarizer, ProviderCaptureSummarizer};
use mem_bridge::capture_provider::HostStructured;
use mem_bridge::host_endpoint::ProcessClock;
use mem_bridge::host_provider::{HostResolverDeps, LazyHostProvider};
use mem_core::env::EnvSource;
use mem_core::logging::LogSink;
use mem_episodic::store::EpisodicStore;
use mem_store::platform::ProcessProbe;

/// The body copied from `.tmp/openapi.json` (`/api/session.post` →
/// `{data: Session.Info}`; required: id, projectID, cost, tokens, time,
/// location).
const MEASURED_2_2_5_CREATE_BODY: &str = r#"{"data":{"id":"ses_2_2_5","projectID":"prj_1","cost":0,"tokens":{"input":10,"output":20,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1,"updated":2},"location":{"directory":"/tmp"}}}"#;

fn sink() -> LogSink {
    static ONCE: OnceLock<LogSink> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = common::temp_dir("host-capture-e2e-log");
        let sink = LogSink::new(dir.join("opencode-mem.log"));
        mem_core::logging::init_process_logging(sink.clone())
            .expect("the per-binary sink installs");
        sink
    })
    .clone()
}

fn sink_text() -> String {
    std::fs::read_to_string(sink().path()).expect("the per-binary sink file exists")
}

struct HostProbe;

impl ProcessProbe for HostProbe {
    fn is_alive(&self, _pid: u32) -> bool {
        true
    }
}

struct NoClock;

impl ProcessClock for NoClock {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// Answers the measured 2.2.5 create response and nothing else.
struct MeasuredHost;

impl HostTransport for MeasuredHost {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        let response = if request.method == mem_ai::provider::HostMethod::Post
            && request.url.ends_with("/session")
        {
            HostResponse {
                status: 200,
                body: MEASURED_2_2_5_CREATE_BODY.to_owned(),
            }
        } else {
            HostResponse {
                status: 404,
                body: String::new(),
            }
        };
        Box::pin(std::future::ready(Ok(response)))
    }
}

fn write_service(state: &Path) {
    let dir = state.join("opencode");
    std::fs::create_dir_all(&dir).expect("create the opencode state dir");
    std::fs::write(
        dir.join("service.json"),
        r#"{"id":"742f10c4","url":"http://127.0.0.1:49374","pid":4242,"password":"s3cret","version":"2.2.5"}"#,
    )
    .expect("write service.json");
}

/// The lazy host, as the CONCRETE handle: the capture seam wraps it in
/// `HostStructured` (ADR-0029), so no `AiProvider` is involved on this path.
fn lazy_provider(state: &Path) -> Arc<LazyHostProvider> {
    let env: BTreeMap<String, String> =
        [("XDG_STATE_HOME".to_string(), state.display().to_string())]
            .into_iter()
            .collect();
    Arc::new(LazyHostProvider::new(HostResolverDeps {
        override_url: None,
        provider_id: Some("opencode-go".to_string()),
        model_id: Some("beam-free".to_string()),
        variant: None,
        env: Arc::new(env) as Arc<dyn EnvSource + Send + Sync>,
        probe: Arc::new(HostProbe),
        clock: Arc::new(NoClock),
        transport: Arc::new(MeasuredHost),
    }))
}

fn service(dir: &TestDir, summarizer: Arc<dyn CaptureSummarizer>) -> CaptureService {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(summarizer),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_against_the_measured_2_2_5_body_dead_letters_with_a_named_reason() {
    let _ = sink();

    let state = TestDir::new("host-capture-e2e-state");
    write_service(state.path());

    let summarizer: Arc<dyn CaptureSummarizer> = Arc::new(ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(state.path()))),
        "beam-free",
        131_072,
    ));
    let dir = TestDir::new("host-capture-e2e-store");
    let capture = service(&dir, summarizer);

    let capture_id = "019a0000-0000-7f00-8000-00000000c001";
    let params = CaptureParams {
        session_id: "sess-2-2-5".to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/host".to_string(),
        agent_id: "agent-1".to_string(),
        transcript: Some("user: fix the retry storm in the login handler".to_string()),
        ..Default::default()
    };
    capture.enqueue(&params).await.expect("enqueue");
    for _ in 0..5 {
        let report = capture.drain_once().await.expect("drain");
        assert_eq!(report.retryable + report.dead_lettered, 1, "{report:?}");
    }

    let content = sink_text();
    let final_line = content
        .lines()
        .rfind(|line| line.contains(capture_id) && line.contains("Auto-capture final error"))
        .unwrap_or_default();
    assert!(
        final_line.contains("\"reason\":\"summarizer_failed\""),
        "the dead-letter carries a reason: {final_line}"
    );
    assert!(
        final_line.contains("\"errorKind\":\"protocol\""),
        "the 2.2.5 response-shape mismatch is named `protocol`: {final_line}"
    );

    let failed = content
        .lines()
        .rfind(|line| line.contains(capture_id) && line.contains("Auto-capture provider failed"))
        .unwrap_or_default();
    assert!(
        failed.contains("\"errorKind\":\"protocol\""),
        "each summarizer failure is named too: {failed}"
    );

    assert!(
        !content.contains("Auto-capture memory persisted"),
        "capture CANNOT persist against 2.2.5 — that is the wire wave's job"
    );
}
