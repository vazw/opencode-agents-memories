//! Issue 174 — `user-prompts` v5: the nullable `user_id` identity column.
//!
//! The column is ADDITIVE and NULLABLE: a fresh file gets it from the v5 ALTER
//! leg (never from the byte-frozen v1 DDL), and a v4 file's existing rows keep
//! `NULL` — the migration never invents an identity.

mod common;

use std::path::Path;

use mem_store::db::{Db, Value, query_one};
use mem_store::schema_migrations::{
    MigrationOptions, USER_PROMPTS_MAX_KNOWN, apply_user_prompts_schema,
};

const PROMPTS_DB: &str = "user-prompts.db";

fn options(db_path: &Path) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(db_path.to_path_buf()),
        label: "test",
        max_known: USER_PROMPTS_MAX_KNOWN,
    }
}

async fn read_user_version(db: &dyn Db) -> i64 {
    query_one(db, "PRAGMA user_version", Vec::new())
        .await
        .expect("read user_version")
        .expect("pragma row")
        .i64("user_version")
        .expect("integer user_version")
}

/// `(declared type, default, notnull)` for one column, or `None` when absent.
async fn column_shape(db: &dyn Db, column: &str) -> Option<(String, Option<String>, i64)> {
    db.query("PRAGMA table_info(user_prompts)", Vec::new())
        .await
        .expect("table_info")
        .into_iter()
        .find(|row| row.text("name").as_deref() == Some(column))
        .map(|row| {
            (
                row.text("type").unwrap_or_default(),
                row.get("dflt_value").and_then(|value| match value {
                    Value::Text(text) => Some(text.clone()),
                    Value::Integer(number) => Some(number.to_string()),
                    Value::Real(number) => Some(number.to_string()),
                    Value::Null | Value::Blob(_) => None,
                }),
                row.i64("notnull").unwrap_or_default(),
            )
        })
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fresh_prompts_db_reaches_v5_with_user_id() {
    let config = common::store_config("prompts-v5-fresh");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    let proof = common::exclusive_root(&config.storage_path);

    let version = apply_user_prompts_schema(db.as_ref(), &options(&db_path), Some(proof.as_ref()))
        .await
        .expect("a fresh file converges to the set's maximum");

    assert_eq!(USER_PROMPTS_MAX_KNOWN, 5, "the constant moved WITH the leg");
    assert_eq!(version, 5, "the applier reports v5");
    assert_eq!(read_user_version(db.as_ref()).await, 5, "stamped 5");

    let shape = column_shape(db.as_ref(), "user_id")
        .await
        .expect("the v5 leg added user_id");
    assert_eq!(shape.0, "TEXT", "user_id is TEXT");
    assert_eq!(shape.1, None, "no default: an unstamped row is NULL");
    assert_eq!(shape.2, 0, "nullable: older rows have no identity");

    // The column exists exactly once (v1 DDL did not grow a copy).
    let columns = common::user_prompt_columns(db.as_ref()).await;
    assert_eq!(
        columns
            .iter()
            .filter(|name| name.as_str() == "user_id")
            .count(),
        1,
        "the additive leg must not duplicate a DDL column: {columns:?}"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v4_fixture_adds_a_null_user_id() {
    let config = common::store_config("prompts-v5-upgrade");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    common::seed_user_prompts(db.as_ref(), 4, true, true)
        .await
        .expect("seed a v4 file with one row");
    let proof = common::exclusive_root(&config.storage_path);

    let version = apply_user_prompts_schema(db.as_ref(), &options(&db_path), Some(proof.as_ref()))
        .await
        .expect("a v4 file upgrades");

    assert_eq!(version, 5);
    assert_eq!(read_user_version(db.as_ref()).await, 5);

    let row = query_one(
        db.as_ref(),
        "SELECT id, project_path, user_id FROM user_prompts WHERE id = ?",
        vec![Value::Text("prompt_legacy".to_string())],
    )
    .await
    .expect("read the seeded row")
    .expect("the v4 row survives");
    assert_eq!(row.text("id").as_deref(), Some("prompt_legacy"));
    assert_eq!(row.text("project_path"), None, "the v4 row's shape is kept");
    assert_eq!(
        row.text("user_id"),
        None,
        "a legacy row's identity is NULL, never a fabricated one"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}
