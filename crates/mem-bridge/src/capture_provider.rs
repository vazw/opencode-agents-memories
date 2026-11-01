//! The capture lane's provider seam (ADR-0029).
//!
//! Capture needs a completion the model must validate against a JSON Schema.
//! Seven of the eight concrete `AiProvider::complete` impls DROP
//! `request.json_schema`, so a capture that asks through the streaming arm gets
//! the prompt's markdown `FORMAT:` block and dead-letters. This module gives
//! the summarizer a handle that cannot make that mistake: [`StructuredCapture`]
//! has no streaming method at all.
//!
//! Three adapters implement it:
//!
//! - [`Structured`] delegates verbatim to a registry-built `AiProviderStructured`.
//! - [`HostStructured`] resolves the lazily-wired OpenCode host on use, and
//!   invalidates the cached origin on ANY failure (a stranger on a recycled port
//!   answers `401`/junk, never `Network`).
//! - [`FilteredStructured`] applies the privacy rules to the request first, and
//!   refuses to dial a conversation those rules emptied.
//!
//! [`CaptureProvider`] carries both halves of one provider for composition: the
//! base handle is for the consolidation summarizer (unwrapped, D-52-7), the
//! structured handle for capture. The pairing is a composition invariant, not a
//! type guarantee — nothing type-checks that both describe the same provider.

use std::sync::Arc;

use mem_ai::privacy::PrivacyConfig;
use mem_ai::provider::{
    AiProvider, AiProviderStructured, BoxFuture, CompletionRequest, OpenCodeHostProvider,
    ProviderError,
};
use mem_store::serde_json::Value;

use crate::host_provider::LazyHostProvider;

/// The only capability capture may hold: one schema-bound completion.
///
/// Deliberately NOT an [`AiProvider`]: with no `complete` in reach, a
/// schema-free capture path cannot be written.
pub trait StructuredCapture: Send + Sync {
    /// Run one structured completion; the reply is the parsed value.
    fn complete_structured<'a>(
        &'a self,
        request: CompletionRequest,
        schema: Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>>;
}

/// The registry handle, delegated verbatim (no second filter, no second parse).
pub struct Structured {
    inner: Arc<dyn AiProviderStructured>,
}

impl Structured {
    /// Wrap a registry-built structured provider.
    pub fn new(inner: Arc<dyn AiProviderStructured>) -> Self {
        Self { inner }
    }
}

impl StructuredCapture for Structured {
    fn complete_structured<'a>(
        &'a self,
        request: CompletionRequest,
        schema: Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move { self.inner.complete_structured(request, schema).await })
    }
}

/// The lazily-resolved OpenCode host, as a structured capability.
pub struct HostStructured {
    lazy: Arc<LazyHostProvider>,
}

impl HostStructured {
    /// Build the adapter. No I/O: the host resolves on the first call.
    pub fn new(lazy: Arc<LazyHostProvider>) -> Self {
        Self { lazy }
    }
}

impl StructuredCapture for HostStructured {
    fn complete_structured<'a>(
        &'a self,
        request: CompletionRequest,
        schema: Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move {
            let inner: Arc<OpenCodeHostProvider> = match self.lazy.resolved() {
                Ok(inner) => inner,
                Err(error) => {
                    self.lazy.invalidate();
                    return Err(error);
                }
            };
            match inner.complete_structured(request, schema).await {
                Ok(value) => Ok(value),
                Err(error) => {
                    // ANY error invalidates the origin (`host_provider` docs).
                    self.lazy.invalidate();
                    Err(error)
                }
            }
        })
    }
}

/// The privacy-guarded adapter: filter the request, never the reply (D8).
pub(crate) struct FilteredStructured {
    inner: Arc<dyn StructuredCapture>,
    config: PrivacyConfig,
}

impl FilteredStructured {
    /// Mint the guard. Only `build_composed_runtime` calls this.
    pub(crate) fn new(inner: Arc<dyn StructuredCapture>, config: PrivacyConfig) -> Self {
        Self { inner, config }
    }
}

impl StructuredCapture for FilteredStructured {
    fn complete_structured<'a>(
        &'a self,
        request: CompletionRequest,
        schema: Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move {
            let filtered = mem_ai::privacy::filter_request_for_delivery(&self.config, request)?;
            self.inner.complete_structured(filtered, schema).await
        })
    }
}

/// Both halves of ONE capture provider, as the engine selector produced them.
pub struct CaptureProvider {
    /// For the consolidation summarizer, which keeps the UNWRAPPED handle.
    pub base: Arc<dyn AiProvider>,
    /// For the capture summarizer, post-filter.
    pub structured: Arc<dyn StructuredCapture>,
}
