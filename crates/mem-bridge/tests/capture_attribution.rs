//! W5.3 Step 10 — child/agent attribution oracles.
//!
//! The wire `parent_session_id` is resolved to the project-visible ROW id
//! (the `agent_sessions.parent_session_id` FK target): several agents may
//! share one wire session id, so the row whose `agent_id` matches wins,
//! else the newest visible row. An absent/cross-project/deleted parent is a
//! typed failure, so a child ahead of its parent stays queued for the
//! bounded retry/dead-letter. Contributor identity never moves to tier-1.

mod common;

use std::sync::Arc;

use common::{FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_bridge::capture::{CaptureParams, CaptureService};
use mem_episodic::store::EpisodicStore;

fn params(capture_id: &str, session_id: &str, agent_id: &str) -> CaptureParams {
    CaptureParams {
        session_id: session_id.to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: agent_id.to_string(),
        agent_name: Some(agent_id.to_string()),
        parent_session_id: None,
        transcript: Some("t".to_string()),
        summary: Some(format!("summary {capture_id}")),
        metadata: None,
    }
}

fn child(capture_id: &str, session_id: &str, agent_id: &str, parent_wire: &str) -> CaptureParams {
    CaptureParams {
        parent_session_id: Some(parent_wire.to_string()),
        ..params(capture_id, session_id, agent_id)
    }
}

async fn service(dir: &TestDir) -> CaptureService {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
}

async fn rows(dir: &TestDir) -> Vec<mem_episodic::record::AgentSession> {
    let store = EpisodicStore::new(dir.path().to_path_buf(), dir.store_config());
    store.ready().await.expect("ready");
    store.list_sessions("/srv/proj", 20).await.expect("list")
}

/// The child's `parent_session_id` is the parent's ROW id (never the wire
/// session id) and the parent is project-visible.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_parent_session_id_resolution() {
    let dir = TestDir::new("w53-parent");
    let service = service(&dir).await;

    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000101",
            "wire-parent",
            "agent-1",
        ))
        .await
        .expect("enqueue parent");
    assert_eq!(service.drain_once().await.expect("drain parent").done, 1);

    service
        .enqueue(&child(
            "019a0000-0000-7f00-8000-000000000102",
            "wire-child",
            "agent-1",
            "wire-parent",
        ))
        .await
        .expect("enqueue child");
    assert_eq!(service.drain_once().await.expect("drain child").done, 1);

    let rows = rows(&dir).await;
    assert_eq!(rows.len(), 2, "parent + child");
    let parent = rows
        .iter()
        .find(|row| row.session_id == "wire-parent")
        .expect("the parent row exists");
    let child = rows
        .iter()
        .find(|row| row.session_id == "wire-child")
        .expect("the child row exists");
    assert_eq!(
        child.parent_session_id.as_deref(),
        Some(parent.id.as_str()),
        "the FK target is the parent's ROW id, not the wire session id"
    );
    assert!(!child.orphaned, "a resolved parent leaves the child linked");
}

/// Several agents share one wire session id: the resolver prefers the row
/// with the SAME `agent_id`, else the newest visible row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_multi_agent_unique_rows() {
    let dir = TestDir::new("w53-multi-agent");
    let service = service(&dir).await;

    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000111",
            "shared",
            "agent-1",
        ))
        .await
        .expect("enqueue agent-1");
    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000112",
            "shared",
            "agent-2",
        ))
        .await
        .expect("enqueue agent-2");
    assert_eq!(service.drain_once().await.expect("drain").done, 2);

    let rows = rows(&dir).await;
    assert_eq!(
        rows.len(),
        2,
        "the (project, session, agent) triple is the key"
    );
    let agent_one = rows
        .iter()
        .find(|row| row.agent_id == "agent-1")
        .expect("agent-1 row");
    let agent_two = rows
        .iter()
        .find(|row| row.agent_id == "agent-2")
        .expect("agent-2 row");
    assert_ne!(agent_one.id, agent_two.id, "distinct rows, distinct ids");
    assert_eq!(
        agent_one.capture_id.as_deref(),
        Some("019a0000-0000-7f00-8000-000000000111")
    );
    assert_eq!(
        agent_two.capture_id.as_deref(),
        Some("019a0000-0000-7f00-8000-000000000112")
    );
}

/// A child that arrives BEFORE its parent stays queued (a consumed attempt
/// per pass, no row written); once the parent lands, the child completes and
/// links. A permanently missing parent dead-letters after five attempts.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_child_session_attribution() {
    let dir = TestDir::new("w53-child-attribution");
    let service = service(&dir).await;

    let child_capture = "019a0000-0000-7f00-8000-000000000121";
    service
        .enqueue(&child(
            child_capture,
            "wire-child",
            "agent-1",
            "wire-parent-late",
        ))
        .await
        .expect("enqueue child");

    let first = service.drain_once().await.expect("child-first drain");
    assert_eq!(first.retryable, 1, "a missing parent consumes one attempt");
    assert_eq!(first.done, 0);
    assert_eq!(service.inbox().attempts(child_capture).await, 1);
    assert_eq!(service.inbox().queued_count().await.expect("count"), 1);
    assert!(
        rows(&dir).await.is_empty(),
        "no row is written without the parent"
    );

    // The parent lands (a later capture for the same wire id).
    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000122",
            "wire-parent-late",
            "agent-1",
        ))
        .await
        .expect("enqueue parent");
    // The FIFO batch is child-first: this pass completes the parent and
    // consumes the child's second attempt; the next pass completes the child.
    let second = service.drain_once().await.expect("second drain");
    assert_eq!(second.done, 1, "the parent completed");
    let third = service.drain_once().await.expect("third drain");
    assert_eq!(
        third.done, 1,
        "the child completes once the parent is visible"
    );

    let final_rows = rows(&dir).await;
    let parent = final_rows
        .iter()
        .find(|row| row.session_id == "wire-parent-late")
        .expect("parent");
    let child_row = final_rows
        .iter()
        .find(|row| row.session_id == "wire-child")
        .expect("child");
    assert_eq!(
        child_row.parent_session_id.as_deref(),
        Some(parent.id.as_str())
    );

    // A permanently absent parent dead-letters (never a silent drop).
    let orphan_capture = "019a0000-0000-7f00-8000-000000000123";
    service
        .enqueue(&child(
            orphan_capture,
            "wire-orphan",
            "agent-1",
            "wire-never",
        ))
        .await
        .expect("enqueue orphan");
    for _ in 0..4 {
        let report = service.drain_once().await.expect("orphan drain");
        assert_eq!(report.retryable, 1, "attempts 1..4 stay retryable");
    }
    let fifth = service.drain_once().await.expect("orphan fifth drain");
    assert_eq!(
        fifth.dead_lettered, 1,
        "the fifth consumed attempt dead-letters"
    );
    assert!(
        rows(&dir)
            .await
            .iter()
            .all(|row| row.session_id != "wire-orphan"),
        "a dead-lettered child never wrote a row"
    );
}
