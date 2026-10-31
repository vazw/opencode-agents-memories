//! `EmbeddingService` embed/cache/timeout contract (blueprint step 11).
//! In-file `FakeEmbedder` per the concurrent-tree protocol: records inputs,
//! counts warmup/embed calls, programmable warmup failure, optional hang via
//! `std::future::pending`. Cache-key semantics mirror the upstream
//! `Map`-based FIFO cache in `embedding.ts` (insert order preserved, no
//! reorder on hit, evict-oldest at `maxCacheSize`).

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mem_core::config::{Config, ConfigPaths, EmbeddingPooling};

use mem_embed::embedder::{BoxFuture, EmbedTask, Embedder};
use mem_embed::error::EmbedError;
use mem_embed::service::{BackendKind, EMBED_TIMEOUT, EmbeddingService, EmbeddingServiceOptions};

/// Deterministic stand-in vector: identical inputs always embed alike, so
/// cache hits/evictions are observable through the returned vectors.
fn fake_vector(input: &str) -> Vec<f32> {
    vec![
        input.as_bytes().iter().map(|b| *b as f32).sum::<f32>() + 0.5,
        input.chars().count() as f32,
    ]
}

struct FakeEmbedder {
    inputs: Mutex<Vec<String>>,
    embed_calls: AtomicUsize,
    warmup_calls: AtomicUsize,
    warmup_error: Option<String>,
    hang: bool,
}

impl FakeEmbedder {
    fn new() -> Self {
        Self {
            inputs: Mutex::new(Vec::new()),
            embed_calls: AtomicUsize::new(0),
            warmup_calls: AtomicUsize::new(0),
            warmup_error: None,
            hang: false,
        }
    }

    fn hanging() -> Self {
        Self {
            hang: true,
            ..Self::new()
        }
    }

    #[allow(dead_code)] // warmup-failure programming belongs to the step-12 battery
    fn failing_warmup(message: &str) -> Self {
        Self {
            warmup_error: Some(message.to_owned()),
            ..Self::new()
        }
    }

    fn inputs(&self) -> Vec<String> {
        self.inputs.lock().unwrap().clone()
    }

    fn counts(&self) -> (usize, usize) {
        (
            self.embed_calls.load(Ordering::SeqCst),
            self.warmup_calls.load(Ordering::SeqCst),
        )
    }
}

impl Embedder for FakeEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async move {
            self.warmup_calls.fetch_add(1, Ordering::SeqCst);
            match &self.warmup_error {
                Some(message) => Err(EmbedError::ModelLoad {
                    model: "fake-model".to_string(),
                    message: message.clone(),
                }),
                None => Ok(()),
            }
        })
    }

    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        let text = text.to_owned();
        Box::pin(async move {
            self.embed_calls.fetch_add(1, Ordering::SeqCst);
            self.inputs.lock().unwrap().push(text.clone());
            if self.hang {
                std::future::pending::<()>().await;
            }
            Ok(fake_vector(&text))
        })
    }
}

fn options(max_cache_size: usize) -> EmbeddingServiceOptions {
    EmbeddingServiceOptions {
        embedding_model: "test-model".to_string(),
        embedding_dimensions: 8,
        use_task_prefixes: false,
        pooling: EmbeddingPooling::Cls,
        query_prefix: None,
        document_prefix: None,
        dtype: None,
        max_tokens: None,
        api_url: None,
        api_key: None,
        cache_root: std::env::temp_dir(),
        max_cache_size,
        embed_timeout: EMBED_TIMEOUT,
        intra_threads: None,
        hub: None,
    }
}

fn base_config(home: &std::path::Path) -> Config {
    let env = BTreeMap::new();
    Config::load(&ConfigPaths::standard(home), &env).expect("default config loads")
}

#[tokio::test]
async fn test_embed_uses_prefixed_input_as_cache_key() {
    let fake = Arc::new(FakeEmbedder::new());
    let mut options = options(100);
    options.use_task_prefixes = true;
    let service = EmbeddingService::new(options, fake.clone());

    service
        .embed("hi", Some(EmbedTask::Document))
        .await
        .unwrap();
    service.embed("hi", Some(EmbedTask::Query)).await.unwrap();

    // The prefixed string — not the raw text — reaches the embedder, and the
    // two tasks stay distinct cache entries.
    assert_eq!(
        fake.inputs(),
        vec!["search_document: hi", "search_query: hi"]
    );
    // Warmup ran once via the gate (sticky; the second embed reuses it).
    assert_eq!(fake.counts(), (2, 1));
}

#[tokio::test]
async fn test_embed_without_task_passes_text_through() {
    let fake = Arc::new(FakeEmbedder::new());
    let service = EmbeddingService::new(options(100), fake.clone());

    let vector = service.embed("hello", None).await.unwrap();

    assert_eq!(fake.inputs(), vec!["hello"]);
    assert_eq!(vector, fake_vector("hello"));
}

#[tokio::test]
async fn test_cache_hit_skips_the_embedder() {
    let fake = Arc::new(FakeEmbedder::new());
    let mut options = options(100);
    options.use_task_prefixes = true;
    let service = EmbeddingService::new(options, fake.clone());

    let first = service
        .embed("hello", Some(EmbedTask::Document))
        .await
        .unwrap();
    let second = service
        .embed("hello", Some(EmbedTask::Document))
        .await
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(fake.counts(), (1, 1));
    assert_eq!(fake.inputs(), vec!["search_document: hello"]);
}

#[tokio::test]
async fn test_cache_evicts_first_inserted_at_max_size() {
    let fake = Arc::new(FakeEmbedder::new());
    let service = EmbeddingService::new(options(2), fake.clone());

    let a_first = service.embed("a", None).await.unwrap();
    service.embed("b", None).await.unwrap();
    service.embed("c", None).await.unwrap();
    // "a" was the oldest entry and must have been evicted to admit "c".
    let a_second = service.embed("a", None).await.unwrap();

    // 4 embeds, all distinct misses; the gate warmup ran exactly once.
    assert_eq!(fake.counts(), (4, 1));
    assert_eq!(a_first, a_second);
    assert_eq!(a_first, fake_vector("a"));
}

#[tokio::test]
async fn test_clear_cache_forces_recompute() {
    let fake = Arc::new(FakeEmbedder::new());
    let service = EmbeddingService::new(options(100), fake.clone());

    service.embed("hello", None).await.unwrap();
    service.embed("hello", None).await.unwrap();
    assert_eq!(fake.counts(), (1, 1));

    service.clear_cache().await;
    let vector = service.embed("hello", None).await.unwrap();

    // Recomputed vector, but the warmup gate stays warm — clear_cache does
    // not reset it.
    assert_eq!(fake.counts(), (2, 1));
    assert_eq!(vector, fake_vector("hello"));
}

#[tokio::test]
async fn test_embed_with_timeout_aborts_after_budget() {
    let fake = Arc::new(FakeEmbedder::hanging());
    let mut options = options(100);
    options.embed_timeout = Duration::from_millis(20);
    let service = EmbeddingService::new(options, fake.clone());

    let started = Instant::now();
    let result = service.embed_with_timeout("hello", None).await;
    let elapsed = started.elapsed();

    match result {
        Err(EmbedError::Timeout { timeout_ms }) => assert_eq!(timeout_ms, 20),
        other => panic!("expected Timeout, got {other:?}"),
    }
    // Bounded wall time: the budget fires, it does not hang with the fake.
    assert!(elapsed < Duration::from_secs(5), "elapsed: {elapsed:?}");
}

#[test]
fn test_from_config_picks_remote_only_when_url_and_key_are_set() {
    let tmp = common::temp_dir("picker-remote");
    let mut cfg = base_config(tmp.path());

    cfg.embedding_api_url = Some("https://api.fake.local/v1".to_string());
    cfg.embedding_api_key = Some("test-key".to_string());
    assert_eq!(
        EmbeddingService::from_config(&cfg).backend_kind(),
        BackendKind::Remote
    );

    // Upstream quirk: url alone (key unset) keeps the local backend.
    cfg.embedding_api_key = None;
    assert_eq!(
        EmbeddingService::from_config(&cfg).backend_kind(),
        BackendKind::Local
    );

    // …and key alone likewise.
    cfg.embedding_api_url = None;
    cfg.embedding_api_key = Some("test-key".to_string());
    assert_eq!(
        EmbeddingService::from_config(&cfg).backend_kind(),
        BackendKind::Local
    );
}

#[test]
fn test_from_config_picks_local_without_remote_credentials() {
    let tmp = common::temp_dir("picker-local");
    let cfg = base_config(tmp.path());
    assert!(cfg.embedding_api_url.is_none() && cfg.embedding_api_key.is_none());
    assert_eq!(
        EmbeddingService::from_config(&cfg).backend_kind(),
        BackendKind::Local
    );
}

#[test]
fn test_cache_fingerprint_lists_invalidation_inputs() {
    let mut options = options(100);
    options.embedding_model = "test-model".to_string();
    options.pooling = EmbeddingPooling::Cls;
    options.use_task_prefixes = true;
    options.query_prefix = None;
    options.document_prefix = Some("passage: ".to_string());
    options.dtype = None;
    let fake = Arc::new(FakeEmbedder::new());
    let service = EmbeddingService::new(options, fake);

    // Upstream `embeddingCacheFingerprint`: model, pooling, query prefix
    // ("" when unset), document prefix, prefixes flag, dtype ("" when unset).
    // `\x001` disambiguates the two adjacent escapes (NUL then the ASCII
    // digit `1`) — a bare `\01` is an octal-lookalike clippy rejects with
    // `-D warnings` and reads identically at runtime.
    assert_eq!(service.fingerprint(), "test-model\0cls\0\0passage: \x001\0");
}
