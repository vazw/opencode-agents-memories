//! Issue 108 — retryability reaches the capture ladder, carried by the closed
//! wire token and the decoded detail. Issue 110 — a retry hint SCHEDULES the
//! row (`not_before = now + retryAfterMs`) instead of dead-lettering it, at any
//! magnitude, consuming no attempt.
//!
//! Six drains through the REAL `CaptureService` with a scripted summarizer
//! whose `last_failure()` is produced by the production classifier over a real
//! decoded host failure (never a hand-written reason). Each row asserts the
//! drain report AND the inbox attempt counter (and, on the scheduling rows, the
//! stored deadline), so "bump then dead-letter", "retry forever" and "stamp
//! some other clock" are all excluded.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_ai::provider::{
    ProviderError, decode_generate_failure, remote::structured::parse_structured_lenient,
};
use mem_bridge::capture::{CaptureParams, CaptureService, DrainReport};
use mem_bridge::capture_prompt::{CaptureSummarizer, SummarizerFailure, classify_provider_error};

/// The measured 308-byte markdown reply (the host answered a capture-sized
/// prompt in prose) — the parse-miss input.
const MEASURED_MARKDOWN_REPLY: &str = "## Request\nBound the unbounded inject barrier to 2 seconds and add logging for timeout events.\n\n## Outcome\nImplemented `INJECT_BARRIER_BUDGET` and an `AtomicBool` latch in `engine.rs` to enforce the 2-second timeout and log when the barrier is exceeded.\n\n**Tags**: `rust`, `concurrency`, `timeout`, `logging`";

/// The scheduling rows' pinned "now" (unix millis, unrelated to wall time):
/// every `not_before` assertion is exact because the injected clock never
/// moves on its own.
const T0: i64 = 1_000_000;

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

/// The same service over a store whose ONLY time source is the injected clock
/// (D-110-2): the writer's `now + hint` and the drain's `now` are the same
/// number, so a deadline assertion can be exact.
fn service_with_clock(
    dir: &TestDir,
    failure: &ProviderError,
    clock_ms: Arc<AtomicI64>,
) -> CaptureService {
    let clock: mem_episodic::store::Clock = {
        let c = clock_ms.clone();
        Arc::new(move || c.load(Ordering::Relaxed))
    };
    let store = Arc::new(
        mem_episodic::store::EpisodicStore::new(dir.path().to_path_buf(), dir.store_config())
            .with_clock(clock),
    );
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

/// Issue 110 (REPLACES `test_108_502_above_the_ceiling_dead_letters`): a `502`
/// carrying a 120 s hint is SCHEDULED — `queued`, no attempt consumed,
/// dead-letter count zero — because the 30 s ceiling is retired.
///
/// Target mutants: (a) the ceiling still dead-letters -> `dead_lettered` 1 and
/// `retryable` 0; (b) bump-then-schedule -> `attempts` 1; (c) the writer stamps
/// its own clock instead of `now + hint` -> `not_before` is not exactly
/// `T0 + 120_000`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_110_502_with_a_120s_hint_is_scheduled_not_dead_lettered() {
    let failure = decode_generate_failure(
        502,
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"overloaded","message":"x","retryAfterMs":120000}"#,
    );
    assert_eq!(failure.retry_after_ms, Some(120_000), "the hint is decoded");
    assert!(
        classify_provider_error(&failure).retryable,
        "an overloaded 502 is the retryable class"
    );

    let dir = TestDir::new("classes-502-scheduled");
    let clock_ms = Arc::new(AtomicI64::new(T0));
    let capture = service_with_clock(&dir, &failure, clock_ms);
    let capture_id = "019a0000-0000-7f00-8000-00000000d003";
    let report = seed_and_drain(&capture, capture_id).await;

    assert_eq!(
        report.dead_lettered, 0,
        "the ceiling is retired, nothing is dead-lettered: {report:?}"
    );
    assert_eq!(report.retryable, 1, "{report:?}");
    assert_eq!(
        capture.inbox().attempts(capture_id).await,
        0,
        "a hinted failure schedules without consuming an attempt"
    );
    assert_eq!(
        capture.inbox().not_before(capture_id).await,
        Some(T0 + 120_000),
        "the deadline is exactly the injected now + the hint"
    );
    assert_eq!(
        capture.inbox().queued_count().await.expect("count"),
        1,
        "the row stays queued and keeps its capacity slot"
    );
}

/// Issue 110 (REPLACES `test_108_502_below_the_ceiling_is_retried`): the hint
/// decides the deadline, not the ladder — a 1500 ms hint schedules
/// `T0 + 1_500` and consumes no attempt.
///
/// Target mutant: the hint is dropped and only the ladder runs -> `attempts` 1
/// and `not_before` `None`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_110_502_with_a_1500ms_hint_is_scheduled() {
    let failure = decode_generate_failure(
        502,
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"rate_limit","message":"x","retryAfterMs":1500}"#,
    );
    assert_eq!(
        classify_provider_error(&failure).reason,
        "host_provider_request_error",
        "the retryable 502 class is named"
    );
    assert_eq!(failure.retry_after_ms, Some(1_500), "the hint is decoded");

    let dir = TestDir::new("classes-502-scheduled-short");
    let clock_ms = Arc::new(AtomicI64::new(T0));
    let capture = service_with_clock(&dir, &failure, clock_ms);
    let capture_id = "019a0000-0000-7f00-8000-00000000d002";
    let report = seed_and_drain(&capture, capture_id).await;

    assert_eq!(report.retryable, 1, "{report:?}");
    assert_eq!(
        capture.inbox().attempts(capture_id).await,
        0,
        "the hint path never charges the ladder"
    );
    assert_eq!(
        capture.inbox().not_before(capture_id).await,
        Some(T0 + 1_500),
        "milliseconds, not a fixed backoff"
    );
}

/// Issue 110: a scheduled capture is invisible to the drain until its deadline
/// and due AT it. The clock is frozen between the first two drains, so only
/// the predicate — never time passing — can make the second drain empty.
///
/// Target mutant: `dequeue` ignores `not_before` -> the second drain reports
/// `retryable` 1 instead of 0.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_110_scheduled_capture_is_not_re_attempted_before_its_deadline() {
    let failure = decode_generate_failure(
        502,
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"rate_limit","message":"x","retryAfterMs":1500}"#,
    );
    let dir = TestDir::new("classes-502-deadline");
    let clock_ms = Arc::new(AtomicI64::new(T0));
    let capture = service_with_clock(&dir, &failure, clock_ms.clone());
    let capture_id = "019a0000-0000-7f00-8000-00000000d006";

    let first = seed_and_drain(&capture, capture_id).await;
    assert_eq!(first.retryable, 1, "{first:?}");

    let second = capture.drain_once().await.expect("second drain");
    assert_eq!(
        second,
        DrainReport::default(),
        "a scheduled capture is not re-attempted before its deadline: {second:?}"
    );

    clock_ms.store(T0 + 1_500, Ordering::SeqCst);
    let third = capture.drain_once().await.expect("third drain");
    assert_eq!(
        third.retryable, 1,
        "the deadline makes the row due again: {third:?}"
    );
    assert_eq!(
        capture.inbox().attempts(capture_id).await,
        0,
        "every attempt was scheduled, none consumed"
    );
}

/// Issue 110: `!retryable` is checked BEFORE the hint, so the terminal class
/// still dead-letters on the first drain with NO attempt bump even though the
/// host attached a 120 s hint. That input is reachable in production
/// (`opencode_host.rs` assigns the terminal token for `content_filter` and
/// attaches `retryAfterMs` on both branches).
///
/// Target mutant: the hint check moved before the `!retryable` check -> the row
/// is scheduled (`dead_lettered` 0, `not_before` `Some`) instead of dead.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_110_terminal_502_with_a_hint_still_dead_letters() {
    let failure = decode_generate_failure(
        502,
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"content_filter","message":"x","retryAfterMs":120000}"#,
    );
    let classified = classify_provider_error(&failure);
    assert!(
        !classified.retryable,
        "content_filter is the terminal class"
    );
    assert_eq!(classified.reason, "host_provider_request_error");
    assert_eq!(
        failure.retry_after_ms,
        Some(120_000),
        "the terminal class still carries the host hint"
    );

    let dir = TestDir::new("classes-502-terminal-hint");
    let clock_ms = Arc::new(AtomicI64::new(T0));
    let capture = service_with_clock(&dir, &failure, clock_ms);
    let capture_id = "019a0000-0000-7f00-8000-00000000d005";
    let report = seed_and_drain(&capture, capture_id).await;

    assert_eq!(report.dead_lettered, 1, "{report:?}");
    assert_eq!(
        capture.inbox().attempts(capture_id).await,
        0,
        "the terminal branch consumes no attempt"
    );
    assert_eq!(
        capture.inbox().not_before(capture_id).await,
        None,
        "a dead row is never scheduled"
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
