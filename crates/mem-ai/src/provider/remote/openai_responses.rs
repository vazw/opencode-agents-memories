//! OpenAI Responses API adapter.
//!
//! Wire: `POST {base}/responses`, `Authorization: Bearer <secret>`. Streaming
//! parses `response.output_text.delta` SSE events; structured output asks for a
//! `json_schema` text format and reads the concatenated `output_text` items.

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
use crate::provider::remote::structured::parse_structured;
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};
use crate::provider::secret_resolver::SecretResolver;

/// Stable provider name.
pub const PROVIDER_NAME: &str = "openai-responses";

/// OpenAI's API base URL.
pub const OPENAI_RESPONSES_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// The `json_schema` format name used for structured output.
pub const OPENAI_RESPONSES_STRUCTURED_FORMAT: &str = "structured_output";

/// Typed config for the OpenAI responses adapter.
pub type OpenAiResponsesConfig = RemoteProviderConfig;

/// A default OpenAI responses config for `model`.
pub fn config(model: impl Into<String>) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model).with_base_url(OPENAI_RESPONSES_DEFAULT_BASE_URL)
}

fn static_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_owned(), CONTENT_TYPE_JSON.to_owned());
    headers
}

fn auth_strategy(config: &RemoteProviderConfig) -> AuthStrategy {
    AuthStrategy::Bearer {
        secret_ref: config.api_key_ref.clone(),
    }
}

fn responses_url(provider: &str, base_url: &str) -> Result<String, ProviderError> {
    let base = trim_base(base_url);
    if base.is_empty() {
        return Err(
            ProviderError::config(format!("{provider} requires a configured base URL"))
                .with_provider(provider),
        );
    }
    Ok(format!("{base}/responses"))
}

fn request_model(config: &RemoteProviderConfig, request: &CompletionRequest) -> String {
    if config.model.trim().is_empty() {
        request.model.clone()
    } else {
        config.model.clone()
    }
}

/// The `input` string sent to the Responses API.
fn build_input(request: &CompletionRequest) -> String {
    let parts: Vec<&str> = conversation(request)
        .map(|message| message.content.as_str())
        .collect();
    if parts.is_empty() {
        return request.last_user_message().unwrap_or_default().to_owned();
    }
    parts.join("\n")
}

/// Build a Responses API request body.
fn build_body(
    config: &RemoteProviderConfig,
    request: &CompletionRequest,
    schema: Option<&Value>,
    stream: bool,
    model: &str,
) -> Value {
    let mut body = json!({
        "model": model,
        "input": build_input(request),
        "stream": stream,
    });
    if let Some(system) = effective_system_prompt(request)
        && !system.is_empty()
    {
        body["instructions"] = json!(system);
    }
    if let Some(schema) = schema {
        body["text"] = json!({
            "format": {
                "type": "json_schema",
                "name": OPENAI_RESPONSES_STRUCTURED_FORMAT,
                "schema": schema,
                "strict": true,
            }
        });
    }
    apply_safe_extra_params(&mut body, &config.extra_params);
    body
}

/// Extract a text delta from one Responses API SSE payload.
fn extract_delta(event: &str) -> Option<String> {
    let value: Value = serde_json::from_str(event).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("response.output_text.delta") {
        return None;
    }
    value
        .get("delta")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Concatenate the `output_text` items from a Responses API body.
fn output_text(value: &Value) -> String {
    if let Some(text) = value.get("output_text").and_then(Value::as_str)
        && !text.trim().is_empty()
    {
        return text.to_owned();
    }
    let mut out = String::new();
    let Some(items) = value.get("output").and_then(Value::as_array) else {
        return out;
    };
    for item in items {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }
    }
    out
}

/// Extract a structured value from a Responses API body.
fn extract_structured(provider: &str, value: &Value) -> Result<Value, ProviderError> {
    // A function_call item (when the caller forced tools) carries arguments.
    if let Some(items) = value.get("output").and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("function_call")
                && let Some(arguments) = item.get("arguments").and_then(Value::as_str)
            {
                return parse_structured(provider, arguments);
            }
        }
    }
    let text = output_text(value);
    if text.trim().is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            "openai-responses response carried no output text",
        )
        .with_provider(provider));
    }
    parse_structured(provider, &text)
}

/// OpenAI Responses provider.
pub struct OpenAiResponsesProvider {
    inner: ProviderInner,
}

impl OpenAiResponsesProvider {
    /// Build the provider over an injected transport + resolver.
    pub fn new(
        config: RemoteProviderConfig,
        resolver: Arc<dyn SecretResolver>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        let client = Arc::new(RemoteHttpClient::new(PROVIDER_NAME, transport, resolver));
        let auth = auth_strategy(&config);
        Self {
            inner: ProviderInner::new(PROVIDER_NAME, config, client, static_headers(), auth),
        }
    }

    /// Build the provider with a real `reqwest` transport.
    pub fn with_reqwest(config: RemoteProviderConfig, resolver: Arc<dyn SecretResolver>) -> Self {
        Self::new(config, resolver, Arc::new(ReqwestTransport::new()))
    }

    /// Build the provider from an already-configured client (test seam).
    pub fn with_client(config: RemoteProviderConfig, client: Arc<RemoteHttpClient>) -> Self {
        let auth = auth_strategy(&config);
        Self {
            inner: ProviderInner::new(PROVIDER_NAME, config, client, static_headers(), auth),
        }
    }

    /// The active config.
    pub fn config(&self) -> &RemoteProviderConfig {
        self.inner.config()
    }

    /// The resolved Responses endpoint.
    pub fn resolve_endpoint(&self) -> Result<String, ProviderError> {
        responses_url(PROVIDER_NAME, &self.inner.config().base_url)
    }

    /// The model id this provider sends.
    pub fn resolve_model(&self) -> String {
        self.inner.config().model.clone()
    }
}

#[async_trait]
impl AiProvider for OpenAiResponsesProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let url = match self.resolve_endpoint() {
            Ok(url) => url,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let model = request_model(self.inner.config(), &request);
        let body = build_body(self.inner.config(), &request, None, true, &model);
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
            "openai-responses provider does not expose embeddings",
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

impl sealed::Sealed for OpenAiResponsesProvider {}

#[async_trait]
impl AiProviderStructured for OpenAiResponsesProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let url = self.resolve_endpoint()?;
        let model = request_model(self.inner.config(), &request);
        let body = build_body(
            self.inner.config(),
            &request,
            Some(&json_schema),
            false,
            &model,
        );
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
    use crate::provider::request::ChatMessage;

    #[test]
    fn endpoint_appends_responses() {
        assert_eq!(
            responses_url(PROVIDER_NAME, "https://api.openai.com/v1/").unwrap(),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn structured_body_requests_json_schema_format() {
        let config = config("gpt-5");
        let request = CompletionRequest::new("gpt-5").with_message(ChatMessage::user("hi"));
        let schema = json!({ "type": "object" });
        let body = build_body(&config, &request, Some(&schema), false, "gpt-5");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(
            body["text"]["format"]["name"],
            OPENAI_RESPONSES_STRUCTURED_FORMAT
        );
        assert_eq!(body["text"]["format"]["schema"], schema);
    }

    #[test]
    fn extract_delta_reads_output_text_delta() {
        let event = r#"{"type":"response.output_text.delta","delta":"hi"}"#;
        assert_eq!(extract_delta(event), Some("hi".to_owned()));
        let started = r#"{"type":"response.created"}"#;
        assert_eq!(extract_delta(started), None);
    }

    #[test]
    fn extract_structured_reads_output_text_items() {
        let value = json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "{\"memory\":\"captured fact\"}" }]
            }]
        });
        assert_eq!(
            extract_structured(PROVIDER_NAME, &value).unwrap()["memory"],
            "captured fact"
        );
    }
}
