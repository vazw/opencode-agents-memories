//! The real AI-provider adapter for profile analysis (W3.2).
//!
//! `mem-ai`'s [`AiProviderStructured`] is sealed to that crate, so this module
//! splits the adapter at a local seam:
//!
//! - [`StructuredCompletion`] — one structured completion over
//!   (system prompt, user prompt, JSON Schema); the gateway talks to this, so
//!   tests can drive it with a recording fake;
//! - [`AiProviderCompletion`] — the production implementation over any
//!   `Arc<dyn AiProviderStructured>`;
//! - [`ProviderAnalysisGateway`] — the [`AnalysisGateway`] the learning round
//!   consumes, holding a native and an optional external completion.
//!
//! Transport ↔ prompt pairing is a property of the *transport*, never of the
//! caller: the native structured-output call always uses
//! [`AnalysisContext::system_prompt_json`], the external tool-call path always
//! uses [`AnalysisContext::system_prompt_tool`].

use std::sync::Arc;
use std::time::Duration;

use mem_ai::provider::{AiProviderStructured, ChatMessage, CompletionRequest};
use mem_store::db::BoxFuture;

use crate::analysis_prompt::analysis_schema;
use crate::learning_service::{AnalysisContext, AnalysisError, AnalysisGateway};

/// Upstream's sentence when the external path is reached with no external
/// provider configured (the round's `external_configured` guard should make
/// this branch unreachable from the service).
pub const EXTERNAL_UNCONFIGURED_MESSAGE: &str =
    "External API not configured for user memory learning";

/// One structured completion behind a local, testable seam.
pub trait StructuredCompletion: Send + Sync {
    /// Runs `user_prompt` under `system_prompt`, constrained by `schema`, and
    /// returns the parsed value (or a provider fault carrying its verbatim
    /// message).
    fn complete<'a>(
        &'a self,
        system_prompt: &'a str,
        user_prompt: &'a str,
        schema: serde_json::Value,
    ) -> BoxFuture<'a, Result<serde_json::Value, AnalysisError>>;
}

/// The production [`StructuredCompletion`] over a `mem-ai` provider.
pub struct AiProviderCompletion {
    provider: Arc<dyn AiProviderStructured>,
    model: String,
    timeout: Option<Duration>,
}

impl AiProviderCompletion {
    /// Adapter over `provider` for `model`.
    pub fn new(provider: Arc<dyn AiProviderStructured>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            timeout: None,
        }
    }

    /// Sets the per-request budget (`None` keeps the provider default).
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }
}

impl StructuredCompletion for AiProviderCompletion {
    fn complete<'a>(
        &'a self,
        system_prompt: &'a str,
        user_prompt: &'a str,
        schema: serde_json::Value,
    ) -> BoxFuture<'a, Result<serde_json::Value, AnalysisError>> {
        Box::pin(async move {
            let mut request = CompletionRequest::new(self.model.clone())
                .with_system(system_prompt)
                .with_message(ChatMessage::user(user_prompt))
                .with_json_schema(schema.clone());
            if let Some(timeout) = self.timeout {
                request = request.with_timeout(timeout);
            }
            self.provider
                .complete_structured(request, schema)
                .await
                // `ProviderError`'s Display is `{provider} {kind}: {message}`:
                // the provider sentence survives as the carrier, unwrapped.
                .map_err(|error| AnalysisError::Provider(error.to_string()))
        })
    }
}

/// The [`AnalysisGateway`] the learning round consumes.
///
/// `external` is optional: upstream's external tool-call path only exists when
/// a memory provider is configured, and [`AnalysisGateway::external_configured`]
/// reports exactly that so the round never attempts an impossible fallback.
pub struct ProviderAnalysisGateway {
    native: Arc<dyn StructuredCompletion>,
    external: Option<Arc<dyn StructuredCompletion>>,
}

impl ProviderAnalysisGateway {
    /// Gateway over the native provider and (optionally) the external one.
    pub fn new(
        native: Arc<dyn StructuredCompletion>,
        external: Option<Arc<dyn StructuredCompletion>>,
    ) -> Self {
        Self { native, external }
    }

    /// Serializes a structured value the way the round's parser expects it.
    fn to_json_text(value: serde_json::Value) -> Result<String, AnalysisError> {
        serde_json::to_string(&value)
            .map_err(|error| AnalysisError::Parse(format!("analysis response: {error}")))
    }
}

impl AnalysisGateway for ProviderAnalysisGateway {
    fn native_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(async move {
            let value = self
                .native
                .complete(
                    &context.system_prompt_json,
                    &context.user_prompt,
                    analysis_schema(),
                )
                .await?;
            Self::to_json_text(value)
        })
    }

    fn external_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(async move {
            let Some(external) = self.external.as_ref() else {
                return Err(AnalysisError::Provider(
                    EXTERNAL_UNCONFIGURED_MESSAGE.to_string(),
                ));
            };
            let value = external
                .complete(
                    &context.system_prompt_tool,
                    &context.user_prompt,
                    analysis_schema(),
                )
                .await?;
            Self::to_json_text(value)
        })
    }

    fn external_configured(&self) -> bool {
        self.external.is_some()
    }
}
