//! W5.3 Step 5 — recapture/upsert oracles.
//!
//! Contract: unique key `(project_id, session_id, agent_id)` unchanged;
//! same `capture_id` → already-applied no-op; a newer UUIDv7-shaped id wins
//! lexicographically and updates the live row in place; an older id is
//! superseded (no write); a tombstone is never mutated; the update path
//! re-runs the parent-visibility and embedding-dimension checks.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use common::{FakeEmbedder, TestDir};
use mem_episodic::record::{NewAgentSession, WorkItem, WorkKind};
use mem_episodic::store::{EpisodicStore, UpsertOutcome};

const DIMS: usize = 8;

fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMS];
    vector[index] = 1.0;
    vector
}

fn store(dir: &TestDir) -> EpisodicStore {
    // Deterministic row ids so the tombstone/linkage assertions are stable.
    let counter = AtomicUsize::new(1);
    EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_id_factory(
        std::sync::Arc::new(move || {
            let n = counter.fetch_add(1, AtomicOrdering::SeqCst);
            format!("019a0000-0000-7f00-8000-{n:012}")
        }),
    )
}

fn capture(capture_id: &str, summary: &str) -> NewAgentSession {
    NewAgentSession {
        project_id: "/srv/proj".to_string(),
        session_id: "sess-1".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: "build".to_string(),
        summary: summary.to_string(),
        completed_work: vec![WorkItem {
            kind: WorkKind::Feature,
            description: format!("work of {capture_id}"),
        }],
        capture_id: Some(capture_id.to_string()),
        ..Default::default()
    }
}

/// Fixed first vector (a clone helper keeps tests explicit about moves).
fn embedding_of(index: usize) -> Vec<f32> {
    unit_vector(index)
}

fn embedder() -> FakeEmbedder {
    FakeEmbedder::with_vectors(vec![embedding_of(0), embedding_of(1)])
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_recapture_updates_latest_capture_in_place() {
    let dir = TestDir::new("w53-recapture");
    let store = store(&dir);
    let embedder = embedder();

    let outcome = store
        .upsert_session(capture("019a-old-1", "first summary"), &embedder)
        .await
        .expect("first upsert inserts");
    assert_eq!(outcome, UpsertOutcome::Inserted);
    let row_first = store.list_sessions("/srv/proj", 10).await.expect("list")[0].clone();
    assert_eq!(row_first.summary, "first summary");
    assert_eq!(row_first.capture_id.as_deref(), Some("019a-old-1"));
    assert_eq!(row_first.id, "019a0000-0000-7f00-8000-000000000001");

    let outcome = store
        .upsert_session(capture("019b-new-2", "second summary"), &embedder)
        .await
        .expect("newer capture wins");
    assert_eq!(outcome, UpsertOutcome::Updated);
    let sessions = store.list_sessions("/srv/proj", 10).await.expect("list");
    assert_eq!(sessions.len(), 1, "still exactly one row: the upsert key");
    let row_second = sessions[0].clone();
    assert_eq!(row_second.summary, "second summary");
    assert_eq!(
        row_second.capture_id.as_deref(),
        Some("019b-new-2"),
        "the row id and triple survive; the capture content moved in place"
    );
    assert_eq!(row_second.id, row_first.id, "no new row was minted");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_recapture_same_capture_id_is_noop() {
    let dir = TestDir::new("w53-recapture-applied");
    let store = store(&dir);
    let embedding = unit_vector(0);
    store
        .upsert_session(
            capture("019a-same-1", "original"),
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("insert");
    let before = store.list_sessions("/srv/proj", 10).await.expect("list")[0].clone();

    let outcome = store
        .upsert_session(
            capture("019a-same-1", "should not apply"),
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("replay");
    assert_eq!(outcome, UpsertOutcome::AlreadyApplied);
    let after = store.list_sessions("/srv/proj", 10).await.expect("list")[0].clone();
    assert_eq!(before, after, "no field changed on same-capture replay");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_recapture_older_capture_is_superseded() {
    let dir = TestDir::new("w53-recapture-old");
    let store = store(&dir);
    let embedding = unit_vector(0);
    store
        .upsert_session(
            capture("019b-new-2", "newest"),
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("insert newest first");
    let before = store.list_sessions("/srv/proj", 10).await.expect("list")[0].clone();

    let outcome = store
        .upsert_session(
            capture("019a-old-1", "older"),
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("superseded is not an error");
    assert_eq!(outcome, UpsertOutcome::Superseded);
    let after = store.list_sessions("/srv/proj", 10).await.expect("list")[0].clone();
    assert_eq!(before, after);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_recapture_does_not_mutate_tombstone() {
    let dir = TestDir::new("w53-recapture-tomb");
    let store = store(&dir);
    let embedding = unit_vector(0);
    let id = store
        .insert_session(
            capture("019a-old-1", "original"),
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("insert");
    assert!(
        store
            .soft_delete_session("/srv/proj", &id)
            .await
            .expect("soft delete"),
        "row tombstoned"
    );

    let outcome = store
        .upsert_session(
            capture("019b-new-2", "mutating attempt"),
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("tombstone must not error");
    assert_eq!(outcome, UpsertOutcome::TombstoneUnchanged);
    let sessions = store.list_sessions("/srv/proj", 10).await.expect("list");
    assert!(sessions.is_empty(), "the tombstone stays invisible");
    assert_eq!(
        store.count_sessions(None).await.expect("count"),
        1,
        "the raw count still shows the tombstone (deleted_at stamped, summary untouched)"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_recapture_revalidates_parent_and_embedding() {
    let dir = TestDir::new("w53-recapture-checks");
    let store = store(&dir);
    let embedding = unit_vector(0);

    // Parent NOT in the same project: the update path refuses before any
    // write, exactly like the insert path.
    let _child = store
        .insert_session(
            NewAgentSession {
                project_id: "/srv/proj".to_string(),
                session_id: "child".to_string(),
                agent_id: "agent-1".to_string(),
                summary: "child row".to_string(),
                capture_id: Some("019a-child-1".to_string()),
                ..Default::default()
            },
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("child inserts");

    store
        .insert_session(
            NewAgentSession {
                project_id: "/srv/other".to_string(),
                session_id: "parent".to_string(),
                agent_id: "agent-1".to_string(),
                summary: "other-project parent".to_string(),
                capture_id: Some("019a-parent-1".to_string()),
                ..Default::default()
            },
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect("parent inserts");

    let error = store
        .upsert_session(
            NewAgentSession {
                project_id: "/srv/proj".to_string(),
                session_id: "child".to_string(),
                agent_id: "agent-1".to_string(),
                summary: "child row".to_string(),
                parent_session_id: Some("019a0000-0000-7f00-8000-000000000002".to_string()),
                capture_id: Some("019b-child-2".to_string()),
                ..Default::default()
            },
            &FakeEmbedder::with_vector(embedding.clone()),
        )
        .await
        .expect_err("same-project rule re-runs on update");
    assert!(
        matches!(error, mem_episodic::error::EpisodicError::Validation { ref field, .. } if field == "parent_session_id"),
        "the same-project rule re-runs on update"
    );

    // Dimension mismatch: the update path runs the embedding stage and the
    // dimension check BEFORE writing.
    let wrong = 4;
    let mut bad_vector = vec![0.0_f32; wrong];
    bad_vector[0] = 1.0;
    let error = store
        .upsert_session(
            NewAgentSession {
                project_id: "/srv/proj".to_string(),
                session_id: "child".to_string(),
                agent_id: "agent-1".to_string(),
                summary: "child row".to_string(),
                capture_id: Some("019b-child-2".to_string()),
                ..Default::default()
            },
            &FakeEmbedder::with_vector(bad_vector),
        )
        .await
        .expect_err("dimension check re-runs on update");
    match error {
        mem_episodic::error::EpisodicError::Validation { field, .. } => {
            assert_eq!(field, "embedding")
        }
        other => panic!("expected embedding validation, got {other:?}"),
    }
}
