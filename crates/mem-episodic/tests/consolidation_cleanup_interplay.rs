//! W4.3 lane — concurrency, busy and the cleanup/dedup interplay (AC-5).
//!
//! The seams under test are `mem_episodic::gate::try_acquire`,
//! `mem_store::operation_lock::acquire_turso_operation_lock` and
//! `Consolidator::run`.

mod common;

use std::sync::Arc;

use common::{
    ConsolidationFixture, DAY_MILLIS, FakeProcessProbe, shard_manager, shard_manager_with_probe,
};
use mem_episodic::gate;
use mem_episodic::promotion::{
    PermanentMemorySink, PromoteOutcome, PromotionRecord, ShardPermanentMemorySink,
};
use mem_store::db::Value;
use mem_store::error::StoreError;
use mem_store::operation_lock::{LockState, OPERATION_LOCK_FILE, acquire_turso_operation_lock};
use mem_store::platform::ProcessProbe;

/// The default window: 30 days.
const DEFAULT_AGE_DAYS: u32 = 30;

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_in_process_gate_returns_busy_with_zero_writes() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    // The test holds the gate, so the run is busy.
    let guard = gate::try_acquire(fixture.store.storage_path()).expect("acquire the gate");

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let report = fixture
        .consolidator(DEFAULT_AGE_DAYS, writer)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert!(report.busy, "the run reports busy");
    assert_eq!(report.agents_considered, 0);
    assert_eq!(report.milestones_written, 0);
    assert_eq!(fixture.summarizer.call_count(), 0, "zero summarizer calls");
    assert_eq!(fixture.sink.call_count(), 0, "zero sink calls");
    assert_eq!(fixture.recorder.call_count(), 0, "zero batch writes");

    drop(guard);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_operation_lock_contention_returns_busy_with_zero_writes() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    // A second, LIVE lock holder blocks the run.
    let probe: Arc<dyn ProcessProbe> = Arc::new(FakeProcessProbe::with_alive(&[]));
    let held = acquire_turso_operation_lock(fixture.store.storage_path(), probe, "test-holder")
        .expect("hold the operation lock");

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let report = fixture
        .consolidator(DEFAULT_AGE_DAYS, writer)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert!(report.busy, "the run reports busy");
    assert_eq!(fixture.summarizer.call_count(), 0);
    assert_eq!(fixture.sink.call_count(), 0);
    assert_eq!(fixture.recorder.call_count(), 0);

    held.release();
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_operation_lock_migration_locked_error_maps_to_busy() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    // A lock file whose owner is alive (per the injected probe).
    let state = LockState {
        pid: 4242,
        timestamp: "2026-10-20T00:00:00.000Z".to_string(),
        operation: Some("test".to_string()),
    };
    std::fs::write(
        fixture.dir.path().join(OPERATION_LOCK_FILE),
        mem_store::serde_json::to_string(&state).expect("lock json"),
    )
    .expect("write the lock file");

    let probe: Arc<dyn ProcessProbe> = Arc::new(FakeProcessProbe::with_alive(&[4242]));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let deps = mem_episodic::consolidation::ConsolidationDeps {
        summarizer: fixture.summarizer.clone(),
        embedder: fixture.embedder.clone(),
        sink: fixture.sink.clone(),
        batch_writer: writer,
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: 200,
        probe,
    };
    let consolidator = mem_episodic::consolidation::Consolidator::new(
        fixture.store.clone(),
        mem_episodic::consolidation::ConsolidationPolicy::new(DEFAULT_AGE_DAYS).expect("policy"),
        deps,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert!(report.busy, "the run reports busy");
    assert_eq!(fixture.summarizer.call_count(), 0);
    assert_eq!(fixture.sink.call_count(), 0);
    assert_eq!(fixture.recorder.call_count(), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_operation_lock_already_exists_io_error_maps_to_busy() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    // A DIRECTORY named like the lock file: the lock body cannot be read
    // (so no live lock is seen) and the `create_new` open fails with
    // `AlreadyExists`.
    std::fs::create_dir(fixture.dir.path().join(OPERATION_LOCK_FILE)).expect("create the lock dir");

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let report = fixture
        .consolidator(DEFAULT_AGE_DAYS, writer)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert!(report.busy, "the run reports busy");
    assert_eq!(fixture.summarizer.call_count(), 0);
    assert_eq!(fixture.sink.call_count(), 0);
    assert_eq!(fixture.recorder.call_count(), 0);
}

#[test]
fn test_in_process_gate_is_keyed_by_canonical_storage_path() {
    let dir = common::TestDir::new("gate-canonical");
    // A second spelling of the SAME directory (a symlink).
    let alias = dir.path().join("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path(), &alias).expect("symlink the storage root");

    let guard = gate::try_acquire(dir.path()).expect("acquire through the real path");
    // The same directory spelled differently contends.
    assert!(
        gate::try_acquire(&alias).is_none(),
        "the two spellings of the same directory contend"
    );
    drop(guard);

    // And after the release the alias spelling acquires the same gate.
    assert!(
        gate::try_acquire(&alias).is_some(),
        "the alias spelling reaches the same gate"
    );
}

#[test]
fn test_in_process_gate_is_released_when_the_guard_is_dropped() {
    let dir = common::TestDir::new("gate-release");
    let guard = gate::try_acquire(dir.path()).expect("acquire");
    assert!(
        gate::try_acquire(dir.path()).is_none(),
        "the held gate blocks a second acquisition"
    );
    drop(guard);
    assert!(
        gate::try_acquire(dir.path()).is_some(),
        "the dropped guard releases the gate"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_crash_after_promotion_before_milestone_does_not_duplicate_memory() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    let shards = shard_manager(fixture.dir.store_config());
    let sink: Arc<dyn PermanentMemorySink> =
        Arc::new(ShardPermanentMemorySink::new(shards.clone(), 8));

    // Pre-insert the deterministic promoted id — the state a crash between
    // the promotion and the milestone write would leave behind.
    let fact_id = mem_episodic::promotion::promoted_fact_id("project-a", "a durable decision");
    let record = PromotionRecord {
        id: fact_id.clone(),
        content: "a durable decision".to_string(),
        vector: vec![0.1_f32; 8],
        container_tag: ConsolidationFixture::scope().container_tag,
        memory_type: "decision".to_string(),
        created_at: common::CONSOLIDATION_NOW,
        source_row_ids: vec!["row-1".to_string()],
    };
    sink.promote("0123456789abcdef", &record)
        .await
        .expect("pre-insert the promoted fact");

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> = Arc::new(
        mem_episodic::consolidation::StoreBatchWriter::new(fixture.store.clone()),
    );
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        fixture.summarizer.clone(),
        fixture.embedder.clone(),
        sink,
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.facts_already_present, 1);
    assert_eq!(report.facts_promoted, 0);

    // Exactly ONE memory row carries the fact.
    let info = shards
        .get_write_shard(mem_store::scope::Scope::Project, "0123456789abcdef")
        .await
        .expect("write shard");
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    let rows = db
        .query(
            "SELECT id FROM memories WHERE container_tag = ?",
            vec![Value::Text(ConsolidationFixture::scope().container_tag)],
        )
        .await
        .expect("query memories");
    assert_eq!(rows.len(), 1, "the promoted fact was not duplicated");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_consolidation_writes_only_promoted_memory_rows() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["decision one", "decision two"],
        )
        .await;

    let shards = shard_manager(fixture.dir.store_config());
    let sink: Arc<dyn PermanentMemorySink> =
        Arc::new(ShardPermanentMemorySink::new(shards.clone(), 8));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> = Arc::new(
        mem_episodic::consolidation::StoreBatchWriter::new(fixture.store.clone()),
    );
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        fixture.summarizer.clone(),
        fixture.embedder.clone(),
        sink,
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");
    assert_eq!(report.facts_promoted, 2);

    let info = shards
        .get_write_shard(mem_store::scope::Scope::Project, "0123456789abcdef")
        .await
        .expect("write shard");
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    let rows = db
        .query(
            "SELECT id FROM memories WHERE container_tag = ?",
            vec![Value::Text(ConsolidationFixture::scope().container_tag)],
        )
        .await
        .expect("query memories");
    assert_eq!(rows.len(), 2, "only the promoted facts are memory rows");
}

// ---------------------------------------------------------------------------
// The own-lock bypass: a lock held by ANOTHER process still blocks the
// promotion, a lock held by THIS process does not.
// ---------------------------------------------------------------------------

/// The scope hash the promotion tests use.
const SCOPE_HASH: &str = "0123456789abcdef";

/// The promotion record the own-lock tests promote.
fn own_lock_record() -> PromotionRecord {
    PromotionRecord {
        id: "episodic-decision-own-lock".to_string(),
        content: "a durable decision".to_string(),
        vector: vec![0.1_f32; 8],
        container_tag: ConsolidationFixture::scope().container_tag,
        memory_type: "decision".to_string(),
        created_at: common::CONSOLIDATION_NOW,
        source_row_ids: vec!["row-1".to_string()],
    }
}

/// Writes a live-looking operation-lock file into `dir`.
fn write_operation_lock(dir: &std::path::Path, pid: u32, operation: &str) {
    let state = LockState {
        pid,
        timestamp: "2026-10-20T00:00:00.000Z".to_string(),
        operation: Some(operation.to_string()),
    };
    std::fs::write(
        dir.join(OPERATION_LOCK_FILE),
        mem_store::serde_json::to_string(&state).expect("lock json"),
    )
    .expect("write the lock file");
}

/// A promotion against a lock held by ANOTHER process fails and writes
/// nothing.
///
/// The own-lock bypass is defended in depth: this crate's
/// `is_own_operation_lock` (a private item, pinned by the crate-internal
/// unit test in `src/promotion.rs`) AND mem-store's
/// `get_write_shard_allowing_own_lock` (which re-reads the live lock and
/// refuses a foreign pid, pinned by
/// `crates/mem-store/tests/turso_shard_rotation.rs`). Either guard alone
/// suffices, so this end-to-end test kills the COMPOSED mutation (both
/// guards broken at once) — the equivalence of the two single mutations is
/// recorded in the ledger.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_refuses_a_foreign_operation_lock_and_writes_nothing() {
    let fixture = ConsolidationFixture::new();
    // A FOREIGN process (pid 4242) holds the operation lock; the injected
    // probe reports it alive.
    write_operation_lock(fixture.dir.path(), 4242, "foreign-migration");

    let shards = shard_manager_with_probe(
        fixture.dir.store_config(),
        Arc::new(FakeProcessProbe::with_alive(&[4242])),
    );
    let sink: Arc<dyn PermanentMemorySink> =
        Arc::new(ShardPermanentMemorySink::new(shards.clone(), 8));

    let error = sink
        .promote(SCOPE_HASH, &own_lock_record())
        .await
        .expect_err("a foreign operation lock blocks the promotion");
    match error {
        mem_episodic::EpisodicError::Store(StoreError::MigrationLocked { pid, .. }) => {
            assert_eq!(pid, 4242, "the foreign owner's pid is reported");
        }
        other => panic!("unexpected error: {other}"),
    }

    // NOTHING was written: the promotion never resolved a write shard, so
    // the shard file was never created.
    let shard_path = shards
        .get_shard_path(mem_store::scope::Scope::Project, SCOPE_HASH, 0)
        .expect("shard path");
    assert!(
        !shard_path.exists(),
        "no shard was created: the promotion never reached the write path"
    );
}

/// The own-lock companion: a lock held by THIS process does not block this
/// process's own promotion.
///
/// This is the behaviour the bypass exists for — the consolidation run holds
/// the operation lock for its whole duration (ADR-0006 D-25), and its
/// promotions land in a shard.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_succeeds_under_our_own_operation_lock() {
    let fixture = ConsolidationFixture::new();
    // OUR OWN process holds the operation lock.
    write_operation_lock(
        fixture.dir.path(),
        std::process::id(),
        "episodic-consolidation",
    );

    let shards = shard_manager_with_probe(
        fixture.dir.store_config(),
        Arc::new(FakeProcessProbe::with_alive(&[std::process::id()])),
    );
    let sink: Arc<dyn PermanentMemorySink> =
        Arc::new(ShardPermanentMemorySink::new(shards.clone(), 8));

    let record = own_lock_record();
    let outcome = sink
        .promote(SCOPE_HASH, &record)
        .await
        .expect("our own operation lock does not block our own promotion");
    assert_eq!(outcome, PromoteOutcome::Inserted);

    // The memory row exists.
    let info = shards
        .get_write_shard_allowing_own_lock(mem_store::scope::Scope::Project, SCOPE_HASH)
        .await
        .expect("the tolerant path resolves the shard under our own lock");
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    let rows = db
        .query(
            "SELECT id FROM memories WHERE id = ?",
            vec![Value::Text(record.id.clone())],
        )
        .await
        .expect("query memories");
    assert_eq!(rows.len(), 1, "the promoted fact landed in the shard");
}
