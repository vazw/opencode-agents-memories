//! Remote provider adapter contract (W2.2 / US 4.1 clients).
//!
//! Ports the upstream behavioural contracts (`anthropic-provider`,
//! `openai-chat-completion-provider`, `minimax-provider`,
//! `atlas-cloud-provider`, `orcarouter-provider`, plus the OpenAI responses and
//! Gemini adapters) onto the W2.1 trait seam: provider name, endpoint/model
//! resolution, auth headers, request essentials, SSE text streaming, and
//! structured output — all driven through an injected mock transport, so no
//! real API key or network is needed.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use mem_ai::provider::{
    AiProvider, AiProviderStructured, AnthropicProvider, AtlasCloudProvider, BoxFuture,
    ChatMessage, CompletionRequest, GeminiProvider, HttpRequest, HttpResponse, HttpTransport,
    MinimaxProvider, ModelInfo, OpenAiChatProvider, OpenAiResponsesProvider, OrcaRouterProvider,
    ProviderConfig, ProviderError, ProviderErrorKind, ProviderKind, ProviderRegistry,
    RemoteProviderConfig, StaticSecretResolver,
};
use serde_json::{Value, json};

/// A transport that replays scripted responses and records every request.
struct ScriptedTransport {
    calls: Mutex<Vec<HttpRequest>>,
    queue: Mutex<VecDeque<(u16, String)>>,
}

impl ScriptedTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            queue: Mutex::new(VecDeque::new()),
        })
    }

    /// Queue one response (in order).
    fn reply(self: &Arc<Self>, status: u16, body: impl Into<String>) -> Arc<Self> {
        self.queue.lock().unwrap().push_back((status, body.into()));
        Arc::clone(self)
    }

    fn calls(&self) -> Vec<HttpRequest> {
        self.calls.lock().unwrap().clone()
    }

    fn last_body(&self) -> Value {
        let calls = self.calls();
        calls
            .last()
            .and_then(|call| call.body.clone())
            .expect("a JSON body was sent")
    }

    fn last_url(&self) -> String {
        self.calls()
            .last()
            .map(|call| call.url.clone())
            .expect("a call")
    }

    fn last_headers(&self) -> BTreeMap<String, String> {
        self.calls()
            .last()
            .map(|call| call.headers.clone())
            .expect("a call")
    }
}

impl HttpTransport for ScriptedTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'static, Result<HttpResponse, ProviderError>> {
        self.calls.lock().unwrap().push(request);
        let (status, body) = self
            .queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((200, "{}".to_owned()));
        Box::pin(async move { Ok(HttpResponse { status, body }) })
    }
}

fn resolver() -> Arc<dyn mem_ai::provider::SecretResolver> {
    Arc::new(StaticSecretResolver::new().with_secret("env:KEY", "sk-secret"))
}

fn config(model: &str, base: &str) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model)
        .with_base_url(base)
        .with_api_key_ref("env:KEY")
}

fn user_request(model: &str) -> CompletionRequest {
    CompletionRequest::new(model).with_message(ChatMessage::user("hello"))
}

async fn collect_text(provider: &dyn AiProvider, request: CompletionRequest) -> (String, bool) {
    let mut stream = provider.complete(request);
    let mut text = String::new();
    let mut done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk ok");
        if let Some(piece) = chunk.get("text").and_then(Value::as_str) {
            text.push_str(piece);
        }
        if chunk.get("done").and_then(Value::as_bool) == Some(true) {
            done = true;
        }
    }
    (text, done)
}

const ANTHROPIC_SSE: &str = concat!(
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello \"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"world\"}}\n\n",
    "data: [DONE]\n\n",
);

const OPENAI_SSE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\n",
    "data: [DONE]\n\n",
);

const RESPONSES_SSE: &str = concat!(
    "event: response.output_text.delta\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello \"}\n\n",
    "event: response.output_text.delta\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\"}\n\n",
    "data: [DONE]\n\n",
);

const GEMINI_SSE: &str = concat!(
    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello \"}]}}]}\n\n",
    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"world\"}]}}]}\n\n",
);

#[tokio::test]
async fn test_anthropic_complete_streams_text() {
    let transport = ScriptedTransport::new().reply(200, ANTHROPIC_SSE);
    let provider = AnthropicProvider::new(
        config("claude-3-5-sonnet-latest", "https://api.anthropic.com/v1"),
        resolver(),
        transport.clone(),
    );

    let (text, done) = collect_text(&provider, user_request("claude-3-5-sonnet-latest")).await;
    assert_eq!(text, "hello world");
    assert!(done, "stream terminates with a done chunk");

    assert_eq!(
        transport.last_url(),
        "https://api.anthropic.com/v1/messages"
    );
    let headers = transport.last_headers();
    assert_eq!(
        headers.get("x-api-key").map(String::as_str),
        Some("sk-secret")
    );
    assert_eq!(
        headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
    let body = transport.last_body();
    assert!(body["max_tokens"].is_u64(), "max_tokens is required");
    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["stream"], true);
}

#[tokio::test]
async fn test_anthropic_structured_output() {
    let transport = ScriptedTransport::new().reply(
        200,
        r#"{"content":[{"type":"tool_use","name":"structured_output","input":{"topic":"auth","count":3}}]}"#,
    );
    let provider = AnthropicProvider::new(
        config("claude-3-5-sonnet-latest", "https://api.anthropic.com/v1"),
        resolver(),
        transport.clone(),
    );

    let schema = json!({
        "type": "object",
        "properties": { "topic": { "type": "string" }, "count": { "type": "number" } },
        "required": ["topic", "count"]
    });
    let value = AiProviderStructured::complete_structured(
        &provider,
        user_request("claude-3-5-sonnet-latest"),
        schema.clone(),
    )
    .await
    .expect("structured output");
    assert_eq!(value["topic"], "auth");
    assert_eq!(value["count"], 3);

    let body = transport.last_body();
    assert_eq!(body["tools"][0]["input_schema"], schema);
    assert_eq!(body["tool_choice"]["type"], "tool");
    assert!(body["max_tokens"].is_u64());
    assert!(
        AiProviderStructured::supports_structured_output(&provider, "claude-3-5-sonnet-latest")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_anthropic_configured_max_tokens_wins() {
    let transport = ScriptedTransport::new().reply(200, ANTHROPIC_SSE);
    let mut cfg = config("claude-3-5-sonnet-latest", "https://api.anthropic.com/v1");
    cfg.max_tokens = Some(2048);
    let provider = AnthropicProvider::new(cfg, resolver(), transport.clone());

    let _ = collect_text(&provider, user_request("claude-3-5-sonnet-latest")).await;
    assert_eq!(transport.last_body()["max_tokens"], 2048);
}

#[tokio::test]
async fn test_openai_chat_complete_streams_text() {
    let transport = ScriptedTransport::new().reply(200, OPENAI_SSE);
    let provider = OpenAiChatProvider::new(
        config("gpt-4o-mini", "https://api.openai.com/v1"),
        resolver(),
        transport.clone(),
    );

    let (text, done) = collect_text(&provider, user_request("gpt-4o-mini")).await;
    assert_eq!(text, "hello world");
    assert!(done);

    assert_eq!(
        transport.last_url(),
        "https://api.openai.com/v1/chat/completions"
    );
    let headers = transport.last_headers();
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer sk-secret")
    );
}

#[tokio::test]
async fn test_openai_chat_structured_output() {
    let transport = ScriptedTransport::new().reply(
        200,
        r#"{"choices":[{"message":{"content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"structured_output","arguments":"{\"memory\":\"captured fact\"}"}}]}}]}"#,
    );
    let provider = OpenAiChatProvider::new(
        config("gpt-4o-mini", "https://api.openai.com/v1"),
        resolver(),
        transport.clone(),
    );

    let value = AiProviderStructured::complete_structured(
        &provider,
        user_request("gpt-4o-mini"),
        json!({ "type": "object" }),
    )
    .await
    .expect("structured output");
    assert_eq!(value["memory"], "captured fact");

    let body = transport.last_body();
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["tool_choice"], "required");
    assert!(body["tools"].is_array());
    let temperature = body["temperature"].as_f64().expect("temperature") as f32;
    assert!(
        (temperature - 0.3).abs() < 1e-6,
        "default temperature {temperature}"
    );
}

#[tokio::test]
async fn test_openai_responses_complete_streams_text() {
    let transport = ScriptedTransport::new().reply(200, RESPONSES_SSE);
    let provider = OpenAiResponsesProvider::new(
        config("gpt-5", "https://api.openai.com/v1"),
        resolver(),
        transport.clone(),
    );

    let (text, done) = collect_text(&provider, user_request("gpt-5")).await;
    assert_eq!(text, "hello world");
    assert!(done);
    assert_eq!(transport.last_url(), "https://api.openai.com/v1/responses");
}

#[tokio::test]
async fn test_openai_responses_structured_output() {
    let transport = ScriptedTransport::new().reply(
        200,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"memory\":\"captured fact\"}"}]}]}"#,
    );
    let provider = OpenAiResponsesProvider::new(
        config("gpt-5", "https://api.openai.com/v1"),
        resolver(),
        transport.clone(),
    );

    let schema = json!({ "type": "object" });
    let value =
        AiProviderStructured::complete_structured(&provider, user_request("gpt-5"), schema.clone())
            .await
            .expect("structured output");
    assert_eq!(value["memory"], "captured fact");
    assert_eq!(transport.last_body()["text"]["format"]["schema"], schema);
}

#[tokio::test]
async fn test_gemini_complete_streams_text() {
    let transport = ScriptedTransport::new().reply(200, GEMINI_SSE);
    let provider = GeminiProvider::new(
        config(
            "gemini-1.5-flash",
            "https://generativelanguage.googleapis.com/v1beta",
        ),
        resolver(),
        transport.clone(),
    );

    let (text, done) = collect_text(&provider, user_request("gemini-1.5-flash")).await;
    assert_eq!(text, "hello world");
    assert!(done);
    assert_eq!(
        transport.last_url(),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:streamGenerateContent?alt=sse"
    );
    let headers = transport.last_headers();
    // The key travels in a header, never in the URL.
    assert_eq!(
        headers.get("x-goog-api-key").map(String::as_str),
        Some("sk-secret")
    );
    assert!(!transport.last_url().contains("sk-secret"));
}

#[tokio::test]
async fn test_gemini_structured_output() {
    let transport = ScriptedTransport::new().reply(
        200,
        r#"{"candidates":[{"content":{"parts":[{"text":"{\"memory\":\"captured fact\"}"}]}}]}"#,
    );
    let provider = GeminiProvider::new(
        config(
            "gemini-1.5-flash",
            "https://generativelanguage.googleapis.com/v1beta",
        ),
        resolver(),
        transport.clone(),
    );

    let schema = json!({ "type": "object" });
    let value = AiProviderStructured::complete_structured(
        &provider,
        user_request("gemini-1.5-flash"),
        schema.clone(),
    )
    .await
    .expect("structured output");
    assert_eq!(value["memory"], "captured fact");
    assert_eq!(
        transport.last_body()["generationConfig"]["responseSchema"],
        schema
    );
}

#[tokio::test]
async fn test_minimax_complete_streams_text() {
    let transport = ScriptedTransport::new().reply(200, ANTHROPIC_SSE);
    let provider = MinimaxProvider::new(
        config("MiniMax-M3", "https://api.minimax.io"),
        resolver(),
        transport.clone(),
    );

    let (text, done) = collect_text(&provider, user_request("MiniMax-M3")).await;
    assert_eq!(text, "hello world");
    assert!(done);
    assert_eq!(
        transport.last_url(),
        "https://api.minimax.io/anthropic/v1/messages"
    );
    let headers = transport.last_headers();
    assert_eq!(
        headers.get("x-api-key").map(String::as_str),
        Some("sk-secret")
    );
    assert_eq!(
        headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
}

#[tokio::test]
async fn test_minimax_structured_output() {
    let transport = ScriptedTransport::new().reply(
        200,
        r#"{"content":[{"type":"tool_use","name":"structured_output","input":{"memory":"captured fact"}}]}"#,
    );
    let provider = MinimaxProvider::new(
        config("MiniMax-M3", "https://api.minimax.io"),
        resolver(),
        transport.clone(),
    );

    let value = AiProviderStructured::complete_structured(
        &provider,
        user_request("MiniMax-M3"),
        json!({ "type": "object" }),
    )
    .await
    .expect("structured output");
    assert_eq!(value["memory"], "captured fact");
    assert_eq!(transport.last_body()["model"], "MiniMax-M3");
}

#[tokio::test]
async fn test_minimax_missing_base_url_is_a_config_error() {
    let transport = ScriptedTransport::new();
    let provider = MinimaxProvider::new(config("MiniMax-M3", ""), resolver(), transport);

    let mut stream = provider.complete(user_request("MiniMax-M3"));
    let error = stream
        .next()
        .await
        .expect("one item")
        .expect_err("config error");
    assert_eq!(error.kind, ProviderErrorKind::Config);
}

#[tokio::test]
async fn test_atlas_cloud_structured_output() {
    let transport = ScriptedTransport::new().reply(
        200,
        r#"{"choices":[{"message":{"content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"structured_output","arguments":"{\"memory\":\"captured fact\"}"}}]}}]}"#,
    );
    let provider = AtlasCloudProvider::new(config("", ""), resolver(), transport.clone());

    let value = AiProviderStructured::complete_structured(
        &provider,
        user_request("atlas"),
        json!({ "type": "object" }),
    )
    .await
    .expect("structured output");
    assert_eq!(value["memory"], "captured fact");

    assert_eq!(
        transport.last_url(),
        "https://api.atlascloud.ai/v1/chat/completions"
    );
    let body = transport.last_body();
    assert_eq!(body["model"], "deepseek-ai/deepseek-v4-pro");
    assert_eq!(body["tool_choice"], "required");
    assert!(body["messages"].is_array());
    assert!(body["tools"].is_array());
}

#[tokio::test]
async fn test_atlas_cloud_complete_streams_text() {
    let transport = ScriptedTransport::new().reply(200, OPENAI_SSE);
    let provider = AtlasCloudProvider::new(
        config("qwen/qwen3-coder", "https://proxy.example.com/v1/"),
        resolver(),
        transport.clone(),
    );

    let (text, done) = collect_text(&provider, user_request("qwen/qwen3-coder")).await;
    assert_eq!(text, "hello world");
    assert!(done);
    assert_eq!(
        transport.last_url(),
        "https://proxy.example.com/v1/chat/completions"
    );
    assert_eq!(transport.last_body()["model"], "qwen/qwen3-coder");
}

#[tokio::test]
async fn test_orcarouter_structured_output() {
    let transport = ScriptedTransport::new().reply(
        200,
        r#"{"choices":[{"message":{"content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"structured_output","arguments":"{\"memory\":\"captured fact\"}"}}]}}]}"#,
    );
    let provider = OrcaRouterProvider::new(config("", ""), resolver(), transport.clone());

    let value = AiProviderStructured::complete_structured(
        &provider,
        user_request("orcarouter/auto"),
        json!({ "type": "object" }),
    )
    .await
    .expect("structured output");
    assert_eq!(value["memory"], "captured fact");

    assert_eq!(
        transport.last_url(),
        "https://api.orcarouter.ai/v1/chat/completions"
    );
    let headers = transport.last_headers();
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer sk-secret")
    );
    let body = transport.last_body();
    assert_eq!(body["model"], "orcarouter/auto");
    assert_eq!(body["tool_choice"], "required");
}

#[tokio::test]
async fn test_orcarouter_rejects_bare_model() {
    let transport = ScriptedTransport::new();
    let provider = OrcaRouterProvider::new(config("gpt-4o-mini", ""), resolver(), transport);

    let mut stream = provider.complete(user_request("gpt-4o-mini"));
    let error = stream
        .next()
        .await
        .expect("one item")
        .expect_err("bare model");
    assert_eq!(error.kind, ProviderErrorKind::Config);
    assert!(error.message.contains("namespaced memoryModel"));
}

#[tokio::test]
async fn test_remote_error_status_maps_to_auth() {
    let transport = ScriptedTransport::new().reply(401, "login fail");
    let provider = OpenAiChatProvider::new(
        config("gpt-4o-mini", "https://api.openai.com/v1"),
        resolver(),
        transport.clone(),
    );

    let mut stream = provider.complete(user_request("gpt-4o-mini"));
    let error = stream.next().await.expect("one item").expect_err("401");
    assert_eq!(error.kind, ProviderErrorKind::Auth);
    assert_eq!(error.status, Some(401));
    // Never retried: exactly one call.
    assert_eq!(transport.calls().len(), 1);
}

#[tokio::test]
async fn test_remote_embed_is_unsupported() {
    let provider = OpenAiChatProvider::new(
        config("gpt-4o-mini", "https://api.openai.com/v1"),
        resolver(),
        ScriptedTransport::new(),
    );
    let error = provider
        .embed(mem_ai::provider::EmbedRequest::new(
            "gpt-4o-mini",
            vec!["x".to_owned()],
        ))
        .await
        .expect_err("unsupported");
    assert_eq!(error.kind, ProviderErrorKind::Unsupported);

    let models: Vec<ModelInfo> = provider.list_models().await.expect("models");
    assert_eq!(models[0].id, "gpt-4o-mini");
    assert_eq!(models[0].provider, "openai-chat");
    assert!(models[0].supports_structured_output);
}

#[test]
fn test_remote_registry_builds_every_provider() {
    let mut registry = ProviderRegistry::new(Arc::new(StaticSecretResolver::new()));
    registry.register_builtin();

    let cases: Vec<(ProviderKind, ProviderConfig, &str, &str)> = vec![
        (
            ProviderKind::Anthropic,
            ProviderConfig::Anthropic(config(
                "claude-3-5-sonnet-latest",
                "https://api.anthropic.com/v1",
            )),
            "anthropic",
            "claude-3-5-sonnet-latest",
        ),
        (
            ProviderKind::OpenAIChat,
            ProviderConfig::OpenAIChat(config("gpt-4o-mini", "https://api.openai.com/v1")),
            "openai-chat",
            "gpt-4o-mini",
        ),
        (
            ProviderKind::OpenAIResponses,
            ProviderConfig::OpenAIResponses(config("gpt-5", "https://api.openai.com/v1")),
            "openai-responses",
            "gpt-5",
        ),
        (
            ProviderKind::Gemini,
            ProviderConfig::Gemini(config(
                "gemini-1.5-flash",
                "https://generativelanguage.googleapis.com/v1beta",
            )),
            "google-gemini",
            "gemini-1.5-flash",
        ),
        (
            ProviderKind::Minimax,
            ProviderConfig::Minimax(config("MiniMax-M3", "https://api.minimax.io")),
            "minimax",
            "MiniMax-M3",
        ),
        (
            ProviderKind::AtlasCloud,
            ProviderConfig::AtlasCloud(config("", "")),
            "atlas-cloud",
            "",
        ),
        (
            ProviderKind::OrcaRouter,
            ProviderConfig::OrcaRouter(config("", "")),
            "orcarouter",
            "",
        ),
    ];

    for (kind, config, name, model) in cases {
        assert_eq!(config.kind(), kind);
        assert_eq!(kind.as_str(), name);
        assert_eq!(config.model(), model);
        let provider = registry.create(&config).expect("builtin factory resolves");
        assert_eq!(provider.name(), name);
        // Erased behind the trait seam.
        let _erased: Arc<dyn AiProvider> = provider;
    }
}

#[test]
fn test_remote_config_surfaces_timeout_and_base_url() {
    let config = ProviderConfig::Anthropic(
        RemoteProviderConfig::new("claude-3-5-sonnet-latest")
            .with_base_url("https://api.anthropic.com/v1")
            .with_timeout(std::time::Duration::from_millis(1234)),
    );
    assert_eq!(config.base_url(), "https://api.anthropic.com/v1");
    assert_eq!(config.timeout(), std::time::Duration::from_millis(1234));
}

#[test]
fn test_minimax_registry_requires_base_url() {
    let mut registry = ProviderRegistry::new(Arc::new(StaticSecretResolver::new()));
    registry.register_builtin();
    let config = ProviderConfig::Minimax(RemoteProviderConfig::new("MiniMax-M3"));
    let error = match registry.create(&config) {
        Ok(_) => panic!("empty base URL must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.kind, ProviderErrorKind::Config);
}
