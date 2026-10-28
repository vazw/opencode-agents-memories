//! W3 — count authority: `delete_memories` writes the ABSOLUTE count in the
//! same transaction as its deletes and returns the value read back.

mod common;

use mem_store::config::StoreConfig;
use mem_store::db::{BatchMode, DbHandle, Statement, Value, query_one};
use mem_store::shard_facts;

const TAG: &str = "opencode_project_a1b2c3d4e5f60004";

async fn open(config: &StoreConfig) -> DbHandle {
    let db = common::open_real_db(config, "shard.db")
        .await
        .expect("open the shard");
    shard_facts::ensure_v3(db.as_ref(), 2)
        .await
        .expect("shard schema");
    db
}

/// Seeds `count` rows and the matching `shard_stats` value.
async fn seed(db: &dyn mem_store::db::Db, count: usize) {
    let vector = mem_store::vector::vector_to_text(&[1.0f32, 0.0]).expect("vector text");
    let mut rows = Vec::new();
    for index in 0..count {
        rows.push(Statement::new(
            "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at) \
             VALUES (?1, ?2, vector32(?3), ?4, ?5, ?5)",
            vec![
                Value::Text(format!("mem-{index}")),
                Value::Text(format!("content {index}")),
                Value::Text(vector.clone()),
                Value::Text(TAG.to_string()),
                Value::Integer(1_000 + index as i64),
            ],
        ));
    }
    rows.push(shard_facts::sync_count_statement());
    db.batch(rows, BatchMode::Write)
        .await
        .expect("seed rows and the absolute count");
}

async fn count(db: &dyn mem_store::db::Db) -> i64 {
    query_one(db, "SELECT COUNT(*) AS count FROM memories", Vec::new())
        .await
        .expect("count query")
        .and_then(|row| row.i64("count"))
        .unwrap_or(0)
}

/// AC8 — three rows, delete two: the stored count is 1, not `3 - 1 = 2`.
#[tokio::test]
async fn test_a_group_delete_writes_the_absolute_count_in_the_same_batch() {
    let config = common::store_config("count-group-delete");
    let db = open(&config).await;
    seed(db.as_ref(), 3).await;

    let deleted = mem_store::vector::delete_memories(
        db.as_ref(),
        &["mem-0".to_string(), "mem-1".to_string()],
    )
    .await
    .expect("delete two rows");
    assert_eq!(deleted, 1);
    assert_eq!(count(db.as_ref()).await, 1);
    assert_eq!(
        shard_facts::read_stored_count(db.as_ref())
            .await
            .expect("stored count"),
        1
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC9 — deleting an absent id leaves the stored count unchanged; a delta on
/// `changes = 0` would write `3 - 1 = 2`.
#[tokio::test]
async fn test_deleting_an_absent_id_leaves_the_count_unchanged() {
    let config = common::store_config("count-absent-delete");
    let db = open(&config).await;
    seed(db.as_ref(), 3).await;

    let deleted = mem_store::vector::delete_memories(db.as_ref(), &["mem-absent".to_string()])
        .await
        .expect("delete an absent id");
    assert_eq!(deleted, 3);
    assert_eq!(count(db.as_ref()).await, 3);
    assert_eq!(
        shard_facts::read_stored_count(db.as_ref())
            .await
            .expect("stored count"),
        3
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC10 — two concurrent deletes from a stale base: every delta ordering
/// lands at most at 1, the absolute count lands at the true 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_deletes_from_a_stale_base_end_at_the_true_count() {
    let config = common::store_config("count-concurrent-delete");
    let first = open(&config).await;
    let second = open(&config).await;
    seed(first.as_ref(), 4).await;

    // Force the stale base the delta API would trust.
    first
        .execute(
            "UPDATE shard_stats SET vector_count = 1 WHERE id = 1",
            Vec::new(),
        )
        .await
        .expect("seed a stale count");

    let first_id = vec!["mem-0".to_string()];
    let second_id = vec!["mem-1".to_string()];
    let (left, right) = tokio::join!(
        mem_store::vector::delete_memories(first.as_ref(), &first_id),
        mem_store::vector::delete_memories(second.as_ref(), &second_id),
    );
    let left = left.expect("first delete");
    let right = right.expect("second delete");
    assert_eq!(count(first.as_ref()).await, 2);
    assert_eq!(
        shard_facts::read_stored_count(first.as_ref())
            .await
            .expect("stored count"),
        2,
        "the absolute count is the true row count (delta orderings give {left}/{right})"
    );

    drop(first);
    drop(second);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}
