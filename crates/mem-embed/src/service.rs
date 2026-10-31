//! `EmbeddingService` — the composition seam over one `Embedder`:
//! task-prefix application, FIFO embedding cache, embed budget, and the
//! sticky warmup gate (upstream #184 semantics).
//!
//! Scaffold state: composition (options, `from_config`, `new`) is complete;
//! cache/timeout bodies land with s3 step 11 and the warmup gate with step 12.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mem_core::config::{Config, EmbeddingPooling, LocalOnnxDtype};

use crate::embedder::{EmbedTask, Embedder};
use crate::error::EmbedError;
use crate::hub::{HubTransport, ReqwestHubTransport};
use crate::local::{LocalLoadOptions, LocalOnnxEmbedder};
use crate::prefix::{PrefixOptions, apply_task_prefix};
use crate::remote::retry::RemoteRetryOptions;
use crate::remote::{RemoteEmbedOptions, RemoteEmbedder, ReqwestTransport};

/// Default whole-embed budget (upstream 30s).
pub const EMBED_TIMEOUT: Duration = Duration::from_secs(30);
/// Default FIFO embedding cache size (upstream 100).
pub const MAX_CACHE_SIZE: usize = 100;

/// Service-level inputs; value-typed (ADR-0002 — upstream's mutable global
/// `CONFIG` had the fingerprint recomputed, here it is immutable per
/// instance).
#[derive(Debug, Clone)]
pub struct EmbeddingServiceOptions {
    pub embedding_model: String,
    pub embedding_dimensions: u32,
    pub use_task_prefixes: bool,
    pub pooling: EmbeddingPooling,
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
    pub dtype: Option<LocalOnnxDtype>,
    /// The configured token-cap override (`None` = the model's own limit).
    pub max_tokens: Option<u32>,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub cache_root: PathBuf,
    pub max_cache_size: usize,
    pub embed_timeout: Duration,
    pub intra_threads: Option<usize>,
    pub hub: Option<Arc<dyn HubTransport>>,
}

impl EmbeddingServiceOptions {
    /// Map the resolved config onto service options: `cache_root` is
    /// `{storagePath}/.cache`, hub fetch defaults to the reqwest transport,
    /// budgets to the upstream constants.
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            embedding_model: cfg.embedding_model.clone(),
            embedding_dimensions: cfg.embedding_dimensions,
            use_task_prefixes: cfg.embedding_use_task_prefixes,
            pooling: cfg.embedding_pooling,
            query_prefix: cfg.embedding_query_prefix.clone(),
            document_prefix: cfg.embedding_document_prefix.clone(),
            dtype: cfg.embedding_dtype,
            max_tokens: cfg.embedding_max_tokens,
            api_url: cfg.embedding_api_url.clone(),
            api_key: cfg.embedding_api_key.clone(),
            cache_root: cfg.storage_path.join(".cache"),
            max_cache_size: MAX_CACHE_SIZE,
            embed_timeout: EMBED_TIMEOUT,
            intra_threads: None,
            hub: Some(Arc::new(ReqwestHubTransport::new())),
        }
    }

    /// The local-loader options for this service: the one construction the
    /// service and the composition root share (so the cap hop is testable).
    pub fn local_load_options(&self) -> LocalLoadOptions {
        LocalLoadOptions {
            model: self.embedding_model.clone(),
            cache_root: self.cache_root.clone(),
            dtype: self.dtype,
            pooling: self.pooling,
            intra_threads: self.intra_threads,
            hub: self.hub.clone(),
            max_tokens: self.max_tokens,
        }
    }
}

/// Which provider lane the service was composed onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Local,
    Remote,
}

/// FIFO vector cache keyed by the final (prefixed) input.
struct EmbeddingCache {
    fingerprint: String,
    map: HashMap<String, Vec<f32>>,
    order: VecDeque<String>,
    max: usize,
}

impl EmbeddingCache {
    fn new(max: usize) -> Self {
        Self {
            fingerprint: String::new(),
            map: HashMap::new(),
            order: VecDeque::new(),
            max,
        }
    }

    fn invalidate_if_stale(&mut self, fingerprint: &str) {
        if self.fingerprint != fingerprint {
            self.map.clear();
            self.order.clear();
            self.fingerprint = fingerprint.to_owned();
        }
    }

    fn get(&self, input: &str) -> Option<Vec<f32>> {
        self.map.get(input).cloned()
    }

    /// Insert with FIFO eviction; never exceeds `max` (no reorder on hit —
    /// upstream `Map` semantics).
    fn insert(&mut self, input: String, vector: Vec<f32>) {
        if self.map.contains_key(&input) {
            return;
        }
        while self.map.len() >= self.max {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.map.remove(&oldest);
                }
                None => break,
            }
        }
        if self.max == 0 {
            return;
        }
        self.order.push_back(input.clone());
        self.map.insert(input, vector);
    }
}

/// Sticky warmup gate (upstream #184: never re-run `Embedder::warmup` after
/// a failure). Single-flight via a shared cell; never holds a lock across an
/// await; no `tokio::spawn` (current-thread runtimes must work).
enum WarmupPhase {
    Cold,
    Warming(Arc<tokio::sync::OnceCell<Result<(), EmbedError>>>),
    Ready,
    Failed(EmbedError),
}

/// One composed embedding provider: prefixes + cache + budget + warmup gate.
pub struct EmbeddingService {
    options: EmbeddingServiceOptions,
    embedder: Arc<dyn Embedder>,
    kind: BackendKind,
    cache: Mutex<EmbeddingCache>,
    warmup_phase: tokio::sync::Mutex<WarmupPhase>,
}

impl EmbeddingService {
    /// Wrap any embedder; backend kind is derived upstream-style: Remote iff
    /// `api_url` AND `api_key` are both set.
    pub fn new(options: EmbeddingServiceOptions, embedder: Arc<dyn Embedder>) -> Self {
        let kind = if options.api_url.is_some() && options.api_key.is_some() {
            BackendKind::Remote
        } else {
            BackendKind::Local
        };
        let cache = Mutex::new(EmbeddingCache::new(options.max_cache_size));
        Self {
            options,
            embedder,
            kind,
            cache,
            warmup_phase: tokio::sync::Mutex::new(WarmupPhase::Cold),
        }
    }

    /// Composition seam (divergence: replaces upstream's process-wide
    /// singleton): pick Remote only when url AND key are both set (upstream
    /// `CONFIG.embeddingApiUrl && CONFIG.embeddingApiKey` quirk), else Local.
    pub fn from_config(cfg: &Config) -> Self {
        let options = EmbeddingServiceOptions::from_config(cfg);
        let embedder: Arc<dyn Embedder> = if options.api_url.is_some() && options.api_key.is_some()
        {
            Arc::new(RemoteEmbedder::new(
                RemoteEmbedOptions {
                    url: options.api_url.clone().unwrap_or_default(),
                    api_key: options.api_key.clone().unwrap_or_default(),
                    model: options.embedding_model.clone(),
                    retry: RemoteRetryOptions::default(),
                },
                Arc::new(ReqwestTransport::new()),
                crate::remote::retry::system_sleep(),
            ))
        } else {
            Arc::new(LocalOnnxEmbedder::new(options.local_load_options()))
        };
        Self::new(options, embedder)
    }

    /// Warm up through the sticky gate (s3 step 12 implementation point).
    pub async fn warmup(&self) -> Result<(), EmbedError> {
        self.ensure_warm().await
    }

    /// True once the warmup gate reached `Ready` (s3 step 12 implementation
    /// point).
    pub async fn is_ready(&self) -> bool {
        matches!(*self.warmup_phase.lock().await, WarmupPhase::Ready)
    }

    pub fn backend_kind(&self) -> BackendKind {
        self.kind
    }

    /// `"\0"`-joined invalidation inputs (upstream `embeddingCacheFingerprint`):
    /// model, pooling, query prefix (`""` when unset), document prefix,
    /// prefixes flag (`1`/`0`), dtype (`""` when unset).
    pub fn fingerprint(&self) -> String {
        let options = &self.options;
        [
            options.embedding_model.as_str(),
            options.pooling.as_str(),
            options.query_prefix.as_deref().unwrap_or(""),
            options.document_prefix.as_deref().unwrap_or(""),
            if options.use_task_prefixes { "1" } else { "0" },
            options.dtype.map(LocalOnnxDtype::as_str).unwrap_or(""),
        ]
        .join("\0")
    }

    /// Embed with task-prefix application and FIFO caching: the prefixed
    /// input is the cache key; hit returns the clone with no embedder call.
    /// Never holds the cache lock across an `.await`; poisoned locks are
    /// recovered, not unwrapped.
    pub async fn embed(&self, text: &str, task: Option<EmbedTask>) -> Result<Vec<f32>, EmbedError> {
        let input = match task {
            Some(task) => {
                let prefix_options = PrefixOptions {
                    use_task_prefixes: self.options.use_task_prefixes,
                    query_prefix: self.options.query_prefix.clone(),
                    document_prefix: self.options.document_prefix.clone(),
                };
                apply_task_prefix(text, task, &prefix_options)
            }
            None => text.to_owned(),
        };
        let fingerprint = self.fingerprint();
        {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.invalidate_if_stale(&fingerprint);
            if let Some(vector) = cache.get(&input) {
                return Ok(vector);
            }
        }
        self.ensure_warm().await?;
        let vector = self.embedder.embed(&input).await?;
        {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.invalidate_if_stale(&fingerprint);
            cache.insert(input, vector.clone());
        }
        Ok(vector)
    }

    /// `embed` wrapped in the service embed budget; elapsed maps to
    /// `EmbedError::Timeout { timeout_ms }`.
    pub async fn embed_with_timeout(
        &self,
        text: &str,
        task: Option<EmbedTask>,
    ) -> Result<Vec<f32>, EmbedError> {
        let timeout = self.options.embed_timeout;
        match tokio::time::timeout(timeout, self.embed(text, task)).await {
            Ok(result) => result,
            Err(_) => Err(EmbedError::Timeout {
                timeout_ms: timeout.as_millis() as u64,
            }),
        }
    }

    /// Drop every cached vector (fingerprint is kept — it is immutable).
    pub async fn clear_cache(&self) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.map.clear();
        cache.order.clear();
    }

    /// Sticky gate body (upstream #184): `Embedder::warmup` runs at most
    /// once; failures poison the gate forever (`WarmupFailed`, never
    /// retried). Single-flight via a shared cell; no lock is ever held
    /// across an `.await`; no `tokio::spawn` (current-thread runtimes work).
    async fn ensure_warm(&self) -> Result<(), EmbedError> {
        let cell = {
            let mut phase = self.warmup_phase.lock().await;
            match &*phase {
                WarmupPhase::Ready => return Ok(()),
                WarmupPhase::Failed(e) => {
                    return Err(EmbedError::WarmupFailed {
                        message: e.to_string(),
                    });
                }
                WarmupPhase::Warming(cell) => cell.clone(),
                WarmupPhase::Cold => {
                    let cell = Arc::new(tokio::sync::OnceCell::new());
                    *phase = WarmupPhase::Warming(cell.clone());
                    cell
                }
            }
        };
        let result = cell
            .get_or_init(|| async { self.embedder.warmup().await })
            .await;
        {
            let mut phase = self.warmup_phase.lock().await;
            // First waiter to finish sets the terminal phase; any waiter
            // arriving after a terminal phase must not disturb it (the cell
            // guarantees every waiter sees the same outcome regardless).
            if matches!(*phase, WarmupPhase::Warming(_)) {
                *phase = match &result {
                    Ok(()) => WarmupPhase::Ready,
                    Err(e) => WarmupPhase::Failed(e.clone()),
                };
            }
        }
        result.clone().map_err(|e| EmbedError::WarmupFailed {
            message: e.to_string(),
        })
    }
}
