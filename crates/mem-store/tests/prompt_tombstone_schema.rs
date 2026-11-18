//! Issue 143 — `user-prompts` v4: the prompt tombstone table.
//!
//! The table holds one row per deleted prompt id, id and deletion time only.
//! The column set is pinned by a `table_info` equality, not a `contains` check,
//! so a later `content` / `summary` / `embedding` / `metadata` column fails here.

mod common;

use common::{exclusive_root, open_real_db, store_config};
use mem_store::config::StoreConfig;
use mem_store::db::{Db, Value, query_one};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    MigrationOptions, USER_PROMPTS_MAX_KNOWN, apply_user_prompts_schema,
};

async fn user_version(db: &dyn Db) -> i64 {
    query_one(db, "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("user_version row")
        .i64("user_version")
        .expect("integer user_version")
}

async fn table_exists(db: &dyn Db, table: &str) -> bool {
    query_one(
        db,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
        vec![Value::Text(table.to_string())],
    )
    .await
    .expect("sqlite_master")
    .is_some()
}

fn prompts_options(config: &StoreConfig, max_known: i64) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("user-prompts.db")),
        label: "user-prompts.db",
        max_known,
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_user_prompts_v4_creates_the_tombstone_table() {
    let config = store_config("prompt-tombstone-v4");
    let db = open_real_db(&config, "user-prompts.db")
        .await
        .expect("open");

    let version = apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, USER_PROMPTS_MAX_KNOWN),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("the prompts fold reaches its maximum");

    assert_eq!(USER_PROMPTS_MAX_KNOWN, 4, "the constant moved WITH the leg");
    assert_eq!(version, 4);
    assert_eq!(user_version(db.as_ref()).await, 4);
    assert!(table_exists(db.as_ref(), "prompt_tombstones").await);

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_tombstone_column_set_is_id_and_deletion_time_only() {
    let config = store_config("prompt-tombstone-columns");
    let db = open_real_db(&config, "user-prompts.db")
        .await
        .expect("open");
    apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, USER_PROMPTS_MAX_KNOWN),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("stamp v4");

    let info = db
        .query("PRAGMA table_info(prompt_tombstones)", Vec::new())
        .await
        .expect("table_info(prompt_tombstones)");
    let columns: Vec<(String, String, i64)> = info
        .iter()
        .map(|row| {
            (
                row.text("name").unwrap_or_default(),
                row.text("type").unwrap_or_default(),
                row.i64("notnull").unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        columns,
        vec![
            ("id".to_string(), "TEXT".to_string(), 0),
            ("deleted_at_unix_ms".to_string(), "INTEGER".to_string(), 1),
        ],
        "the tombstone holds an id and a deletion time, nothing else"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_a_four_file_is_refused_by_a_three_only_build() {
    let config = store_config("prompt-tombstone-refuse");
    let db = open_real_db(&config, "user-prompts.db")
        .await
        .expect("open");
    apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, USER_PROMPTS_MAX_KNOWN),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("stamp v4");

    let refused = apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, 3),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect_err("a three-only build refuses a v4 file");
    match refused {
        StoreError::SchemaNewerThanBuild {
            found, supported, ..
        } => assert_eq!((found, supported), (4, 3)),
        other => panic!("expected SchemaNewerThanBuild, got {other:?}"),
    }

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}
