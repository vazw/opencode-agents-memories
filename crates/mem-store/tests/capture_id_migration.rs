//! W5.1 step 4 — `capture_id` additive migration oracles.
//!
//! ADR-0007 D-36/D-47: the episodic row gains a nullable `capture_id TEXT`
//! column as an **additive** v4 migration through the existing runner. No new
//! table, no new migration directory, no recapture/upsert policy (that is
//! W5.3's).

mod common;

use common::{open_real_db, store_config};
use mem_store::db::{Db, query_one};
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, episodic_migrations,
};

const DIMS: u32 = 8;

async fn table_columns(db: &dyn Db) -> Vec<String> {
    db.query("PRAGMA table_info(agent_sessions)", Vec::new())
        .await
        .expect("table_info")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect()
}

async fn user_version(db: &dyn Db) -> i64 {
    query_one(db, "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("user_version row")
        .i64("user_version")
        .expect("integer user_version")
}

fn options(config: &mem_store::config::StoreConfig) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("episodic.db")),
        label: "episodic-capture-id",
    }
}

fn through_v3() -> Vec<mem_store::schema_migrations::SchemaMigration> {
    episodic_migrations(DIMS)
        .into_iter()
        .filter(|migration| migration.version <= 3)
        .collect()
}

#[tokio::test]
async fn test_capture_id_migration_adds_column() {
    let config = store_config("capture-id-column");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    let options = options(&config);

    // Fresh database: the full set reaches v4 and the column exists.
    let version = apply_schema_migrations(&*db, &episodic_migrations(DIMS), &options)
        .await
        .expect("apply fresh");
    assert_eq!(version, 5, "the episodic migration set's max version is v5");
    assert_eq!(user_version(&*db).await, 5);

    let columns = table_columns(&*db).await;
    assert!(
        columns.contains(&"capture_id".to_string()),
        "capture_id column present: {columns:?}"
    );
    // Additive column: appended after the v2 columns, nullable TEXT, no default.
    assert_eq!(
        columns.last().map(String::as_str),
        Some("capture_id"),
        "the ALTER appends: {columns:?}"
    );
    let info = db
        .query("PRAGMA table_info(agent_sessions)", Vec::new())
        .await
        .expect("table_info");
    let capture_id = info
        .iter()
        .find(|row| row.text("name").as_deref() == Some("capture_id"))
        .expect("capture_id row");
    assert_eq!(capture_id.text("type").as_deref(), Some("TEXT"));
    assert_eq!(
        capture_id.i64("notnull"),
        Some(0),
        "nullable: {capture_id:?}"
    );
    // The column carries no default: `dflt_value` is SQL NULL (or absent).
    assert!(
        !matches!(capture_id.get("dflt_value"), Some(value) if !value.is_null()),
        "no default: {capture_id:?}"
    );

    // Insert-only path keeps working: the column is not bound, so it is NULL.
    db.execute(
        "INSERT INTO agent_sessions (id, project_id, session_id, agent_id, parent_session_id, \
         summary, metadata, embedding, created_at, updated_at, deleted_at, orphaned) \
         VALUES ('row-1', 'proj', 'sess', 'agent', NULL, 'summary', '{}', vector32('[0,0,0,0,0,0,0,0]'), 1, 1, NULL, 0)",
        Vec::new(),
    )
    .await
    .expect("insert without capture_id");
    let stored = query_one(
        &*db,
        "SELECT capture_id FROM agent_sessions WHERE id = 'row-1'",
        Vec::new(),
    )
    .await
    .expect("select capture_id")
    .expect("row");
    assert!(
        stored.is_null("capture_id"),
        "unset capture_id is SQL NULL: {stored:?}"
    );
}

#[tokio::test]
async fn test_capture_id_migration_idempotent() {
    let config = store_config("capture-id-idempotent");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    let options = options(&config);

    // A pre-existing v3 database (the state every W4.x install is in).
    let version = apply_schema_migrations(&*db, &through_v3(), &options)
        .await
        .expect("apply through v3");
    assert_eq!(version, 3);
    assert!(
        !table_columns(&*db)
            .await
            .contains(&"capture_id".to_string())
    );

    // The v4 step upgrades it in place.
    let first = apply_schema_migrations(&*db, &episodic_migrations(DIMS), &options)
        .await
        .expect("apply v4");
    assert_eq!(first, 5);

    // Re-applying is a no-op: no duplicate column, version unchanged.
    let second = apply_schema_migrations(&*db, &episodic_migrations(DIMS), &options)
        .await
        .expect("re-apply");
    assert_eq!(second, 5, "nothing is pending after v5");
    assert_eq!(user_version(&*db).await, 5);

    let columns = table_columns(&*db).await;
    let capture_id_count = columns
        .iter()
        .filter(|name| name.as_str() == "capture_id")
        .count();
    assert_eq!(capture_id_count, 1, "exactly one column: {columns:?}");
    assert_eq!(columns.len(), 13, "v1+v2 columns plus capture_id");
}
