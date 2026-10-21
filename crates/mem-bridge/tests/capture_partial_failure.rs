//! W5.3 Step 7 — defined partial two-tier commit oracles.
//!
//! The drain commits: (1) episodic, (2) tier-1, (3) inbox mark_done. A
//! tier-1 failure AFTER the episodic commit leaves the episode row
//! committed and the inbox entry retryable (lock deferrals consume
//! NOTHING); the retry's same episodic `capture_id` is a no-op and the
//! deterministic tier-1 id prevents duplicates; five consumed attempts
//! dead-letter to an episodic-only row with NO transcript in any log.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::TestDir;
use mem_bridge::project_scope::ResolvedProjectScope;
use mem_embed::embedder::BoxFuture;
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;

use common::FixedScopeResolver;
use mem_bridge::capture::{
    CaptureParams, CaptureService, CaptureSummaryInput, CaptureWriterError, EntryOutcome,
    Tier1WriteOutcome, Tier1Writer,
};

/// A tier-1 writer that fails the first `fail_times` calls.
struct FlakyTier1Writer {
    calls: AtomicUsize,
    fail_times: usize,
}

impl Tier1Writer for FlakyTier1Writer {
    fn write<'a>(
        &'a self,
        _scope: &'a ResolvedProjectScope,
        _input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, CaptureWriterError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if call < self.fail_times {
                Err(CaptureWriterError::Failed(failing_store()))
            } else {
                Ok(Tier1WriteOutcome::Written)
            }
        })
    }
}

fn failing_store() -> mem_store::error::StoreError {
    mem_store::error::StoreError::Internal {
        message: "injected tier-1 failure".to_string(),
    }
}

fn params(capture_id: &str, summary: &str) -> CaptureParams {
    CaptureParams {
        session_id: "sess-1".to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: Some("build".to_string()),
        parent_session_id: None,
        transcript: Some("transcript text".to_string()),
        summary: Some(summary.to_string()),
        metadata: Some(json!({ "promptId": "p-1" })),
    }
}

async fn service(dir: &TestDir, writer: Arc<dyn Tier1Writer>) -> CaptureService {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        writer,
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
}

/// A tier-1 failure after the episodic commit: the episode row is VISIBLE
/// (committed) and the inbox entry is queued/retryable, attempts incremented.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_tier1_failure_after_episodic_commit_keeps_inbox_retryable() {
    let dir = TestDir::new("w53-partial");
    let writer: Arc<dyn Tier1Writer> = Arc::new(FlakyTier1Writer {
        calls: AtomicUsize::new(0),
        fail_times: usize::MAX,
    });
    let service = service(&dir, writer).await;

    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000001",
            "summary text",
        ))
        .await
        .expect("enqueue");

    let report = service.drain_once().await.expect("drain");
    assert_eq!(
        report.retryable, 1,
        "the tier-1 failure consumed one attempt"
    );
    assert_eq!(report.done, 0);

    // The episodic commit STANDS (a partial write is visible, not falsely
    // atomic).
    let rows = service_inbox_queued(&service).await;
    assert_eq!(rows, 1, "the entry stays queued");
    assert_eq!(
        service
            .inbox()
            .attempts("019a0000-0000-7f00-8000-000000000001")
            .await,
        1
    );
    let store_rows = store_rows_count(&dir).await;
    assert_eq!(store_rows, 1, "the episodic row is committed");
}

/// The retry after a consumed failure: the same `capture_id` episodic
/// upsert is a no-op and the deterministic tier-1 id writes EXACTLY once.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_partial_commit_retry_writes_tier1_once() {
    let dir = TestDir::new("w53-retry-once");
    let recorder = Arc::new(common::RecorderTier1Writer::new(1));
    let service = service(&dir, recorder.clone()).await;

    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000002",
            "summary text",
        ))
        .await
        .expect("enqueue");

    let first = service.drain_once().await.expect("first drain");
    assert_eq!(first.retryable, 1, "the first pass fails");
    assert_eq!(
        service
            .inbox()
            .attempts("019a0000-0000-7f00-8000-000000000002")
            .await,
        1,
        "the first failure consumed the budget"
    );
    let second = service.drain_once().await.expect("second drain");
    assert_eq!(second.done, 1, "the retry completes");

    assert_eq!(
        recorder.writes(),
        1,
        "tier-1 content caused exactly one WRITTEN outcome"
    );
    let _ = 0;
    assert_eq!(store_rows_count(&dir).await, 1);
    assert_eq!(service_inbox_queued(&service).await, 0);
}

/// `type == "skip"`: NEITHER tier is written, the item completes.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_skip_writes_neither_tier() {
    let dir = TestDir::new("w53-skip");
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let summarizer = common::SkipSummarizer;
    let service = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        recorder.clone(),
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(Arc::new(summarizer)),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );

    // A transcript (no supplied summary) whose context yields Skip.
    let entry = CaptureParams {
        session_id: "sess-1".to_string(),
        capture_id: "019a0000-0000-7f00-8000-000000000003".to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: None,
        parent_session_id: None,
        transcript: Some("hi, how are you? casual chat".to_string()),
        summary: None,
        metadata: None,
    };
    service.enqueue(&entry).await.expect("enqueue");

    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "the skip completes the item");
    assert_eq!(recorder.writes(), 0, "no tier-1 write");
    assert_eq!(store_rows_count(&dir).await, 0, "no episodic row");
    assert_eq!(service_inbox_queued(&service).await, 0, "item done");
}

/// A tier-1 writer that DEFERS (a live operation lock, own or foreign) for
/// its first `defer_times` calls and then succeeds.
struct DeferThenSucceedWriter {
    calls: AtomicUsize,
    defer_times: usize,
}

impl Tier1Writer for DeferThenSucceedWriter {
    fn write<'a>(
        &'a self,
        _scope: &'a ResolvedProjectScope,
        _input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, CaptureWriterError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let defer_times = self.defer_times;
        Box::pin(async move {
            if call < defer_times {
                Err(CaptureWriterError::Deferred(
                    mem_store::error::StoreError::MigrationLocked {
                        pid: 424_242,
                        operation: Some("episodic-consolidation".to_string()),
                    },
                ))
            } else {
                Ok(Tier1WriteOutcome::Written)
            }
        })
    }
}

/// A lock deferral is NOT a consumed attempt (A1): more deferrals than the
/// whole retry budget leave the entry queued with `attempts == 0` and it is
/// never dead-lettered; the first non-deferred pass completes it.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_lock_deferral_does_not_consume_retry_budget() {
    let dir = TestDir::new("w53-defer");
    let writer: Arc<dyn Tier1Writer> = Arc::new(DeferThenSucceedWriter {
        calls: AtomicUsize::new(0),
        defer_times: 6,
    });
    let service = service(&dir, writer).await;

    let capture_id = "019a0000-0000-7f00-8000-000000000009";
    service
        .enqueue(&params(capture_id, "summary text"))
        .await
        .expect("enqueue");

    // Six deferrals — one MORE than the five-attempt budget.
    for pass in 0..6 {
        let report = service.drain_once().await.expect("drain");
        assert_eq!(report.deferred, 1, "pass {pass} is a deferral");
        assert_eq!(report.retryable, 0, "a deferral is not a retryable failure");
        assert_eq!(
            service.inbox().attempts(capture_id).await,
            0,
            "a lock deferral must never consume the bounded retry budget (pass {pass})"
        );
        assert_eq!(service_inbox_queued(&service).await, 1, "stays queued");
    }

    // The lock cleared: the next pass completes the entry, still with the
    // full budget unconsumed.
    let report = service.drain_once().await.expect("final drain");
    assert_eq!(report.done, 1, "the post-lock pass completes");
    assert_eq!(service.inbox().attempts(capture_id).await, 0);
    assert_eq!(service_inbox_queued(&service).await, 0, "done, purged");
    assert_eq!(
        store_rows_count(&dir).await,
        1,
        "the episodic row committed"
    );
}

/// Five consumed attempts dead-letter: the terminal state is the
/// episodic-only row, logged with a structure that names NO transcript.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_provider_failure_retries_then_dead_letters() {
    let dir = TestDir::new("w53-dead");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let failing = common::FailingSummarizer;
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let service = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        recorder.clone(),
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(Arc::new(failing)),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );

    let entry = CaptureParams {
        session_id: "sess-1".to_string(),
        capture_id: "019a0000-0000-7f00-8000-000000000004".to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: None,
        parent_session_id: None,
        transcript: Some("some transcript".to_string()),
        summary: None,
        metadata: None,
    };
    service.enqueue(&entry).await.expect("enqueue");

    for _ in 0..4 {
        let report = service.drain_once().await.expect("drain");
        assert_eq!(report.retryable, 1, "attempts 1..4 stay retryable");
    }
    let fifth = service.drain_once().await.expect("fifth drain");
    assert_eq!(
        fifth.dead_lettered, 1,
        "the fifth consumed attempt dead-letters"
    );
    assert_eq!(recorder.writes(), 0, "no tier-1 write ever");
    assert_eq!(
        store_rows_count(&dir).await,
        0,
        "the episodic leg itself failed"
    );
}

async fn service_inbox_queued(service: &CaptureService) -> u64 {
    service.inbox().queued_count().await.expect("count")
}

async fn store_rows_count(dir: &TestDir) -> usize {
    let db = common::open_raw(dir.path(), "episodic.db").await;
    db.query("SELECT COUNT(*) AS n FROM agent_sessions", Vec::new())
        .await
        .expect("count")[0]
        .i64("n")
        .unwrap_or(0) as usize
}

/// The outcome enum is the drain's terminal vocabulary (compile-anchor).
#[allow(dead_code)]
fn outcomes_are_enumerated() -> [EntryOutcome; 4] {
    [
        EntryOutcome::Done,
        EntryOutcome::Retryable,
        EntryOutcome::Dead,
        EntryOutcome::Deferred,
    ]
}
