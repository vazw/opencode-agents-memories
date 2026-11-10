//! Retention sweep test suite (issue 39).
//! Includes insert path tests and shard-level retention sweep tests.

mod common;

use std::collections::HashSet;

use mem_store::db::query_one;
use mem_store::shard_facts;
use mem_store::vector::{MemoryInsert, insert_vector};

const DIMS: u32 = 2;
const RETENTION_MS: i64 = 1000;
const TWO_RETENTION_MS: i64 = 2000;
const CUTOFF: i64 = 100_000;

fn test_record(id: &str, stamp: Option<i64>) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: format!("content for {id}"),
        vector: vec![1.0, 0.0],
        tags_vector: None,
        container_tag: "opencode_project_test".to_string(),
        tags: None,
        memory_type: None,
        created_at: 1000,
        updated_at: 1000,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
        source_prompt_created_at: stamp,
    }
}

async fn setup_sweep_shard(
    prefix: &str,
) -> (
    mem_store::shard_manager::ShardManager,
    mem_store::shard_manager::ShardInfo,
    mem_store::db::DbHandle,
) {
    let config = common::test_store_config(prefix, DIMS, 10_000);
    let (_cfg, _conn, manager) = common::real_components(config).await;
    let shard = manager
        .get_write_shard(mem_core::scope::Scope::Project, "a1b2c3d4e5f60001")
        .await
        .expect("write shard");
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    (manager, shard, db)
}

async fn seed_memory(
    db: &dyn mem_store::db::Db,
    id: &str,
    container_tag: &str,
    updated_at: i64,
    stamp: Option<i64>,
    is_pinned: bool,
) {
    let mut rec = test_record(id, stamp);
    rec.container_tag = container_tag.to_string();
    rec.updated_at = updated_at;
    insert_vector(db, &rec).await.expect("insert memory");
    if is_pinned {
        db.execute(
            "UPDATE memories SET is_pinned = 1 WHERE id = ?",
            vec![mem_store::db::Value::Text(id.to_string())],
        )
        .await
        .expect("pin memory");
    }
}

#[tokio::test]
async fn insert_persists_stamp_and_roundtrips_null() {
    let config = common::store_config("retention-insert-null");
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open");
    shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("ensure_v3");

    let rec = test_record("mem_null_stamp", None);
    insert_vector(db.as_ref(), &rec).await.expect("insert");

    let row = query_one(
        db.as_ref(),
        "SELECT source_prompt_created_at FROM memories WHERE id = 'mem_null_stamp'",
        Vec::new(),
    )
    .await
    .expect("query")
    .expect("exists");

    assert!(
        row.i64("source_prompt_created_at").is_none(),
        "source_prompt_created_at should be NULL"
    );
}

#[tokio::test]
async fn insert_persists_stamp_value() {
    let config = common::store_config("retention-insert-val");
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open");
    shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("ensure_v3");

    let rec = test_record("mem_with_stamp", Some(9876543210));
    insert_vector(db.as_ref(), &rec).await.expect("insert");

    let row = query_one(
        db.as_ref(),
        "SELECT source_prompt_created_at FROM memories WHERE id = 'mem_with_stamp'",
        Vec::new(),
    )
    .await
    .expect("query")
    .expect("exists");

    assert_eq!(
        row.i64("source_prompt_created_at"),
        Some(9876543210),
        "source_prompt_created_at should match inserted value"
    );
}

#[tokio::test]
async fn retention_row1_stamp_null_deleted() {
    let (manager, _shard, db) = setup_sweep_shard("ret-row1").await;
    seed_memory(
        db.as_ref(),
        "mem_null_expired",
        "opencode_project_test",
        CUTOFF - 100,
        None,
        false,
    )
    .await;

    let grace_ids = HashSet::new();
    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 1);
    assert_eq!(outcome.deleted_ids, vec!["mem_null_expired".to_string()]);
    assert_eq!(outcome.project_count, 1);
    assert_eq!(outcome.user_count, 0);

    let row = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_null_expired'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(row.is_none(), "NULL stamp row must be deleted");
}

#[tokio::test]
async fn retention_row2_recent_stamp_survives() {
    let (manager, _shard, db) = setup_sweep_shard("ret-row2").await;
    let stamp = CUTOFF - RETENTION_MS + 1;
    let updated_at = CUTOFF - 10 * RETENTION_MS;
    seed_memory(
        db.as_ref(),
        "mem_recent_stamp",
        "opencode_project_test",
        updated_at,
        Some(stamp),
        false,
    )
    .await;

    let grace_ids = HashSet::new();
    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 0);
    assert!(outcome.deleted_ids.is_empty());

    let row = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_recent_stamp'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(row.is_some(), "recent stamp row must survive");
}

#[tokio::test]
async fn retention_row3_expired_stamp_deleted() {
    let (manager, _shard, db) = setup_sweep_shard("ret-row3").await;
    let stamp = CUTOFF - TWO_RETENTION_MS - 1;
    let updated_at = CUTOFF - 100;
    seed_memory(
        db.as_ref(),
        "mem_expired_stamp",
        "opencode_user_test",
        updated_at,
        Some(stamp),
        false,
    )
    .await;

    let grace_ids = HashSet::new();
    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 1);
    assert_eq!(outcome.deleted_ids, vec!["mem_expired_stamp".to_string()]);
    assert_eq!(outcome.user_count, 1);

    let row = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_expired_stamp'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(row.is_none(), "expired stamp row must be deleted");
}

#[tokio::test]
async fn retention_row3b_grace_boundary_pair() {
    let (manager, _shard, db) = setup_sweep_shard("ret-row3b").await;
    let stamp_at_boundary = CUTOFF - TWO_RETENTION_MS;
    let stamp_in_grace = CUTOFF - TWO_RETENTION_MS + 1;
    let updated_at = CUTOFF - 100;

    seed_memory(
        db.as_ref(),
        "mem_boundary_lapsed",
        "opencode_project_test",
        updated_at,
        Some(stamp_at_boundary),
        false,
    )
    .await;
    seed_memory(
        db.as_ref(),
        "mem_boundary_spared",
        "opencode_project_test",
        updated_at,
        Some(stamp_in_grace),
        false,
    )
    .await;

    let grace_ids = HashSet::new();
    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 1);
    assert_eq!(outcome.deleted_ids, vec!["mem_boundary_lapsed".to_string()]);

    let row_lapsed = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_boundary_lapsed'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(
        row_lapsed.is_none(),
        "exact boundary s = c - 2r must be deleted"
    );

    let row_spared = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_boundary_spared'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(
        row_spared.is_some(),
        "boundary s = c - 2r + 1 must be spared by grace"
    );
}

#[tokio::test]
async fn retention_row4_grace_exemption_survives() {
    let (manager, _shard, db) = setup_sweep_shard("ret-row4").await;
    let stamp = CUTOFF - 1500;
    let updated_at = CUTOFF - 100;

    seed_memory(
        db.as_ref(),
        "mem_stamped_grace",
        "opencode_project_test",
        updated_at,
        Some(stamp),
        false,
    )
    .await;
    seed_memory(
        db.as_ref(),
        "mem_null_grace",
        "opencode_project_test",
        updated_at,
        None,
        false,
    )
    .await;

    let mut grace_ids = HashSet::new();
    grace_ids.insert("mem_null_grace".to_string());

    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 0);
    assert!(outcome.deleted_ids.is_empty());

    let row_stamped = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_stamped_grace'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(row_stamped.is_some(), "stamped grace row must survive");

    let row_null = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_null_grace'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(
        row_null.is_some(),
        "null-stamp linked grace row must survive"
    );
}

#[tokio::test]
async fn retention_row5_young_updated_at_survives() {
    let (manager, _shard, db) = setup_sweep_shard("ret-row5").await;
    let stamp = CUTOFF - 10 * RETENTION_MS;
    let updated_at = CUTOFF;

    seed_memory(
        db.as_ref(),
        "mem_young_updated_at",
        "opencode_project_test",
        updated_at,
        Some(stamp),
        false,
    )
    .await;

    let grace_ids = HashSet::new();
    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 0);

    let row = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_young_updated_at'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(row.is_some(), "u == c row must survive");
}

#[tokio::test]
async fn pinned_memory_survives() {
    let (manager, _shard, db) = setup_sweep_shard("ret-pinned").await;
    let stamp = CUTOFF - TWO_RETENTION_MS - 1;
    let updated_at = CUTOFF - 100;

    seed_memory(
        db.as_ref(),
        "mem_pinned",
        "opencode_project_test",
        updated_at,
        Some(stamp),
        true,
    )
    .await;

    let grace_ids = HashSet::new();
    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 0);
    assert_eq!(outcome.protected_memories_skipped, 1);

    let row = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_pinned'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(row.is_some(), "pinned memory must survive sweep");
}

#[tokio::test]
async fn vector_count_adopted_after_deletions() {
    let (manager, shard, db) = setup_sweep_shard("ret-count").await;
    let expired_stamp = CUTOFF - TWO_RETENTION_MS - 1;
    let updated_at = CUTOFF - 100;

    seed_memory(
        db.as_ref(),
        "mem_del_1",
        "opencode_project_test",
        updated_at,
        Some(expired_stamp),
        false,
    )
    .await;
    seed_memory(
        db.as_ref(),
        "mem_del_2",
        "opencode_project_test",
        updated_at,
        Some(expired_stamp),
        false,
    )
    .await;
    seed_memory(
        db.as_ref(),
        "mem_keep",
        "opencode_project_test",
        updated_at,
        Some(CUTOFF),
        false,
    )
    .await;

    // Set stale registry count to verify adoption overwrites with real count
    manager
        .adopt_stored_count(db.as_ref(), shard.id)
        .await
        .expect("initial adopt");
    manager
        .set_vector_count(shard.id, 7)
        .await
        .expect("set stale count");

    let grace_ids = HashSet::new();
    let outcome = manager
        .run_retention_sweep(CUTOFF, RETENTION_MS, TWO_RETENTION_MS, &grace_ids)
        .await
        .expect("sweep");

    assert_eq!(outcome.deleted_count, 2);

    let updated_shard = manager
        .get_shard_by_id(shard.id)
        .await
        .expect("read shard")
        .expect("shard exists");
    assert_eq!(
        updated_shard.vector_count, 1,
        "registry vector_count must be adopted exactly as COUNT(*)"
    );
}

#[tokio::test]
async fn run_failed_mid_phase_leaves_no_early_death() {
    let (manager, _shard, db) = setup_sweep_shard("ret-idempotent").await;
    let s = 1_000_000_i64;
    let r = 1_000_i64;
    let two_r = 2_000_i64;
    let c1 = 1_001_500_i64;
    let updated_at = 900_000_i64;

    seed_memory(
        db.as_ref(),
        "mem_stateless",
        "opencode_project_test",
        updated_at,
        Some(s),
        false,
    )
    .await;

    let grace_ids = HashSet::new();

    // Run 1 at c1: spared by grace
    let outcome1 = manager
        .run_retention_sweep(c1, r, two_r, &grace_ids)
        .await
        .expect("sweep 1");
    assert_eq!(outcome1.deleted_count, 0, "run 1 must spare row in grace");

    // Run 2 at c1 (restart after failure): STILL spared
    let outcome2 = manager
        .run_retention_sweep(c1, r, two_r, &grace_ids)
        .await
        .expect("sweep 2");
    assert_eq!(
        outcome2.deleted_count, 0,
        "run 2 at c1 must still spare row"
    );

    // Run 3 at c2 = s + 2r (grace lapsed)
    let c2 = s + two_r;
    let outcome3 = manager
        .run_retention_sweep(c2, r, two_r, &grace_ids)
        .await
        .expect("sweep 3");
    assert_eq!(
        outcome3.deleted_count, 1,
        "run 3 at c2 must delete row as grace lapsed"
    );

    let row = query_one(
        db.as_ref(),
        "SELECT id FROM memories WHERE id = 'mem_stateless'",
        Vec::new(),
    )
    .await
    .expect("query");
    assert!(row.is_none(), "row must be deleted once grace lapses");
}
