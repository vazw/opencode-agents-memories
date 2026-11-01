//! The capture-lane provider seam (ADR-0029).
//!
//! Three adapters, three oracles: `Structured` must not filter or re-parse what
//! the registry-built adapter already parsed; `FilteredStructured` must filter
//! the REQUEST before the inner call; and it must refuse to dial a conversation
//! the rules emptied — the 25.5 s round-trip with the capture prompt stripped is
//! the failure that guard exists to prevent.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mem_ai::provider::request::{ChatMessage, CompletionRequest, EmbedRequest, ModelInfo};
use mem_ai::provider::{
    AiProvider, BoxFuture, CompletionStream, HttpRequest, HttpResponse, HttpTransport,
    ProviderConfig, ProviderError, ProviderRegistry, RemoteProviderConfig, StaticSecretResolver,
};
use mem_bridge::capture_provider::{CaptureProvider, Structured, StructuredCapture};
use mem_bridge::composition::build_composed_runtime;
use mem_store::serde_json::{Value, json};

/// The tool-call body the OpenAI-compatible structured arm answers with.
const TOOL_CALL_BODY: &str =
    r#"{"choices":[{"message":{"tool_calls":[{"function":{"arguments":"{\"a\":1}"}}]}}]}"#;

/// Records every outbound HTTP request and answers one fixed body.
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
        self.calls.lock().expect("calls lock").clone()
    }
}

impl HttpTransport for RecordingTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'static, Result<HttpResponse, ProviderError>> {
        self.calls.lock().expect("calls lock").push(request);
        let body = self.body.clone();
        Box::pin(std::future::ready(Ok(HttpResponse { status: 200, body })))
    }
}

fn capture_schema() -> Value {
    mem_store::serde_json::from_str(mem_bridge::capture_prompt::CAPTURE_OUTPUT_SCHEMA)
        .expect("the frozen capture schema parses")
}

fn remote_config() -> RemoteProviderConfig {
    let mut config = RemoteProviderConfig::new("capture-model");
    config.base_url = "https://capture.example/v1".to_owned();
    config
}

/// A double that is both halves of a `CaptureProvider`: it records every
/// request it is handed and answers with one fixed value.
struct RecordingCapture {
    requests: Mutex<Vec<CompletionRequest>>,
    reply: Value,
}

impl RecordingCapture {
    fn new(reply: Value) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            reply,
        })
    }

    fn requests(&self) -> Vec<CompletionRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl AiProvider for RecordingCapture {
    fn name(&self) -> &str {
        "recording-capture"
    }

    fn complete(&self, _request: CompletionRequest) -> CompletionStream {
        Box::pin(mem_episodic::futures::stream::iter(Vec::<
            Result<Value, ProviderError>,
        >::new()))
    }

    fn embed<'life0, 'async_trait>(
        &'life0 self,
        _request: EmbedRequest,
    ) -> BoxFuture<'async_trait, Result<Vec<Vec<f32>>, ProviderError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn list_models<'life0, 'async_trait>(
        &'life0 self,
    ) -> BoxFuture<'async_trait, Result<Vec<ModelInfo>, ProviderError>>
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
    ) -> BoxFuture<'async_trait, Result<u32, ProviderError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(0) })
    }
}

impl StructuredCapture for RecordingCapture {
    fn complete_structured<'a>(
        &'a self,
        request: CompletionRequest,
        _schema: Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        self.requests.lock().expect("requests lock").push(request);
        let reply = self.reply.clone();
        Box::pin(async move { Ok(reply) })
    }
}

struct NoopEmbedder;

impl mem_embed::embedder::Embedder for NoopEmbedder {
    fn warmup(
        &self,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<(), mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(
        &self,
        _text: &str,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<Vec<f32>, mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(vec![0.0; 8]) })
    }
}

fn load_config(root: &std::path::Path, home: &std::path::Path) -> mem_core::config::Config {
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.to_string_lossy().to_string())]
        .into_iter()
        .collect();
    let mut config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(home), &env)
            .expect("the default config loads");
    config.storage_path = root.to_path_buf();
    config.embedding_dimensions = 8;
    config
}

// ---------------------------------------------------------------------------
// Row 1 — `Structured` delegates verbatim
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_structured_delegates_verbatim() {
    // Mutant: `Structured` re-filters the request or re-parses the value. The
    // registry-built adapter already parsed the tool-call arguments; a second
    // filter/parse would change the value or send a second round-trip.
    let transport = RecordingTransport::new(TOOL_CALL_BODY);
    let mut registry = ProviderRegistry::new(Arc::new(
        StaticSecretResolver::new().with_secret("env:KEY", "sk-secret"),
    ));
    registry.register_builtin_with(transport.clone());
    let handle = registry
        .create_structured(&ProviderConfig::OpenAIChat(remote_config()))
        .expect("the built-in openai-chat factory builds a structured handle");
    let structured = Structured::new(handle);

    let value = structured
        .complete_structured(
            CompletionRequest::new("capture-model").with_message(ChatMessage::user("capture this")),
            capture_schema(),
        )
        .await
        .expect("the structured arm answers the mocked tool call");

    assert_eq!(
        value,
        json!({ "a": 1 }),
        "the adapter's parsed value, verbatim"
    );
    let calls = transport.calls();
    assert_eq!(calls.len(), 1, "exactly one round-trip: {calls:?}");
    assert_eq!(
        calls[0].body.as_ref().expect("a JSON body")["tool_choice"],
        "required",
        "the body is the one the adapter built"
    );
}

// ---------------------------------------------------------------------------
// Row 2 — the mined guard filters the REQUEST
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_filtered_structured_redacts_before_the_inner_call() {
    // Mutant: the filter applied to the returned value, or not at all. The
    // recorded system prompt is the only place a request-side filter shows.
    let root = common::temp_dir("capture-provider-redact");
    let home = common::temp_dir("capture-provider-redact-home");
    let config = load_config(&root, &home);

    let recorder = RecordingCapture::new(json!({ "summary": "s", "type": "task", "tags": [] }));
    let runtime = build_composed_runtime(
        &root,
        &config,
        Arc::new(NoopEmbedder),
        Some(CaptureProvider {
            base: recorder.clone() as Arc<dyn AiProvider>,
            structured: recorder.clone() as Arc<dyn StructuredCapture>,
        }),
    )
    .await
    .expect("the runtime composes");

    let structured = runtime
        .provider
        .clone()
        .expect("the composed runtime exposes the filtered capture handle");
    let returned = structured
        .complete_structured(
            CompletionRequest::new("m")
                .with_system("<private>secret-token</private>")
                .with_message(ChatMessage::user("capture this")),
            capture_schema(),
        )
        .await
        .expect("the inner double answers");

    let recorded = recorder.requests();
    assert_eq!(recorded.len(), 1, "the inner call happens once");
    let system = recorded[0].system.clone().unwrap_or_default();
    assert!(
        system.contains("[REDACTED]"),
        "the request is filtered before the inner call: {system}"
    );
    assert!(
        !system.contains("secret-token"),
        "the secret never reaches the inner provider: {system}"
    );
    assert_eq!(
        returned,
        json!({ "summary": "s", "type": "task", "tags": [] }),
        "the reply is never filtered (D8)"
    );

    drop(runtime);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&home);
}

// ---------------------------------------------------------------------------
// Row 3 — an emptied conversation is never dialled
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_filtered_structured_never_calls_an_emptied_conversation() {
    // Mutant: delegate without the guard — the provider is dialled with only
    // the capture system prompt left, i.e. a 25.5 s round-trip for nothing.
    let root = common::temp_dir("capture-provider-emptied");
    let home = common::temp_dir("capture-provider-emptied-home");
    let mut config = load_config(&root, &home);
    config.chat_message.filter_injected_prompts = true;
    config
        .chat_message
        .injection_markers
        .push("<my-banner>".to_string());

    let recorder = RecordingCapture::new(json!({ "summary": "s", "type": "task", "tags": [] }));
    let runtime = build_composed_runtime(
        &root,
        &config,
        Arc::new(NoopEmbedder),
        Some(CaptureProvider {
            base: recorder.clone() as Arc<dyn AiProvider>,
            structured: recorder.clone() as Arc<dyn StructuredCapture>,
        }),
    )
    .await
    .expect("the runtime composes");

    let structured = runtime
        .provider
        .clone()
        .expect("the composed runtime exposes the filtered capture handle");
    let error = structured
        .complete_structured(
            CompletionRequest::new("m")
                .with_system("capture system prompt")
                .with_message(ChatMessage::user("<my-banner>x</my-banner>")),
            capture_schema(),
        )
        .await
        .expect_err("an emptied conversation must fail closed");

    assert_eq!(error.kind, mem_ai::provider::ProviderErrorKind::Config);
    assert_eq!(error.message, "all messages were filtered as injected");
    assert_eq!(
        recorder.requests().len(),
        0,
        "the inner provider is never dialled"
    );

    drop(runtime);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&home);
}
