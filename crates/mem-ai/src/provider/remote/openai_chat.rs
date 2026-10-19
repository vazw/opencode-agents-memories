//! OpenAI Chat Completions adapter (also the base wire for the
//! OpenAI-compatible Atlas Cloud and OrcaRouter gateways).
//!
//! Wire: `POST {base}/chat/completions`, `Authorization: Bearer <secret>`.
//! Streaming parses SSE `choices[0].delta.content`; structured output forces a
//! `structured_output` function call and reads its `arguments` (falling back
//! to JSON content).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::opencode_host::estimate_token_count;
use crate::provider::provider_trait::{AiProvider, AiProviderStructured, CompletionStream, sealed};
use crate::provider::remote::anthropic::trim_base;
use crate::provider::remote::auth::AuthStrategy;
use crate::provider::remote::plumbing::{
    CONTENT_TYPE_JSON, HttpTransport, ProviderInner, RemoteHttpClient, RemoteProviderConfig,
    ReqwestTransport, apply_safe_extra_params, completion_from_sse, conversation,
    effective_system_prompt,
};
use crate::provider::remote::structured::{StructuredAccumulator, parse_structured};
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo, Role};
use crate::provider::secret_resolver::SecretResolver;

/// Stable provider name.
pub const PROVIDER_NAME: &str = "openai-chat";

/// OpenAI's API base URL.
pub const OPENAI_CHAT_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Default sampling temperature (upstream `memoryTemperature ?? 0.3`).
pub const OPENAI_CHAT_DEFAULT_TEMPERATURE: f32 = 0.3;

/// Function name used to force structured output.
pub const OPENAI_CHAT_STRUCTURED_TOOL: &str = "structured_output";

/// Typed config for the OpenAI chat-completion adapter.
pub type OpenAiChatConfig = RemoteProviderConfig;

/// A default OpenAI chat config for `model`.
pub fn config(model: impl Into<String>) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model).with_base_url(OPENAI_CHAT_DEFAULT_BASE_URL)
}

/// Static (non-auth) headers for an OpenAI-compatible request.
pub(crate) fn static_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_owned(), CONTENT_TYPE_JSON.to_owned());
    headers
}

/// Bearer auth strategy for an OpenAI-compatible config.
pub(crate) fn auth_strategy(config: &RemoteProviderConfig) -> AuthStrategy {
    AuthStrategy::Bearer {
        secret_ref: config.api_key_ref.clone(),
    }
}

/// Build the shared inner state for `provider` from an already-built client.
pub(crate) fn inner_with_client(
    provider: &str,
    config: RemoteProviderConfig,
    client: Arc<RemoteHttpClient>,
) -> ProviderInner {
    let auth = auth_strategy(&config);
    ProviderInner::new(provider, config, client, static_headers(), auth)
}

/// Build the shared inner state for `provider` over an injected transport.
pub(crate) fn build_inner(
    provider: &str,
    config: RemoteProviderConfig,
    resolver: Arc<dyn SecretResolver>,
    transport: Arc<dyn HttpTransport>,
) -> ProviderInner {
    let client = Arc::new(RemoteHttpClient::new(provider, transport, resolver));
    inner_with_client(provider, config, client)
}

/// The `{base}/chat/completions` endpoint, or a config error for an empty base.
pub(crate) fn chat_completions_url(
    provider: &str,
    base_url: &str,
) -> Result<String, ProviderError> {
    let base = trim_base(base_url);
    if base.is_empty() {
        return Err(
            ProviderError::config(format!("{provider} requires a configured base URL"))
                .with_provider(provider),
        );
    }
    Ok(format!("{base}/chat/completions"))
}

/// The model actually sent (config wins, request fills gaps).
pub(crate) fn request_model(config: &RemoteProviderConfig, request: &CompletionRequest) -> String {
    if config.model.trim().is_empty() {
        request.model.clone()
    } else {
        config.model.clone()
    }
}

/// Build the OpenAI `messages` array (system first, then the conversation).
pub(crate) fn build_messages(request: &CompletionRequest) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = effective_system_prompt(request)
        && !system.is_empty()
    {
        messages.push(json!({ "role": Role::System.as_str(), "content": system }));
    }
    for message in conversation(request) {
        messages.push(json!({ "role": message.role.as_str(), "content": message.content }));
    }
    let has_non_system = messages
        .iter()
        .any(|message| message.get("role").and_then(Value::as_str) != Some(Role::System.as_str()));
    if !has_non_system {
        messages.push(json!({
            "role": Role::User.as_str(),
            "content": request.last_user_message().unwrap_or_default(),
        }));
    }
    messages
}

/// Build an OpenAI Chat Completions request body.
pub(crate) fn build_body(
    config: &RemoteProviderConfig,
    request: &CompletionRequest,
    schema: Option<&Value>,
    stream: bool,
    model: &str,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": build_messages(request),
        "stream": stream,
        "temperature": config.temperature.unwrap_or(OPENAI_CHAT_DEFAULT_TEMPERATURE),
    });

    if let Some(schema) = schema {
        body["tools"] = json!([{
            "type": "function",
            "function": {
                "name": OPENAI_CHAT_STRUCTURED_TOOL,
                "description": "Return the result as a single JSON object matching the schema.",
                "parameters": schema,
            },
        }]);
        body["tool_choice"] = if config.force_tool_choice {
            json!("required")
        } else {
            json!("auto")
        };
    }

    apply_safe_extra_params(&mut body, &config.extra_params);
    body
}

/// Extract the streamed text delta from one Chat Completions SSE payload.
pub(crate) fn extract_delta(event: &str) -> Option<String> {
    let value: Value = serde_json::from_str(event).ok()?;
    value
        .get("choices")
        .and_then(Value::as_array)?
        .first()
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Detect the `{ "status": "...", "msg": "..." }` error envelope some
/// gateways return with a 200 status.
pub(crate) fn error_envelope(value: &Value) -> Option<String> {
    let status = value.get("status").and_then(Value::as_str)?;
    let message = value.get("msg").and_then(Value::as_str)?;
    Some(format!("{status}: {message}"))
}

/// Extract a structured value from a Chat Completions response body.
pub(crate) fn extract_structured(provider: &str, value: &Value) -> Result<Value, ProviderError> {
    if let Some(message) = error_envelope(value) {
        return Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            format!("provider reported an error body: {message}"),
        )
        .with_provider(provider));
    }

    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"));

    if let Some(message) = choice {
        if let Some(tool_call) = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .and_then(|calls| calls.first())
            .and_then(|call| call.get("function"))
        {
            if let Some(arguments) = tool_call.get("arguments").and_then(Value::as_str) {
                let mut accumulator = StructuredAccumulator::new();
                accumulator.push(arguments);
                return accumulator.finish(provider);
            }
            if let Some(arguments) = tool_call.get("arguments")
                && (arguments.is_object() || arguments.is_array())
            {
                return Ok(arguments.clone());
            }
        }
        if let Some(content) = message.get("content").and_then(Value::as_str)
            && !content.trim().is_empty()
        {
            return parse_structured(provider, content);
        }
    }

    Err(ProviderError::new(
        ProviderErrorKind::Protocol,
        "provider response carried no structured content",
    )
    .with_provider(provider))
}

/// Shared streaming body for every OpenAI-compatible adapter.
pub(crate) fn complete_with(
    inner: ProviderInner,
    url: String,
    model: String,
    request: CompletionRequest,
) -> CompletionStream {
    let body = build_body(inner.config(), &request, None, true, &model);
    completion_from_sse(
        inner,
        url,
        body,
        request.cancellation.clone(),
        request.timeout,
        extract_delta,
    )
}

/// Shared structured body for every OpenAI-compatible adapter.
pub(crate) async fn structured_with(
    inner: &ProviderInner,
    provider: &str,
    url: String,
    model: String,
    request: CompletionRequest,
    json_schema: Value,
) -> Result<Value, ProviderError> {
    let body = build_body(inner.config(), &request, Some(&json_schema), false, &model);
    let value = inner
        .post_json(&url, body, request.timeout, &request.cancellation)
        .await?;
    extract_structured(provider, &value)
}

/// OpenAI Chat Completions provider.
pub struct OpenAiChatProvider {
    inner: ProviderInner,
}

impl OpenAiChatProvider {
    /// Build the provider over an injected transport + resolver.
    pub fn new(
        config: RemoteProviderConfig,
        resolver: Arc<dyn SecretResolver>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            inner: build_inner(PROVIDER_NAME, config, resolver, transport),
        }
    }

    /// Build the provider with a real `reqwest` transport.
    pub fn with_reqwest(config: RemoteProviderConfig, resolver: Arc<dyn SecretResolver>) -> Self {
        Self::new(config, resolver, Arc::new(ReqwestTransport::new()))
    }

    /// Build the provider from an already-configured client (test seam).
    pub fn with_client(config: RemoteProviderConfig, client: Arc<RemoteHttpClient>) -> Self {
        Self {
            inner: inner_with_client(PROVIDER_NAME, config, client),
        }
    }

    /// The active config.
    pub fn config(&self) -> &RemoteProviderConfig {
        self.inner.config()
    }

    /// The resolved chat-completions endpoint.
    pub fn resolve_endpoint(&self) -> Result<String, ProviderError> {
        chat_completions_url(PROVIDER_NAME, &self.inner.config().base_url)
    }

    /// The model id this provider sends.
    pub fn resolve_model(&self) -> String {
        self.inner.config().model.clone()
    }

    /// The provider tag used for sessions/diagnostics.
    pub fn provider_tag(&self) -> &str {
        PROVIDER_NAME
    }
}

#[async_trait]
impl AiProvider for OpenAiChatProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let url = match self.resolve_endpoint() {
            Ok(url) => url,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let model = request_model(self.inner.config(), &request);
        complete_with(self.inner.clone(), url, model, request)
    }

    async fn embed(&self, _request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unsupported,
            "openai-chat provider does not expose embeddings",
        )
        .with_provider(PROVIDER_NAME))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut info = ModelInfo::new(self.resolve_model(), PROVIDER_NAME);
        info.supports_structured_output = true;
        Ok(vec![info])
    }

    async fn count_tokens(&self, _model: &str, text: &str) -> Result<u32, ProviderError> {
        Ok(estimate_token_count(text))
    }
}

impl sealed::Sealed for OpenAiChatProvider {}

#[async_trait]
impl AiProviderStructured for OpenAiChatProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let url = self.resolve_endpoint()?;
        let model = request_model(self.inner.config(), &request);
        structured_with(&self.inner, PROVIDER_NAME, url, model, request, json_schema).await
    }

    async fn supports_structured_output(&self, _model: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::request::ChatMessage;

    #[test]
    fn endpoint_appends_chat_completions() {
        assert_eq!(
            chat_completions_url(PROVIDER_NAME, "https://api.openai.com/v1/").unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url(PROVIDER_NAME, "").unwrap_err().kind,
            ProviderErrorKind::Config
        );
    }

    #[test]
    fn body_carries_default_temperature_and_tool_choice() {
        let config = config("gpt-4o-mini");
        let request = CompletionRequest::new("gpt-4o-mini").with_message(ChatMessage::user("hi"));
        let schema = json!({ "type": "object" });
        let body = build_body(&config, &request, Some(&schema), false, "gpt-4o-mini");
        assert_eq!(body["temperature"], OPENAI_CHAT_DEFAULT_TEMPERATURE);
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(
            body["tools"][0]["function"]["name"],
            OPENAI_CHAT_STRUCTURED_TOOL
        );
    }

    #[test]
    fn extract_delta_reads_first_choice_content() {
        let event = r#"{"choices":[{"delta":{"content":"hey"}}]}"#;
        assert_eq!(extract_delta(event), Some("hey".to_owned()));
        let done = r#"{"choices":[{"delta":{}}]}"#;
        assert_eq!(extract_delta(done), None);
    }

    #[test]
    fn extract_structured_prefers_tool_call_arguments() {
        let value = json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": OPENAI_CHAT_STRUCTURED_TOOL, "arguments": "{\"memory\":\"captured fact\"}" }
                    }]
                }
            }]
        });
        assert_eq!(
            extract_structured(PROVIDER_NAME, &value).unwrap()["memory"],
            "captured fact"
        );
    }

    #[test]
    fn extract_structured_falls_back_to_content_json() {
        let value = json!({
            "choices": [{
                "message": { "content": "```json\n{\"a\":1}\n```" }
            }]
        });
        assert_eq!(extract_structured(PROVIDER_NAME, &value).unwrap()["a"], 1);
    }

    #[test]
    fn error_envelope_is_detected() {
        let value = json!({ "status": "error", "msg": "bad key" });
        let error = extract_structured(PROVIDER_NAME, &value).expect_err("error body");
        assert_eq!(error.kind, ProviderErrorKind::Protocol);
        assert!(error.message.contains("bad key"));
    }
}
