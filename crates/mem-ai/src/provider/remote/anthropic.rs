//! Anthropic Messages adapter — the exemplar remote provider.
//!
//! Speaks the Anthropic Messages wire format (`POST {base}/messages`,
//! `x-api-key` auth, `anthropic-version: 2023-06-01`) over the shared plumbing.
//! Streaming parses SSE `content_block_delta` events into `{ "text": ... }`
//! chunks; structured output injects the JSON schema as an Anthropic tool and
//! reads the `tool_use.input` block (falling back to JSON-in-text).
//!
//! The wire helpers here are `pub(crate)` so the Anthropic-compatible MiniMax
//! adapter reuses them verbatim.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::opencode_host::estimate_token_count;
use crate::provider::provider_trait::{AiProvider, AiProviderStructured, CompletionStream, sealed};
use crate::provider::remote::auth::AuthStrategy;
use crate::provider::remote::plumbing::{
    CONTENT_TYPE_JSON, HttpTransport, ProviderInner, RemoteHttpClient, RemoteProviderConfig,
    ReqwestTransport, apply_safe_extra_params, completion_from_sse, conversation,
    effective_system_prompt,
};
use crate::provider::remote::structured::parse_structured;
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};
use crate::provider::secret_resolver::SecretResolver;

/// Stable provider name.
pub const PROVIDER_NAME: &str = "anthropic";

/// Anthropic's API base URL.
pub const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";

/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic requires `max_tokens`; this is the upstream default.
pub const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 4096;

/// Tool name used to force structured output.
pub const ANTHROPIC_STRUCTURED_TOOL: &str = "structured_output";

/// Typed config for the Anthropic adapter.
pub type AnthropicConfig = RemoteProviderConfig;

/// A default Anthropic config for `model`.
pub fn config(model: impl Into<String>) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model).with_base_url(ANTHROPIC_DEFAULT_BASE_URL)
}

/// Trim a base URL and drop trailing slashes.
pub(crate) fn trim_base(base: &str) -> &str {
    base.trim().trim_end_matches('/')
}

/// The static headers every Anthropic-format request sends.
pub(crate) fn static_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_owned(), CONTENT_TYPE_JSON.to_owned());
    headers.insert("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned());
    headers
}

/// The `x-api-key` auth strategy for an Anthropic-format config.
pub(crate) fn auth_strategy(config: &RemoteProviderConfig) -> AuthStrategy {
    AuthStrategy::ApiKey {
        header: "x-api-key".to_owned(),
        secret_ref: config.api_key_ref.clone(),
    }
}

/// Build the shared [`ProviderInner`] for an Anthropic-format provider.
pub(crate) fn build_inner(
    provider: &str,
    config: RemoteProviderConfig,
    resolver: Arc<dyn SecretResolver>,
    transport: Arc<dyn HttpTransport>,
) -> ProviderInner {
    let client = Arc::new(RemoteHttpClient::new(provider, transport, resolver));
    inner_with_client(provider, config, client)
}

/// Build the shared inner state from an already-built client (test seam).
pub(crate) fn inner_with_client(
    provider: &str,
    config: RemoteProviderConfig,
    client: Arc<RemoteHttpClient>,
) -> ProviderInner {
    let auth = auth_strategy(&config);
    ProviderInner::new(provider, config, client, static_headers(), auth)
}

/// The model actually sent in the body (config wins, request fills gaps).
pub(crate) fn request_model(config: &RemoteProviderConfig, request: &CompletionRequest) -> String {
    if config.model.trim().is_empty() {
        request.model.clone()
    } else {
        config.model.clone()
    }
}

/// Build an Anthropic Messages request body.
///
/// `schema` is `Some` for structured output (the schema becomes a tool and the
/// tool call is forced); `stream` toggles SSE.
pub(crate) fn build_body(
    config: &RemoteProviderConfig,
    request: &CompletionRequest,
    schema: Option<&Value>,
    stream: bool,
) -> Value {
    let mut messages: Vec<Value> = conversation(request)
        .map(|message| json!({ "role": message.role.as_str(), "content": message.content }))
        .collect();
    if messages.is_empty() {
        messages.push(json!({
            "role": "user",
            "content": request.last_user_message().unwrap_or_default(),
        }));
    }

    let max_tokens = request
        .max_tokens
        .or(config.max_tokens)
        .unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS);

    let mut body = json!({
        "model": request_model(config, request),
        "max_tokens": max_tokens,
        "messages": messages,
    });

    if let Some(system) = effective_system_prompt(request)
        && !system.is_empty()
    {
        body["system"] = json!(system);
    }
    if stream {
        body["stream"] = json!(true);
    }
    if let Some(schema) = schema {
        body["tools"] = json!([{
            "name": ANTHROPIC_STRUCTURED_TOOL,
            "description": "Return the result as a single JSON object matching the schema.",
            "input_schema": schema,
        }]);
        body["tool_choice"] = json!({ "type": "tool", "name": ANTHROPIC_STRUCTURED_TOOL });
    }
    if let Some(temperature) = config.temperature {
        body["temperature"] = json!(temperature);
    }
    apply_safe_extra_params(&mut body, &config.extra_params);
    body
}

/// Extract a text delta from one Anthropic SSE `data:` payload.
pub(crate) fn extract_delta(event: &str) -> Option<String> {
    let value: Value = serde_json::from_str(event).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("content_block_delta") {
        return None;
    }
    value
        .get("delta")
        .and_then(|delta| delta.get("text"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Extract a structured value from an Anthropic response body.
///
/// Prefers the forced `tool_use.input` block; falls back to parsing the
/// concatenated text blocks as JSON.
pub(crate) fn extract_structured(provider: &str, value: &Value) -> Result<Value, ProviderError> {
    let content = value.get("content").and_then(Value::as_array);
    if let Some(blocks) = content {
        for block in blocks {
            if block.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(input) = block.get("input")
                && (input.is_object() || input.is_array())
            {
                return Ok(input.clone());
            }
        }
        let text: String = blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect();
        if !text.trim().is_empty() {
            return parse_structured(provider, &text);
        }
    }
    Err(ProviderError::new(
        ProviderErrorKind::Protocol,
        "anthropic response carried no structured content",
    )
    .with_provider(provider))
}

/// Anthropic Messages provider.
pub struct AnthropicProvider {
    inner: ProviderInner,
}

impl AnthropicProvider {
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

    /// The resolved Messages endpoint URL.
    pub fn resolve_endpoint(&self) -> Result<String, ProviderError> {
        let base = trim_base(&self.inner.config().base_url);
        if base.is_empty() {
            return Err(
                ProviderError::config("anthropic requires a configured base URL")
                    .with_provider(PROVIDER_NAME),
            );
        }
        Ok(format!("{base}/messages"))
    }

    /// The model id this provider sends.
    pub fn resolve_model(&self) -> String {
        self.inner.config().model.clone()
    }
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let url = match self.resolve_endpoint() {
            Ok(url) => url,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let body = build_body(self.inner.config(), &request, None, true);
        completion_from_sse(
            self.inner.clone(),
            url,
            body,
            request.cancellation.clone(),
            request.timeout,
            extract_delta,
        )
    }

    async fn embed(&self, _request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unsupported,
            "anthropic provider does not expose embeddings",
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

impl sealed::Sealed for AnthropicProvider {}

#[async_trait]
impl AiProviderStructured for AnthropicProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let url = self.resolve_endpoint()?;
        let body = build_body(self.inner.config(), &request, Some(&json_schema), false);
        let value = self
            .inner
            .post_json(&url, body, request.timeout, &request.cancellation)
            .await?;
        extract_structured(PROVIDER_NAME, &value)
    }

    async fn supports_structured_output(&self, _model: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_points_at_anthropic() {
        let config = config("claude-3-5-sonnet-latest");
        assert_eq!(config.base_url, ANTHROPIC_DEFAULT_BASE_URL);
        assert_eq!(
            config.timeout,
            crate::provider::remote::plumbing::DEFAULT_TIMEOUT
        );
    }

    #[test]
    fn body_always_carries_max_tokens() {
        let config = config("claude-3-5-sonnet-latest");
        let request = CompletionRequest::new("claude-3-5-sonnet-latest")
            .with_message(crate::provider::request::ChatMessage::user("hi"));
        let body = build_body(&config, &request, None, false);
        assert_eq!(body["max_tokens"], ANTHROPIC_DEFAULT_MAX_TOKENS);

        let config = RemoteProviderConfig {
            max_tokens: Some(2048),
            ..config
        };
        let body = build_body(&config, &request, None, false);
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn extra_params_do_not_clobber_model() {
        let mut config = config("claude-3-5-sonnet-latest");
        config
            .extra_params
            .insert("model".to_owned(), json!("overwritten"));
        config
            .extra_params
            .insert("thinking".to_owned(), json!({ "type": "adaptive" }));
        let request = CompletionRequest::new("claude-3-5-sonnet-latest")
            .with_message(crate::provider::request::ChatMessage::user("hi"));
        let body = build_body(&config, &request, None, false);
        assert_eq!(body["model"], "claude-3-5-sonnet-latest");
        assert_eq!(body["thinking"], json!({ "type": "adaptive" }));
    }

    #[test]
    fn structured_body_injects_a_forced_tool() {
        let config = config("claude-3-5-sonnet-latest");
        let request = CompletionRequest::new("claude-3-5-sonnet-latest")
            .with_message(crate::provider::request::ChatMessage::user("hi"));
        let schema = json!({ "type": "object", "properties": { "a": { "type": "string" } } });
        let body = build_body(&config, &request, Some(&schema), false);
        assert_eq!(body["tools"][0]["name"], ANTHROPIC_STRUCTURED_TOOL);
        assert_eq!(body["tools"][0]["input_schema"], schema);
        assert_eq!(body["tool_choice"]["type"], "tool");
    }

    #[test]
    fn extracts_text_delta_only_from_delta_events() {
        let event = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}"#;
        assert_eq!(extract_delta(event), Some("hi".to_owned()));
        let other = r#"{"type":"message_start"}"#;
        assert_eq!(extract_delta(other), None);
    }

    #[test]
    fn extracts_tool_use_input() {
        let value = json!({
            "content": [{
                "type": "tool_use",
                "name": ANTHROPIC_STRUCTURED_TOOL,
                "input": { "memory": "captured fact" }
            }]
        });
        assert_eq!(
            extract_structured(PROVIDER_NAME, &value).unwrap()["memory"],
            "captured fact"
        );
    }
}
