//! The crate seam: one object that turns text into `Vec<f32>`.

use std::future::Future;
use std::pin::Pin;

use crate::error::EmbedError;

/// Boxed `Send` future used to keep the trait dyn-compatible.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Which side of a similarity search a piece of text belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedTask {
    Document,
    Query,
}

impl EmbedTask {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Query => "query",
        }
    }
}

impl std::fmt::Display for EmbedTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an embedder observed about input truncation.
///
/// `cap_tokens` is the resolved cap the provider applied (`None` = unbounded),
/// `truncated_inputs` counts inputs that lost at least one token, and
/// `dropped_tokens` is the total removed. The remote lane reports nothing
/// (`None` from [`Embedder::truncation_stats`]): server-side truncation is
/// invisible to us.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TruncationSnapshot {
    pub cap_tokens: Option<u64>,
    pub truncated_inputs: u64,
    pub dropped_tokens: u64,
}

/// Embedding provider seam consumed by W1.3 / mem-ai / W2.1.
///
/// Divergence from upstream (recorded): the trait receives the FINAL text.
/// Task prefixes are applied by `EmbeddingService`/callers via
/// [`crate::prefix`], mirroring the observable behavior of upstream `embed()`
/// at the service seam rather than inside the provider.
pub trait Embedder: Send + Sync {
    /// Load/prime the provider. Called at most once per service instance by
    /// the sticky warmup gate (`EmbeddingService::warmup`).
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>>;

    /// Embed final (already-prefixed) text into a vector.
    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>>;

    /// Truncation counters for this provider, or `None` when unknown (the
    /// default: the remote lane cannot see server-side truncation).
    fn truncation_stats(&self) -> Option<TruncationSnapshot> {
        None
    }
}
