//! W5.3 Step 10 — capture concurrency oracle.
//!
//! Drain passes are serialized behind ONE Tokio mutex: two concurrent
//! `drain_once` futures never have two tier-1 writes in flight, and every
//! enqueued capture is processed exactly once (no duplicate episodic row, no
//! duplicate tier-1 row). The enqueue path itself stays bounded/atomic.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::{FixedScopeResolver, TestDir};
use mem_bridge::capture::{
    CaptureParams, CaptureService, CaptureSummaryInput, CaptureWriterError, Tier1WriteOutcome,
    Tier1Writer,
};
use mem_bridge::project_scope::ResolvedProjectScope;
use mem_embed::embedder::BoxFuture;
use mem_episodic::store::EpisodicStore;

/// A tier-1 writer that records CONCURRENT entry: if a second write begins
/// while another is in flight, the drains interleaved.
struct OverlapDetectingWriter {
    in_flight: AtomicUsize,
    overlap: AtomicBool,
    writes: AtomicUsize,
}

impl Tier1Writer for OverlapDetectingWriter {
    fn write<'a>(
        &'a self,
        _scope: &'a ResolvedProjectScope,
        _input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, CaptureWriterError>> {
        let observed = self.in_flight.fetch_add(1, Ordering::SeqCst);
        if observed > 0 {
            self.overlap.store(true, Ordering::SeqCst);
        }
        Box::pin(async move {
            // Widen the window: a second pass would observe the overlap.
            mem_store::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(Tier1WriteOutcome::Written)
        })
    }
}

fn params(index: usize) -> CaptureParams {
    CaptureParams {
        session_id: format!("sess-{index}"),
        capture_id: format!("019a0000-0000-7f00-8000-{index:012}"),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: None,
        parent_session_id: None,
        transcript: Some("t".to_string()),
        summary: Some(format!("summary {index}")),
        metadata: None,
    }
}

/// Concurrent `drain_once` calls are serialized: no two tier-1 writes are
/// ever in flight together, and each capture is written exactly once.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_auto_capture_serializes_concurrent_captures() {
    let dir = TestDir::new("w53-concurrency");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let writer = Arc::new(OverlapDetectingWriter {
        in_flight: AtomicUsize::new(0),
        overlap: AtomicBool::new(false),
        writes: AtomicUsize::new(0),
    });
    let service = Arc::new(CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        writer.clone(),
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    ));

    for index in 0..6 {
        service.enqueue(&params(index)).await.expect("enqueue");
    }

    // Three concurrent drain passes over the same inbox.
    let (a, b, c) = mem_store::tokio::join!(
        service.drain_once(),
        service.drain_once(),
        service.drain_once()
    );
    let reports = [
        a.expect("drain a"),
        b.expect("drain b"),
        c.expect("drain c"),
    ];
    let done: u64 = reports.iter().map(|report| report.done).sum();
    let failed: u64 = reports
        .iter()
        .map(|report| report.retryable + report.dead_lettered)
        .sum();
    assert_eq!(done, 6, "every enqueued capture completed exactly once");
    assert_eq!(failed, 0, "no entry was retried or dead-lettered");
    assert!(
        !writer.overlap.load(Ordering::SeqCst),
        "drain passes must not overlap: the tier-1 seam saw two writes in flight"
    );
    assert_eq!(
        writer.writes.load(Ordering::SeqCst),
        6,
        "one tier-1 write per capture"
    );
    assert_eq!(service.inbox().queued_count().await.expect("count"), 0);

    let sessions = EpisodicStore::new(dir.path().to_path_buf(), dir.store_config());
    sessions.ready().await.expect("ready");
    let rows = sessions.list_sessions("/srv/proj", 50).await.expect("list");
    assert_eq!(
        rows.len(),
        6,
        "one episodic row per capture, never a duplicate"
    );
}
