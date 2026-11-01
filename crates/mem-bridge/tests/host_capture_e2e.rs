//! The DOCUMENTED wire, end to end, against the real lazy provider.
//!
//! Every shape here is READ from the OpenCode HTTP API description
//! (`docs/evidence/issue-88-host-wire-contract-20261101/`): ONE stateless
//! `POST /api/experimental/generate/text` with `{"prompt": …}` (plus `model`)
//! replying `{"data":{"text": …}}`. **No exchange with a running host has ever
//! been captured**, so the bodies below are schema-derived samples, and the
//! harness's answers are this file's own — not host answers.
//!
//! What these rows therefore prove is harness behaviour plus wire-independent
//! facts: the call count, the URL path, the declared body keys, and the absence
//! of any `session/…` template. They do not prove the live host behaves so.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_ai::provider::{
    BoxFuture, HostMethod, HostRequest, HostResponse, HostTransport, ProviderError,
};
use mem_bridge::capture::{CaptureParams, CaptureService};
use mem_bridge::capture_prompt::{CaptureSummarizer, ProviderCaptureSummarizer};
use mem_bridge::capture_provider::HostStructured;
use mem_bridge::host_endpoint::ProcessClock;
use mem_bridge::host_provider::{HostResolverDeps, LazyHostProvider};
use mem_core::env::EnvSource;
use mem_core::logging::LogSink;
use mem_episodic::store::EpisodicStore;
use mem_store::platform::ProcessProbe;
use serde_json::Value;

/// The sample generate reply: the documented `{"data":{"text": …}}` envelope,
/// carrying the capture JSON as TEXT.
const SAMPLED_2_2_5_GENERATE_BODY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/evidence/issue-88-host-wire-contract-20261101/generate-body.sample.json"
));

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

/// Answers the sampled wire and records every call, body included.
///
/// One `SampledHost` per row: `calls` is the row's own, so no row can observe
/// another's traffic (each row also owns a distinct capture id).
struct SampledHost {
    /// `None` ⇒ the generate route answers `404`, leaving the wire assertion to
    /// the row that pins it.
    reply_body: Option<&'static str>,
    calls: Mutex<Vec<(HostMethod, String, Option<Value>)>>,
}

impl SampledHost {
    fn new() -> Self {
        Self {
            reply_body: None,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn with_reply(mut self, reply_body: &'static str) -> Self {
        self.reply_body = Some(reply_body);
        self
    }

    fn calls(&self) -> Vec<(HostMethod, String, Option<Value>)> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl HostTransport for SampledHost {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        self.calls.lock().expect("calls lock").push((
            request.method,
            request.url.clone(),
            request.body.clone(),
        ));
        let response = if request.method == HostMethod::Post
            && request.url.ends_with("/experimental/generate/text")
        {
            match self.reply_body {
                Some(reply_body) => HostResponse {
                    status: 200,
                    body: reply_body.to_owned(),
                },
                None => HostResponse {
                    status: 404,
                    body: String::new(),
                },
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
fn lazy_provider(state: &Path, host: Arc<SampledHost>) -> Arc<LazyHostProvider> {
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
        transport: host,
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

/// One capture over the sampled generate shape, through the real summarizer.
///
/// `drains` attempts, so the caller controls whether the recorded sequence is
/// one attempt's (1) or the whole retry ladder's (5, the dead-letter bound).
/// `capture_id` must be distinct per row — the log sink is a process-wide
/// `OnceLock` shared by every row in this binary, so every assertion below is
/// keyed to its own id and nothing asserts an ABSENCE over that sink.
async fn run_capture(
    summarizer: Arc<dyn CaptureSummarizer>,
    capture_id: &str,
    session_id: &str,
    store: &TestDir,
    drains: u32,
) -> mem_bridge::capture::DrainReport {
    let capture = service(store, summarizer);
    let params = CaptureParams {
        session_id: session_id.to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/host".to_string(),
        agent_id: "agent-1".to_string(),
        transcript: Some("user: fix the retry storm in the login handler".to_string()),
        ..Default::default()
    };
    capture.enqueue(&params).await.expect("enqueue");
    let mut report = mem_bridge::capture::DrainReport::default();
    for _ in 0..drains.max(1) {
        report = capture.drain_once().await.expect("drain");
    }
    report
}

/// Row 17 — AC2 on the capture lane: ONE stateless call to the documented route
/// and its body is exactly `prompt` + `model`.
///
/// The route is pinned HERE rather than by the harness dispatcher, so keeping a
/// session path, or leaving `agent`/`format`/`tools` in the body, reds this row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sampled_2_2_5_prompt_posts_to_generate_with_only_a_prompt() {
    let _ = sink();

    let state = TestDir::new("host-capture-e2e-prompt-state");
    write_service(state.path());
    let host = Arc::new(SampledHost::new());
    let summarizer: Arc<dyn CaptureSummarizer> = Arc::new(ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(
            state.path(),
            host.clone(),
        ))),
        "beam-free",
        131_072,
    ));
    let dir = TestDir::new("host-capture-e2e-prompt-store");
    let _ = run_capture(
        summarizer,
        "019a0000-0000-7f00-8000-00000000c004",
        "sess-prompt-route",
        &dir,
        1,
    )
    .await;

    let calls = host.calls();
    assert_eq!(calls.len(), 1, "exactly one stateless call: {calls:?}");
    assert_eq!(
        calls[0].1, "http://127.0.0.1:49374/api/experimental/generate/text",
        "the call takes the documented route: {calls:?}"
    );
    let body = calls[0].2.as_ref().expect("the call carries a JSON body");
    let mut keys: Vec<&String> = body
        .as_object()
        .expect("the body is an object")
        .keys()
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["model", "prompt"], "declared keys only: {body}");
}

/// Row 18 — ACL3's next observable step: over the sampled wire a capture
/// PERSISTS.
///
/// The reply's text is the capture JSON, so `parse_structured_lenient` takes it
/// directly and `parse_capture_value` turns it into a capture. A reply parser
/// left on the pre-2.2.5 `info.structured_output` envelope, or a body that is
/// not `prompt` + `model`, reds this row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sampled_2_2_5_wire_persists_a_capture() {
    let _ = sink();

    let state = TestDir::new("host-capture-e2e-persist-state");
    write_service(state.path());
    let host = Arc::new(SampledHost::new().with_reply(SAMPLED_2_2_5_GENERATE_BODY));
    let summarizer: Arc<dyn CaptureSummarizer> = Arc::new(ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(
            state.path(),
            host.clone(),
        ))),
        "beam-free",
        131_072,
    ));
    let dir = TestDir::new("host-capture-e2e-persist-store");
    let capture_id = "019a0000-0000-7f00-8000-00000000c002";
    let report = run_capture(summarizer, capture_id, "sess-persist", &dir, 1).await;

    assert_eq!(report.done, 1, "the capture persisted: {report:?}");
    assert_eq!(report.dead_lettered, 0, "no dead letter: {report:?}");

    let content = sink_text();
    let persisted = content
        .lines()
        .rfind(|line| line.contains(capture_id) && line.contains("Auto-capture memory persisted"))
        .unwrap_or_default();
    assert!(
        persisted.contains("\"memoryType\":\"bugFix\""),
        "the persisted line names this capture and its type: {persisted}"
    );

    // The prompt body carried the schema by now — that assertion lives in
    // `host_capture_provider.rs`; here the wire facts are the route and the
    // one-call sequence.
    let calls = host.calls();
    assert_eq!(calls.len(), 1, "exactly one stateless call: {calls:?}");
    assert_eq!(
        calls[0].1, "http://127.0.0.1:49374/api/experimental/generate/text",
        "the call reaches the documented route"
    );
}

/// Row 19 — AC6: a shape mismatch is NAMED on the terminal line.
///
/// The generate answer carries `{"data":{}}` (no `text`), so every attempt fails
/// with `WireShape`; the ladder dead-letters at its fifth attempt. The line must name
/// the reason and the kind, and must not be flattened to `summarizer_failed` —
/// the token a model-side JSON miss also produces.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_names_a_wire_shape_mismatch() {
    let _ = sink();

    let state = TestDir::new("host-capture-e2e-wire-shape-state");
    write_service(state.path());
    let host = Arc::new(SampledHost::new().with_reply(r#"{"data":{}}"#));
    let summarizer: Arc<dyn CaptureSummarizer> = Arc::new(ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(
            state.path(),
            host.clone(),
        ))),
        "beam-free",
        131_072,
    ));
    let dir = TestDir::new("host-capture-e2e-wire-shape-store");
    let capture_id = "019a0000-0000-7f00-8000-00000000c003";
    let report = run_capture(summarizer, capture_id, "sess-wire-shape", &dir, 5).await;

    assert_eq!(
        report.dead_lettered, 1,
        "the ladder dead-letters: {report:?}"
    );

    let content = sink_text();
    let final_line = content
        .lines()
        .rfind(|line| line.contains(capture_id) && line.contains("Auto-capture final error"))
        .unwrap_or_default();
    assert!(
        final_line.contains("\"reason\":\"host_wire_shape_mismatch\""),
        "the dead-letter names the shape mismatch: {final_line}"
    );
    assert!(
        final_line.contains("\"errorKind\":\"wire-shape\""),
        "the kind travels beside it: {final_line}"
    );
    assert!(
        !final_line.contains("summarizer_failed"),
        "the shape mismatch is never flattened into the default: {final_line}"
    );
}
