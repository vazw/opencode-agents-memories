//! OrcaRouter adapter — an OpenAI-compatible model gateway.
//!
//! [OrcaRouter](https://www.orcarouter.ai) is an OpenAI-compatible gateway that
//! rejects bare model names, so the model must be namespaced (e.g.
//! `orcarouter/auto`, `deepseek/deepseek-v4-flash`). This adapter reuses the
//! OpenAI chat-completion wire and only overrides the resolved endpoint, the
//! model resolution (validating the namespace), and the provider tag.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::opencode_host::estimate_token_count;
use crate::provider::provider_trait::{AiProvider, AiProviderStructured, CompletionStream, sealed};
use crate::provider::remote::anthropic::trim_base;
use crate::provider::remote::openai_chat;
use crate::provider::remote::plumbing::{
    HttpTransport, ProviderInner, RemoteHttpClient, RemoteProviderConfig, ReqwestTransport,
};
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};
use crate::provider::secret_resolver::SecretResolver;

/// Stable provider name.
pub const PROVIDER_NAME: &str = "orcarouter";

/// OrcaRouter gateway endpoint used when `base_url` is omitted.
pub const ORCAROUTER_API_URL: &str = "https://api.orcarouter.ai/v1";

/// Default model: the gateway's routing alias.
pub const ORCAROUTER_DEFAULT_MODEL: &str = "orcarouter/auto";

/// Typed config for the OrcaRouter adapter.
pub type OrcaRouterConfig = RemoteProviderConfig;

/// A default OrcaRouter config for `model` (empty → provider default).
pub fn config(model: impl Into<String>) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model)
}

/// OrcaRouter provider.
pub struct OrcaRouterProvider {
    inner: ProviderInner,
}

impl OrcaRouterProvider {
    /// Build the provider over an injected transport + resolver.
    pub fn new(
        config: RemoteProviderConfig,
        resolver: Arc<dyn SecretResolver>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            inner: openai_chat::build_inner(PROVIDER_NAME, config, resolver, transport),
        }
    }

    /// Build the provider with a real `reqwest` transport.
    pub fn with_reqwest(config: RemoteProviderConfig, resolver: Arc<dyn SecretResolver>) -> Self {
        Self::new(config, resolver, Arc::new(ReqwestTransport::new()))
    }

    /// Build the provider from an already-configured client (test seam).
    pub fn with_client(config: RemoteProviderConfig, client: Arc<RemoteHttpClient>) -> Self {
        Self {
            inner: openai_chat::inner_with_client(PROVIDER_NAME, config, client),
        }
    }

    /// The active config.
    pub fn config(&self) -> &RemoteProviderConfig {
        self.inner.config()
    }

    /// The resolved chat-completions endpoint (defaults to the gateway).
    pub fn resolve_endpoint(&self) -> Result<String, ProviderError> {
        let base = trim_base(&self.inner.config().base_url);
        let base = if base.is_empty() {
            ORCAROUTER_API_URL
        } else {
            base
        };
        openai_chat::chat_completions_url(PROVIDER_NAME, base)
    }

    /// The resolved model, defaulting to `orcarouter/auto`.
    ///
    /// A bare (unnamespaced) model is rejected with a helpful message rather
    /// than a gateway-side `model_not_found`.
    pub fn resolve_model(&self) -> Result<String, ProviderError> {
        let model = self.inner.config().model.trim();
        if model.is_empty() {
            return Ok(ORCAROUTER_DEFAULT_MODEL.to_owned());
        }
        if !model.contains('/') {
            return Err(ProviderError::config(format!(
                "OrcaRouter requires a namespaced memoryModel (e.g. \"orcarouter/auto\", \"openai/gpt-5.5\", \"deepseek/deepseek-v4-flash\"). Got: {model}"
            ))
            .with_provider(PROVIDER_NAME));
        }
        Ok(model.to_owned())
    }
}

#[async_trait]
impl AiProvider for OrcaRouterProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let url = match self.resolve_endpoint() {
            Ok(url) => url,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let model = match self.resolve_model() {
            Ok(model) => model,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        openai_chat::complete_with(self.inner.clone(), url, model, request)
    }

    async fn embed(&self, _request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unsupported,
            "orcarouter provider does not expose embeddings",
        )
        .with_provider(PROVIDER_NAME))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut info = ModelInfo::new(self.resolve_model()?, PROVIDER_NAME);
        info.supports_structured_output = true;
        Ok(vec![info])
    }

    async fn count_tokens(&self, _model: &str, text: &str) -> Result<u32, ProviderError> {
        Ok(estimate_token_count(text))
    }
}

impl sealed::Sealed for OrcaRouterProvider {}

#[async_trait]
impl AiProviderStructured for OrcaRouterProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let url = self.resolve_endpoint()?;
        let model = self.resolve_model()?;
        openai_chat::structured_with(&self.inner, PROVIDER_NAME, url, model, request, json_schema)
            .await
    }

    async fn supports_structured_output(&self, _model: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(model: &str, base: &str) -> OrcaRouterProvider {
        OrcaRouterProvider::with_client(
            config(model).with_base_url(base),
            Arc::new(RemoteHttpClient::new(
                PROVIDER_NAME,
                Arc::new(ReqwestTransport::new()),
                Arc::new(crate::provider::secret_resolver::StaticSecretResolver::new()),
            )),
        )
    }

    #[test]
    fn defaults_endpoint_and_model() {
        let provider = provider("", "");
        assert_eq!(
            provider.resolve_endpoint().unwrap(),
            "https://api.orcarouter.ai/v1/chat/completions"
        );
        assert_eq!(provider.resolve_model().unwrap(), ORCAROUTER_DEFAULT_MODEL);
    }

    #[test]
    fn trailing_slash_is_stripped_and_namespaced_model_kept() {
        let provider = provider(
            "deepseek/deepseek-v4-flash",
            "https://proxy.example.com/v1/",
        );
        assert_eq!(
            provider.resolve_endpoint().unwrap(),
            "https://proxy.example.com/v1/chat/completions"
        );
        assert_eq!(
            provider.resolve_model().unwrap(),
            "deepseek/deepseek-v4-flash"
        );
    }

    #[test]
    fn bare_model_is_rejected_with_a_hint() {
        let provider = provider("gpt-4o-mini", "");
        let error = provider.resolve_model().expect_err("bare model rejected");
        assert_eq!(error.kind, ProviderErrorKind::Config);
        assert!(error.message.contains("namespaced memoryModel"));
    }
}
