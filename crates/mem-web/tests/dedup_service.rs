//! `DedupService` over an in-memory fake store — the disabled posture, the
//! per-shard walk, delete accounting, partial failure and the run guard.
//!
//! The fake's `delete` is **stateful**: a successful delete removes the row
//! from its vector, so a later run over the same fake sees the deduplicated
//! state. That is what makes the guard-release assertion (`0` deletions on the
//! third run) strict rather than a counter artefact.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mem_web::api::deduplication::{
    DISABLED_MESSAGE, DedupMemory, DedupService, DedupShard, DedupStore, RUNNING_MESSAGE,
};
use mem_web::api::ports::PortError;
use tokio::sync::Notify;

const DIMENSIONS: usize = 768;
const TAG: &str = "tag_a";

fn mem(id: &str, content: &str, created_at: i64, vector: Vec<f32>) -> DedupMemory {
    DedupMemory {
        id: id.to_string(),
        content: content.to_string(),
        container_tag: TAG.to_string(),
        created_at,
        vector: Some(vector),
    }
}

fn shard(shard_id: i64) -> DedupShard {
    DedupShard {
        shard_id,
        db_path: PathBuf::from(format!("/tmp/fake-shard-{shard_id}.db")),
    }
}

fn unit_x() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[0] = 1.0;
    vector
}

fn unit_y() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[1] = 1.0;
    vector
}

/// The handshake of the concurrency test: `entered` fires once `memories()` is
/// in flight, `release` lets it finish.
struct Gate {
    entered: Notify,
    release: Notify,
}

struct FakeDedupStore {
    shards: Vec<DedupShard>,
    rows: Mutex<Vec<(i64, DedupMemory)>>,
    deleted: Mutex<Vec<String>>,
    attempts: AtomicUsize,
    shards_calls: AtomicUsize,
    memories_calls: AtomicUsize,
    fail_delete_at: Option<usize>,
    gate: Option<Arc<Gate>>,
}

impl FakeDedupStore {
    fn new(shards: Vec<DedupShard>, rows: Vec<(i64, DedupMemory)>) -> Self {
        Self {
            shards,
            rows: Mutex::new(rows),
            deleted: Mutex::new(Vec::new()),
            attempts: AtomicUsize::new(0),
            shards_calls: AtomicUsize::new(0),
            memories_calls: AtomicUsize::new(0),
            fail_delete_at: None,
            gate: None,
        }
    }

    fn gate(mut self, gate: Arc<Gate>) -> Self {
        self.gate = Some(gate);
        self
    }

    fn fail_delete_at(mut self, attempt: usize) -> Self {
        self.fail_delete_at = Some(attempt);
        self
    }

    fn shards_calls(&self) -> usize {
        self.shards_calls.load(Ordering::SeqCst)
    }

    fn memories_calls(&self) -> usize {
        self.memories_calls.load(Ordering::SeqCst)
    }

    fn delete_attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted
            .lock()
            .map(|deleted| deleted.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl DedupStore for FakeDedupStore {
    async fn shards(&self) -> Result<Vec<DedupShard>, PortError> {
        self.shards_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.shards.clone())
    }

    async fn memories(&self, shard: &DedupShard) -> Result<Vec<DedupMemory>, PortError> {
        let call = self.memories_calls.fetch_add(1, Ordering::SeqCst);
        // The lock is released before any await (never held across it).
        let rows = self
            .rows
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default();

        // The gate is ONE-SHOT: only the first `memories()` pauses. A
        // permanently gated fake would deadlock the third run of the guard
        // test, which waits for a release that will never come again.
        if let Some(gate) = &self.gate
            && call == 0
        {
            gate.entered.notify_one();
            gate.release.notified().await;
        }

        Ok(rows
            .into_iter()
            .filter(|(shard_id, _)| *shard_id == shard.shard_id)
            .map(|(_, row)| row)
            .collect())
    }

    async fn delete(&self, _shard: &DedupShard, memory_id: &str) -> Result<(), PortError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_delete_at == Some(attempt) {
            return Err(PortError::Store("forced delete failure".to_string()));
        }
        if let Ok(mut deleted) = self.deleted.lock() {
            deleted.push(memory_id.to_string());
        }
        if let Ok(mut rows) = self.rows.lock() {
            rows.retain(|(_, row)| row.id != memory_id);
        }
        Ok(())
    }
}

#[tokio::test]
async fn test_dedup_service_answers_a_zero_report_for_no_shards() {
    let store = Arc::new(FakeDedupStore::new(Vec::new(), Vec::new()));
    let service = DedupService::new(store.clone(), true, 0.9);

    let report = service.run().await.expect("a zero-shard store succeeds");
    assert_eq!(report.exact_duplicates_deleted, 0);
    assert!(report.near_duplicate_groups.is_empty());
    assert_eq!(store.memories_calls(), 0, "no shard is walked");
}

#[tokio::test]
async fn test_dedup_service_disabled_touches_no_shard() {
    let store = Arc::new(FakeDedupStore::new(vec![shard(1)], Vec::new()));
    let service = DedupService::new(store.clone(), false, 0.9);

    assert_eq!(
        service.run().await,
        Err(PortError::Invalid(DISABLED_MESSAGE.to_string()))
    );
    assert_eq!(
        store.shards_calls(),
        0,
        "a disabled service never lists shards"
    );
}

#[tokio::test]
async fn test_dedup_service_deletes_only_the_exact_duplicates() {
    let store = Arc::new(FakeDedupStore::new(
        vec![shard(1)],
        vec![
            (1, mem("dup_1", "same content", 3000, unit_x())),
            (1, mem("dup_2", "same content", 2000, unit_x())),
            (1, mem("dup_3", "same content", 1000, unit_x())),
            (1, mem("unique", "orthogonal content", 500, unit_y())),
        ],
    ));
    let service = DedupService::new(store.clone(), true, 0.9);

    let report = service.run().await.expect("the walk succeeds");
    assert_eq!(report.exact_duplicates_deleted, 2);
    assert!(report.near_duplicate_groups.is_empty());
    assert_eq!(
        store.deleted(),
        vec!["dup_2".to_string(), "dup_3".to_string()],
        "only the two older exact duplicates are deleted, newest first"
    );
    assert!(
        !store.deleted().contains(&"unique".to_string()),
        "the orthogonal row is never deleted"
    );
}

#[tokio::test]
async fn test_dedup_service_continues_after_a_failed_delete() {
    let store = Arc::new(
        FakeDedupStore::new(
            vec![shard(1)],
            vec![
                (1, mem("dup_1", "same content", 4000, unit_x())),
                (1, mem("dup_2", "same content", 3000, unit_x())),
                (1, mem("dup_3", "same content", 2000, unit_x())),
                (1, mem("dup_4", "same content", 1000, unit_x())),
            ],
        )
        .fail_delete_at(1),
    );
    let service = DedupService::new(store.clone(), true, 0.9);

    let report = service
        .run()
        .await
        .expect("upstream logs the delete error and continues");
    assert_eq!(
        report.exact_duplicates_deleted, 2,
        "only the two successful deletions are counted"
    );
    assert_eq!(
        store.delete_attempts(),
        3,
        "all three deletions are attempted"
    );
    assert_eq!(
        store.deleted(),
        vec!["dup_2".to_string(), "dup_4".to_string()],
        "the failed middle delete is not counted"
    );
    assert!(report.near_duplicate_groups.is_empty());
}

#[tokio::test]
async fn test_near_groups_are_per_shard() {
    // Split over two shards: one member each, so no exact group fires in
    // either shard.
    let split = Arc::new(FakeDedupStore::new(
        vec![shard(1), shard(2)],
        vec![
            (1, mem("mem_a", "same content", 2000, unit_x())),
            (2, mem("mem_b", "same content", 1000, unit_x())),
        ],
    ));
    let report = DedupService::new(split.clone(), true, 0.9)
        .run()
        .await
        .expect("the split walk succeeds");
    assert_eq!(report.exact_duplicates_deleted, 0);
    assert!(report.near_duplicate_groups.is_empty());
    assert!(split.deleted().is_empty());

    // The control: the same two rows in ONE shard are an exact group.
    let combined = Arc::new(FakeDedupStore::new(
        vec![shard(1)],
        vec![
            (1, mem("mem_a", "same content", 2000, unit_x())),
            (1, mem("mem_b", "same content", 1000, unit_x())),
        ],
    ));
    let report = DedupService::new(combined.clone(), true, 0.9)
        .run()
        .await
        .expect("the combined walk succeeds");
    assert_eq!(report.exact_duplicates_deleted, 1);
    assert_eq!(combined.deleted(), vec!["mem_b".to_string()]);
}

#[tokio::test]
async fn test_dedup_service_rejects_a_second_run_while_the_first_is_in_flight() {
    let gate = Arc::new(Gate {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let store = Arc::new(
        FakeDedupStore::new(
            vec![shard(1)],
            vec![
                (1, mem("dup_1", "same content", 2000, unit_x())),
                (1, mem("dup_2", "same content", 1000, unit_x())),
            ],
        )
        .gate(gate.clone()),
    );
    let service = Arc::new(DedupService::new(store.clone(), true, 0.9));

    let first = tokio::spawn({
        let service = service.clone();
        async move { service.run().await }
    });
    gate.entered.notified().await;

    let second = service.run().await;
    assert_eq!(
        second,
        Err(PortError::Invalid(RUNNING_MESSAGE.to_string())),
        "a second run started while the first is in flight is refused"
    );
    assert!(
        store.deleted().is_empty(),
        "the refused run deleted nothing: {:?}",
        store.deleted()
    );

    gate.release.notify_one();
    let report = first
        .await
        .expect("the first run's task joins")
        .expect("the first run succeeds");
    assert_eq!(report.exact_duplicates_deleted, 1);
    assert_eq!(
        store.deleted(),
        vec!["dup_2".to_string()],
        "the first run deletes exactly once"
    );

    // `RunGuard::drop` released the flag on the successful exit path.
    let third = service.run().await.expect("the guard was released");
    assert_eq!(
        third.exact_duplicates_deleted, 0,
        "the store is already deduplicated"
    );
    assert_eq!(store.deleted(), vec!["dup_2".to_string()]);
}
