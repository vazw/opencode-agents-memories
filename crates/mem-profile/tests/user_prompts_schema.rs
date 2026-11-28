//! The production `user_prompts` upgrade path (issue 43 W5): `ready()` applies
//! the v2 columns to a legacy file, and refuses a newer file with the typed
//! error carrying the production label.

mod common;

use mem_profile::error::ProfileError;
use mem_profile::prompt::{USER_PROMPTS_DB_NAME, UserPromptManager};
use mem_store::db::{Db, Value, query_one};
use mem_store::error::StoreError;
use mem_store::schema_migrations::USER_PROMPTS_MAX_KNOWN;

const MAX_RETRIES: u32 = 3;

/// Seeds `user-prompts.db` under the manager's storage root through a direct
/// factory open (never the manager), then drops the handle so `ready()` owns
/// the first open.
async fn seed_user_prompts(
    config: &mem_store::config::StoreConfig,
    version: i64,
    with_v2_columns: bool,
    with_row: bool,
) {
    let db = common::open_real_db(config, USER_PROMPTS_DB_NAME)
        .await
        .expect("seed open");
    let mut ddl = String::from(
        "CREATE TABLE user_prompts (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            project_path TEXT,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            captured INTEGER DEFAULT 0,
            user_learning_captured BOOLEAN DEFAULT 0,
            linked_memory_id TEXT",
    );
    if with_v2_columns {
        ddl.push_str(", capture_attempts INTEGER DEFAULT 0, provider_id TEXT, model_id TEXT");
    }
    ddl.push(')');
    db.execute(&ddl, Vec::new()).await.expect("seed table");
    if with_row {
        db.execute(
            "INSERT INTO user_prompts (id, session_id, message_id, content, created_at, captured)
             VALUES ('prompt_legacy', 'session_legacy', 'message_legacy', 'preserve this prompt', 123, 1)",
            Vec::new(),
        )
        .await
        .expect("seed row");
    }
    db.execute(&format!("PRAGMA user_version = {version}"), Vec::new())
        .await
        .expect("seed version");
    drop(db);
}

async fn column_names(db: &dyn Db) -> Vec<String> {
    db.query("PRAGMA table_info(user_prompts)", Vec::new())
        .await
        .expect("table_info")
        .into_iter()
        .filter_map(|row| row.text("name"))
        .collect()
}

async fn user_version(db: &dyn Db) -> i64 {
    let row = query_one(db, mem_store::sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("read version")
        .expect("pragma row");
    row.i64("user_version").expect("user_version column")
}

/// Whether `name` is a table in the open file (`sqlite_master`).
async fn table_exists(db: &dyn Db, name: &str) -> bool {
    !db.query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
        vec![Value::Text(name.to_string())],
    )
    .await
    .expect("query sqlite_master")
    .is_empty()
}

#[tokio::test]
async fn test_ready_upgrades_a_legacy_prompts_file_to_the_current_schema() {
    let dir = common::temp_dir("prompts-upgrade");
    let config = common::store_config(dir.clone());
    seed_user_prompts(&config, 1, false, true).await;
    let manager = UserPromptManager::new(dir.clone(), config, MAX_RETRIES);

    let db = manager
        .ready()
        .await
        .expect("ready upgrades the legacy file");

    // Bounded by range, not by equality: each leg stamps a fixed numeral and
    // gates on `max_known`, so a bump that adds a leg moves the stamp while a
    // bare constant bump does not. The v4 leg's table is proved separately
    // below, which is what shows the ceiling's leg actually ran.
    let stamped = user_version(db.as_ref()).await;
    assert!(
        (3..=USER_PROMPTS_MAX_KNOWN).contains(&stamped),
        "the open stamps between the v3 sweep stamp and the ceiling, got {stamped}"
    );
    let columns = column_names(db.as_ref()).await;
    for expected in ["capture_attempts", "provider_id", "model_id"] {
        assert!(
            columns.iter().any(|name| name == expected),
            "column {expected} added: {columns:?}"
        );
    }
    assert!(
        table_exists(db.as_ref(), "prompt_tombstones").await,
        "the v4 tombstone leg ran: columns {columns:?}"
    );
    let row = query_one(
        db.as_ref(),
        "SELECT content, capture_attempts FROM user_prompts WHERE id = 'prompt_legacy'",
        Vec::new(),
    )
    .await
    .expect("read the legacy row")
    .expect("the seeded row survives the upgrade");
    assert_eq!(row.text("content").as_deref(), Some("preserve this prompt"));
    assert_eq!(row.i64("capture_attempts"), Some(0));

    let id = manager
        .save_prompt("session-new", "message-new", "/tmp/proj", "hello", None)
        .await
        .expect("a save after the upgrade succeeds");
    let saved = query_one(
        db.as_ref(),
        "SELECT content FROM user_prompts WHERE id = ?",
        vec![Value::Text(id.clone())],
    )
    .await
    .expect("read the saved prompt")
    .expect("the saved prompt exists");
    assert_eq!(saved.text("content").as_deref(), Some("hello"));

    drop(db);
    common::cleanup_temp_dir(&dir);
}

#[tokio::test]
async fn test_ready_refuses_a_newer_prompts_file_with_the_typed_label() {
    let dir = common::temp_dir("prompts-newer");
    let config = common::store_config(dir.clone());
    // One above this build's maximum, so the no-downgrade refusal is what is
    // pinned — not a known version. Derived, so a future bump moves the seed.
    seed_user_prompts(&config, USER_PROMPTS_MAX_KNOWN + 1, true, true).await;
    let manager = UserPromptManager::new(dir.clone(), config, MAX_RETRIES);

    match manager.ready().await {
        Err(ProfileError::Store(StoreError::SchemaNewerThanBuild {
            label,
            found,
            supported,
        })) => {
            assert_eq!(label, USER_PROMPTS_DB_NAME, "the production label");
            assert_eq!(
                found,
                USER_PROMPTS_MAX_KNOWN + 1,
                "the refusal names the file"
            );
            assert_eq!(supported, USER_PROMPTS_MAX_KNOWN);
        }
        Ok(_) => panic!("a file above the ceiling must be refused through ready()"),
        Err(other) => panic!("expected the typed refusal, got {other:?}"),
    }
    assert_eq!(USER_PROMPTS_DB_NAME, "user-prompts.db");

    common::cleanup_temp_dir(&dir);
}
