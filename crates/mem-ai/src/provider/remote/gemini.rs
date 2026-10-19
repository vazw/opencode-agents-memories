//! Google Gemini adapter.
//!
//! Wire: `POST {base}/models/{model}:generateContent` (non-stream) or
//! `:streamGenerateContent?alt=sse` (stream). Streaming parses SSE
//! `candidates[0].content.parts[].text`; structured output sets
//! `generationConfig.responseMimeType`/`responseSchema`.
//!
//! Security divergence from upstream: the API key travels in the
//! `x-goog-api-key` header rather than a `?key=` query parameter, so resolved
//! secrets never appear in URLs (which are far more likely to be logged).

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
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo, Role};
use crate::provider::secret_resolver::SecretResolver;

/// Stable provider name.
pub const PROVIDER_NAME: &str = "google-gemini";

/// Google AI Studio API base URL.
pub const GEMINI_DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Default model.
pub const GEMINI_DEFAULT_MODEL: &str = "gemini-1.5-flash";

/// Default sampling temperature.
pub const GEMINI_DEFAULT_TEMPERATURE: f32 = 0.3;

/// Typed config for the Gemini adapter.
pub type GeminiConfig = RemoteProviderConfig;

/// A default Gemini config for `model`.
pub fn config(model: impl Into<String>) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model).with_base_url(GEMINI_DEFAULT_BASE_URL)
}

fn static_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_owned(), CONTENT_TYPE_JSON.to_owned());
    headers
}

fn auth_strategy(config: &RemoteProviderConfig) -> AuthStrategy {
    AuthStrategy::ApiKey {
        header: "x-goog-api-key".to_owned(),
        secret_ref: config.api_key_ref.clone(),
    }
}

fn base_url(config: &RemoteProviderConfig) -> Result<String, ProviderError> {
    let base = trim_base(&config.base_url);
    if base.is_empty() {
        return Err(
            ProviderError::config("google-gemini requires a configured base URL")
                .with_provider(PROVIDER_NAME),
        );
    }
    Ok(base.to_owned())
}

/// Map a chat role onto the Gemini `user`/`model` role.
fn gemini_role(role: Role) -> &'static str {
    match role {
        Role::Assistant => "model",
        _ => "user",
    }
}

/// Build the Gemini `contents` array.
fn build_contents(request: &CompletionRequest) -> Vec<Value> {
    let mut contents: Vec<Value> = conversation(request)
        .map(|message| {
            json!({
                "role": gemini_role(message.role),
                "parts": [{ "text": message.content }],
            })
        })
        .collect();
    if contents.is_empty() {
        contents.push(json!({
            "role": "user",
            "parts": [{ "text": request.last_user_message().unwrap_or_default() }],
        }));
    }
    contents
}

/// Build a Gemini request body.
fn build_body(
    config: &RemoteProviderConfig,
    request: &CompletionRequest,
    schema: Option<&Value>,
) -> Value {
    let mut generation_config = json!({
        "temperature": config.temperature.unwrap_or(GEMINI_DEFAULT_TEMPERATURE),
    });
    if let Some(schema) = schema {
        generation_config["responseMimeType"] = json!("application/json");
        generation_config["responseSchema"] = schema.clone();
    }

    let mut body = json!({
        "contents": build_contents(request),
        "generationConfig": generation_config,
    });
    if let Some(system) = effective_system_prompt(request)
        && !system.is_empty()
    {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    apply_safe_extra_params(&mut body, &config.extra_params);
    body
}

/// Join the text parts of the first candidate.
fn candidate_text(value: &Value) -> Option<String> {
    let parts = value
        .get("candidates")?
        .as_array()?
        .first()?
        .get("content")?
        .get("parts")?
        .as_array()?;
    let mut text = String::new();
    for part in parts {
        if let Some(piece) = part.get("text").and_then(Value::as_str) {
            text.push_str(piece);
        }
    }
    if text.is_empty() { None } else { Some(text) }
}

/// Extract a text delta from one Gemini SSE payload.
fn extract_delta(event: &str) -> Option<String> {
    let value: Value = serde_json::from_str(event).ok()?;
    candidate_text(&value)
}

/// Extract a structured value from a Gemini response body.
fn extract_structured(provider: &str, value: &Value) -> Result<Value, ProviderError> {
    // A forced function call (when tools are used) carries structured args.
    if let Some(parts) = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(args) = part.get("functionCall").and_then(|call| call.get("args"))
                && (args.is_object() || args.is_array())
            {
                return Ok(args.clone());
            }
        }
    }
    match candidate_text(value) {
        Some(text) if !text.trim().is_empty() => parse_structured(provider, &text),
        _ => Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            "gemini response carried no content",
        )
        .with_provider(provider)),
    }
}

/// Google Gemini provider.
pub struct GeminiProvider {
    inner: ProviderInner,
}

impl GeminiProvider {
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

    /// The model id this provider sends (defaults to `gemini-1.5-flash`).
    pub fn resolve_model(&self) -> String {
        let model = self.inner.config().model.trim();
        if model.is_empty() {
            GEMINI_DEFAULT_MODEL.to_owned()
        } else {
            model.to_owned()
        }
    }

    /// The `generateContent` endpoint (non-streaming).
    pub fn resolve_endpoint(&self) -> Result<String, ProviderError> {
        let base = base_url(self.inner.config())?;
        Ok(format!(
            "{base}/models/{}:generateContent",
            self.resolve_model()
        ))
    }

    /// The streaming `streamGenerateContent` endpoint.
    pub fn resolve_stream_endpoint(&self) -> Result<String, ProviderError> {
        let base = base_url(self.inner.config())?;
        Ok(format!(
            "{base}/models/{}:streamGenerateContent?alt=sse",
            self.resolve_model()
        ))
    }
}

#[async_trait]
impl AiProvider for GeminiProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let url = match self.resolve_stream_endpoint() {
            Ok(url) => url,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let body = build_body(self.inner.config(), &request, None);
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
            "google-gemini provider does not expose embeddings",
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

impl sealed::Sealed for GeminiProvider {}

#[async_trait]
impl AiProviderStructured for GeminiProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let base = base_url(self.inner.config())?;
        let url = format!("{base}/models/{}:generateContent", self.resolve_model());
        let body = build_body(self.inner.config(), &request, Some(&json_schema));
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
    fn endpoints_use_generate_content_paths() {
        let provider = GeminiProvider::with_client(
            config("gemini-1.5-flash"),
            Arc::new(RemoteHttpClient::new(
                PROVIDER_NAME,
                Arc::new(ReqwestTransport::new()),
                Arc::new(crate::provider::secret_resolver::StaticSecretResolver::new()),
            )),
        );
        assert_eq!(
            provider.resolve_endpoint().unwrap(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:generateContent"
        );
        assert_eq!(
            provider.resolve_stream_endpoint().unwrap(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn structured_body_sets_response_schema() {
        let config = config("gemini-1.5-flash");
        let request =
            CompletionRequest::new("gemini-1.5-flash").with_message(ChatMessage::user("hi"));
        let schema = json!({ "type": "object" });
        let body = build_body(&config, &request, Some(&schema));
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(body["generationConfig"]["responseSchema"], schema);
    }

    #[test]
    fn contents_map_assistant_to_model_role() {
        let request = CompletionRequest::new("gemini-1.5-flash")
            .with_message(ChatMessage::user("hi"))
            .with_message(ChatMessage::assistant("hello"));
        let contents = build_contents(&request);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
    }

    #[test]
    fn extract_delta_joins_candidate_text_parts() {
        let event = r#"{"candidates":[{"content":{"parts":[{"text":"he"},{"text":"y"}]}}]}"#;
        assert_eq!(extract_delta(event), Some("hey".to_owned()));
    }

    #[test]
    fn extract_structured_reads_candidate_text() {
        let value = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "{\"memory\":\"captured fact\"}" }] }
            }]
        });
        assert_eq!(
            extract_structured(PROVIDER_NAME, &value).unwrap()["memory"],
            "captured fact"
        );
    }
}
