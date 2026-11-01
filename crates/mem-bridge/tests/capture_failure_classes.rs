//! Issue 108 — retryability reaches the capture ladder, carried by the closed
//! wire token and the decoded detail.
//!
//! Four drains through the REAL `CaptureService` with a scripted summarizer
//! whose `last_failure()` is produced by the production classifier over a real
//! decoded host failure (never a hand-written reason). Each row asserts the
//! drain report AND the inbox attempt counter, so "bump then dead-letter" and
//! "retry forever" are both excluded.

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_ai::provider::{
    ProviderError, decode_generate_failure, remote::structured::parse_structured_lenient,
};
use mem_bridge::capture::{CaptureParams, CaptureService, DrainReport};
use mem_bridge::capture_prompt::{CaptureSummarizer, SummarizerFailure, classify_provider_error};

/// The measured 308-byte markdown reply (the host answered a capture-sized
/// prompt in prose) — the parse-miss input.
const MEASURED_MARKDOWN_REPLY: &str = "## Request\nBound the unbounded inject barrier to 2 seconds and add logging for timeout events.\n\n## Outcome\nImplemented `INJECT_BARRIER_BUDGET` and an `AtomicBool` latch in `engine.rs` to enforce the 2-second timeout and log when the barrier is exceeded.\n\n**Tags**: `rust`, `concurrency`, `timeout`, `logging`";

/// A summarizer that always fails with one classified failure.
struct ScriptedSummarizer {
    failure: SummarizerFailure,
}

impl CaptureSummarizer for ScriptedSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> mem_embed::embedder::BoxFuture<
        'a,
        Result<mem_bridge::capture::CaptureSummaryOutput, mem_episodic::error::EpisodicError>,
    > {
        Box::pin(async move {
            Err(mem_episodic::error::EpisodicError::Summary {
                message: "scripted summarizer failure".to_string(),
            })
        })
    }

    fn last_failure(&self) -> Option<SummarizerFailure> {
        Some(self.failure.clone())
    }
}

fn service(dir: &TestDir, failure: &ProviderError) -> CaptureService {
    let store = Arc::new(mem_episodic::store::EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(Arc::new(ScriptedSummarizer {
            failure: classify_provider_error(failure),
        })),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
}

async fn seed_and_drain(capture: &CaptureService, capture_id: &str) -> DrainReport {
    capture
        .enqueue(&CaptureParams {
            session_id: "sess-classes".to_string(),
            capture_id: capture_id.to_string(),
            project_id: "/srv/classes".to_string(),
            agent_id: "agent-1".to_string(),
            transcript: Some("user: fix the retry storm in the login handler".to_string()),
            ..Default::default()
        })
        .await
        .expect("enqueue");
    capture.drain_once().await.expect("drain")
}

/// A permanent `422` is dead-lettered on the FIRST drain and consumes no
/// attempt: burning five attempts on a class no retry can fix is the defect.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_108_422_dead_letters_on_the_first_drain() {
    let failure = decode_generate_failure(
        422,
        r#"{"_tag":"UnsupportedModelError","providerID":"p","modelID":"m","capability":"structured_output","message":"x"}"#,
    );
    assert_eq!(
        classify_provider_error(&failure).reason,
        "host_model_unsupported",
        "the 422 class is named"
    );

    let dir = TestDir::new("classes-422");
    let capture = service(&dir, &failure);
    let capture_id = "019a0000-0000-7f00-8000-00000000d001";
    let report = seed_and_drain(&capture, capture_id).await;

    assert_eq!(report.dead_lettered, 1, "{report:?}");
    assert_eq!(
        capture.inbox().attempts(capture_id).await,
        0,
        "a permanent class must not consume an attempt"
    );
}

/// A `502` with a small `retryAfterMs` is retried as a bounded refusal: one
/// attempt consumed, nothing dead-lettered.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_108_502_below_the_ceiling_is_retried() {
    let failure = decode_generate_failure(
        502,
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"rate_limit","message":"x","retryAfterMs":1500}"#,
    );
    assert_eq!(
        classify_provider_error(&failure).reason,
        "host_provider_request_error",
        "the retryable 502 class is named"
    );
    assert_eq!(failure.retry_after_ms, Some(1500), "the hint is decoded");

    let dir = TestDir::new("classes-502-retry");
    let capture = service(&dir, &failure);
    let capture_id = "019a0000-0000-7f00-8000-00000000d002";
    let report = seed_and_drain(&capture, capture_id).await;

    assert_eq!(report.retryable, 1, "{report:?}");
    assert_eq!(capture.inbox().attempts(capture_id).await, 1);
}

/// A `502` whose hint exceeds the 30 s ceiling dead-letters immediately, and
/// BRANCH 2 MUST NOT BUMP: a bump-then-dead-letter implementation would pass a
/// dead-letter-only assertion.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_108_502_above_the_ceiling_dead_letters() {
    let failure = decode_generate_failure(
        502,
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"overloaded","message":"x","retryAfterMs":120000}"#,
    );
    assert_eq!(failure.retry_after_ms, Some(120_000), "the hint is decoded");

    let dir = TestDir::new("classes-502-dead");
    let capture = service(&dir, &failure);
    let capture_id = "019a0000-0000-7f00-8000-00000000d003";
    let report = seed_and_drain(&capture, capture_id).await;

    assert_eq!(report.dead_lettered, 1, "{report:?}");
    assert_eq!(
        capture.inbox().attempts(capture_id).await,
        0,
        "branch 2 dead-letters without bumping"
    );
}

/// A model-output miss is the model's, not the wire's: it stays retryable and
/// is NAMED rather than flattened into `summarizer_failed`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_108_parse_miss_is_retryable() {
    let failure = parse_structured_lenient("p", MEASURED_MARKDOWN_REPLY)
        .expect_err("prose carries no structured value");
    assert_eq!(
        classify_provider_error(&failure).reason,
        "capture_output_not_json",
        "the parse miss is named"
    );

    let dir = TestDir::new("classes-parse-miss");
    let capture = service(&dir, &failure);
    let capture_id = "019a0000-0000-7f00-8000-00000000d004";
    let report = seed_and_drain(&capture, capture_id).await;

    assert_eq!(report.retryable, 1, "{report:?}");
    assert_eq!(capture.inbox().attempts(capture_id).await, 1);
}
