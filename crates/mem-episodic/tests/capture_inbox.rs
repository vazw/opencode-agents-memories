//! W5.3 Step 4 — the durable capture inbox (`episodic.db`) and the v5
//! migration oracles.
//!
//! Contract (blueprint Step 4 / ADR-0007 D-48): enqueue is idempotent on
//! `capture_id`; capacity (1,024 queued) is enforced by one atomic
//! conditional insert (no count-then-insert race); dequeue is FIFO;
//! `mark_done` purges the row in ONE `BatchMode::Write` write — the row is
//! absent on return and no separate `purge_done` reaper exists.

mod common;

use std::sync::Arc;

use mem_store::serde_json::json;

use mem_episodic::inbox::{CaptureInbox, INBOX_CAPACITY, InboxError, InboxNewEntry};
use mem_episodic::store::EpisodicStore;

async fn fixture() -> (common::TestDir, Arc<EpisodicStore>, CaptureInbox) {
    let dir = common::TestDir::new("w53-inbox");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    // Force the migration set (v1..v5) to apply.
    store.ready().await.expect("store ready");
    let inbox = CaptureInbox::new(store.clone());
    (dir, store, inbox)
}

fn entry(capture_id: &str, sequence: u64) -> InboxNewEntry {
    InboxNewEntry {
        capture_id: capture_id.to_string(),
        project_id: "/srv/proj".to_string(),
        session_id: "sess-1".to_string(),
        agent_id: "agent-1".to_string(),
        payload: json!({ "transcript": "hello", "n": sequence }),
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_enqueue_dequeue_roundtrip() {
    let (_dir, _store, inbox) = fixture().await;

    inbox
        .enqueue(entry("019a0000-0000-7f00-8000-000000000001", 1))
        .await
        .expect("first enqueue succeeds");
    inbox
        .enqueue(entry("019a0000-0000-7f00-8000-000000000002", 2))
        .await
        .expect("second enqueue succeeds");

    let batch = inbox.dequeue(10).await.expect("dequeue");
    let ids: Vec<&str> = batch.iter().map(|row| row.capture_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "019a0000-0000-7f00-8000-000000000001",
            "019a0000-0000-7f00-8000-000000000002",
        ],
        "dequeue is FIFO by created_at"
    );
    // The raw payload arrives as received: `transcript`, `n`.
    assert_eq!(
        batch[0].payload.get("transcript").and_then(|t| t.as_str()),
        Some("hello")
    );
    assert_eq!(batch[1].payload.get("n").and_then(|n| n.as_i64()), Some(2));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_duplicate_capture_id_is_idempotent() {
    let (_dir, _store, inbox) = fixture().await;
    inbox
        .enqueue(entry("019a0000-0000-7f00-8000-000000000001", 1))
        .await
        .expect("first enqueue");
    inbox
        .enqueue(entry("019a0000-0000-7f00-8000-000000000001", 2))
        .await
        .expect("duplicate capture_id is idempotent, not an error");

    assert_eq!(
        inbox.queued_count().await.expect("count"),
        1,
        "no second row"
    );
    let batch = inbox.dequeue(10).await.expect("dequeue");
    assert_eq!(batch.len(), 1, "exactly one entry survives the duplicate");
    assert_eq!(batch[0].payload.get("n").and_then(|n| n.as_i64()), Some(1));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_full_returns_full_error() {
    let (_dir, _store, inbox) = fixture().await;
    assert_eq!(INBOX_CAPACITY, 1024, "the bound is 1,024 queued entries");
    for i in 0..INBOX_CAPACITY {
        inbox
            .enqueue(entry(&format!("019a{i:012}1"), i as u64))
            .await
            .expect("each fill entry is enqueued");
    }
    let outcome = inbox.enqueue(entry("019aduplicate0overflow", 9)).await;
    match outcome {
        Err(InboxError::Full) => {}
        other => panic!("expected InboxError::Full, got {other:?}"),
    }
}

/// D-48: `mark_done` performs the done/delete statements in ONE
/// `BatchMode::Write` transaction; the row is ABSENT on return, and there
/// is no separate `purge_done` method (no reaper exists).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_done_row_is_purged_in_the_same_write() {
    let (dir, _store, inbox) = fixture().await;
    inbox
        .enqueue(entry("019a0000-0000-7f00-8000-000000000001", 1))
        .await
        .expect("enqueue");

    let purged = inbox
        .mark_done("019a0000-0000-7f00-8000-000000000001")
        .await
        .expect("mark_done");
    assert!(purged, "the row was removed");

    assert_eq!(
        inbox.queued_count().await.expect("count"),
        0,
        "no queued rows remain"
    );
    let raw = common::open_raw(dir.path(), "episodic.db").await;
    let rows = raw
        .query("SELECT * FROM capture_inbox", Vec::new())
        .await
        .expect("read raw inbox");
    assert!(
        rows.is_empty(),
        "the done row is fully purged in the same write: {rows:?}"
    );

    // Idempotent: a second mark_done removes nothing but never errors.
    let second = inbox
        .mark_done("019a0000-0000-7f00-8000-000000000001")
        .await
        .expect("second mark_done");
    assert!(!second, "nothing left to purge");
}

/// The v5 migration adds the inbox table to the EPISODIC database and is
/// idempotent (a re-open re-runs the set as a no-op at v5; the enqueued row
/// survives the rebuild).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_inbox_migration_v5_adds_table_and_is_idempotent() {
    let (dir, _store, inbox) = fixture().await;
    let db = common::open_raw(dir.path(), "episodic.db").await;
    let version_row = mem_store::db::query_one(db.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("user_version row");
    assert_eq!(
        version_row.i64("user_version"),
        Some(5),
        "the episodic db is at v5"
    );
    let tables: Vec<String> = db
        .query(
            "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
            Vec::new(),
        )
        .await
        .expect("tables")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect();
    assert!(tables.contains(&"capture_inbox".to_string()));

    inbox
        .enqueue(entry("019a0000-0000-7f00-8000-000000000005", 5))
        .await
        .expect("enqueue");

    // A rebuilt store over the same root re-applies the set as a no-op
    // (still v5) and the inbox row survives the rebuild (durable).
    let reopened = EpisodicStore::new(dir.path().to_path_buf(), dir.store_config());
    reopened.ready().await.expect("migration re-runs as no-op");
    let rebuilt_version = mem_store::db::query_one(db.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("user_version row");
    assert_eq!(rebuilt_version.i64("user_version"), Some(5));
    assert_eq!(
        inbox.queued_count().await.expect("count"),
        1,
        "the entry survives"
    );
}
