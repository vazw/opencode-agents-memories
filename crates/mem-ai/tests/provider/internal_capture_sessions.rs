//! Internal capture session tracking (upstream `internal-capture-sessions.test.ts`).
//!
//! The guard's job: a session created for an internal structured-output prompt
//! must never be re-captured (no feedback loop), and the tracked set is
//! process-global so every capture path sees the same state.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use mem_ai::provider::{
    HttpRequest, HttpResponse, HttpTransport, INTERNAL_CAPTURE_SESSION_TITLE,
    InternalCaptureSessions, ProviderConfig, ProviderError, ProviderRegistry, RemoteProviderConfig,
    StaticSecretResolver, is_internal_capture_session_title,
};
use serde_json::{Value, json};

#[test]
fn test_internal_capture_session_not_recaptured() {
    let sessions = InternalCaptureSessions::with_grace(Duration::from_secs(60));

    assert!(sessions.should_capture("ses_other"));
    assert!(!sessions.is_tracked("ses_live"));

    sessions.track("ses_live");
    assert!(sessions.is_tracked("ses_live"));
    // While tracked, the capture path must skip this session entirely.
    assert!(!sessions.should_capture("ses_live"));

    // Untrack keeps the id in the grace window so a late `session.idle` still
    // matches (upstream 60s grace), i.e. still not re-captured.
    sessions.untrack("ses_live");
    assert!(sessions.is_tracked("ses_live"));
    assert!(!sessions.should_capture("ses_live"));

    // With no grace, untracking releases immediately.
    let immediate = InternalCaptureSessions::with_grace(Duration::ZERO);
    immediate.track("ses_now");
    assert!(!immediate.should_capture("ses_now"));
    immediate.untrack("ses_now");
    assert!(immediate.should_capture("ses_now"));

    // The shared title constant identifies internal sessions.
    assert!(is_internal_capture_session_title(
        INTERNAL_CAPTURE_SESSION_TITLE
    ));
    assert!(!is_internal_capture_session_title("normal session"));
}

#[test]
fn test_internal_session_set_global() {
    let _gate = crate::lock_global_capture();
    let first = InternalCaptureSessions::global();
    let second = InternalCaptureSessions::global();

    first.clear();
    first.track("ses_global");

    // Both handles observe the same process-global set.
    assert!(second.is_tracked("ses_global"));
    assert!(!second.should_capture("ses_global"));

    second.untrack("ses_global");
    assert!(first.is_tracked("ses_global"), "grace keeps it visible");

    first.clear();
    assert!(!second.is_tracked("ses_global"));
}

// ---------------------------------------------------------------------------
// ADR-0029 — the registry must be able to hand out the structured capability
// ---------------------------------------------------------------------------

/// The tool-call body the OpenCode-compatible structured arm answers with.
const STRUCTURED_TOOL_CALL_BODY: &str = r#"{"choices":[{"message":{"tool_calls":[{"function":{"arguments":"{\"summary\":\"x\"}"}}]}}]}"#;

/// The SSE text body the streaming arm answers with.
const TEXT_SSE_BODY: &str =
    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";

/// Records every outbound request and answers one fixed body.
struct RecordingTransport {
    calls: Mutex<Vec<HttpRequest>>,
    body: String,
}

impl RecordingTransport {
    fn new(body: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            body: body.to_owned(),
        })
    }

    fn calls(&self) -> Vec<HttpRequest> {
        self.calls.lock().unwrap().clone()
    }
}

impl HttpTransport for RecordingTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> mem_ai::provider::BoxFuture<'static, Result<HttpResponse, ProviderError>> {
        self.calls.lock().unwrap().push(request);
        let body = self.body.clone();
        Box::pin(async move { Ok(HttpResponse { status: 200, body }) })
    }
}

fn remote_config() -> RemoteProviderConfig {
    let mut config = RemoteProviderConfig::new("capture-model");
    config.base_url = "https://capture.example/v1".to_owned();
    config
}

fn registry_over(transport: Arc<RecordingTransport>) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new(Arc::new(
        StaticSecretResolver::new().with_secret("env:KEY", "sk-secret"),
    ));
    registry.register_builtin_with(transport);
    registry
}

/// The capability `create` used to throw away: a registry-built adapter must
/// answer `complete_structured` with the parsed tool-call arguments.
#[tokio::test]
async fn test_registry_create_structured_yields_a_structured_handle() {
    let transport = RecordingTransport::new(STRUCTURED_TOOL_CALL_BODY);
    let registry = registry_over(transport.clone());

    let structured = registry
        .create_structured(&ProviderConfig::OpenAIChat(remote_config()))
        .expect("the built-in openai-chat factory builds a structured handle");
    let value = structured
        .complete_structured(
            mem_ai::provider::CompletionRequest::new("capture-model")
                .with_message(mem_ai::provider::ChatMessage::user("capture this")),
            json!({ "type": "object", "required": ["summary"] }),
        )
        .await
        .expect("the structured arm answers the mocked tool call");

    assert_eq!(value["summary"], "x");
    assert_eq!(transport.calls().len(), 1, "one structured round-trip");
}

/// `create` keeps its `Arc<dyn AiProvider>` contract: narrowing it to the
/// structured-only handle would break every `wrap`/`complete` caller.
#[tokio::test]
async fn test_registry_create_still_returns_the_base_handle() {
    let transport = RecordingTransport::new(TEXT_SSE_BODY);
    let registry = registry_over(transport);

    let provider = registry
        .create(&ProviderConfig::OpenAIChat(remote_config()))
        .expect("the built-in openai-chat factory builds a base handle");
    let mut stream = provider.complete(
        mem_ai::provider::CompletionRequest::new("capture-model")
            .with_message(mem_ai::provider::ChatMessage::user("hi")),
    );

    let mut text = String::new();
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let chunk: Value = chunk.expect("chunk ok");
        if let Some(delta) = chunk.get("text").and_then(Value::as_str) {
            text.push_str(delta);
        }
        if chunk.get("done").and_then(Value::as_bool) == Some(true) {
            saw_done = true;
        }
    }

    assert_eq!(
        text, "ok",
        "the base handle streams the adapter's `{{text}}` chunks"
    );
    assert!(saw_done, "the stream terminates with a done chunk");
}
