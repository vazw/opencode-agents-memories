//! `PrivacyFilterLayer` — the `AiProvider` middleware that deploys the three
//! filters, plus the in-crate structured-capability tests.
//!
//! # Semantics (D8 — request-side only)
//!
//! Filtering happens exactly once per **outbound** request: completion
//! `messages` and `system`, embed inputs, `count_tokens` text and the
//! structured request. Everything coming back — completion chunks (including
//! the `{ "json": … }` structured chunk), embed vectors, token counts, model
//! lists and structured return values — is forwarded verbatim; the privacy
//! boundary is "nothing we *send* passes unfiltered".
//!
//! Two failure/alignment rules ride along:
//!
//! - `complete` drops injected user messages and returns a single `Err` chunk
//!   when the request *had* messages and all were dropped, instead of sending
//!   an empty conversation. Assistant/system messages are never dropped (their
//!   text is still private-stripped).
//! - `embed` **never drops inputs**: index alignment with the returned vectors
//!   is an invariant, so a fully-private input stays in place as `[REDACTED]`.
//!   Storage-layer callers reject fully-private content *before* embedding.
//!
//! `model`, `json_schema`, `temperature`, `max_tokens`, `timeout`,
//! `cancellation` and `extra_params` pass through untouched.

use std::sync::Arc;

use async_trait::async_trait;

use crate::privacy::injection::InjectionFilter;
use crate::privacy::tags::strip_private_content;
use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::provider_trait::{AiProvider, AiProviderStructured, CompletionStream};
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo, Role};

/// Configuration for [`PrivacyFilterLayer`].
#[derive(Debug, Clone)]
pub struct PrivacyConfig {
    /// Replace `<private>…</private>` regions with `[REDACTED]`.
    pub strip_private: bool,
    /// Drop injected user messages.
    pub filter_injected_prompts: bool,
    /// Markers identifying machine-injected text.
    pub blocklist: Vec<String>,
    /// Markers exempt from the blocklist (the plugin's own prompts).
    pub allowlist: Vec<String>,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            strip_private: true,
            filter_injected_prompts: true,
            blocklist: crate::privacy::injection::DEFAULT_INJECTION_MARKERS
                .iter()
                .map(|marker| (*marker).to_owned())
                .collect(),
            allowlist: crate::privacy::injection::DEFAULT_INTERNAL_PROMPT_MARKERS
                .iter()
                .map(|marker| (*marker).to_owned())
                .collect(),
        }
    }
}

/// Wraps [`AiProvider`]s with the privacy filters.
#[derive(Debug, Clone, Default)]
pub struct PrivacyFilterLayer {
    config: PrivacyConfig,
}

impl PrivacyFilterLayer {
    /// A layer with the given configuration.
    pub fn new(config: PrivacyConfig) -> Self {
        Self { config }
    }

    /// The layer's configuration.
    pub fn config(&self) -> &PrivacyConfig {
        &self.config
    }

    /// Wrap a provider that has no structured-output capability.
    pub fn wrap(&self, inner: Arc<dyn AiProvider>) -> Arc<PrivacyFilterProvider> {
        Arc::new(PrivacyFilterProvider {
            inner,
            structured: None,
            config: self.config.clone(),
        })
    }

    /// Wrap a provider that does have structured output.
    ///
    /// Both handles are stored, so no trait upcasting is needed.
    pub fn wrap_structured<T: AiProviderStructured + 'static>(
        &self,
        inner: Arc<T>,
    ) -> Arc<PrivacyFilterProvider> {
        let base: Arc<dyn AiProvider> = inner.clone();
        let structured: Arc<dyn AiProviderStructured> = inner;
        Arc::new(PrivacyFilterProvider {
            inner: base,
            structured: Some(structured),
            config: self.config.clone(),
        })
    }
}

/// The middleware provider. Fields are private: it must behave exactly like the
/// provider it wraps, from the caller's point of view.
pub struct PrivacyFilterProvider {
    inner: Arc<dyn AiProvider>,
    structured: Option<Arc<dyn AiProviderStructured>>,
    config: PrivacyConfig,
}

impl std::fmt::Debug for PrivacyFilterProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivacyFilterProvider")
            .field("inner", &self.inner.name())
            .field("structured", &self.structured.is_some())
            .field("config", &self.config)
            .finish()
    }
}

impl PrivacyFilterProvider {
    /// The configuration in force.
    pub fn config(&self) -> &PrivacyConfig {
        &self.config
    }

    /// Apply the request-side rules: drop injected user messages, then strip
    /// private content from what survives, then touch nothing else.
    fn filter_request(&self, mut request: CompletionRequest) -> CompletionRequest {
        if self.config.filter_injected_prompts {
            let filter =
                InjectionFilter::new(self.config.blocklist.clone(), self.config.allowlist.clone());
            request
                .messages
                .retain(|message| message.role != Role::User || !filter.is_injected(message));
        }

        if self.config.strip_private {
            for message in &mut request.messages {
                message.content = strip_private_content(&message.content);
            }
            if let Some(system) = request.system.take() {
                request.system = Some(strip_private_content(&system));
            }
        }

        request
    }
}

#[async_trait]
impl AiProvider for PrivacyFilterProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let had_messages = !request.messages.is_empty();
        let filtered = self.filter_request(request);

        if had_messages && filtered.messages.is_empty() {
            // Never send an empty conversation: fail loudly, and do not call
            // the inner provider at all.
            return Box::pin(futures::stream::once(async {
                Err(ProviderError::config(
                    "all messages were filtered as injected",
                ))
            }));
        }

        // The returned stream is not inspected, buffered, reordered or
        // rewritten (D8).
        self.inner.complete(filtered)
    }

    async fn embed(&self, mut request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        if self.config.strip_private {
            for input in &mut request.input {
                *input = strip_private_content(input);
            }
        }
        // Inputs are never dropped or reordered: the caller zips the returned
        // vectors to them.
        self.inner.embed(request).await
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }

    async fn count_tokens(&self, model: &str, text: &str) -> Result<u32, ProviderError> {
        let filtered = if self.config.strip_private {
            strip_private_content(text)
        } else {
            text.to_owned()
        };
        self.inner.count_tokens(model, &filtered).await
    }
}

#[async_trait]
impl AiProviderStructured for PrivacyFilterProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let Some(structured) = &self.structured else {
            return Err(ProviderError::new(
                ProviderErrorKind::Unsupported,
                "privacy layer wraps a provider without structured output",
            ));
        };
        // The request is filtered; the returned value is not (D8).
        structured
            .complete_structured(self.filter_request(request), json_schema)
            .await
    }

    async fn supports_structured_output(&self, model: &str) -> Result<bool, ProviderError> {
        let Some(structured) = &self.structured else {
            return Err(ProviderError::new(
                ProviderErrorKind::Unsupported,
                "privacy layer wraps a provider without structured output",
            ));
        };
        structured.supports_structured_output(model).await
    }
}

impl crate::provider::provider_trait::sealed::Sealed for PrivacyFilterProvider {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::provider::provider_trait::sealed::Sealed;
    use crate::provider::request::{ChatMessage, Role};
    use async_trait::async_trait;
    use serde_json::json;

    const SECRET: &str = "sk-live-abc123";
    const INJECTED: &str = "<system-reminder>\n[BACKGROUND TASK RESULT READY]\n</system-reminder>";

    struct MockStructured {
        requests: Mutex<Vec<crate::provider::request::CompletionRequest>>,
        response: serde_json::Value,
        supports: bool,
    }

    impl Sealed for MockStructured {}

    #[async_trait]
    impl crate::provider::provider_trait::AiProvider for MockStructured {
        fn name(&self) -> &str {
            "mock-structured"
        }

        fn complete(
            &self,
            _request: crate::provider::request::CompletionRequest,
        ) -> crate::provider::provider_trait::CompletionStream {
            Box::pin(futures::stream::once(async {
                Ok(json!({"text": "unused"}))
            }))
        }

        async fn embed(
            &self,
            _request: crate::provider::request::EmbedRequest,
        ) -> Result<Vec<Vec<f32>>, crate::provider::error::ProviderError> {
            Ok(Vec::new())
        }

        async fn list_models(
            &self,
        ) -> Result<Vec<crate::provider::request::ModelInfo>, crate::provider::error::ProviderError>
        {
            Ok(Vec::new())
        }

        async fn count_tokens(
            &self,
            _model: &str,
            _text: &str,
        ) -> Result<u32, crate::provider::error::ProviderError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl crate::provider::provider_trait::AiProviderStructured for MockStructured {
        async fn complete_structured(
            &self,
            request: crate::provider::request::CompletionRequest,
            _json_schema: serde_json::Value,
        ) -> Result<serde_json::Value, crate::provider::error::ProviderError> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.clone())
        }

        async fn supports_structured_output(
            &self,
            _model: &str,
        ) -> Result<bool, crate::provider::error::ProviderError> {
            Ok(self.supports)
        }
    }

    #[tokio::test]
    async fn test_layer_structured_completion_is_filtered() {
        // (a) the request the mock receives is filtered; (b) the value the layer
        // returns is the mock's value, verbatim (D8: request-side only).
        let response = json!({ "note": format!("<private>{SECRET}</private>") });
        let inner = Arc::new(MockStructured {
            requests: Mutex::new(Vec::new()),
            response: response.clone(),
            supports: true,
        });
        let wrapped = PrivacyFilterLayer::default().wrap_structured(inner.clone());
        let request = crate::provider::request::CompletionRequest::new("m")
            .with_message(ChatMessage::user(format!(
                "hello <private>{SECRET}</private>"
            )))
            .with_message(ChatMessage::user(INJECTED));

        let returned = wrapped
            .complete_structured(request, json!({ "type": "object" }))
            .await
            .unwrap();

        let recorded = inner.requests.lock().unwrap();
        let recorded = recorded.last().expect("the mock must receive the request");
        assert_eq!(recorded.messages.len(), 1);
        assert_eq!(recorded.messages[0].content, "hello [REDACTED]");
        assert_eq!(recorded.messages[0].role, Role::User);
        assert_eq!(returned, response);
    }

    #[tokio::test]
    async fn test_layer_structured_absent_reports_unsupported() {
        // Wrapping must never silently disable a capability.
        let inner: Arc<dyn crate::provider::provider_trait::AiProvider> = Arc::new(PlainProvider);
        let wrapped = PrivacyFilterLayer::default().wrap(inner);

        let error = wrapped
            .complete_structured(
                crate::provider::request::CompletionRequest::new("m"),
                json!({ "type": "object" }),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::provider::error::ProviderErrorKind::Unsupported
        );
        assert_eq!(
            error.message,
            "privacy layer wraps a provider without structured output"
        );

        let error = wrapped.supports_structured_output("m").await.unwrap_err();
        assert_eq!(
            error.kind,
            crate::provider::error::ProviderErrorKind::Unsupported
        );
    }

    #[tokio::test]
    async fn test_layer_structured_capability_is_forwarded() {
        let inner = Arc::new(MockStructured {
            requests: Mutex::new(Vec::new()),
            response: json!({}),
            supports: true,
        });
        let wrapped = PrivacyFilterLayer::default().wrap_structured(inner);
        assert!(wrapped.supports_structured_output("m").await.unwrap());
    }

    struct PlainProvider;

    #[async_trait]
    impl crate::provider::provider_trait::AiProvider for PlainProvider {
        fn name(&self) -> &str {
            "plain"
        }

        fn complete(
            &self,
            _request: crate::provider::request::CompletionRequest,
        ) -> crate::provider::provider_trait::CompletionStream {
            Box::pin(futures::stream::once(async {
                Ok(json!({"text": "plain"}))
            }))
        }

        async fn embed(
            &self,
            _request: crate::provider::request::EmbedRequest,
        ) -> Result<Vec<Vec<f32>>, crate::provider::error::ProviderError> {
            Ok(Vec::new())
        }

        async fn list_models(
            &self,
        ) -> Result<Vec<crate::provider::request::ModelInfo>, crate::provider::error::ProviderError>
        {
            Ok(Vec::new())
        }

        async fn count_tokens(
            &self,
            _model: &str,
            _text: &str,
        ) -> Result<u32, crate::provider::error::ProviderError> {
            Ok(0)
        }
    }
}
