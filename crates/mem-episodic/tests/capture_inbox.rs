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
use std::sync::atomic::{AtomicI64, Ordering};

use mem_store::serde_json::json;

use mem_episodic::inbox::{CaptureInbox, INBOX_CAPACITY, InboxError, InboxNewEntry};
use mem_episodic::store::EpisodicStore;

/// The pinned "now" of the scheduling tests (unix millis, unrelated to wall
/// time): every deadline assertion is exact because the clock never moves on
/// its own.
const T0: i64 = 1_000_000;

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

/// The scheduling fixture: a store whose ONLY time source is the injected
/// clock, so the writer's `now + hint` and the drain's `now` cannot disagree
/// (D-110-2). Returns the handle the test advances by hand.
async fn clocked_fixture(
    prefix: &str,
    now: i64,
) -> (common::TestDir, Arc<AtomicI64>, CaptureInbox) {
    let dir = common::TestDir::new(prefix);
    let clock_ms = Arc::new(AtomicI64::new(now));
    let clock: mem_episodic::store::Clock = {
        let c = clock_ms.clone();
        Arc::new(move || c.load(Ordering::Relaxed))
    };
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock),
    );
    store.ready().await.expect("store ready");
    let inbox = CaptureInbox::new(store);
    (dir, clock_ms, inbox)
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
        Some(8),
        "the episodic db is at v8 (v5 inbox, v6 retention schema, v7 not_before, v8 sweep stamp)"
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
    // (still v8) and the inbox row survives the rebuild (durable).
    let reopened = EpisodicStore::new(dir.path().to_path_buf(), dir.store_config());
    reopened.ready().await.expect("migration re-runs as no-op");
    let rebuilt_version = mem_store::db::query_one(db.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("user_version row");
    assert_eq!(rebuilt_version.i64("user_version"), Some(8));
    assert_eq!(
        inbox.queued_count().await.expect("count"),
        1,
        "the entry survives"
    );
}

/// Issue 110: a row with no schedule (`not_before IS NULL`) is immediately
/// eligible — the pre-v7 behaviour, preserved for every existing row.
///
/// Target mutant: `WHERE status = 'queued' AND not_before <= ?` (the NULL arm
/// dropped) never returns the row, because `NULL <= ?` is NULL.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_null_not_before_is_immediately_eligible() {
    let (_dir, _clock, inbox) = clocked_fixture("inbox-null-eligible", T0).await;
    let capture_id = "019a0000-0000-7f00-8000-000000000006";
    inbox
        .enqueue(entry(capture_id, 6))
        .await
        .expect("enqueue an unscheduled entry");

    let batch = inbox.dequeue(10).await.expect("dequeue");
    assert_eq!(
        batch.len(),
        1,
        "a NULL not_before is immediately eligible: {batch:?}"
    );
    assert_eq!(batch[0].capture_id, capture_id);
}

/// Issue 110: a scheduled row is skipped until its deadline, eligible AT it.
///
/// Target mutants: (a) `not_before < ?` instead of `<=` — the read at exactly
/// `T0 + 120_000` returns nothing; (b) `dequeue` reads the wall clock instead
/// of the injected one — the read at `T0` returns the row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_scheduled_row_is_skipped_until_not_before() {
    let (_dir, clock, inbox) = clocked_fixture("inbox-scheduled", T0).await;
    let capture_id = "019a0000-0000-7f00-8000-000000000007";
    inbox.enqueue(entry(capture_id, 7)).await.expect("enqueue");

    let not_before = inbox
        .schedule_retry(capture_id, 120_000)
        .await
        .expect("schedule the retry");
    assert_eq!(
        not_before,
        T0 + 120_000,
        "the deadline is the injected now + the hint"
    );
    assert_eq!(
        inbox.queued_count().await.expect("count"),
        1,
        "a scheduled row keeps its capacity slot"
    );

    clock.store(T0, Ordering::SeqCst);
    let early = inbox.dequeue(10).await.expect("dequeue at T0");
    assert!(early.is_empty(), "not eligible at T0: {early:?}");

    clock.store(T0 + 119_999, Ordering::SeqCst);
    let one_millisecond_early = inbox
        .dequeue(10)
        .await
        .expect("dequeue one millisecond early");
    assert!(
        one_millisecond_early.is_empty(),
        "not eligible one millisecond early: {one_millisecond_early:?}"
    );

    clock.store(T0 + 120_000, Ordering::SeqCst);
    let due = inbox.dequeue(10).await.expect("dequeue at the deadline");
    assert_eq!(due.len(), 1, "`<=` not `<`: the deadline itself is due");
    assert_eq!(due[0].capture_id, capture_id);
}

/// Issue 110: an absurd hint saturates at `i64::MAX` instead of wrapping — the
/// row stays `queued` and keeps its payload, never becoming eligible now.
///
/// Target mutant: `now + ms as i64` wraps negative, making the row eligible
/// immediately (the silent opposite of the hint's intent).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_schedule_saturates_on_an_absurd_hint() {
    let (_dir, _clock, inbox) = clocked_fixture("inbox-saturate", T0).await;
    let capture_id = "019a0000-0000-7f00-8000-000000000008";
    inbox.enqueue(entry(capture_id, 8)).await.expect("enqueue");

    let not_before = inbox
        .schedule_retry(capture_id, u64::MAX)
        .await
        .expect("schedule an absurd hint");
    assert_eq!(not_before, i64::MAX, "the stamp saturates, never wraps");
    assert_eq!(
        inbox.not_before(capture_id).await,
        Some(i64::MAX),
        "the stored deadline is i64::MAX"
    );
    assert_eq!(
        inbox.queued_count().await.expect("count"),
        1,
        "the row is still queued (never dead-lettered)"
    );
    let batch = inbox.dequeue(10).await.expect("dequeue");
    assert!(batch.is_empty(), "an i64::MAX row is never due: {batch:?}");
}
