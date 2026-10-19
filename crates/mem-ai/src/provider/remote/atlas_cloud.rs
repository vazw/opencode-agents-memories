//! Atlas Cloud adapter — an OpenAI-compatible gateway.
//!
//! [Atlas Cloud](https://www.atlascloud.ai) exposes an OpenAI-compatible Chat
//! Completions API. This adapter reuses the OpenAI chat-completion wire and
//! only overrides the resolved endpoint, the default model, and the provider
//! tag. `base_url`/`model` default to the Atlas endpoint and
//! `deepseek-ai/deepseek-v4-pro`, so a minimal config only needs a provider +
//! API key.

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
pub const PROVIDER_NAME: &str = "atlas-cloud";

/// Atlas Cloud OpenAI-compatible endpoint used when `base_url` is omitted.
pub const ATLAS_CLOUD_API_URL: &str = "https://api.atlascloud.ai/v1";

/// Default model when no model is configured.
pub const ATLAS_CLOUD_DEFAULT_MODEL: &str = "deepseek-ai/deepseek-v4-pro";

/// Typed config for the Atlas Cloud adapter.
pub type AtlasCloudConfig = RemoteProviderConfig;

/// A default Atlas Cloud config for `model` (empty → provider default).
pub fn config(model: impl Into<String>) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model)
}

/// Atlas Cloud provider.
pub struct AtlasCloudProvider {
    inner: ProviderInner,
}

impl AtlasCloudProvider {
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

    /// The resolved chat-completions endpoint (defaults to Atlas Cloud).
    pub fn resolve_endpoint(&self) -> Result<String, ProviderError> {
        let base = trim_base(&self.inner.config().base_url);
        let base = if base.is_empty() {
            ATLAS_CLOUD_API_URL
        } else {
            base
        };
        openai_chat::chat_completions_url(PROVIDER_NAME, base)
    }

    /// The resolved model (defaults to `deepseek-ai/deepseek-v4-pro`).
    pub fn resolve_model(&self) -> String {
        let model = self.inner.config().model.trim();
        if model.is_empty() {
            ATLAS_CLOUD_DEFAULT_MODEL.to_owned()
        } else {
            model.to_owned()
        }
    }
}

#[async_trait]
impl AiProvider for AtlasCloudProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let url = match self.resolve_endpoint() {
            Ok(url) => url,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        openai_chat::complete_with(self.inner.clone(), url, self.resolve_model(), request)
    }

    async fn embed(&self, _request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unsupported,
            "atlas-cloud provider does not expose embeddings",
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

impl sealed::Sealed for AtlasCloudProvider {}

#[async_trait]
impl AiProviderStructured for AtlasCloudProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let url = self.resolve_endpoint()?;
        openai_chat::structured_with(
            &self.inner,
            PROVIDER_NAME,
            url,
            self.resolve_model(),
            request,
            json_schema,
        )
        .await
    }

    async fn supports_structured_output(&self, _model: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_endpoint_and_model() {
        let provider = AtlasCloudProvider::with_client(
            config(""),
            Arc::new(RemoteHttpClient::new(
                PROVIDER_NAME,
                Arc::new(crate::provider::remote::plumbing::ReqwestTransport::new()),
                Arc::new(crate::provider::secret_resolver::StaticSecretResolver::new()),
            )),
        );
        assert_eq!(
            provider.resolve_endpoint().unwrap(),
            "https://api.atlascloud.ai/v1/chat/completions"
        );
        assert_eq!(provider.resolve_model(), ATLAS_CLOUD_DEFAULT_MODEL);
    }

    #[test]
    fn trailing_slash_is_stripped() {
        let provider = AtlasCloudProvider::with_client(
            config("qwen/qwen3-coder").with_base_url("https://proxy.example.com/v1/"),
            Arc::new(RemoteHttpClient::new(
                PROVIDER_NAME,
                Arc::new(crate::provider::remote::plumbing::ReqwestTransport::new()),
                Arc::new(crate::provider::secret_resolver::StaticSecretResolver::new()),
            )),
        );
        assert_eq!(
            provider.resolve_endpoint().unwrap(),
            "https://proxy.example.com/v1/chat/completions"
        );
        assert_eq!(provider.resolve_model(), "qwen/qwen3-coder");
    }
}
