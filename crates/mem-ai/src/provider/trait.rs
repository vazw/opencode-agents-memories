//! The provider seam: [`AiProvider`] plus the sealed [`AiProviderStructured`]
//! extension.
//!
//! Everything above this module talks to `dyn AiProvider` only — no concrete
//! provider type crosses the boundary (`test_no_provider_leakage`).

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::provider::error::ProviderError;
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};

/// Boxed `Send` future used to keep the trait dyn-compatible.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Boxed, `Send` stream of completion chunks.
///
/// Chunks are `serde_json::Value` (the variance-tolerant unit on the wire):
/// text deltas arrive as `{ "text": "..." }` and the stream terminates with a
/// `{ "done": true }` chunk. Structured completions surface the parsed value
/// as `{ "json": ... }`.
pub type CompletionStream =
    Pin<Box<dyn Stream<Item = Result<serde_json::Value, ProviderError>> + Send + 'static>>;

/// The unified provider trait (upstream `BaseAIProvider` + factory parity).
///
/// Async methods are `#[async_trait]`-boxed so the trait stays object-safe and
/// providers can be held as `Arc<dyn AiProvider>`.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Stable provider name, e.g. `"opencode-host"`.
    fn name(&self) -> &str;

    /// Start a streaming completion. The returned stream owns everything it
    /// needs and is `'static`, so the caller may hold it past the borrow.
    fn complete(&self, request: CompletionRequest) -> CompletionStream;

    /// Embed a batch of inputs in one round-trip.
    async fn embed(&self, request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError>;

    /// List the models this provider can route to.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    /// Count tokens for `text` under `model`.
    async fn count_tokens(&self, model: &str, text: &str) -> Result<u32, ProviderError>;
}

/// Sealing so [`AiProviderStructured`] cannot be implemented outside `mem-ai`.
pub(crate) mod sealed {
    /// Marker trait; only providers in this crate implement it.
    pub trait Sealed {}
}

/// Structured-output capability, deliberately kept OFF [`AiProvider`] so
/// providers that lack it stay usable through the base trait.
///
/// Sealed: only providers in this crate can opt in.
#[async_trait]
pub trait AiProviderStructured: AiProvider + sealed::Sealed {
    /// Run one structured completion; `json_schema` is the pass-through JSON
    /// Schema the model must validate against. Returns the parsed value.
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError>;

    /// Whether `model` supports structured output (drives provider selection).
    async fn supports_structured_output(&self, model: &str) -> Result<bool, ProviderError>;
}
