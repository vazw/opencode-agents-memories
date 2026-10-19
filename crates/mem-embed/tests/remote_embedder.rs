//! Remote embedder + transport battery (blueprint step 8). Covers the
//! `RemoteEmbedder` `Embedder` impl: the warmup "ping" probe (upstream probe
//! through the full request pipeline), embed through the retry path with the
//! configured endpoint options, warmup-failure propagation, and the upstream
//! retry-option defaults (8s / 3 / 500ms).

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mem_embed::remote::{
    REMOTE_ATTEMPT_TIMEOUT_MS, REMOTE_MAX_ATTEMPTS, REMOTE_RETRY_BACKOFF_MS, RemoteEmbedOptions,
    RemoteHttpResponse, RemoteRequest, RemoteRetryOptions, RemoteTransport, SleepFn,
};
use mem_embed::{BoxFuture, EmbedError, Embedder};

const OK_BODY: &str = r#"{"data":[{"embedding":[0.25,0.5]}]}"#;

/// Records every request; `failing` makes every attempt return 500.
struct RecordingTransport {
    failing: bool,
    calls: AtomicUsize,
    requests: Mutex<Vec<RemoteRequest>>,
}

impl RecordingTransport {
    fn new(failing: bool) -> Self {
        Self {
            failing,
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

impl RemoteTransport for RecordingTransport {
    fn post_json(
        &self,
        request: RemoteRequest,
    ) -> BoxFuture<'_, Result<RemoteHttpResponse, EmbedError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        let failing = self.failing;
        Box::pin(async move {
            if failing {
                Ok(RemoteHttpResponse {
                    status: 500,
                    body: String::new(),
                })
            } else {
                Ok(RemoteHttpResponse {
                    status: 200,
                    body: OK_BODY.to_owned(),
                })
            }
        })
    }
}

/// Immediate-resolve sleep so backoff is recorded, never waited on.
fn recording_sleep() -> (SleepFn, Arc<Mutex<Vec<Duration>>>) {
    let recorded: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&recorded);
    let sleep: SleepFn = Arc::new(move |duration| -> BoxFuture<'static, ()> {
        sink.lock().unwrap().push(duration);
        Box::pin(std::future::ready(()))
    });
    (sleep, recorded)
}

fn test_options() -> RemoteEmbedOptions {
    RemoteEmbedOptions {
        url: "https://api.test/v1".to_owned(),
        api_key: "embed-key".to_owned(),
        model: "embed-model".to_owned(),
        retry: RemoteRetryOptions {
            attempt_timeout: Duration::from_millis(50),
            max_attempts: 2,
            backoff: Duration::from_millis(7),
        },
    }
}

#[tokio::test]
async fn test_warmup_sends_ping_probe() {
    let transport = Arc::new(RecordingTransport::new(false));
    let (sleep, _sleeps) = recording_sleep();
    let embedder = mem_embed::RemoteEmbedder::new(
        test_options(),
        transport.clone() as Arc<dyn RemoteTransport>,
        sleep,
    );
    embedder.warmup().await.expect("probe succeeds");
    let requests = transport.requests();
    assert_eq!(requests.len(), 1, "exactly one probe call");
    assert_eq!(requests[0].input, "ping");
    assert_eq!(requests[0].url, "https://api.test/v1/embeddings");
}

#[tokio::test]
async fn test_embed_calls_retry_with_configured_options() {
    let transport = Arc::new(RecordingTransport::new(false));
    let (sleep, _sleeps) = recording_sleep();
    let embedder = mem_embed::RemoteEmbedder::new(
        test_options(),
        transport.clone() as Arc<dyn RemoteTransport>,
        sleep,
    );
    let embedding = embedder.embed("hello world").await.expect("embed succeeds");
    assert_eq!(embedding, vec![0.25f32, 0.5f32]);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.input, "hello world");
    assert_eq!(request.model, "embed-model");
    assert_eq!(request.url, "https://api.test/v1/embeddings");
    assert_eq!(request.api_key, "embed-key");
    assert_eq!(request.attempt_timeout, Duration::from_millis(50));
}

#[tokio::test]
async fn test_warmup_failure_propagates() {
    let transport = Arc::new(RecordingTransport::new(true));
    let (sleep, sleeps) = recording_sleep();
    let embedder = mem_embed::RemoteEmbedder::new(
        test_options(),
        transport.clone() as Arc<dyn RemoteTransport>,
        sleep,
    );
    let error = embedder.warmup().await.expect_err("warmup must fail");
    match &error {
        mem_embed::EmbedError::RemoteHttp { status, .. } => assert_eq!(*status, 500),
        other => panic!("expected RemoteHttp, got {other:?}"),
    }
    assert_eq!(transport.calls(), 2, "max_attempts = 2 consumed");
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_millis(7)],
        "linear backoff 7ms x1"
    );
}

#[test]
fn test_retry_option_defaults_match_upstream() {
    let defaults = RemoteRetryOptions::default();
    assert_eq!(defaults.attempt_timeout, Duration::from_millis(8000));
    assert_eq!(defaults.max_attempts, 3);
    assert_eq!(defaults.backoff, Duration::from_millis(500));
    assert_eq!(REMOTE_ATTEMPT_TIMEOUT_MS, 8000);
    assert_eq!(REMOTE_MAX_ATTEMPTS, 3);
    assert_eq!(REMOTE_RETRY_BACKOFF_MS, 500);
}
