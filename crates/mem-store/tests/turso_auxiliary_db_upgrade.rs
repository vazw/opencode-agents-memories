//! User-prompts auxiliary DB upgrade battery (blueprint W1.2 step 6).
//!
//! Adapted from upstream `tests/turso-auxiliary-db-upgrade.test.ts` — the
//! legacy-9-column case runs through our `ensure_user_prompt_columns`
//! instead of the upstream userPromptManager wiring (manager arrives with
//! the memory waves; the COLUMN-UPGRADE semantics are what this battery
//! pins).

mod common;

use mem_store::db::Value;
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, ensure_user_prompt_columns, user_prompts_migrations,
};

async fn legacy_prompts_db(config: &mem_store::config::StoreConfig) -> mem_store::db::DbHandle {
    let db = common::open_real_db(config, "user-prompts.db")
        .await
        .expect("open");
    // Upstream legacy shape: 9 columns, no capture_attempts/provider_id/model_id.
    db.execute(
        "CREATE TABLE user_prompts (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            project_path TEXT,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            captured INTEGER DEFAULT 0,
            user_learning_captured BOOLEAN DEFAULT 0,
            linked_memory_id TEXT
        )",
        Vec::new(),
    )
    .await
    .expect("legacy table");
    db.execute(
        "INSERT INTO user_prompts (
            id, session_id, message_id, project_path, content, created_at, captured
        ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        vec![
            Value::Text("prompt_legacy".to_string()),
            Value::Text("session_legacy".to_string()),
            Value::Text("message_legacy".to_string()),
            Value::Text("/legacy/project".to_string()),
            Value::Text("preserve this prompt".to_string()),
            Value::Integer(123),
            Value::Integer(1),
        ],
    )
    .await
    .expect("legacy row");
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

// ---------------------------------------------------------------------------
// 1/3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_preserves_legacy_prompts_while_adding_newer_columns() {
    let config = common::store_config("aux-upgrade");
    let db = legacy_prompts_db(&config).await;

    // The v1 baseline migration is idempotent on the legacy shape.
    apply_schema_migrations(
        db.as_ref(),
        &user_prompts_migrations(),
        &MigrationOptions {
            db_path: Some(config.storage_path.join("user-prompts.db")),
            label: "test",
        },
        Some(common::exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("baseline applies");
    ensure_user_prompt_columns(db.as_ref())
        .await
        .expect("upgrade");

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
// 2/3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_repeated_upgrade_tolerates_duplicate_columns() {
    let config = common::store_config("aux-repeat");
    let db = legacy_prompts_db(&config).await;
    ensure_user_prompt_columns(db.as_ref())
        .await
        .expect("first upgrade");
    ensure_user_prompt_columns(db.as_ref())
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

// ---------------------------------------------------------------------------
// 3/3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_non_duplicate_errors_are_swallowed_after_warning() {
    let config = common::store_config("aux-swallow");
    // No user_prompts table at all: every ALTER fails with a non-duplicate
    // error. Upstream `ensureUserPromptColumns` logs
    // ("Failed to add user_prompts column") and continues — startup language:
    // `catch (error) { if (!message.includes("duplicate column")) { log(...) } }`
    // — never aborts the process.
    let db = common::open_real_db(&config, "empty.db")
        .await
        .expect("open");
    ensure_user_prompt_columns(db.as_ref())
        .await
        .expect("still Ok(())");
    // even a repeated missing-table upgrade stays tolerated
    ensure_user_prompt_columns(db.as_ref())
        .await
        .expect("repeated, still Ok(())");
}
