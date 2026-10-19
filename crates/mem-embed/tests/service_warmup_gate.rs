//! Sticky warmup gate contract (blueprint step 12, upstream #184 semantics):
//! `Embedder::warmup` runs at most once per service; a failure poisons the
//! gate forever (`WarmupFailed`, never a retry); concurrent embeds share one
//! in-flight warmup; `is_ready` tracks the gate. In-file `FakeEmbedder` per
//! the concurrent-tree protocol (no cross-file test helpers).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mem_core::config::EmbeddingPooling;

use mem_embed::embedder::{BoxFuture, EmbedTask, Embedder};
use mem_embed::error::EmbedError;
use mem_embed::service::{EMBED_TIMEOUT, EmbeddingService, EmbeddingServiceOptions};

struct FakeEmbedder {
    inputs: Mutex<Vec<String>>,
    embed_calls: AtomicUsize,
    warmup_calls: AtomicUsize,
    warmup_error: Option<String>,
    warmup_delay: Option<Duration>,
}

impl FakeEmbedder {
    fn new() -> Self {
        Self {
            inputs: Mutex::new(Vec::new()),
            embed_calls: AtomicUsize::new(0),
            warmup_calls: AtomicUsize::new(0),
            warmup_error: None,
            warmup_delay: None,
        }
    }

    fn failing_warmup(message: &str) -> Self {
        Self {
            warmup_error: Some(message.to_owned()),
            ..Self::new()
        }
    }

    fn slow_warmup(delay: Duration) -> Self {
        Self {
            warmup_delay: Some(delay),
            ..Self::new()
        }
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
            if let Some(delay) = self.warmup_delay {
                tokio::time::sleep(delay).await;
            }
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
            Ok(vec![text.len() as f32])
        })
    }
}

fn options() -> EmbeddingServiceOptions {
    EmbeddingServiceOptions {
        embedding_model: "test-model".to_string(),
        embedding_dimensions: 8,
        use_task_prefixes: false,
        pooling: EmbeddingPooling::Cls,
        query_prefix: None,
        document_prefix: None,
        dtype: None,
        api_url: None,
        api_key: None,
        cache_root: std::env::temp_dir(),
        max_cache_size: 100,
        embed_timeout: EMBED_TIMEOUT,
        intra_threads: None,
        hub: None,
    }
}

#[tokio::test]
async fn test_embed_warms_up_once_when_cold() {
    let fake = Arc::new(FakeEmbedder::new());
    let service = EmbeddingService::new(options(), fake.clone());

    service.embed("hello", None).await.unwrap();

    assert_eq!(fake.counts(), (1, 1));
}

#[tokio::test]
async fn test_concurrent_embeds_share_a_single_warmup() {
    // The fake's warmup sleeps 20ms so the other three join!ed embeds
    // observe the Warming phase and await the same cell.
    let fake = Arc::new(FakeEmbedder::slow_warmup(Duration::from_millis(20)));
    let service = Arc::new(EmbeddingService::new(options(), fake.clone()));

    let (r1, r2, r3, r4) = tokio::join!(
        service.embed("a", None),
        service.embed("b", None),
        service.embed("c", None),
        service.embed("d", None),
    );
    let _ = &service;

    assert!(r1.is_ok() && r2.is_ok() && r3.is_ok() && r4.is_ok());
    assert_eq!(fake.warmup_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fake.embed_calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn test_warmup_failure_is_sticky_and_named() {
    let fake = Arc::new(FakeEmbedder::failing_warmup("model exploded"));
    let service = EmbeddingService::new(options(), fake.clone());

    let first = service.embed("hello", None).await;
    let second = service.embed("hello", None).await;

    let Err(EmbedError::WarmupFailed { message }) = &first else {
        panic!("expected WarmupFailed, got {first:?}");
    };
    assert!(
        message.contains("model exploded"),
        "gate error must name the cause: {message}"
    );
    let Err(EmbedError::WarmupFailed {
        message: second_message,
    }) = &second
    else {
        panic!("expected sticky WarmupFailed, got {second:?}");
    };
    assert!(second_message.contains("model exploded"));

    // Sticky: the embedder warmup ran exactly once, embeds never reached it.
    assert_eq!(fake.counts(), (0, 1));
}

#[tokio::test]
async fn test_is_ready_tracks_warmup_state() {
    let fake = Arc::new(FakeEmbedder::new());
    let service = EmbeddingService::new(options(), fake.clone());
    assert!(!service.is_ready().await, "cold service is not ready");

    service.warmup().await.unwrap();
    assert!(service.is_ready().await, "warm service is ready");

    let failing = Arc::new(FakeEmbedder::failing_warmup("no gpu"));
    let doomed = EmbeddingService::new(options(), failing);
    assert!(doomed.warmup().await.is_err());
    assert!(
        !doomed.is_ready().await,
        "failed warmup never reports ready"
    );
}

#[tokio::test]
async fn test_warmup_success_is_sticky_across_embeds() {
    let fake = Arc::new(FakeEmbedder::new());
    let service = EmbeddingService::new(options(), fake.clone());

    service
        .embed("hello", Some(EmbedTask::Document))
        .await
        .unwrap();
    service
        .embed("world", Some(EmbedTask::Document))
        .await
        .unwrap();

    assert_eq!(fake.counts(), (2, 1));
}
