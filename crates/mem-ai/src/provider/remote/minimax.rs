//! MiniMax adapter.
//!
//! MiniMax exposes an Anthropic Messages-compatible endpoint for its text
//! models. This adapter reuses the Anthropic wire helpers and only overrides
//! the resolved endpoint (`<base>/anthropic/v1/messages`) and the provider
//! tag, so MiniMax stays distinguishable from Anthropic.
//!
//! The base URL is normalised so a user may supply the host with or without a
//! trailing `/anthropic`, `/anthropic/v1`, or `/anthropic/v1/messages` suffix.
//! Unlike Anthropic, MiniMax has no default endpoint: an empty base URL is a
//! configuration error.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::opencode_host::estimate_token_count;
use crate::provider::provider_trait::{AiProvider, AiProviderStructured, CompletionStream, sealed};
use crate::provider::remote::anthropic;
use crate::provider::remote::plumbing::{
    HttpTransport, ProviderInner, RemoteHttpClient, RemoteProviderConfig, ReqwestTransport,
    completion_from_sse,
};
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};
use crate::provider::secret_resolver::SecretResolver;

/// Stable provider name.
pub const PROVIDER_NAME: &str = "minimax";

/// Re-exported for registry parity.
pub const MINIMAX_PROVIDER_NAME: &str = PROVIDER_NAME;

/// Typed config for the MiniMax adapter.
pub type MinimaxConfig = RemoteProviderConfig;

/// A MiniMax config for `model` (base URL must be supplied by the caller).
pub fn config(model: impl Into<String>) -> RemoteProviderConfig {
    RemoteProviderConfig::new(model)
}

/// Normalise a configured base URL to the MiniMax Messages endpoint.
pub fn normalize_base(base: &str) -> Result<String, ProviderError> {
    let mut normalised = base.trim().trim_end_matches('/').to_owned();
    if normalised.is_empty() {
        return Err(
            ProviderError::config("MiniMax provider requires a configured base URL")
                .with_provider(PROVIDER_NAME),
        );
    }
    for suffix in ["/anthropic/v1/messages", "/anthropic/v1", "/anthropic"] {
        if let Some(stripped) = normalised.strip_suffix(suffix) {
            normalised = stripped.trim_end_matches('/').to_owned();
            break;
        }
    }
    if normalised.is_empty() {
        return Err(
            ProviderError::config("MiniMax provider requires a host in its base URL")
                .with_provider(PROVIDER_NAME),
        );
    }
    Ok(format!("{normalised}/anthropic/v1/messages"))
}

/// MiniMax provider (Anthropic Messages wire format).
pub struct MinimaxProvider {
    inner: ProviderInner,
}

impl MinimaxProvider {
    /// Build the provider over an injected transport + resolver.
    pub fn new(
        config: RemoteProviderConfig,
        resolver: Arc<dyn SecretResolver>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            inner: build_inner(config, resolver, transport),
        }
    }

    /// Build the provider with a real `reqwest` transport.
    pub fn with_reqwest(config: RemoteProviderConfig, resolver: Arc<dyn SecretResolver>) -> Self {
        Self::new(config, resolver, Arc::new(ReqwestTransport::new()))
    }

    /// Build the provider from an already-configured client (test seam).
    pub fn with_client(config: RemoteProviderConfig, client: Arc<RemoteHttpClient>) -> Self {
        Self {
            inner: anthropic::inner_with_client(PROVIDER_NAME, config, client),
        }
    }

    /// The active config.
    pub fn config(&self) -> &RemoteProviderConfig {
        self.inner.config()
    }

    /// The resolved MiniMax Messages endpoint.
    pub fn resolve_endpoint(&self) -> Result<String, ProviderError> {
        normalize_base(&self.inner.config().base_url)
    }

    /// The model id this provider sends.
    pub fn resolve_model(&self) -> String {
        self.inner.config().model.clone()
    }
}

fn build_inner(
    config: RemoteProviderConfig,
    resolver: Arc<dyn SecretResolver>,
    transport: Arc<dyn HttpTransport>,
) -> ProviderInner {
    let client = Arc::new(RemoteHttpClient::new(PROVIDER_NAME, transport, resolver));
    anthropic::inner_with_client(PROVIDER_NAME, config, client)
}

#[async_trait]
impl AiProvider for MinimaxProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let url = match self.resolve_endpoint() {
            Ok(url) => url,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let body = anthropic::build_body(self.inner.config(), &request, None, true);
        completion_from_sse(
            self.inner.clone(),
            url,
            body,
            request.cancellation.clone(),
            request.timeout,
            anthropic::extract_delta,
        )
    }

    async fn embed(&self, _request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unsupported,
            "minimax provider does not expose embeddings",
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

impl sealed::Sealed for MinimaxProvider {}

#[async_trait]
impl AiProviderStructured for MinimaxProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let url = self.resolve_endpoint()?;
        let body = anthropic::build_body(self.inner.config(), &request, Some(&json_schema), false);
        let value = self
            .inner
            .post_json(&url, body, request.timeout, &request.cancellation)
            .await?;
        anthropic::extract_structured(PROVIDER_NAME, &value)
    }

    async fn supports_structured_output(&self, _model: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_global_and_china_endpoints() {
        assert_eq!(
            normalize_base("https://api.minimax.io").unwrap(),
            "https://api.minimax.io/anthropic/v1/messages"
        );
        assert_eq!(
            normalize_base("https://api.minimaxi.com").unwrap(),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );
    }

    #[test]
    fn normalizes_suffixed_and_trailing_slash_bases() {
        for base in [
            "https://api.minimax.io/anthropic/v1",
            "https://api.minimax.io/anthropic/",
            "https://api.minimax.io/anthropic/v1/messages",
            "https://api.minimax.io/",
        ] {
            assert_eq!(
                normalize_base(base).unwrap(),
                "https://api.minimax.io/anthropic/v1/messages",
                "base {base}"
            );
        }
    }

    #[test]
    fn empty_base_is_a_configuration_error() {
        let error = normalize_base("").expect_err("empty base");
        assert_eq!(error.kind, ProviderErrorKind::Config);
    }
}
