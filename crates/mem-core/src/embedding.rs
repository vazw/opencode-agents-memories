//! The embedding identity (W2): the typed document a shard stores under
//! `shard_metadata.embedding_fingerprint`.
//!
//! It is deliberately a *storage* identity, not the embedding service's cache
//! key: `mem_embed::service::EmbeddingService::fingerprint()` answers a
//! different question with different pinned bytes (residual R-4). It carries
//! no credential and no API URL — a URL can carry a token in its query
//! string, and the fingerprint lands in a plaintext shard that backups copy.

use serde::Serialize;

use crate::config::{Config, EmbeddingPooling, LocalOnnxDtype};

/// The resolved embedding identity of this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingIdentity {
    /// The resolved embedding model name.
    pub model: String,
    /// The resolved vector dimensions.
    pub dimensions: u32,
    /// The local pooling strategy.
    pub pooling: EmbeddingPooling,
    /// The local ONNX dtype (`None` for provider-backed embeddings).
    pub dtype: Option<LocalOnnxDtype>,
    /// Whether task prefixes are applied.
    pub use_task_prefixes: bool,
    /// The query prefix, when configured.
    pub query_prefix: Option<String>,
    /// The document prefix, when configured.
    pub document_prefix: Option<String>,
}

impl EmbeddingIdentity {
    /// Maps every identity input from the resolved config.
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            model: cfg.embedding_model.clone(),
            dimensions: cfg.embedding_dimensions,
            pooling: cfg.embedding_pooling,
            dtype: cfg.embedding_dtype,
            use_task_prefixes: cfg.embedding_use_task_prefixes,
            query_prefix: cfg.embedding_query_prefix.clone(),
            document_prefix: cfg.embedding_document_prefix.clone(),
        }
    }

    /// The canonical storage fingerprint: a JSON document with `"v":1`.
    ///
    /// Fallible by signature: an empty string on a serializer error would be
    /// unequal forever and force a re-embed on every open (ADR-0013 forbids
    /// `unwrap`/`unwrap_or_default`). No value built by [`Self::from_config`]
    /// can fail — every field is a primitive — but the caller decides.
    ///
    /// `FingerprintV1` is a private `Serialize` view because
    /// `EmbeddingPooling` and `LocalOnnxDtype` implement `Deserialize` only
    /// (the first manually, the second not at all), and `config/schema.rs` is
    /// not this module's to change. `serde_json::json!` is rejected: its
    /// `Map` sorts keys, so the field order would be alphabetical and
    /// undocumented.
    pub fn fingerprint(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&FingerprintV1 {
            v: 1,
            model: &self.model,
            dims: self.dimensions,
            pooling: self.pooling.as_str(),
            dtype: self.dtype.map(LocalOnnxDtype::as_str),
            task_prefixes: self.use_task_prefixes,
            query_prefix: self.query_prefix.as_deref(),
            document_prefix: self.document_prefix.as_deref(),
        })
    }
}

/// The versioned canonical shape. Field order is declaration order (no
/// `json!` map, no `skip_serializing_if`), so the bytes are stable across
/// values; `None` renders as `null` rather than disappearing.
#[derive(Serialize)]
struct FingerprintV1<'a> {
    v: u8,
    model: &'a str,
    dims: u32,
    pooling: &'a str,
    dtype: Option<&'a str>,
    task_prefixes: bool,
    query_prefix: Option<&'a str>,
    document_prefix: Option<&'a str>,
}
