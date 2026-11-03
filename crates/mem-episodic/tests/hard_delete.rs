//! Issue 32 — the HARD delete: `EpisodicStore::hard_delete_session`.
//!
//! The user-facing session delete must ERASE (row incl. text + embedding, raw
//! context, segments, inbox) instead of tombstoning, must orphan children
//! (FK `ON DELETE SET NULL`), must answer `false` for every invisible row
//! (unknown, wrong project, already tombstoned — D9's no-existence-leak) and
//! must refuse while another consolidation/purge holds the in-process gate.
//!
//! Every erasure assertion is a DIRECT `SELECT`, never an API assertion: a
//! soft-delete mutant must red here.

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, TestDir};
use mem_episodic::EpisodicError;
use mem_episodic::inbox::{CaptureInbox, InboxNewEntry};
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_store::db::{Value, query_one};

const PROJECT: &str = "project-a";
const SESSION: &str = "session-a";
const AGENT: &str = "agent-a";

async fn store(dir: &TestDir) -> Arc<EpisodicStore> {
    let store = Arc::new(EpisodicStore::new(dir.to_path_buf(), dir.store_config()));
    store.ready().await.expect("ready");
    store
}

async fn insert_session(
    store: &Arc<EpisodicStore>,
    project: &str,
    session: &str,
    agent: &str,
) -> String {
    let embedder = FakeEmbedder::with_vector(vec![0.1; 8]);
    store
        .insert_session(
            NewAgentSession {
                project_id: project.to_string(),
                session_id: session.to_string(),
                agent_id: agent.to_string(),
                summary: "the summary".to_string(),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert session")
}

async fn insert_child(store: &Arc<EpisodicStore>, parent_row_id: &str) -> String {
    let embedder = FakeEmbedder::with_vector(vec![0.1; 8]);
    store
        .insert_session(
            NewAgentSession {
                project_id: PROJECT.to_string(),
                session_id: "child-session".to_string(),
                agent_id: AGENT.to_string(),
                summary: "the child".to_string(),
                parent_session_id: Some(parent_row_id.to_string()),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert child")
}

async fn enqueue_inbox(store: &Arc<EpisodicStore>, capture_id: &str) {
    CaptureInbox::new(store.clone())
        .enqueue(InboxNewEntry {
            capture_id: capture_id.to_string(),
            project_id: PROJECT.to_string(),
            session_id: SESSION.to_string(),
            agent_id: AGENT.to_string(),
            payload: mem_store::serde_json::json!({ "summary": "queued" }),
        })
        .await
        .expect("enqueue");
}

async fn seed_full_triple(store: &Arc<EpisodicStore>) -> String {
    let row_id = insert_session(store, PROJECT, SESSION, AGENT).await;
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, "raw bytes")
        .await
        .expect("raw");
    enqueue_inbox(store, "cap-1").await;
    row_id
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hard_delete_erases_the_session_row_text_and_embedding() {
    let dir = TestDir::new("i32-hard-row");
    let store = store(&dir).await;
    let row_id = insert_session(&store, PROJECT, SESSION, AGENT).await;
    let db = store.ready().await.expect("ready");
    let before = query_one(
        db.as_ref(),
        "SELECT id, summary, length(embedding) AS embedding_len FROM agent_sessions WHERE id = ?",
        vec![Value::Text(row_id.clone())],
    )
    .await
    .expect("select before")
    .expect("the row exists");
    assert!(
        before.text("summary").is_some(),
        "fixture carries a summary"
    );
    assert!(
        before.i64("embedding_len").unwrap_or(0) > 0,
        "fixture carries an embedding"
    );

    assert!(
        store
            .hard_delete_session(PROJECT, &row_id)
            .await
            .expect("hard delete"),
        "a visible row is erased"
    );

    // The DIRECT query is the measurement — an API assertion would pass on a
    // tombstone.
    let after = query_one(
        db.as_ref(),
        "SELECT id FROM agent_sessions WHERE id = ?",
        vec![Value::Text(row_id.clone())],
    )
    .await
    .expect("select after");
    assert!(after.is_none(), "the row (text + embedding) is gone");
    assert_eq!(
        store.count_sessions(Some(PROJECT)).await.expect("count"),
        0,
        "no session row survives"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hard_delete_erases_raw_context_segments_and_inbox() {
    let dir = TestDir::new("i32-hard-triple");
    let store = store(&dir).await;
    let row_id = seed_full_triple(&store).await;
    let segments = store
        .segments_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("segments")
        .len();
    assert!(segments > 0, "fixture carries a derived segment");

    assert!(
        store
            .hard_delete_session(PROJECT, &row_id)
            .await
            .expect("hard delete")
    );

    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_none(),
        "the raw context is erased"
    );
    assert!(
        store
            .segments_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("segments")
            .is_empty(),
        "the derived segments are erased"
    );
    let db = store.ready().await.expect("ready");
    let inbox = query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM capture_inbox WHERE project_id = ? AND session_id = ? AND agent_id = ?",
        vec![
            Value::Text(PROJECT.to_string()),
            Value::Text(SESSION.to_string()),
            Value::Text(AGENT.to_string()),
        ],
    )
    .await
    .expect("select inbox")
    .expect("count row");
    assert_eq!(inbox.i64("count"), Some(0), "the inbox payloads are gone");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hard_delete_orphans_children_and_the_fk_nulls_the_link() {
    let dir = TestDir::new("i32-hard-children");
    let store = store(&dir).await;
    let parent_id = insert_session(&store, PROJECT, SESSION, AGENT).await;
    let child_id = insert_child(&store, &parent_id).await;

    assert!(
        store
            .hard_delete_session(PROJECT, &parent_id)
            .await
            .expect("hard delete")
    );

    let db = store.ready().await.expect("ready");
    let child = query_one(
        db.as_ref(),
        "SELECT orphaned, parent_session_id FROM agent_sessions WHERE id = ?",
        vec![Value::Text(child_id)],
    )
    .await
    .expect("select child")
    .expect("the child survives");
    assert_eq!(child.i64("orphaned"), Some(1), "the child is orphaned");
    assert!(
        child.is_null("parent_session_id"),
        "the FK ON DELETE SET NULL severs the link"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hard_delete_of_an_unknown_wrong_project_or_tombstoned_row_is_false() {
    let dir = TestDir::new("i32-hard-invisible");
    let store = store(&dir).await;
    let row_id = insert_session(&store, PROJECT, SESSION, AGENT).await;

    assert!(
        !store
            .hard_delete_session(PROJECT, "no-such-row")
            .await
            .expect("unknown id"),
        "an unknown id writes nothing"
    );
    assert!(
        !store
            .hard_delete_session("project-b", &row_id)
            .await
            .expect("wrong project"),
        "a wrong-project id writes nothing"
    );
    assert!(
        store
            .soft_delete_session(PROJECT, &row_id)
            .await
            .expect("soft delete"),
        "fixture tombstoned the row"
    );
    assert!(
        !store
            .hard_delete_session(PROJECT, &row_id)
            .await
            .expect("tombstoned id"),
        "a tombstoned row is invisible to the hard delete (D9)"
    );

    let db = store.ready().await.expect("ready");
    let row = query_one(
        db.as_ref(),
        "SELECT deleted_at FROM agent_sessions WHERE id = ?",
        vec![Value::Text(row_id.clone())],
    )
    .await
    .expect("select")
    .expect("the tombstoned row still exists");
    assert!(
        !row.is_null("deleted_at"),
        "the tombstone is untouched — the CLI purge owns it"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hard_delete_is_refused_while_the_gate_is_held() {
    let dir = TestDir::new("i32-hard-gate");
    let store = store(&dir).await;
    let row_id = seed_full_triple(&store).await;

    let guard = mem_episodic::gate::try_acquire(store.storage_path()).expect("gate is free");
    let error = store
        .hard_delete_session(PROJECT, &row_id)
        .await
        .expect_err("a held gate refuses the hard delete");
    assert!(
        matches!(error, EpisodicError::Busy),
        "expected Busy, got {error:?}"
    );
    let db = store.ready().await.expect("ready");
    let row = query_one(
        db.as_ref(),
        "SELECT id FROM agent_sessions WHERE id = ?",
        vec![Value::Text(row_id.clone())],
    )
    .await
    .expect("select");
    assert!(row.is_some(), "the row survives the refusal");
    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "the raw context survives the refusal"
    );

    drop(guard);
    assert!(
        store
            .hard_delete_session(PROJECT, &row_id)
            .await
            .expect("hard delete after the gate released"),
        "the released gate lets the delete through"
    );
}
