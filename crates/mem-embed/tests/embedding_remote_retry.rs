//! Port of upstream `tests/embedding-remote-retry.test.ts` (vendored clone at
//! `example/opencode-mem`, commit 906038e). The first nine tests mirror the
//! upstream `fetchRemoteEmbeddingWithRetry` suite case-for-case, traced by the
//! upstream `it(...)` names:
//!
//! | upstream `it(...)`                                                                   | this file |
//! | ------------------------------------------------------------------------------------- | --- |
//! | "recovers when the first attempt hangs past the per-attempt AbortSignal timeout"       | `test_retry_recovers_after_attempt_timeout` |
//! | "retries on network TypeError and succeeds on a later attempt"                        | `test_retry_recovers_after_network_failure` |
//! | "fails fast on invalid JSON without consuming another attempt"                        | `test_invalid_json_fails_fast_without_another_attempt` |
//! | "passes AbortSignal.timeout to each fetch attempt"                                    | `test_attempt_timeout_is_passed_to_each_transport_call` |
//! | "retries on 429 and succeeds on a later attempt"                                      | `test_retry_recovers_after_429` |
//! | "retries on 500 and succeeds on a later attempt"                                      | `test_retry_recovers_after_500` |
//! | "fails fast on non-retryable 4xx without consuming another attempt"                   | `test_non_retryable_4xx_fails_fast` |
//! | "throws the last error after exhausting all attempts on persistent 5xx"               | `test_persistent_5xx_exhausts_all_attempts_and_returns_last_error` |
//! | "preserves the request shape: base url, bearer auth, prefixed input, model"           | `test_request_shape_carries_url_bearer_auth_input_and_model` |
//!
//! The tenth (`test_errors_never_include_the_api_key`) is the blueprint's
//! justified addition (secret hygiene). Mirror battery options match upstream
//! `RETRY_OPTS`: attempt timeout 50ms, max attempts 3, backoff 5ms.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mem_embed::BoxFuture;
use mem_embed::error::EmbedError;
use mem_embed::remote::{
    RemoteEmbedOptions, RemoteHttpResponse, RemoteRequest, RemoteRetryOptions, RemoteTransport,
    SleepFn,
};

/// Upstream `jsonResponse` payload; exact binary halves so the parsed f32s
/// compare without epsilon.
const OK_BODY: &str = r#"{"data":[{"embedding":[0.25,0.5]}]}"#;
const EMBEDDING: [f32; 2] = [0.25, 0.5];

#[derive(Debug, Clone)]
enum Behavior {
    /// 200 + valid embeddings body.
    Ok,
    /// Non-2xx status with an empty body.
    Status(u16),
    /// Arbitrary status + body (invalid JSON cases).
    Body(u16, &'static str),
    /// Transport-level network failure (`RemoteNetwork`).
    Network,
    /// Never resolves — the hung-connection analogue of the upstream
    /// AbortSignal timeout case.
    Hang,
}

struct FakeTransport {
    behaviors: Vec<Behavior>,
    calls: AtomicUsize,
    requests: Mutex<Vec<RemoteRequest>>,
}

impl FakeTransport {
    fn new(behaviors: Vec<Behavior>) -> Self {
        Self {
            behaviors,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn requests(&self) -> Vec<RemoteRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl RemoteTransport for FakeTransport {
    fn post_json(
        &self,
        request: RemoteRequest,
    ) -> BoxFuture<'_, Result<RemoteHttpResponse, EmbedError>> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        // Past the scripted prefix, repeat the last behavior (persistent cases).
        let behavior = self
            .behaviors
            .get(index)
            .cloned()
            .unwrap_or_else(|| self.behaviors.last().expect("scripted behaviors").clone());
        match behavior {
            Behavior::Ok => Box::pin(async {
                Ok(RemoteHttpResponse {
                    status: 200,
                    body: OK_BODY.to_owned(),
                })
            }),
            Behavior::Status(status) => Box::pin(async move {
                Ok(RemoteHttpResponse {
                    status,
                    body: String::new(),
                })
            }),
            Behavior::Body(status, body) => Box::pin(async move {
                Ok(RemoteHttpResponse {
                    status,
                    body: body.to_owned(),
                })
            }),
            Behavior::Network => Box::pin(async {
                Err(EmbedError::RemoteNetwork {
                    message: "fetch failed".to_owned(),
                })
            }),
            Behavior::Hang => Box::pin(std::future::pending()),
        }
    }
}

fn retry_options() -> RemoteEmbedOptions {
    RemoteEmbedOptions {
        url: "https://fake.local/v1".to_owned(),
        api_key: "test-key".to_owned(),
        model: "test-model".to_owned(),
        retry: RemoteRetryOptions {
            attempt_timeout: Duration::from_millis(50),
            max_attempts: 3,
            backoff: Duration::from_millis(5),
        },
    }
}

/// Recording sleep: captures every requested backoff and resolves immediately,
/// so retry bookkeeping is asserted without real waiting.
fn recording_sleep() -> (SleepFn, Arc<Mutex<Vec<Duration>>>) {
    let recorded: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&recorded);
    let sleep: SleepFn = Arc::new(move |duration| -> BoxFuture<'static, ()> {
        sink.lock().unwrap().push(duration);
        Box::pin(std::future::ready(()))
    });
    (sleep, recorded)
}

#[tokio::test]
async fn test_retry_recovers_after_attempt_timeout() {
    // Upstream: attempt 1 hangs past the 50ms per-attempt timeout; attempt 2
    // succeeds. The one bounded real wait (~50ms) proves the timeout fires.
    let transport = FakeTransport::new(vec![Behavior::Hang, Behavior::Ok]);
    let (sleep, sleeps) = recording_sleep();
    let embedding = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect("second attempt recovers");
    assert_eq!(embedding, EMBEDDING);
    assert_eq!(transport.calls(), 2);
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_millis(5)],
        "the timed-out attempt consumed exactly one linear backoff"
    );
}

#[tokio::test]
async fn test_retry_recovers_after_network_failure() {
    // Upstream "network TypeError" analogue: RemoteNetwork once, then 200.
    let transport = FakeTransport::new(vec![Behavior::Network, Behavior::Ok]);
    let (sleep, sleeps) = recording_sleep();
    let embedding = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect("second attempt recovers");
    assert_eq!(embedding, EMBEDDING);
    assert_eq!(transport.calls(), 2);
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_millis(5)],
        "one linear backoff between attempt 1 and 2"
    );
}

#[tokio::test]
async fn test_invalid_json_fails_fast_without_another_attempt() {
    let transport = FakeTransport::new(vec![Behavior::Body(200, "not json")]);
    let (sleep, _sleeps) = recording_sleep();
    let error = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect_err("invalid JSON must fail");
    assert!(
        matches!(error, EmbedError::RemoteInvalidJson { .. }),
        "expected RemoteInvalidJson, got {error:?}"
    );
    assert!(
        error.to_string().contains("Invalid embedding API JSON"),
        "display: {error}"
    );
    assert!(!error.is_retryable());
    assert_eq!(
        transport.calls(),
        1,
        "invalid JSON must not consume another attempt"
    );
}

#[tokio::test]
async fn test_attempt_timeout_is_passed_to_each_transport_call() {
    // Upstream "passes AbortSignal.timeout to each fetch attempt": the
    // recorded attempt_timeout on every request equals the configured budget.
    let transport = FakeTransport::new(vec![Behavior::Status(500)]);
    let (sleep, _sleeps) = recording_sleep();
    let _ = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect_err("persistent 500 fails");
    let requests = transport.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.attempt_timeout, Duration::from_millis(50));
    }
}

#[tokio::test]
async fn test_retry_recovers_after_429() {
    let transport = FakeTransport::new(vec![Behavior::Status(429), Behavior::Ok]);
    let (sleep, sleeps) = recording_sleep();
    let embedding = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect("second attempt recovers");
    assert_eq!(embedding, EMBEDDING);
    assert_eq!(transport.calls(), 2);
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_millis(5)]);
}

#[tokio::test]
async fn test_retry_recovers_after_500() {
    let transport = FakeTransport::new(vec![Behavior::Status(500), Behavior::Ok]);
    let (sleep, sleeps) = recording_sleep();
    let embedding = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect("second attempt recovers");
    assert_eq!(embedding, EMBEDDING);
    assert_eq!(transport.calls(), 2);
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_millis(5)]);
}

#[tokio::test]
async fn test_non_retryable_4xx_fails_fast() {
    let transport = FakeTransport::new(vec![Behavior::Status(401)]);
    let (sleep, _sleeps) = recording_sleep();
    let error = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect_err("401 must fail");
    match &error {
        EmbedError::RemoteHttp { status, .. } => assert_eq!(*status, 401),
        other => panic!("expected RemoteHttp, got {other:?}"),
    }
    // Divergence (5) pinned: the transport carries status + body only, so
    // status_text is derived as "" (upstream used fetch Response.statusText).
    assert_eq!(error.to_string(), "API embedding failed: 401 ");
    assert_eq!(transport.calls(), 1, "401 must not consume another attempt");
}

#[tokio::test]
async fn test_persistent_5xx_exhausts_all_attempts_and_returns_last_error() {
    let transport = FakeTransport::new(vec![Behavior::Status(503)]);
    let (sleep, sleeps) = recording_sleep();
    let error = fetch(&transport, "hello", &retry_options(), &sleep)
        .await
        .expect_err("persistent 503 must fail");
    match &error {
        EmbedError::RemoteHttp { status, .. } => assert_eq!(*status, 503),
        other => panic!("expected RemoteHttp, got {other:?}"),
    }
    assert_eq!(transport.calls(), 3, "all attempts consumed");
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_millis(5), Duration::from_millis(10)],
        "linear backoff: 5ms x1, then 5ms x2"
    );
}

#[tokio::test]
async fn test_request_shape_carries_url_bearer_auth_input_and_model() {
    let transport = FakeTransport::new(vec![Behavior::Ok]);
    let (sleep, _sleeps) = recording_sleep();
    let embedding = fetch(&transport, "search_query: hello", &retry_options(), &sleep)
        .await
        .expect("single success");
    assert_eq!(embedding, EMBEDDING);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.url, "https://fake.local/v1/embeddings");
    // The bearer credential travels as `api_key`; `ReqwestTransport`
    // composes the `Authorization: Bearer <key>` header from it.
    assert_eq!(request.api_key, "test-key");
    assert_eq!(request.input, "search_query: hello");
    assert_eq!(request.model, "test-model");
    assert_eq!(request.attempt_timeout, Duration::from_millis(50));
}

#[tokio::test]
async fn test_errors_never_include_the_api_key() {
    let mut options = retry_options();
    options.api_key = "super-secret-key-42".to_owned();
    let transport = FakeTransport::new(vec![Behavior::Status(503)]);
    let (sleep, _sleeps) = recording_sleep();
    let error = fetch(&transport, "hello", &options, &sleep)
        .await
        .expect_err("persistent 503 must fail");
    let display = error.to_string();
    assert!(
        !display.contains("super-secret-key-42"),
        "error Display leaked the api key: {display}"
    );
    assert!(
        !display.contains("Bearer"),
        "error Display mentions Bearer: {display}"
    );
}

/// Local alias keeps the mirror bodies readable and uniform.
async fn fetch(
    transport: &dyn RemoteTransport,
    input: &str,
    options: &RemoteEmbedOptions,
    sleep: &SleepFn,
) -> Result<Vec<f32>, EmbedError> {
    mem_embed::remote::retry::fetch_remote_embedding_with_retry(transport, input, options, sleep)
        .await
}
