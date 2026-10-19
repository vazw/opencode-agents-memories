//! mem-embed — embedding pipeline: local ONNX (ort) + remote providers
//! behind the `Embedder` seam producing `Vec<f32>`.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-embed";

pub mod embedder;
pub mod error;
pub mod hub;
pub mod local;
pub mod prefix;
pub mod remote;
pub mod runtime;
pub mod service;

pub use embedder::{BoxFuture, EmbedTask, Embedder};
pub use error::EmbedError;
pub use hub::{HubTransport, ReqwestHubTransport, ensure_model_files, huggingface_file_url};
pub use local::{LocalLoadOptions, LocalOnnxEmbedder, resolve_local_onnx_dtype};
pub use prefix::{
    NOMIC_DOCUMENT_PREFIX, NOMIC_QUERY_PREFIX, PrefixOptions, apply_task_prefix,
    resolve_task_prefix,
};
pub use remote::{
    RemoteEmbedOptions, RemoteEmbedder, RemoteHttpResponse, RemoteRequest, RemoteRetryOptions,
    RemoteTransport, ReqwestTransport, fetch_remote_embedding_with_retry,
};
pub use runtime::{format_runtime_init_error, ort_environment};
pub use service::{
    BackendKind, EMBED_TIMEOUT, EmbeddingService, EmbeddingServiceOptions, MAX_CACHE_SIZE,
};
