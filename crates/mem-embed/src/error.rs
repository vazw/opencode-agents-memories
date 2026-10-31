//! `EmbedError` — the crate-wide error surface.
//!
//! Mirrors the upstream error classes behind `embedding.ts`: runtime-load,
//! model/tokenizer IO, inference, hub-fetch, and remote-provider failures,
//! with `is_retryable` reproducing the upstream `isRetryableEmbeddingError`
//! table (429 / 5xx / network / per-attempt timeout retry; everything else
//! fails fast).

use std::path::PathBuf;

/// All failures surfaced by the embedding pipeline.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum EmbedError {
    /// The ONNX runtime could not be initialized in this process.
    #[error(
        "ONNX runtime failed to load: {message}. Set embeddingApiUrl and embeddingApiKey to use a remote embedding provider instead."
    )]
    RuntimeInit { message: String },

    /// The configured `embeddingModel` is not a usable cache-relative id.
    #[error("Invalid embedding model id: {model}")]
    InvalidModelId { model: String },

    /// Required model files are missing and no hub fetch was available.
    #[error(
        "Embedding model files missing for {model}: {path}. Provision the cache with scripts/provision-onnx-cache.sh — the engine never downloads models at runtime."
    )]
    ModelFilesMissing { model: String, path: PathBuf },

    /// The session could not be committed from the model bytes.
    #[error("Failed to load embedding model {model}: {message}")]
    ModelLoad { model: String, message: String },

    /// `tokenizer.json` could not be read/parsed.
    #[error("Failed to load tokenizer {path}: {message}")]
    TokenizerLoad { path: PathBuf, message: String },

    /// Encoding text into token ids failed.
    #[error("Tokenization failed: {message}")]
    Tokenize { message: String },

    /// ONNX inference failed.
    #[error("ONNX inference failed: {message}")]
    Inference { message: String },

    /// Model output did not have the expected tensor shape.
    #[error("Unexpected tensor shape: {message}")]
    TensorShape { message: String },

    /// The `spawn_blocking` task panicked or was cancelled.
    #[error("Embedding task join failed: {message}")]
    TaskJoin { message: String },

    /// A hub download returned a non-2xx HTTP status.
    #[error("Failed to download {url} (HTTP {status})")]
    HubHttp { url: String, status: u16 },

    /// A hub download failed at the transport level.
    #[error("Failed to download {url}: {message}")]
    HubTransport { url: String, message: String },

    /// A hub-relative path tried to escape the model directory.
    #[error("Refusing to write outside the model directory: {path}")]
    HubPathEscape { path: String },

    /// The remote embedding API returned a non-2xx status (byte-compatible
    /// with upstream `API embedding failed: {status} {statusText}`).
    #[error("API embedding failed: {status} {status_text}")]
    RemoteHttp { status: u16, status_text: String },

    /// The remote embedding API returned unparseable or shapeless JSON.
    #[error("Invalid embedding API JSON: {message}")]
    RemoteInvalidJson { message: String },

    /// The remote embedding API was unreachable.
    #[error("Embedding API network error: {message}")]
    RemoteNetwork { message: String },

    /// One remote attempt exceeded its per-attempt budget.
    #[error("Remote embedding attempt timed out after {timeout_ms}ms")]
    RemoteTimeout { timeout_ms: u64 },

    /// The whole embed exceeded the service embed budget.
    #[error("Embedding timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Warmup failed; the gate is sticky (upstream #184) and later calls
    /// report this without re-running `Embedder::warmup`.
    #[error("Embedding warmup failed: {message}")]
    WarmupFailed { message: String },

    /// `embeddingMaxTokens` is larger than the model's own declared position
    /// limit. Refused at load, never clamped: clamping would re-hide the crash
    /// the operator asked to avoid.
    #[error(
        "embeddingMaxTokens {configured} exceeds the model's {model_limit} token positions. Lower embeddingMaxTokens to at most {model_limit}, or unset it to use the model's own limit."
    )]
    MaxTokensExceedsModelLimit { configured: u32, model_limit: usize },
}

impl EmbedError {
    /// Upstream `isRetryableEmbeddingError`: retry exactly 429 / 5xx remote
    /// HTTP statuses, network failures, and per-attempt timeouts.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RemoteHttp { status, .. } => *status == 429 || *status >= 500,
            Self::RemoteNetwork { .. } | Self::RemoteTimeout { .. } => true,
            _ => false,
        }
    }
}
