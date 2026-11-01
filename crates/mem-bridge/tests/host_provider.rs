//! `LazyHostProvider` contract: resolve on use, cache on success, clear on ANY
//! inner error.
//!
//! Every test injects a recording transport, a mutable liveness probe, a fixed
//! start-time clock and a temp state dir. Nothing touches the network, the real
//! process environment or a real `/proc`.

mod common;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::TestDir;
use mem_ai::provider::request::{ChatMessage, CompletionRequest};
use mem_ai::provider::{
    AiProvider, BoxFuture, CompletionStream, HostMethod, HostRequest, HostResponse, HostTransport,
    ProviderError, ProviderErrorKind,
};
use mem_bridge::host_endpoint::ProcessClock;
use mem_bridge::host_provider::{HostResolverDeps, LazyHostProvider};
use mem_core::env::EnvSource;
use mem_episodic::futures::StreamExt as _;
use mem_store::platform::ProcessProbe;
use serde_json::Value;

const HOST_PID: u32 = 4242;

#[derive(Clone, Debug)]
struct RecordedCall {
    method: HostMethod,
    url: String,
    body: Option<Value>,
}

/// Answers the pre-2.2.5 session protocol (a transport double only — the
/// measured 2.2.5 shapes are pinned in `mem-ai`'s provider suite) and records
/// every call's method/url/body.
#[derive(Default)]
struct RecordingTransport {
    calls: Mutex<Vec<RecordedCall>>,
    /// Scripted `POST …/session` answers; empty ⇒ `{"id":"ses_test"}`.
    session_responses: Mutex<VecDeque<HostResponse>>,
}

impl RecordingTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn queue_session(&self, responses: &[HostResponse]) {
        let mut queue = self
            .session_responses
            .lock()
            .expect("session responses lock");
        queue.clear();
        queue.extend(responses.iter().cloned());
    }

    fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn posts(&self) -> Vec<RecordedCall> {
        self.calls()
            .into_iter()
            .filter(|call| call.method == HostMethod::Post)
            .collect()
    }
}

impl HostTransport for RecordingTransport {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        self.calls.lock().expect("calls lock").push(RecordedCall {
            method: request.method,
            url: request.url.clone(),
            body: request.body.clone(),
        });
        let response = match request.method {
            HostMethod::Post if request.url.ends_with("/session") => self
                .session_responses
                .lock()
                .expect("session responses lock")
                .pop_front()
                .unwrap_or(HostResponse {
                    status: 200,
                    body: r#"{"id":"ses_test"}"#.to_owned(),
                }),
            HostMethod::Post if request.url.contains("/message") => HostResponse {
                status: 200,
                body: r#"{"info":{},"parts":[{"type":"text","text":"ok"}]}"#.to_owned(),
            },
            HostMethod::Delete => HostResponse {
                status: 200,
                body: "true".to_owned(),
            },
            _ => HostResponse {
                status: 404,
                body: String::new(),
            },
        };
        Box::pin(std::future::ready(Ok(response)))
    }
}

/// A probe whose answer the test can change mid-flight.
struct MutableProbe {
    alive: Mutex<BTreeSet<u32>>,
}

impl MutableProbe {
    fn new(alive: &[u32]) -> Arc<Self> {
        Arc::new(Self {
            alive: Mutex::new(alive.iter().copied().collect()),
        })
    }

    fn kill(&self, pid: u32) {
        self.alive.lock().expect("probe lock").remove(&pid);
    }

    fn revive(&self, pid: u32) {
        self.alive.lock().expect("probe lock").insert(pid);
    }
}

impl ProcessProbe for MutableProbe {
    fn is_alive(&self, pid: u32) -> bool {
        self.alive.lock().expect("probe lock").contains(&pid)
    }
}

#[derive(Default)]
struct FixedClock {
    starts: BTreeMap<u32, u64>,
}

impl ProcessClock for FixedClock {
    fn start_time(&self, pid: u32) -> Option<u64> {
        self.starts.get(&pid).copied()
    }
}

fn service_json(port: u16, pid: u32, password: &str, version: &str) -> String {
    format!(
        r#"{{"url":"http://127.0.0.1:{port}","pid":{pid},"password":"{password}","version":"{version}"}}"#
    )
}

fn write_service(state: &Path, body: &str) {
    let dir = state.join("opencode");
    std::fs::create_dir_all(&dir).expect("create the opencode state dir");
    std::fs::write(dir.join("service.json"), body).expect("write service.json");
}

fn deps(
    state: &Path,
    transport: Arc<RecordingTransport>,
    probe: Arc<MutableProbe>,
) -> HostResolverDeps {
    let mut env = BTreeMap::new();
    env.insert("XDG_STATE_HOME".to_string(), state.display().to_string());
    let env: Arc<dyn EnvSource + Send + Sync> = Arc::new(env);
    HostResolverDeps {
        override_url: None,
        provider_id: Some("opencode-go".to_string()),
        model_id: Some("beam-free".to_string()),
        variant: None,
        env,
        probe,
        clock: Arc::new(FixedClock::default()),
        transport,
    }
}

fn capture_request() -> CompletionRequest {
    CompletionRequest::new("ignored-by-the-host-provider")
        .with_system("summarise")
        .with_message(ChatMessage::user("capture this"))
}

async fn drain(stream: CompletionStream) -> Vec<Result<Value, ProviderError>> {
    let mut chunks = Vec::new();
    let mut stream = stream;
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk);
    }
    chunks
}

/// 3a — a failed resolution is never cached: the next call resolves again.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_lazy_provider_retries_resolution_after_a_missing_service_file() {
    let state = TestDir::new("lazy-missing-file");
    let transport = RecordingTransport::new();
    let provider = LazyHostProvider::new(deps(
        state.path(),
        transport.clone(),
        MutableProbe::new(&[]),
    ));

    // (a) No service file: a NAMED Config failure, nothing dialled.
    let chunks = drain(provider.complete(capture_request())).await;
    assert_eq!(chunks.len(), 1, "{chunks:?}");
    let error = chunks[0].as_ref().expect_err("resolution fails closed");
    assert_eq!(error.kind, ProviderErrorKind::Config);
    assert_eq!(error.message, "service_file_missing");
    assert!(
        transport.calls().is_empty(),
        "a resolution failure never dials: {:?}",
        transport.calls()
    );

    // (b) The host starts: the next call resolves and reaches it.
    write_service(
        state.path(),
        &service_json(49374, HOST_PID, "s3cret", "2.2.5"),
    );
    let probe = MutableProbe::new(&[HOST_PID]);
    let provider = LazyHostProvider::new(deps(state.path(), transport.clone(), probe));
    let chunks = drain(provider.complete(capture_request())).await;
    assert!(
        chunks.iter().all(Result::is_ok),
        "the retried resolution succeeds: {chunks:?}"
    );

    // The POSTs, in order. (The trailing DELETE is `run()`'s cleanup and is not
    // part of this comparison.)
    let posts = transport.posts();
    let urls: Vec<&str> = posts.iter().map(|call| call.url.as_str()).collect();
    assert_eq!(
        urls,
        vec![
            "http://127.0.0.1:49374/api/session",
            "http://127.0.0.1:49374/api/session/ses_test/message",
        ]
    );

    // The model comes from the WIRING (the provider ignores
    // `CompletionRequest.model`), and it lands in the PROMPT body.
    let model = posts[1].body.as_ref().expect("prompt body")["model"].clone();
    assert_eq!(model["modelID"], "beam-free");
    assert_eq!(model["providerID"], "opencode-go");
}

/// 3b — a successful exchange caches the origin; a later dead pid is irrelevant
/// until an inner error clears it.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_cached_origin_survives_a_successful_exchange() {
    let state = TestDir::new("lazy-cached-origin");
    write_service(
        state.path(),
        &service_json(49374, HOST_PID, "s3cret", "2.2.5"),
    );
    let transport = RecordingTransport::new();
    let probe = MutableProbe::new(&[HOST_PID]);
    let provider = LazyHostProvider::new(deps(state.path(), transport.clone(), probe.clone()));

    let first = drain(provider.complete(capture_request())).await;
    assert!(first.iter().all(Result::is_ok), "{first:?}");

    // The host dies with no transport error in between: the cache survives.
    probe.kill(HOST_PID);
    let second = drain(provider.complete(capture_request())).await;
    assert!(
        second.iter().all(Result::is_ok),
        "the cached origin is used without re-resolution: {second:?}"
    );

    let creates: Vec<RecordedCall> = transport
        .posts()
        .into_iter()
        .filter(|call| call.url.ends_with("/session"))
        .collect();
    assert_eq!(creates.len(), 2, "two captures, two creates: {creates:?}");
    assert!(
        creates
            .iter()
            .all(|call| call.url == "http://127.0.0.1:49374/api/session"),
        "both creates use the cached origin: {creates:?}"
    );
}

/// 3c — `warm()` reports the reason and caches nothing.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_warm_reports_the_reason_without_caching_a_failure() {
    let state = TestDir::new("lazy-warm");
    write_service(
        state.path(),
        &service_json(49374, HOST_PID, "s3cret", "2.2.5"),
    );
    let transport = RecordingTransport::new();
    let probe = MutableProbe::new(&[]);
    let provider = LazyHostProvider::new(deps(state.path(), transport.clone(), probe.clone()));

    assert_eq!(provider.warm(), Some("host_pid_dead"));
    let chunks = drain(provider.complete(capture_request())).await;
    assert_eq!(
        chunks[0].as_ref().expect_err("still dead").message,
        "host_pid_dead"
    );

    // The host comes back: the startup failure was not cached.
    probe.revive(HOST_PID);
    let chunks = drain(provider.complete(capture_request())).await;
    assert!(
        chunks.iter().all(Result::is_ok),
        "a fresh resolution succeeds after the failure: {chunks:?}"
    );
    assert_eq!(provider.warm(), None);
}

/// 3d — `Auth`, `HttpStatus` and `Protocol` all clear the cache, so a moved
/// origin is picked up on the next attempt.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_any_inner_error_clears_the_cached_origin() {
    let state = TestDir::new("lazy-clear-on-error");
    write_service(
        state.path(),
        &service_json(5001, HOST_PID, "s3cret", "2.2.5"),
    );
    let transport = RecordingTransport::new();
    let probe = MutableProbe::new(&[HOST_PID]);
    let provider = LazyHostProvider::new(deps(state.path(), transport.clone(), probe));

    transport.queue_session(&[
        HostResponse {
            status: 401,
            body: String::new(),
        },
        HostResponse {
            status: 404,
            body: String::new(),
        },
        HostResponse {
            status: 200,
            body: r#"{"oops":true}"#.to_owned(),
        },
    ]);

    let auth = drain(provider.complete(capture_request())).await;
    assert_eq!(
        auth[0].as_ref().expect_err("401").kind,
        ProviderErrorKind::Auth
    );

    let status = drain(provider.complete(capture_request())).await;
    let status = status[0].as_ref().expect_err("404");
    assert_eq!(status.kind, ProviderErrorKind::HttpStatus);
    assert_eq!(status.status, Some(404));

    let protocol = drain(provider.complete(capture_request())).await;
    assert_eq!(
        protocol[0].as_ref().expect_err("junk 200").kind,
        ProviderErrorKind::Protocol
    );

    // The host moved to B: the next attempt re-resolves and reaches it.
    write_service(
        state.path(),
        &service_json(5002, HOST_PID, "s3cret", "2.2.5"),
    );
    let chunks = drain(provider.complete(capture_request())).await;
    assert!(
        chunks.iter().all(Result::is_ok),
        "the moved origin resolves: {chunks:?}"
    );
    let creates: Vec<RecordedCall> = transport
        .posts()
        .into_iter()
        .filter(|call| call.url.ends_with("/session"))
        .collect();
    assert_eq!(creates.len(), 4, "{creates:?}");
    assert_eq!(creates[3].url, "http://127.0.0.1:5002/api/session");
}

/// R22 — the engine's provider factory honours `opencodeBaseUrl`, and the
/// transport seam is injectable: the override row is falsifiable only because
/// the recording mock sees the dialled URL.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_engine_host_provider_honours_the_opencode_base_url_override() {
    let state = TestDir::new("engine-host-override-state");
    let home = TestDir::new("engine-host-override-home");

    let env_map: BTreeMap<String, String> =
        [("HOME".to_string(), home.path().display().to_string())]
            .into_iter()
            .collect();
    let mut config = mem_core::config::Config::load(
        &mem_core::config::ConfigPaths::standard(home.path()),
        &env_map,
    )
    .expect("the default config loads");
    config.opencode_base_url = Some("http://127.0.0.1:5099".to_string());
    config.opencode_provider = Some("opencode-go".to_string());
    config.opencode_model = Some("beam-free".to_string());

    let transport = RecordingTransport::new();
    let host_env: Arc<dyn EnvSource + Send + Sync> = Arc::new({
        let mut env = BTreeMap::new();
        // An EMPTY state dir: branch A resolves without credentials.
        env.insert(
            "XDG_STATE_HOME".to_string(),
            state.path().display().to_string(),
        );
        env
    });
    let provider = mem_bridge::engine::engine_host_provider(
        &config,
        host_env,
        MutableProbe::new(&[]),
        Arc::new(FixedClock::default()),
        transport.clone(),
    );

    let chunks = drain(provider.complete(capture_request())).await;
    assert!(chunks.iter().all(Result::is_ok), "{chunks:?}");
    let creates: Vec<RecordedCall> = transport
        .posts()
        .into_iter()
        .filter(|call| call.url.ends_with("/session"))
        .collect();
    assert_eq!(creates.len(), 1, "{creates:?}");
    assert_eq!(creates[0].url, "http://127.0.0.1:5099/api/session");
}
