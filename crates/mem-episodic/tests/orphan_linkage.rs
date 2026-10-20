//! W4.2 Steps 4–5 — soft delete, visibility, orphan prevention and
//! parent/child linkage (net-new, R10: no upstream oracle).
//!
//! Intent ported from the upstream `memory-timeline-orphan` suite: a record
//! whose linked parent is gone is **kept** in the timeline and flagged, never
//! dropped. Upstream's subject is memory↔prompt pairs, so the port is
//! intent-level (and stronger for episodic rows: ordering is chronological,
//! and the D-12 invariant is asserted directly).

mod common;

use common::{FakeEmbedder, TestDir};
use mem_episodic::error::EpisodicError;
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_episodic::timeline::TimelineFilter;
use mem_store::db::{Value, query_one};

const DIMS: usize = 8;

fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMS];
    vector[index] = 1.0;
    vector
}

fn fixture(prefix: &str) -> (TestDir, EpisodicStore) {
    let dir = TestDir::new(prefix);
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    (dir, store)
}

/// Inserts a child row bound to an already-visible parent row id through the
/// public write surface (the FK proves existence; the store proves scope).
async fn insert_child(
    store: &EpisodicStore,
    project_id: &str,
    session_id: &str,
    parent_row_id: &str,
    vector: Vec<f32>,
) -> String {
    let embedder = FakeEmbedder::with_vector(vector);
    let new = NewAgentSession {
        project_id: project_id.to_string(),
        session_id: session_id.to_string(),
        agent_id: "agent-1".to_string(),
        summary: "child session".to_string(),
        parent_session_id: Some(parent_row_id.to_string()),
        ..Default::default()
    };
    store
        .insert_session(new, &embedder)
        .await
        .expect("insert child session")
}

async fn corrupt_metadata(store: &EpisodicStore, id: &str) {
    let db = store.ready().await.expect("ready");
    db.execute(
        "UPDATE agent_sessions SET metadata = ? WHERE id = ?",
        vec![
            Value::Text("{not valid json".to_string()),
            Value::Text(id.to_string()),
        ],
    )
    .await
    .expect("corrupt metadata");
}

/// Raw stored parent link (bypasses the visibility rule on purpose).
async fn stored_parent(store: &EpisodicStore, id: &str) -> Option<String> {
    let db = store.ready().await.expect("ready");
    query_one(
        db.as_ref(),
        "SELECT parent_session_id FROM agent_sessions WHERE id = ?",
        vec![Value::Text(id.to_string())],
    )
    .await
    .expect("select parent")
    .expect("row present")
    .text("parent_session_id")
}

fn assert_not_found(error: EpisodicError, expected_entity: &str) {
    match error {
        EpisodicError::NotFound { entity } => assert_eq!(entity, expected_entity),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

fn assert_validation_field(error: EpisodicError, expected: &str) {
    match error {
        EpisodicError::Validation { field, .. } => assert_eq!(field, expected),
        other => panic!("expected Validation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Step 4 — soft delete, visibility, orphan prevention
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_session_returns_true_then_false() {
    let (_dir, store) = fixture("soft-delete-idempotent");
    let (id, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "doomed",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &id)
            .await
            .expect("first soft delete")
    );
    assert!(
        !store
            .soft_delete_session("project-a", &id)
            .await
            .expect("second soft delete")
    );
    assert!(
        store.get_session(&id).await.expect("get").is_none(),
        "a soft-deleted row is invisible by id"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_session_is_project_scoped() {
    let (_dir, store) = fixture("soft-delete-scope");
    let (id, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    // A wrong-project delete never writes (and never leaks existence).
    assert!(
        !store
            .soft_delete_session("project-b", &id)
            .await
            .expect("wrong project")
    );
    assert!(
        store.get_session(&id).await.expect("get").is_some(),
        "the row survives a wrong-project delete"
    );
    assert_eq!(
        store
            .count_sessions(Some("project-a"))
            .await
            .expect("count"),
        1
    );

    // A completely unknown id is likewise a no-op.
    assert!(
        !store
            .soft_delete_session("project-a", "missing-id")
            .await
            .expect("missing id")
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_session_hides_row_from_list() {
    let (_dir, store) = fixture("soft-delete-list");
    let (doomed, _t) = common::insert_row(
        &store,
        "project-a",
        "s-doomed",
        "agent-1",
        "doomed",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (kept, _t) = common::insert_row(
        &store,
        "project-a",
        "s-kept",
        "agent-1",
        "kept",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &doomed)
            .await
            .expect("soft delete")
    );

    let listed: Vec<String> = store
        .list_sessions("project-a", 50)
        .await
        .expect("list")
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(listed, vec![kept.clone()]);
    assert!(!listed.contains(&doomed));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_session_hides_row_from_search() {
    let (_dir, store) = fixture("soft-delete-search");
    let (doomed, _t) = common::insert_row(
        &store,
        "project-a",
        "s-doomed",
        "agent-1",
        "doomed",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (kept, _t) = common::insert_row(
        &store,
        "project-a",
        "s-kept",
        "agent-1",
        "kept",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &doomed)
            .await
            .expect("soft delete")
    );

    let hits: Vec<String> = store
        .search_sessions("project-a", &unit_vector(0), 50)
        .await
        .expect("search")
        .into_iter()
        .map(|hit| hit.session.id)
        .collect();
    assert_eq!(hits, vec![kept]);
    assert!(!hits.contains(&doomed));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_session_hides_row_from_get_session() {
    let (_dir, store) = fixture("soft-delete-get");
    let (id, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &id)
            .await
            .expect("soft delete")
    );
    assert!(
        store.get_session(&id).await.expect("get").is_none(),
        "get_session is a visible-only read"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_session_excludes_row_from_project_count() {
    let (_dir, store) = fixture("soft-delete-count");
    let (doomed, _t) = common::insert_row(
        &store,
        "project-a",
        "s-doomed",
        "agent-1",
        "doomed",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    common::insert_row(
        &store,
        "project-a",
        "s-kept",
        "agent-1",
        "kept",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &doomed)
            .await
            .expect("soft delete")
    );
    assert_eq!(
        store
            .count_sessions(Some("project-a"))
            .await
            .expect("project count"),
        1,
        "the project count uses the visibility rule"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_session_keeps_row_visible_to_raw_diagnostics() {
    let (_dir, store) = fixture("soft-delete-diagnostics");
    let (id, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &id)
            .await
            .expect("soft delete")
    );

    // The two documented raw seams stay unchanged (D-11): the global count is
    // diagnostics and the embedding read-back is the re-embed seam.
    assert_eq!(store.count_sessions(None).await.expect("global"), 1);
    assert!(
        store
            .embedding_for(&id)
            .await
            .expect("embedding_for")
            .is_some(),
        "the row and its embedding survive a soft delete"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_session_detail_returns_full_record() {
    let (_dir, store) = fixture("session-detail-full");
    let (id, created_at) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "detailed row",
        vec!["src/lib.rs".to_string()],
        unit_vector(0),
    )
    .await;

    let session = store
        .session_detail("project-a", &id)
        .await
        .expect("session_detail")
        .expect("row present");
    assert_eq!(session.id, id);
    assert_eq!(session.project_id, "project-a");
    assert_eq!(session.session_id, "s-1");
    assert_eq!(session.summary, "detailed row");
    assert_eq!(session.metadata.files_touched, vec!["src/lib.rs"]);
    assert_eq!(session.embedding, unit_vector(0));
    assert_eq!(session.created_at, created_at);
    assert!(session.deleted_at.is_none());
    assert!(!session.orphaned);

    // Wrong project ⇒ invisible, and an unknown id is a clean `None`.
    assert!(
        store
            .session_detail("project-b", &id)
            .await
            .expect("wrong project")
            .is_none()
    );
    assert!(
        store
            .session_detail("project-a", "missing-id")
            .await
            .expect("missing")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_session_detail_hides_soft_deleted_row() {
    let (_dir, store) = fixture("session-detail-hidden");
    let (id, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &id)
            .await
            .expect("soft delete")
    );
    assert!(
        store
            .session_detail("project-a", &id)
            .await
            .expect("session_detail")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_session_detail_rejects_corrupt_metadata() {
    let (_dir, store) = fixture("session-detail-corrupt");
    let (id, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    corrupt_metadata(&store, &id).await;

    // By-id reads fail closed (F5) — nothing to skip.
    let error = store
        .session_detail("project-a", &id)
        .await
        .expect_err("corrupt metadata rejected");
    assert!(matches!(error, EpisodicError::Json { .. }), "got {error:?}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_excludes_soft_deleted_rows() {
    let (_dir, store) = fixture("timeline-soft-deleted");
    let (doomed, _t) = common::insert_row(
        &store,
        "project-a",
        "s-doomed",
        "agent-1",
        "doomed",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (kept, _t) = common::insert_row(
        &store,
        "project-a",
        "s-kept",
        "agent-1",
        "kept",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &doomed)
            .await
            .expect("soft delete")
    );

    let ids: Vec<String> = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("timeline")
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    assert_eq!(ids, vec![kept]);
    assert!(!ids.contains(&doomed));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_excludes_soft_deleted_rows() {
    let (_dir, store) = fixture("semantic-soft-deleted");
    let (doomed, _t) = common::insert_row(
        &store,
        "project-a",
        "s-doomed",
        "agent-1",
        "doomed",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (kept, _t) = common::insert_row(
        &store,
        "project-a",
        "s-kept",
        "agent-1",
        "kept",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &doomed)
            .await
            .expect("soft delete")
    );

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let ids: Vec<String> = store
        .semantic_timeline("project-a", "query", &embedder, &TimelineFilter::default())
        .await
        .expect("semantic timeline")
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    assert_eq!(ids, vec![kept]);
    assert!(!ids.contains(&doomed));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_parent_flags_children_orphaned() {
    let (_dir, store) = fixture("soft-delete-parent-flags");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let child = insert_child(&store, "project-a", "s-child", &parent, unit_vector(1)).await;

    assert!(
        store
            .soft_delete_session("project-a", &parent)
            .await
            .expect("soft delete parent")
    );

    let stored_child = store
        .get_session(&child)
        .await
        .expect("get child")
        .expect("child stays visible");
    assert!(stored_child.orphaned, "the child is flagged (D-12)");
    assert_eq!(
        stored_child.parent_session_id.as_deref(),
        Some(parent.as_str()),
        "a soft delete keeps the stored link; the flag is the signal"
    );
    assert!(
        store
            .get_session(&parent)
            .await
            .expect("get parent")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_keeps_entry_whose_parent_is_gone() {
    let (_dir, store) = fixture("timeline-orphan-kept");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let child = insert_child(&store, "project-a", "s-child", &parent, unit_vector(1)).await;

    // Hard delete: the FK nulls the child's link, the flag is already set.
    assert!(store.delete_session(&parent).await.expect("delete parent"));

    // Upstream intent: a record whose linked parent is missing is KEPT.
    let listed = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("timeline");
    let ids: Vec<String> = listed.iter().map(|summary| summary.id.clone()).collect();
    assert_eq!(ids, vec![child.clone()], "the orphaned entry survives");
    let summary = listed
        .iter()
        .find(|summary| summary.id == child)
        .expect("entry");
    assert!(summary.orphaned, "the entry surfaces the flag");
    assert_eq!(summary.parent_session_id, None);
    common::assert_no_dangling_parent_links(&store, &listed).await;
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_keeps_parent_and_child_together() {
    let (_dir, store) = fixture("timeline-pairs-together");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let child = insert_child(&store, "project-a", "s-child", &parent, unit_vector(1)).await;

    let listed = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("timeline");
    let ids: Vec<String> = listed.iter().map(|summary| summary.id.clone()).collect();
    assert_eq!(
        ids,
        vec![child.clone(), parent.clone()],
        "both records render in one result set (chronological order)"
    );
    let child_summary = listed
        .iter()
        .find(|summary| summary.id == child)
        .expect("child present");
    assert_eq!(
        child_summary.parent_session_id.as_deref(),
        Some(parent.as_str()),
        "the child's link resolves inside the same result set"
    );
    assert!(!child_summary.orphaned);
    common::assert_no_dangling_parent_links(&store, &listed).await;
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_after_soft_delete_parent_has_no_dangling_reference() {
    let (_dir, store) = fixture("timeline-soft-delete-dangling");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let child = insert_child(&store, "project-a", "s-child", &parent, unit_vector(1)).await;

    assert!(
        store
            .soft_delete_session("project-a", &parent)
            .await
            .expect("soft delete parent")
    );

    let listed = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("timeline");
    let ids: Vec<String> = listed.iter().map(|summary| summary.id.clone()).collect();
    assert_eq!(ids, vec![child], "the parent is gone, the child is kept");
    assert!(listed[0].orphaned, "and it is flagged");
    common::assert_no_dangling_parent_links(&store, &listed).await;
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_after_hard_delete_parent_has_no_dangling_reference() {
    let (_dir, store) = fixture("timeline-hard-delete-dangling");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let child = insert_child(&store, "project-a", "s-child", &parent, unit_vector(1)).await;

    assert!(store.delete_session(&parent).await.expect("delete parent"));

    let listed = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("timeline");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, child);
    assert!(listed[0].orphaned);
    assert_eq!(
        listed[0].parent_session_id, None,
        "the FK ON DELETE SET NULL clears the stored link"
    );
    common::assert_no_dangling_parent_links(&store, &listed).await;
}

// ---------------------------------------------------------------------------
// Step 5 — parent/child linking + store seams
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_sets_parent_and_clears_orphan_flag() {
    let (_dir, store) = fixture("link-parent-clears-orphan");
    let (first_parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent-1",
        "agent-1",
        "first parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let child = insert_child(
        &store,
        "project-a",
        "s-child",
        &first_parent,
        unit_vector(1),
    )
    .await;

    // Sever the first link so the child really carries the orphan flag.
    assert!(
        store
            .delete_session(&first_parent)
            .await
            .expect("delete first parent")
    );
    assert!(
        store
            .get_session(&child)
            .await
            .expect("get")
            .expect("child visible")
            .orphaned
    );

    let (second_parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent-2",
        "agent-1",
        "second parent",
        Vec::new(),
        unit_vector(2),
    )
    .await;

    store
        .link_parent("project-a", &child, &second_parent)
        .await
        .expect("link parent");

    let relinked = store
        .get_session(&child)
        .await
        .expect("get")
        .expect("child visible");
    assert_eq!(
        relinked.parent_session_id.as_deref(),
        Some(second_parent.as_str())
    );
    assert!(!relinked.orphaned, "the link clears the orphan flag");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_rejects_self_link() {
    let (_dir, store) = fixture("link-self");
    let (child, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let error = store
        .link_parent("project-a", &child, &child)
        .await
        .expect_err("self link rejected");
    assert_validation_field(error, "parent_session_id");
    assert_eq!(stored_parent(&store, &child).await, None);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_rejects_cross_project_parent() {
    let (_dir, store) = fixture("link-cross-project");
    let (child, _t) = common::insert_row(
        &store,
        "project-a",
        "s-child",
        "agent-1",
        "child",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (foreign_parent, _t) = common::insert_row(
        &store,
        "project-b",
        "s-parent",
        "agent-1",
        "foreign parent",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    let error = store
        .link_parent("project-a", &child, &foreign_parent)
        .await
        .expect_err("cross-project parent rejected");
    assert_validation_field(error, "parent_session_id");
    assert_eq!(
        stored_parent(&store, &child).await,
        None,
        "no write on rejection"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_rejects_invisible_child() {
    let (_dir, store) = fixture("link-invisible-child");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    // Variant a: a nonexistent child id.
    let error = store
        .link_parent("project-a", "missing-child", &parent)
        .await
        .expect_err("missing child rejected");
    assert_not_found(error, "child_session");

    // Variant b: a soft-deleted child row — the link stays untouched.
    let (deleted_child, _t) = common::insert_row(
        &store,
        "project-a",
        "s-deleted-child",
        "agent-1",
        "deleted child",
        Vec::new(),
        unit_vector(1),
    )
    .await;
    assert!(
        store
            .soft_delete_session("project-a", &deleted_child)
            .await
            .expect("soft delete child")
    );
    let error = store
        .link_parent("project-a", &deleted_child, &parent)
        .await
        .expect_err("soft-deleted child rejected");
    assert_not_found(error, "child_session");
    assert_eq!(
        stored_parent(&store, &deleted_child).await,
        None,
        "the invisible child keeps its stored link"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_rejects_missing_parent() {
    let (_dir, store) = fixture("link-missing-parent");
    let (child, _t) = common::insert_row(
        &store,
        "project-a",
        "s-child",
        "agent-1",
        "child",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let error = store
        .link_parent("project-a", &child, "missing-parent")
        .await
        .expect_err("missing parent rejected");
    assert_not_found(error, "parent_session");
    assert_eq!(stored_parent(&store, &child).await, None);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_rejects_soft_deleted_parent() {
    let (_dir, store) = fixture("link-soft-deleted-parent");
    let (child, _t) = common::insert_row(
        &store,
        "project-a",
        "s-child",
        "agent-1",
        "child",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    assert!(
        store
            .soft_delete_session("project-a", &parent)
            .await
            .expect("soft delete parent")
    );
    let error = store
        .link_parent("project-a", &child, &parent)
        .await
        .expect_err("soft-deleted parent rejected");
    assert_not_found(error, "parent_session");
    assert_eq!(stored_parent(&store, &child).await, None);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_rejects_cycle() {
    let (_dir, store) = fixture("link-cycle");
    let (r1, _t) = common::insert_row(
        &store,
        "project-a",
        "s-1",
        "agent-1",
        "one",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (r2, _t) = common::insert_row(
        &store,
        "project-a",
        "s-2",
        "agent-1",
        "two",
        Vec::new(),
        unit_vector(1),
    )
    .await;
    let (r3, _t) = common::insert_row(
        &store,
        "project-a",
        "s-3",
        "agent-1",
        "three",
        Vec::new(),
        unit_vector(2),
    )
    .await;

    store
        .link_parent("project-a", &r2, &r1)
        .await
        .expect("r2 -> r1");
    store
        .link_parent("project-a", &r3, &r2)
        .await
        .expect("r3 -> r2");

    // r1 -> r3 would close r1 -> r3 -> r2 -> r1.
    let error = store
        .link_parent("project-a", &r1, &r3)
        .await
        .expect_err("cycle rejected");
    assert_validation_field(error, "parent_session_id");
    assert_eq!(
        stored_parent(&store, &r1).await,
        None,
        "the cycle-creating write never happened"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_link_parent_allows_deep_chain() {
    let (_dir, store) = fixture("link-deep-chain");
    let mut ids = Vec::new();
    for index in 0..10 {
        let (id, _t) = common::insert_row(
            &store,
            "project-a",
            &format!("s-{index}"),
            "agent-1",
            "chain node",
            Vec::new(),
            unit_vector(index % DIMS),
        )
        .await;
        ids.push(id);
    }

    for window in ids.windows(2) {
        store
            .link_parent("project-a", &window[1], &window[0])
            .await
            .expect("chain link");
    }
    assert_eq!(
        stored_parent(&store, &ids[9]).await,
        Some(ids[8].clone()),
        "the deepest link is stored"
    );
    assert_eq!(stored_parent(&store, &ids[0]).await, None);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_unlink_parent_clears_link_and_ignores_soft_deleted_child() {
    let (_dir, store) = fixture("unlink-parent");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let child = insert_child(&store, "project-a", "s-child", &parent, unit_vector(1)).await;

    assert!(
        store
            .unlink_parent("project-a", &child)
            .await
            .expect("first unlink")
    );
    assert_eq!(stored_parent(&store, &child).await, None);
    assert!(
        !store
            .unlink_parent("project-a", &child)
            .await
            .expect("second unlink"),
        "unlink is idempotent"
    );

    // A soft-deleted child can never match the `deleted_at IS NULL` guard.
    let doomed = insert_child(&store, "project-a", "s-doomed", &parent, unit_vector(2)).await;
    assert!(
        store
            .soft_delete_session("project-a", &doomed)
            .await
            .expect("soft delete child")
    );
    assert!(
        !store
            .unlink_parent("project-a", &doomed)
            .await
            .expect("unlink invisible child"),
        "an invisible child yields false"
    );
    assert_eq!(
        stored_parent(&store, &doomed).await.as_deref(),
        Some(parent.as_str()),
        "the invisible child's stored link is untouched"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_rejects_missing_parent_id() {
    let (_dir, store) = fixture("insert-missing-parent");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let new = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "s-child".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "child".to_string(),
        parent_session_id: Some("missing-parent".to_string()),
        ..Default::default()
    };

    let error = store
        .insert_session(new, &embedder)
        .await
        .expect_err("missing parent rejected");
    assert_not_found(error, "parent_session");
    assert_eq!(store.count_sessions(None).await.expect("count"), 0);
    assert!(
        embedder.calls().is_empty(),
        "the parent pre-check runs before the embedder"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_rejects_parent_from_another_project() {
    let (_dir, store) = fixture("insert-foreign-parent");
    let (foreign_parent, _t) = common::insert_row(
        &store,
        "project-b",
        "s-parent",
        "agent-1",
        "foreign parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(unit_vector(1));
    let new = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "s-child".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "child".to_string(),
        parent_session_id: Some(foreign_parent),
        ..Default::default()
    };

    let error = store
        .insert_session(new, &embedder)
        .await
        .expect_err("cross-project parent rejected");
    assert_validation_field(error, "parent_session_id");
    assert_eq!(
        store.count_sessions(None).await.expect("count"),
        1,
        "only the foreign parent row exists"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_rejects_soft_deleted_parent() {
    let (_dir, store) = fixture("insert-soft-deleted-parent");
    let (parent, _t) = common::insert_row(
        &store,
        "project-a",
        "s-parent",
        "agent-1",
        "parent",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    assert!(
        store
            .soft_delete_session("project-a", &parent)
            .await
            .expect("soft delete parent")
    );

    let embedder = FakeEmbedder::with_vector(unit_vector(1));
    let new = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "s-child".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "child".to_string(),
        parent_session_id: Some(parent),
        ..Default::default()
    };

    let error = store
        .insert_session(new, &embedder)
        .await
        .expect_err("soft-deleted parent rejected");
    assert_not_found(error, "parent_session");
    assert!(
        embedder.calls().is_empty(),
        "the parent pre-check runs before the embedder"
    );
}
