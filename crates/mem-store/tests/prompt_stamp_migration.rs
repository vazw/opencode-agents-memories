//! Migration tests for shard schema v4: `source_prompt_created_at` (issue 39).

mod common;

use mem_store::db::{Value, query_one};
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::shard_facts;
use mem_store::sql;

const DIMS: u32 = 2;

#[tokio::test]
async fn migration_no_backfill_and_pre_v4_row_reads_null() {
    let config = common::store_config("prompt-stamp-mig-v3");
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open");
    let through_v3: Vec<_> = memory_shard_migrations(DIMS)
        .into_iter()
        .filter(|m| m.version <= 3)
        .collect();
    let v3_res = apply_schema_migrations(
        db.as_ref(),
        &through_v3,
        &MigrationOptions {
            db_path: None,
            label: "v3-fixture",
            max_known: 3,
        },
        None,
    )
    .await
    .expect("apply v1..v3");
    assert_eq!(v3_res, 3);

    let vector = mem_store::vector::vector_to_text(&[1.0f32, 2.0f32]).expect("vector");
    db.execute(
        "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at)
         VALUES (?1, ?2, vector32(?3), 'opencode_project_test', 1000, 1000)",
        vec![
            Value::Text("mem_pre_v4".to_string()),
            Value::Text("content".to_string()),
            Value::Text(vector),
        ],
    )
    .await
    .expect("seed row");

    let version = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("apply v4");
    assert_eq!(version, 4);

    let row = query_one(
        db.as_ref(),
        "SELECT source_prompt_created_at FROM memories WHERE id = 'mem_pre_v4'",
        Vec::new(),
    )
    .await
    .expect("query row")
    .expect("row exists");

    assert!(
        row.i64("source_prompt_created_at").is_none(),
        "source_prompt_created_at must be NULL without backfill"
    );
}

#[tokio::test]
async fn fresh_root_reaches_v4_in_one_open() {
    let config = common::store_config("prompt-stamp-mig-fresh");
    let db = common::open_real_db(&config, "fresh.db")
        .await
        .expect("open");
    let version = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("ensure_v3");
    assert_eq!(version, 4);

    let row = query_one(db.as_ref(), sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("user_version")
        .expect("row");
    assert_eq!(row.i64("user_version"), Some(4));

    let columns = db
        .query("PRAGMA table_info(memories)", Vec::new())
        .await
        .expect("table_info");
    assert_eq!(columns.len(), 19);
    assert!(
        columns
            .iter()
            .any(|r| r.text("name").as_deref() == Some("source_prompt_created_at"))
    );
}

#[tokio::test]
async fn second_apply_is_noop_v4() {
    let config = common::store_config("prompt-stamp-mig-noop");
    let db = common::open_real_db(&config, "noop.db")
        .await
        .expect("open");
    let version1 = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("first ensure");
    assert_eq!(version1, 4);

    let version2 = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("second ensure");
    assert_eq!(version2, 4);

    let row = query_one(db.as_ref(), sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("user_version")
        .expect("row");
    assert_eq!(row.i64("user_version"), Some(4));
}

#[tokio::test]
async fn v2_file_upgrades_to_v4_in_one_open() {
    let config = common::store_config("prompt-stamp-mig-v2-to-v4");
    let db = common::open_real_db(&config, "v2.db").await.expect("open");
    let through_v2: Vec<_> = memory_shard_migrations(DIMS)
        .into_iter()
        .filter(|m| m.version <= 2)
        .collect();
    let v2_res = apply_schema_migrations(
        db.as_ref(),
        &through_v2,
        &MigrationOptions {
            db_path: None,
            label: "v2-fixture",
            max_known: 2,
        },
        None,
    )
    .await
    .expect("apply v1..v2");
    assert_eq!(v2_res, 2);

    let version = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("upgrade v2->v4");
    assert_eq!(version, 4);

    let columns = db
        .query("PRAGMA table_info(memories)", Vec::new())
        .await
        .expect("table_info");
    assert_eq!(columns.len(), 19);
    assert!(
        columns
            .iter()
            .any(|r| r.text("name").as_deref() == Some("source_prompt_created_at"))
    );
}
