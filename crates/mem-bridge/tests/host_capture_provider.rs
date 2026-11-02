//! The host arm's STRUCTURED path (ADR-0029), pinned over a fake host.
//!
//! The fake answers the **documented** wire: one stateless
//! `POST /api/experimental/generate/text` replying `{"data": {"text": …}}`. That
//! shape is READ from `docs/evidence/issue-88-host-wire-contract-20261101/` — a
//! schema sample, never a transcript. The rows evidence the WIRING, not the
//! owner's live host.
//!
//! Every row is hermetic: `service.json` lives under a temp `XDG_STATE_HOME`,
//! the liveness probe is a double, and every HTTP call goes through the injected
//! `HostTransport`.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_ai::provider::{
    BoxFuture, HostMethod, HostRequest, HostResponse, HostTransport, ProviderError,
    ProviderErrorKind,
};
use mem_bridge::capture::{CaptureParams, CaptureService};
use mem_bridge::capture_context::{SUMMARY_OUTPUT_RESERVE_BYTES, SUMMARY_REQUEST_OVERHEAD_BYTES};
use mem_bridge::capture_prompt::{CaptureSummarizer, ProviderCaptureSummarizer};
use mem_bridge::capture_provider::{HostStructured, StructuredCapture};
use mem_bridge::host_endpoint::ProcessClock;
use mem_bridge::host_provider::{HostResolverDeps, LazyHostProvider};
use mem_core::env::EnvSource;
use mem_episodic::store::EpisodicStore;
use mem_store::platform::ProcessProbe;
use mem_store::serde_json::Value;

/// The production capture budget (upstream `DEFAULT_AUTO_CAPTURE_MAX_CONTEXT_BYTES`).
const TOTAL: usize = 131_072;

/// The structured reply the fake host gives: the documented
/// `{"data":{"text": …}}` envelope, carrying a valid capture object as TEXT.
const STRUCTURED_REPLY: &str =
    r#"{"data":{"text":"{\"summary\":\"s\",\"type\":\"task\",\"tags\":[]}"}}"#;

struct AliveProbe;

impl ProcessProbe for AliveProbe {
    fn is_alive(&self, _pid: u32) -> bool {
        true
    }
}

struct DeadProbe;

impl ProcessProbe for DeadProbe {
    fn is_alive(&self, _pid: u32) -> bool {
        false
    }
}

struct NoClock;

impl ProcessClock for NoClock {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// Routes the stateless generate route.
///
/// `refusing_origin` marks the ONE origin that answers `401` on its first
/// generate call (the recycled-port stranger the cache must not keep).
struct FakeHost {
    calls: Mutex<Vec<HostRequest>>,
    generates: AtomicUsize,
    refusing_origin: Option<String>,
    refused: AtomicUsize,
}

impl FakeHost {
    fn new(refusing_origin: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            generates: AtomicUsize::new(0),
            refusing_origin: refusing_origin.map(str::to_owned),
            refused: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> Vec<HostRequest> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn generates(&self) -> usize {
        self.generates.load(Ordering::SeqCst)
    }
}

impl HostTransport for FakeHost {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        self.calls.lock().expect("calls lock").push(request.clone());
        let refusing = self
            .refusing_origin
            .as_deref()
            .is_some_and(|origin| request.url.starts_with(origin));
        let response = match request.method {
            HostMethod::Post if request.url.ends_with("/experimental/generate/text") => {
                self.generates.fetch_add(1, Ordering::SeqCst);
                if refusing && self.refused.fetch_add(1, Ordering::SeqCst) == 0 {
                    HostResponse {
                        status: 401,
                        body: String::new(),
                    }
                } else {
                    HostResponse {
                        status: 200,
                        body: STRUCTURED_REPLY.to_owned(),
                    }
                }
            }
            _ => HostResponse {
                status: 404,
                body: String::new(),
            },
        };
        Box::pin(std::future::ready(Ok(response)))
    }
}

fn write_service(state: &Path, port: u16) {
    let dir = state.join("opencode");
    std::fs::create_dir_all(&dir).expect("create the opencode state dir");
    std::fs::write(
        dir.join("service.json"),
        format!(
            r#"{{"url":"http://127.0.0.1:{port}","pid":4242,"password":"s3cret","version":"2.2.5"}}"#
        ),
    )
    .expect("write service.json");
}

fn lazy_provider(
    state: &Path,
    transport: Arc<dyn HostTransport>,
    probe: Arc<dyn ProcessProbe>,
) -> Arc<LazyHostProvider> {
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
        probe,
        clock: Arc::new(NoClock),
        transport,
    }))
}

fn capture_schema() -> Value {
    mem_store::serde_json::from_str(mem_bridge::capture_prompt::CAPTURE_OUTPUT_SCHEMA)
        .expect("the frozen capture schema parses")
}

fn prompt_request() -> mem_ai::provider::CompletionRequest {
    mem_ai::provider::CompletionRequest::new("beam-free")
        .with_system(mem_bridge::capture_prompt::CAPTURE_SYSTEM_PROMPT)
        .with_message(mem_ai::provider::ChatMessage::user("user: capture this"))
}

/// A resolution failure must arrive as a `Config` error NAMING the resolver's
/// reason — flattened to `summarizer_failed`/`Network`, the operator loses the
/// only signal that says "your service.json pid is dead".
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_host_structured_maps_a_resolve_failure_to_a_config_error() {
    let state = TestDir::new("host-capture-provider-dead-pid");
    write_service(state.path(), 4021);
    let transport = FakeHost::new(None);
    let structured = HostStructured::new(lazy_provider(
        state.path(),
        transport.clone(),
        Arc::new(DeadProbe),
    ));

    let error = structured
        .complete_structured(prompt_request(), capture_schema())
        .await
        .expect_err("a dead host pid cannot serve a capture");

    assert_eq!(error.kind, ProviderErrorKind::Config);
    assert_eq!(error.message, "host_pid_dead");
    assert_eq!(
        transport.calls().len(),
        0,
        "nothing is dialled when resolution fails"
    );

    // The summarizer carries the same reason onto the log line.
    let summarizer = ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(
            state.path(),
            FakeHost::new(None),
            Arc::new(DeadProbe),
        ))),
        "beam-free",
        131_072,
    );
    let _ = summarizer
        .summarize("user: capture this")
        .await
        .expect_err("a dead host pid cannot serve a capture");
    let failure = summarizer.last_failure().expect("the failure is recorded");
    assert_eq!(failure.reason, "host_pid_dead");
    assert_eq!(failure.error_kind, "config");
}

/// A `401` on the cached origin must drop that origin: the next use re-reads
/// `service.json`, so a host restart (or a stranger on a recycled port) heals
/// without an engine restart.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_host_structured_invalidates_the_cache_on_any_error() {
    let state = TestDir::new("host-capture-provider-recycle");
    write_service(state.path(), 4031);
    let transport = FakeHost::new(Some("http://127.0.0.1:4031"));
    let structured = HostStructured::new(lazy_provider(
        state.path(),
        transport.clone(),
        Arc::new(AliveProbe),
    ));

    let error = structured
        .complete_structured(prompt_request(), capture_schema())
        .await
        .expect_err("the first host reply is a 401");
    assert_eq!(error.status, Some(401), "the first attempt is refused");

    // The host moves (a restart writes a new origin). Without invalidation the
    // cached provider keeps dialling 4031 and the second call fails too.
    write_service(state.path(), 4032);
    structured
        .complete_structured(prompt_request(), capture_schema())
        .await
        .expect("the re-resolved origin answers");

    assert_eq!(transport.generates(), 2, "one generate call per attempt");
    let generates: Vec<String> = transport
        .calls()
        .into_iter()
        .filter(|call| {
            call.method == HostMethod::Post && call.url.ends_with("/experimental/generate/text")
        })
        .map(|call| call.url)
        .collect();
    assert_eq!(generates.len(), 2, "{generates:?}");
    assert!(
        generates[0].contains(":4031"),
        "the first attempt dialled the stale origin: {generates:?}"
    );
    assert!(
        generates[1].contains(":4032"),
        "the second attempt re-resolved the host: {generates:?}"
    );
}

/// The acceptance row for the host arm: a structured host reply persists.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_host_capture_persists_against_a_structured_host() {
    let state = TestDir::new("host-capture-provider-persist-state");
    write_service(state.path(), 4041);
    let transport = FakeHost::new(None);
    let summarizer: Arc<dyn CaptureSummarizer> = Arc::new(ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(
            state.path(),
            transport.clone(),
            Arc::new(AliveProbe),
        ))),
        "beam-free",
        131_072,
    ));
    let dir = TestDir::new("host-capture-provider-persist-store");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    let capture = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(summarizer),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );

    let params = CaptureParams {
        session_id: "sess-structured".to_string(),
        capture_id: "019a0000-0000-7f00-8000-00000000c0aa".to_string(),
        project_id: "/srv/structured".to_string(),
        agent_id: "agent-1".to_string(),
        transcript: Some("user: capture this".to_string()),
        ..Default::default()
    };
    capture.enqueue(&params).await.expect("enqueue");

    let report = capture.drain_once().await.expect("drain");

    assert_eq!(report.done, 1, "the capture persisted: {report:?}");
    assert_eq!(report.dead_lettered, 0, "no dead letter: {report:?}");
}

/// The schema must reach the WIRE, not just the `complete_structured` call:
/// accepted and then dropped in the prompt body is the same dead-letter.
///
/// The stateless generate route has no `format` field, so the schema travels
/// in the prompt TEXT, after the answer instruction.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_host_prompt_body_carries_the_schema() {
    let state = TestDir::new("host-capture-provider-schema-state");
    write_service(state.path(), 4051);
    let transport = FakeHost::new(None);
    let summarizer = ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(
            state.path(),
            transport.clone(),
            Arc::new(AliveProbe),
        ))),
        "beam-free",
        131_072,
    );

    summarizer
        .summarize("user: capture this")
        .await
        .expect("the fake host answers a capture object");

    let prompt = transport
        .calls()
        .into_iter()
        .find(|call| {
            call.method == HostMethod::Post && call.url.ends_with("/experimental/generate/text")
        })
        .expect("the prompt is sent");
    let body = prompt.body.expect("the prompt carries a JSON body");
    let mut keys: Vec<&String> = body
        .as_object()
        .expect("the prompt body is an object")
        .keys()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["model", "prompt"],
        "the route declares exactly these keys: {body}"
    );

    let text = body["prompt"]
        .as_str()
        .expect("the prompt text is a string");
    assert!(
        text.contains(r#""required": ["summary", "type", "tags"]"#),
        "the capture schema's required list is in the prompt text"
    );
    assert!(
        text.contains(mem_bridge::capture_context::SCHEMA_ANSWER_INSTRUCTION),
        "the schema is presented as an answer contract, not a trailing blob"
    );
}

/// Issue 111 — the budget the builder promises is the budget the HOST receives.
///
/// `build_bounded_summary_prompt` charges `system + prompt + output_reserve +
/// overhead <= total`, but the route folds `system` and the user text with a
/// separator of its own (`prompt_text`). The assertion is made on the recorded
/// generate body, i.e. the assembled request, not on the context slice.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_issue_111_the_assembled_host_prompt_fits_the_capture_budget() {
    let state = TestDir::new("issue-111-assembled-prompt-budget");
    write_service(state.path(), 4021);
    let transport = FakeHost::new(None);
    let summarizer = ProviderCaptureSummarizer::new(
        Arc::new(HostStructured::new(lazy_provider(
            state.path(),
            transport.clone(),
            Arc::new(AliveProbe),
        ))),
        "beam-free",
        TOTAL,
    );
    let context = "A".repeat(200_000);
    summarizer
        .summarize(&context)
        .await
        .expect("the fake host answers a capture");

    let calls = transport.calls();
    assert_eq!(calls.len(), 1, "one stateless generate call");
    let body = calls[0]
        .body
        .clone()
        .expect("the generate call carries a body");
    let prompt = body["prompt"]
        .as_str()
        .expect("the route takes a prompt string");

    let output_reserve = usize::min(SUMMARY_OUTPUT_RESERVE_BYTES, TOTAL / 8);
    let overhead = SUMMARY_REQUEST_OVERHEAD_BYTES;
    let charged = prompt.len() + output_reserve + overhead;
    assert!(
        charged <= TOTAL,
        "the assembled request fits the budget: {charged} > {TOTAL}"
    );
    let slack = TOTAL - charged;
    assert!(
        slack <= 64,
        "the clamp fills the budget rather than over-reserving: {slack} bytes unused"
    );
}
