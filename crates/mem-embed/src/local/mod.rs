//! Local ONNX lane: model-cache layout + dtype resolution
//! ([`model_cache`]), tokenizer IO ([`tokenize`]), pooling ([`pooling`]),
//! the ort session wrapper ([`session`]) and the `Embedder` implementation
//! that ties them together.

pub mod model_cache;
pub mod pooling;
pub mod session;
pub mod tokenize;

pub use model_cache::{
    LocalOnnxDtype, ONNX_DTYPE_VARIANTS, model_cache_root, resolve_local_onnx_dtype,
    weights_file_name,
};
pub use pooling::{EmbeddingPooling, l2_normalize_f32, pool_token_embeddings};
pub use session::LoadedSession;
pub use tokenize::{TokenizedInput, encode_text, load_tokenizer};

use std::path::{Component, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokenizers::pipeline::PipelineTokenizer;
use tokio::sync::OnceCell;

use crate::embedder::{BoxFuture, Embedder, TruncationSnapshot};
use crate::error::EmbedError;
use crate::hub::{HubTransport, ensure_model_files};

/// Cache-relative file names the loader needs besides the weights.
const TOKENIZER_FILE: &str = "tokenizer.json";
const CONFIG_FILE: &str = "config.json";
/// Weights file used when no dtype is declared and nothing is cached
/// (transformers.js' default).
const DEFAULT_WEIGHTS_FILE: &str = "onnx/model.onnx";

/// Everything `LocalOnnxEmbedder` needs to lazily load a cached model.
#[derive(Debug, Clone)]
pub struct LocalLoadOptions {
    pub model: String,
    pub cache_root: std::path::PathBuf,
    pub dtype: Option<LocalOnnxDtype>,
    pub pooling: EmbeddingPooling,
    pub intra_threads: Option<usize>,
    pub hub: Option<Arc<dyn HubTransport>>,
    /// The configured token-cap override (`None` = use the model's own
    /// declared position limit).
    pub max_tokens: Option<u32>,
}

/// The shared truncation counters behind [`Embedder::truncation_stats`].
///
/// `cap_tokens` is stored as `0` for "unbounded" (a resolved cap is always at
/// least 2), so the snapshot is three relaxed atomic loads. Atomics because
/// `embed` runs on `spawn_blocking` threads while the bridge reads the
/// counters from the runtime.
#[derive(Debug, Clone, Default)]
pub struct TruncationStats {
    cap_tokens: Arc<AtomicU64>,
    truncated_inputs: Arc<AtomicU64>,
    dropped_tokens: Arc<AtomicU64>,
}

impl TruncationStats {
    fn new(cap_tokens: Option<usize>) -> Self {
        let stats = Self::default();
        stats
            .cap_tokens
            .store(cap_tokens.map_or(0, |cap| cap as u64), Ordering::Relaxed);
        stats
    }

    fn record(&self, dropped: usize) {
        self.truncated_inputs.fetch_add(1, Ordering::Relaxed);
        self.dropped_tokens
            .fetch_add(dropped as u64, Ordering::Relaxed);
    }

    /// The counters as the bridge/CLI read them.
    pub fn snapshot(&self) -> TruncationSnapshot {
        let cap = self.cap_tokens.load(Ordering::Relaxed);
        TruncationSnapshot {
            cap_tokens: (cap != 0).then_some(cap),
            truncated_inputs: self.truncated_inputs.load(Ordering::Relaxed),
            dropped_tokens: self.dropped_tokens.load(Ordering::Relaxed),
        }
    }
}

/// A loaded model: session + tokenizer + the pooling strategy to apply.
struct LocalLoaded {
    session: LoadedSession,
    tokenizer: PipelineTokenizer,
    pooling: EmbeddingPooling,
    /// The resolved token cap (`usize::MAX` = unbounded).
    max_tokens: usize,
    stats: TruncationStats,
    /// The model id, for the per-input truncation event.
    model: String,
}

impl LocalLoaded {
    /// Tokenize → run → pool (blocking; called inside `spawn_blocking`).
    fn embed_blocking(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let TokenizedInput {
            ids,
            attention_mask,
            type_ids,
            dropped_tokens,
        } = encode_text(&self.tokenizer, text, self.max_tokens)?;
        if dropped_tokens > 0 {
            self.stats.record(dropped_tokens);
            // Counts only — never the text, never a path. The file sink is the
            // per-input channel; a bare `tracing::warn!` here would reach no
            // sink at all.
            mem_core::logging::log(
                "embedding.input_truncated",
                Some(&serde_json::json!({
                    "model": self.model.as_str(),
                    "cap_tokens": self.max_tokens,
                    "dropped_tokens": dropped_tokens,
                    "text_chars": text.chars().count(),
                })),
            );
        }
        let (shape, data) =
            self.session
                .run(&ids, attention_mask.as_deref(), type_ids.as_deref())?;
        pool_token_embeddings(&data, &shape, self.pooling, attention_mask.as_deref())
    }
}

/// ONNX-backed embedder (load deferred to warmup/first embed).
pub struct LocalOnnxEmbedder {
    options: LocalLoadOptions,
    /// Sticky load result: never retried after a failure (upstream #184
    /// "initializing forever"), shared by every caller via single-flight.
    loaded: OnceCell<Result<Arc<LocalLoaded>, EmbedError>>,
}

impl LocalOnnxEmbedder {
    pub fn new(options: LocalLoadOptions) -> Self {
        Self {
            options,
            loaded: OnceCell::new(),
        }
    }

    /// The loaded model, loading it once on first use.
    async fn ensure_loaded(&self) -> Result<Arc<LocalLoaded>, EmbedError> {
        match self.loaded.get_or_init(|| self.load()).await {
            Ok(loaded) => Ok(loaded.clone()),
            Err(error) => Err(error.clone()),
        }
    }

    /// Resolve the cache layout, acquire anything missing, and load.
    ///
    /// Load order: validate the model id (before any filesystem access) →
    /// resolve the weights file → fetch missing files through the hub seam
    /// (when configured) → read the weights → commit the session → read the
    /// tokenizer.
    async fn load(&self) -> Result<Arc<LocalLoaded>, EmbedError> {
        let model = &self.options.model;
        validate_model_id(model)?;
        let model_dir = model_cache_root(model, &self.options.cache_root);

        // Explicit config wins. `auto` behaves as unset: upstream forwards
        // "auto" to transformers.js, whose own resolution is exactly this
        // cache probe (recorded divergence).
        let effective_dtype = match self.options.dtype {
            Some(LocalOnnxDtype::Auto) | None => {
                resolve_local_onnx_dtype(model, &self.options.cache_root)
            }
            Some(dtype) => Some(dtype),
        };
        let weights_file = effective_dtype
            .and_then(weights_file_name)
            .unwrap_or(DEFAULT_WEIGHTS_FILE);

        let required = [weights_file, TOKENIZER_FILE, CONFIG_FILE];
        let mut missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|file| !model_dir.join(file).exists())
            .collect();
        if !missing.is_empty()
            && let Some(hub) = self.options.hub.as_deref()
        {
            ensure_model_files(model, &model_dir, &missing, hub).await?;
            missing = required
                .iter()
                .copied()
                .filter(|file| !model_dir.join(file).exists())
                .collect();
        }
        if let Some(file) = missing.first() {
            return Err(EmbedError::ModelFilesMissing {
                model: model.clone(),
                path: model_dir.join(file),
            });
        }

        let weights_path = model_dir.join(weights_file);
        let bytes = std::fs::read(&weights_path).map_err(|error| EmbedError::ModelLoad {
            model: model.clone(),
            message: format!("{weights_path:?}: {error}"),
        })?;
        let session = LoadedSession::load(model, &bytes, self.options.intra_threads)?;
        let tokenizer = load_tokenizer(&model_dir.join(TOKENIZER_FILE))?;

        // The one-time special-token probe stays inline: `load()` is already a
        // blocking path (the `std::fs::read` of the weights and
        // `Session::load` above), so an empty-string encode adds no new class
        // of blocking. `spawn_blocking` here would need an `Arc` round-trip of
        // the tokenizer for one encode and would not fix the far larger
        // blocking read already on this path.
        let special_tokens = encode_text(&tokenizer, "", usize::MAX)?.ids.len();
        let declared = model_cache::read_declared_position_limit(model, &self.options.cache_root);
        let resolved =
            model_cache::effective_max_tokens(declared, special_tokens, self.options.max_tokens)?;
        let source = match (resolved, self.options.max_tokens) {
            (None, _) => "none",
            (Some(_), Some(_)) => "configured",
            (Some(_), None) => "model",
        };
        mem_core::logging::log(
            "embedding.input_cap_resolved",
            Some(&serde_json::json!({
                "model": model.as_str(),
                "cap_tokens": resolved,
                "special_tokens": special_tokens,
                "source": source,
            })),
        );
        let stats = TruncationStats::new(resolved);

        tracing::debug!(model = %model, weights = weights_file, "loaded local ONNX embedding model");
        Ok(Arc::new(LocalLoaded {
            session,
            tokenizer,
            pooling: self.options.pooling,
            max_tokens: resolved.unwrap_or(usize::MAX),
            stats,
            model: model.clone(),
        }))
    }
}

impl Embedder for LocalOnnxEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async move { self.ensure_loaded().await.map(|_loaded| ()) })
    }

    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        // Own the text first: the returned future is bounded by `&self`, so it
        // must not borrow the `text` parameter.
        let owned_text = text.to_owned();
        Box::pin(async move {
            let loaded = self.ensure_loaded().await?;
            // ort is synchronous: tokenization + inference never run on the
            // async runtime.
            tokio::task::spawn_blocking(move || loaded.embed_blocking(&owned_text))
                .await
                .map_err(|error| EmbedError::TaskJoin {
                    message: error.to_string(),
                })?
        })
    }

    fn truncation_stats(&self) -> Option<TruncationSnapshot> {
        self.loaded
            .get()
            .and_then(|result| result.as_ref().ok())
            .map(|loaded| loaded.stats.snapshot())
    }
}

/// Reject model ids that could reach outside the cache root.
///
/// A model id is a cache subdirectory (`{storagePath}/.cache/{model}`): empty
/// ids, absolute paths and any `..` component are refused before the first
/// filesystem access.
fn validate_model_id(model: &str) -> Result<(), EmbedError> {
    let invalid = || EmbedError::InvalidModelId {
        model: model.to_string(),
    };
    if model.trim().is_empty() {
        return Err(invalid());
    }
    let path = Path::new(model);
    if path.is_absolute() {
        return Err(invalid());
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(invalid());
    }
    Ok(())
}
