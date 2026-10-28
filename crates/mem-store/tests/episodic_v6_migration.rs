//! W7.1 — episodic v6: the convergent retention schema.
//!
//! What v6 adds, and what it deliberately does NOT add:
//! - `agent_sessions.agent_role TEXT NOT NULL DEFAULT 'main'` (one `ALTER`);
//! - `episodic_raw_context` (keyed by the logical triple, **no FK**);
//! - `episodic_segments` (derived, regenerable, **no FK, no cascade**);
//! - `idx_episodic_segments_session`.
//!
//! There is no `raw_context` column on `agent_sessions` (M1) and no
//! `ON DELETE` anywhere: an invariant that says "raw context is never
//! destroyed by consolidation" cannot be implemented by a schema feature
//! whose only job is to destroy it.

mod common;

use common::{open_real_db, store_config};
use mem_store::db::{BatchMode, Db, DbHandle, ExecResult, Statement, Value, query_one};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    EpisodicSchemaOutcome, MigrationOptions, PragmaOutcome, apply_episodic_schema,
    apply_schema_migrations, episodic_migrations,
};

const DIMS: u32 = 2;

/// The memoised root proof (a destructive upgrade needs one).
fn root_proof(
    config: &mem_store::config::StoreConfig,
) -> std::sync::Arc<mem_store::delete_guard::ExclusiveRoot> {
    common::exclusive_root(&config.storage_path)
}

fn options(config: &mem_store::config::StoreConfig) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("episodic.db")),
        label: "episodic-v6",
    }
}

async fn user_version(db: &dyn Db) -> i64 {
    query_one(db, "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("user_version row")
        .i64("user_version")
        .expect("integer user_version")
}

async fn table_columns(db: &dyn Db, table: &str) -> Vec<String> {
    db.query(&format!("PRAGMA table_info({table})"), Vec::new())
        .await
        .expect("table_info")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect()
}

/// Builds a genuine v5 database (the migration set direct callers see).
async fn v5_database(config: &mem_store::config::StoreConfig) -> DbHandle {
    let db = open_real_db(config, "episodic.db").await.expect("open");
    let version = apply_schema_migrations(
        &*db,
        &episodic_migrations(DIMS),
        &options(config),
        Some(root_proof(config).as_ref()),
    )
    .await
    .expect("apply v1..v5");
    assert_eq!(version, 5, "the direct set stays at v5");
    db
}

async fn insert_v5_row(db: &dyn Db, id: &str) {
    db.execute(
        "INSERT INTO agent_sessions (id, capture_id, project_id, session_id, agent_id, \
         parent_session_id, summary, metadata, embedding, created_at, updated_at, deleted_at, \
         orphaned) VALUES (?, NULL, 'proj', 'sess', 'agent', NULL, 'the summary', \
         '{\"timestamp\":1,\"agentName\":\"agent\"}', vector32('[0,0]'), 7, 7, NULL, 0)",
        vec![Value::Text(id.to_string())],
    )
    .await
    .expect("insert v5 row");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_upgrade_preserves_existing_rows_and_defaults() {
    let config = store_config("v6-upgrade");
    let db = v5_database(&config).await;
    insert_v5_row(&*db, "row-v5").await;

    let outcome = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("v6 apply");
    assert_eq!(outcome.version, 6);
    assert_eq!(user_version(&*db).await, 6);

    // The existing row survives untouched; `agent_role` defaults to 'main'.
    let row = query_one(
        &*db,
        "SELECT summary, metadata, created_at, updated_at, agent_role FROM agent_sessions WHERE id = 'row-v5'",
        Vec::new(),
    )
    .await
    .expect("select")
    .expect("row survives");
    assert_eq!(row.text("summary").as_deref(), Some("the summary"));
    assert!(
        row.text("metadata")
            .expect("metadata")
            .contains("agentName")
    );
    assert_eq!(row.i64("created_at"), Some(7));
    assert_eq!(row.i64("updated_at"), Some(7));
    assert_eq!(row.text("agent_role").as_deref(), Some("main"));

    // Both retention tables exist and are empty.
    for table in ["episodic_raw_context", "episodic_segments"] {
        assert!(
            !table_columns(&*db, table).await.is_empty(),
            "{table} exists"
        );
        let count = query_one(
            &*db,
            &format!("SELECT COUNT(*) AS count FROM {table}"),
            Vec::new(),
        )
        .await
        .expect("count")
        .expect("count row");
        assert_eq!(count.i64("count"), Some(0), "{table} starts empty");
    }

    // `raw_context` is NOT a column on agent_sessions (M1).
    let columns = table_columns(&*db, "agent_sessions").await;
    assert!(
        !columns.contains(&"raw_context".to_string()),
        "no raw_context column: {columns:?}"
    );
    assert_eq!(columns.len(), 14, "13 v5 columns + agent_role: {columns:?}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_creates_the_retention_tables_and_index() {
    let config = store_config("v6-tables");
    let db = v5_database(&config).await;
    apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("v6 apply");

    let raw_columns = table_columns(&*db, "episodic_raw_context").await;
    assert_eq!(
        raw_columns,
        [
            "project_id",
            "session_id",
            "agent_id",
            "raw_context",
            "byte_len",
            "captured_at"
        ]
        .map(str::to_string)
        .to_vec()
    );
    let segment_columns = table_columns(&*db, "episodic_segments").await;
    assert_eq!(
        segment_columns,
        [
            "project_id",
            "session_id",
            "agent_id",
            "segment_index",
            "raw_content",
            "byte_offset",
            "byte_len",
            "created_at"
        ]
        .map(str::to_string)
        .to_vec()
    );

    let index_rows = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_episodic_segments_session'",
            Vec::new(),
        )
        .await
        .expect("sqlite_master");
    assert_eq!(index_rows.len(), 1, "the segment index exists once");

    // NO foreign key on either retention table: the absence IS the invariant.
    let fks = db
        .query("PRAGMA foreign_key_list(episodic_segments)", Vec::new())
        .await
        .expect("foreign_key_list(segments)");
    assert!(fks.is_empty(), "no FK on episodic_segments: {fks:?}");
    let fks = db
        .query("PRAGMA foreign_key_list(episodic_raw_context)", Vec::new())
        .await
        .expect("foreign_key_list(raw)");
    assert!(fks.is_empty(), "no FK on episodic_raw_context: {fks:?}");

    // No `CHECK` on agent_role (Q6): adding a role must not need a rebuild.
    let ddl = query_one(
        &*db,
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'episodic_raw_context'",
        Vec::new(),
    )
    .await
    .expect("ddl")
    .expect("row");
    let ddl = ddl.text("sql").unwrap_or_default();
    assert!(!ddl.contains("FOREIGN KEY"), "{ddl}");
    assert!(!ddl.contains("REFERENCES"), "{ddl}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_is_idempotent_on_reopen() {
    let config = store_config("v6-idempotent");
    let db = v5_database(&config).await;
    let first = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("first apply");
    assert_eq!(first.version, 6);
    let second = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("second apply");
    assert_eq!(second.version, 6);
    assert_eq!(second.pragma, PragmaOutcome::NotAttempted);
    assert_eq!(user_version(&*db).await, 6);
    let columns = table_columns(&*db, "agent_sessions").await;
    assert_eq!(
        columns
            .iter()
            .filter(|name| name.as_str() == "agent_role")
            .count(),
        1,
        "no duplicate agent_role: {columns:?}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_database_opens_with_a_v5_only_migration_set() {
    let config = store_config("v6-v5-only");
    let db = v5_database(&config).await;
    apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("v6 apply");

    // A v5-only binary (the rollback path) sees zero pending migrations and
    // returns the CURRENT version, 6.
    let version = apply_schema_migrations(
        &*db,
        &episodic_migrations(DIMS),
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("v5-only set over a v6 database");
    assert_eq!(version, 6, "the runner returns the current version");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fresh_root_reaches_v6_in_one_open() {
    let config = store_config("v6-fresh");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    let outcome = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("fresh apply");
    assert_eq!(outcome.version, 6);
    assert_eq!(user_version(&*db).await, 6);

    // Fresh root: NO backup (nothing existed to back up).
    let entries: Vec<String> = std::fs::read_dir(&config.storage_path)
        .expect("read storage root")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !entries.iter().any(|name| name.ends_with(".bak")),
        "no backup on a fresh root: {entries:?}"
    );
}

/// Keeps the imports honest: the outcome type is what the apply returns and
/// a `BatchMode::Write` double is only used by the convergence suite.
#[allow(dead_code)]
fn _type_witness(_: EpisodicSchemaOutcome, _: Option<Statement>, _: Option<BatchMode>) {}

#[allow(dead_code)]
async fn _read_all(_: &dyn Db) -> Result<Vec<ExecResult>, StoreError> {
    Ok(Vec::new())
}
