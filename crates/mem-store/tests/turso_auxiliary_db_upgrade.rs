//! User-prompts auxiliary DB upgrade battery (blueprint W1.2 step 6).
//!
//! Adapted from upstream `tests/turso-auxiliary-db-upgrade.test.ts`. Both
//! remaining cases run through [`apply_user_prompts_schema`] (issue 43 W5),
//! which probes `PRAGMA table_info` before ALTERing; the column table lives in
//! `mem_store::user_prompts_sql`.
//!
//! Case dispositions, declared: case 2 is renamed because the applier
//! **prevents** the duplicate instead of tolerating the error; upstream's
//! "swallow non-duplicate errors" case is **retired** in favour of
//! `test_missing_table_at_v1_is_surfaced_not_swallowed` in
//! `tests/user_prompts_schema_v2.rs` (the port refuses where upstream
//! swallowed).

mod common;

use mem_store::db::Value;
use mem_store::schema_migrations::{
    MigrationOptions, USER_PROMPTS_MAX_KNOWN, apply_user_prompts_schema,
};

async fn legacy_prompts_db(config: &mem_store::config::StoreConfig) -> mem_store::db::DbHandle {
    let db = common::open_real_db(config, "user-prompts.db")
        .await
        .expect("open");
    // Upstream legacy shape: 9 columns, one row, no version pragma.
    common::seed_user_prompts(db.as_ref(), 0, false, true)
        .await
        .expect("seed the legacy shape");
    db
}

async fn column_names(db: &mem_store::db::DbHandle) -> Vec<String> {
    db.query("PRAGMA table_info(user_prompts)", Vec::new())
        .await
        .expect("table_info")
        .into_iter()
        .map(|row| row.text("name").expect("name column"))
        .collect()
}

fn migration_options(config: &mem_store::config::StoreConfig) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("user-prompts.db")),
        label: "test",
        max_known: USER_PROMPTS_MAX_KNOWN,
    }
}

// ---------------------------------------------------------------------------
// 1/2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_preserves_legacy_prompts_while_adding_newer_columns() {
    let config = common::store_config("aux-upgrade");
    let db = legacy_prompts_db(&config).await;
    let proof = common::exclusive_root(&config.storage_path);

    apply_user_prompts_schema(
        db.as_ref(),
        &migration_options(&config),
        Some(proof.as_ref()),
    )
    .await
    .expect("the legacy file upgrades");

    let rows = db
        .query(
            "SELECT id, capture_attempts, provider_id, model_id FROM user_prompts WHERE id = ?",
            vec![Value::Text("prompt_legacy".to_string())],
        )
        .await
        .expect("legacy row survives");
    assert_eq!(rows.len(), 1, "row preserved");
    let row = &rows[0];
    assert_eq!(
        row.i64("capture_attempts"),
        Some(0),
        "default capture_attempts"
    );
    assert!(row.is_null("provider_id"), "default provider_id NULL");
    assert!(row.is_null("model_id"), "default model_id NULL");

    let names = column_names(&db).await;
    for expected in ["capture_attempts", "provider_id", "model_id"] {
        assert!(
            names.contains(&expected.to_string()),
            "column {expected} added"
        );
    }
}

// ---------------------------------------------------------------------------
// 2/2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_repeated_upgrade_adds_no_duplicate_columns() {
    let config = common::store_config("aux-repeat");
    let db = legacy_prompts_db(&config).await;
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&config);

    apply_user_prompts_schema(db.as_ref(), &options, Some(proof.as_ref()))
        .await
        .expect("first upgrade");
    apply_user_prompts_schema(db.as_ref(), &options, Some(proof.as_ref()))
        .await
        .expect("repeat upgrade");

    let names = column_names(&db).await;
    for expected in ["capture_attempts", "provider_id", "model_id"] {
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == expected)
                .count(),
            1,
            "no duplicate {expected}"
        );
    }
}
