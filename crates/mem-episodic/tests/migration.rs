//! W4.1 lane A — episodic migration/schema integration tests (net-new, R10:
//! no upstream oracle; the acceptance tests define the schema).
//!
//! All seven cases run against a real turso database through
//! `EpisodicStore::ready()` or a raw factory handle. Inserts here are raw SQL
//! by design: this lane proves the *schema* (DDL, FK, UNIQUE, embedding
//! column) before `insert_session` lands in a later tracer step.

mod common;

use common::TestDir;
use mem_episodic::store::{EPISODIC_DB_NAME, EPISODIC_MIGRATION_LABEL, EpisodicStore};
use mem_store::db::{Db, Value, query_one};
use mem_store::episodic_sql::{EPISODIC_INDEXES, EPISODIC_V2_INDEXES};
use mem_store::schema_migrations::{
    EPISODIC_MAX_KNOWN, MigrationOptions, SchemaMigration, apply_schema_migrations,
    episodic_migrations,
};
use mem_store::vector::vector_to_text;
use mem_store::vector_utils::parse_extracted_vector;

const DIMS: u32 = 8;

/// The column order, declaration order preserved (v1's ten + the two v2
/// `ALTER TABLE ADD COLUMN` appends, then v4's additive `capture_id`).
const V2_COLUMNS: [&str; 14] = [
    "id",
    "project_id",
    "session_id",
    "agent_id",
    "parent_session_id",
    "summary",
    "metadata",
    "embedding",
    "created_at",
    "updated_at",
    "deleted_at",
    "orphaned",
    "capture_id",
    "agent_role",
];

fn migration_options(store: &EpisodicStore) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(store.db_path().to_path_buf()),
        label: EPISODIC_MIGRATION_LABEL,
        max_known: EPISODIC_MAX_KNOWN,
    }
}

/// `PRAGMA table_info` column names in declaration order.
async fn table_columns(db: &dyn Db) -> Vec<String> {
    db.query("PRAGMA table_info(agent_sessions)", Vec::new())
        .await
        .expect("table_info")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect()
}

/// Applies only the version-1 migration, simulating a pre-W4.2 on-disk db.
async fn apply_v1_only(db: &dyn Db, db_path: &std::path::Path) {
    let v1: Vec<SchemaMigration> = episodic_migrations(DIMS)
        .into_iter()
        .filter(|migration| migration.version == 1)
        .collect();
    apply_schema_migrations(
        db,
        &v1,
        &MigrationOptions {
            db_path: Some(db_path.to_path_buf()),
            label: EPISODIC_MIGRATION_LABEL,
            max_known: EPISODIC_MAX_KNOWN,
        },
        // v1 over a version-0 file takes no backup; no proof is needed.
        None,
    )
    .await
    .expect("apply v1 only");
}

/// Raw schema-level insert (Step 2 has no `insert_session` yet).
#[allow(clippy::too_many_arguments)]
async fn raw_insert(
    db: &dyn Db,
    id: &str,
    project_id: &str,
    session_id: &str,
    agent_id: &str,
    parent_session_id: Option<&str>,
    summary: &str,
    metadata: &str,
    embedding: &[f32],
    created_at: i64,
    updated_at: i64,
) -> Result<mem_store::db::ExecResult, mem_store::error::StoreError> {
    db.execute(
        "INSERT INTO agent_sessions (id, project_id, session_id, agent_id, parent_session_id, summary, metadata, embedding, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, vector32(?), ?, ?)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(project_id.to_string()),
            Value::Text(session_id.to_string()),
            Value::Text(agent_id.to_string()),
            parent_session_id.map(str::to_string).into(),
            Value::Text(summary.to_string()),
            Value::Text(metadata.to_string()),
            Value::Text(vector_to_text(embedding).expect("vector serializes")),
            Value::Integer(created_at),
            Value::Integer(updated_at),
        ],
    )
    .await
}

fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMS as usize];
    vector[index] = 1.0;
    vector
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fresh_db_creates_agent_sessions_v2() {
    let dir = TestDir::new("migration-fresh-v2");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    assert_eq!(store.dimensions(), DIMS);
    assert_eq!(store.db_path(), dir.path().join(EPISODIC_DB_NAME));

    let db = store.ready().await.expect("ready");

    // user_version == 6 after the v1..v5 set plus the convergent v6 retention schema.
    let row = query_one(db.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma user_version")
        .expect("user_version row");
    assert_eq!(row.i64("user_version"), Some(6));

    // All fourteen columns, in declaration order (v6 appends agent_role).
    let columns = table_columns(db.as_ref()).await;
    assert_eq!(
        columns,
        V2_COLUMNS
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    );

    // The v1 six plus the v2 one index.
    let indexes: Vec<String> = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'agent_sessions' AND name LIKE 'idx_%' ORDER BY name",
            Vec::new(),
        )
        .await
        .expect("sqlite_master indexes")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect();
    for statement in EPISODIC_INDEXES.into_iter().chain(EPISODIC_V2_INDEXES) {
        let name = statement
            .split_whitespace()
            .nth(5)
            .expect("index statement carries a name")
            .to_string();
        assert!(
            indexes.contains(&name),
            "missing index {name}; present: {indexes:?}"
        );
    }
    assert_eq!(
        indexes.len(),
        EPISODIC_INDEXES.len() + EPISODIC_V2_INDEXES.len(),
        "indexes: {indexes:?}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fresh_db_is_idempotent() {
    let dir = TestDir::new("migration-idempotent");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());

    let first = store.ready().await.expect("first ready");
    let second = store.ready().await.expect("second ready");
    assert!(std::sync::Arc::ptr_eq(&first, &second));

    // Re-applying returns the current version with no error.
    let version = apply_schema_migrations(
        second.as_ref(),
        &episodic_migrations(DIMS),
        &migration_options(&store),
        // v6 is already applied: pending is empty, so no backup arises.
        None,
    )
    .await
    .expect("re-apply");
    assert_eq!(version, 6);

    let tables: Vec<String> = second
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'agent_sessions'",
            Vec::new(),
        )
        .await
        .expect("table lookup")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect();
    assert_eq!(tables, vec!["agent_sessions".to_string()]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_upgrade_does_not_touch_existing_migrations() {
    let dir = TestDir::new("migration-upgrade");

    // Pre-existing on-disk state before the episodic migration runs.
    let raw = common::open_raw(dir.path(), EPISODIC_DB_NAME).await;
    raw.execute(
        "CREATE TABLE IF NOT EXISTS pre_existing (id INTEGER PRIMARY KEY, note TEXT NOT NULL)",
        Vec::new(),
    )
    .await
    .expect("create sentinel");
    raw.execute(
        "INSERT INTO pre_existing (id, note) VALUES (1, 'keep me')",
        Vec::new(),
    )
    .await
    .expect("seed sentinel");
    drop(raw);

    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    let db = store.ready().await.expect("ready");

    let sentinel = query_one(
        db.as_ref(),
        "SELECT note FROM pre_existing WHERE id = 1",
        Vec::new(),
    )
    .await
    .expect("select sentinel")
    .expect("sentinel row");
    assert_eq!(sentinel.text("note").as_deref(), Some("keep me"));

    let version = query_one(db.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("row");
    assert_eq!(version.i64("user_version"), Some(6));

    // No other migration set is applied to the dedicated episodic database.
    let foreign_tables: Vec<String> = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('user_profiles', 'user_prompts', 'memories', 'shards')",
            Vec::new(),
        )
        .await
        .expect("foreign table lookup")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect();
    assert!(
        foreign_tables.is_empty(),
        "leaked tables: {foreign_tables:?}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_foreign_key_list_agent_sessions() {
    let dir = TestDir::new("migration-fk");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    let db = store.ready().await.expect("ready");

    let rows = db
        .query("PRAGMA foreign_key_list(agent_sessions)", Vec::new())
        .await
        .expect("foreign_key_list");
    assert_eq!(rows.len(), 1, "one FK expected: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.text("table").as_deref(), Some("agent_sessions"));
    assert_eq!(row.text("from").as_deref(), Some("parent_session_id"));
    assert_eq!(row.text("to").as_deref(), Some("id"));
    assert_eq!(row.text("on_delete").as_deref(), Some("SET NULL"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_unique_constraint_scope_triple() {
    let dir = TestDir::new("migration-unique");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    let db = store.ready().await.expect("ready");

    raw_insert(
        db.as_ref(),
        "row-1",
        "project-a",
        "session-1",
        "agent-1",
        None,
        "first",
        "{}",
        &unit_vector(0),
        1,
        1,
    )
    .await
    .expect("first insert");

    let duplicate = raw_insert(
        db.as_ref(),
        "row-2",
        "project-a",
        "session-1",
        "agent-1",
        None,
        "second",
        "{}",
        &unit_vector(1),
        2,
        2,
    )
    .await;
    assert!(duplicate.is_err(), "duplicate triple must be rejected");
    assert_eq!(store.count_sessions(None).await.expect("count"), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_round_trips_scope_binding() {
    let dir = TestDir::new("migration-scope-roundtrip");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    let db = store.ready().await.expect("ready");

    let metadata = r#"{"timestamp":7,"agentName":"builder"}"#;
    // FK is live (`PRAGMA foreign_keys = ON`), so the referenced parent row
    // must exist before the child can bind to it.
    raw_insert(
        db.as_ref(),
        "row-parent",
        "project-a",
        "session-parent",
        "agent-9",
        None,
        "parent summary",
        metadata,
        &unit_vector(2),
        9,
        9,
    )
    .await
    .expect("insert parent row");
    raw_insert(
        db.as_ref(),
        "row-bind",
        "project-a",
        "session-9",
        "agent-9",
        Some("row-parent"),
        "a summary",
        metadata,
        &unit_vector(3),
        11,
        12,
    )
    .await
    .expect("insert scope binding");

    let row = query_one(
        db.as_ref(),
        "SELECT id, project_id, session_id, agent_id, parent_session_id, summary, metadata, created_at, updated_at FROM agent_sessions WHERE id = ?",
        vec![Value::Text("row-bind".to_string())],
    )
    .await
    .expect("select row")
    .expect("row present");

    assert_eq!(row.text("project_id").as_deref(), Some("project-a"));
    assert_eq!(row.text("session_id").as_deref(), Some("session-9"));
    assert_eq!(row.text("agent_id").as_deref(), Some("agent-9"));
    assert_eq!(row.text("parent_session_id").as_deref(), Some("row-parent"));
    assert_eq!(row.text("summary").as_deref(), Some("a summary"));
    assert_eq!(row.text("metadata").as_deref(), Some(metadata));
    assert_eq!(row.i64("created_at"), Some(11));
    assert_eq!(row.i64("updated_at"), Some(12));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_embedding_column_accepts_vector32() {
    let dir = TestDir::new("migration-vector32");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    let db = store.ready().await.expect("ready");

    let embedding: Vec<f32> = (0..DIMS).map(|index| index as f32 * 0.5).collect();
    raw_insert(
        db.as_ref(),
        "row-vector",
        "project-a",
        "session-1",
        "agent-1",
        None,
        "vector row",
        "{}",
        &embedding,
        1,
        1,
    )
    .await
    .expect("insert vector row");

    let row = query_one(
        db.as_ref(),
        "SELECT vector_extract(embedding) AS embedding FROM agent_sessions WHERE id = ?",
        vec![Value::Text("row-vector".to_string())],
    )
    .await
    .expect("extract embedding")
    .expect("row present");
    let raw = row
        .get("embedding")
        .cloned()
        .expect("embedding column present");
    let parsed = parse_extracted_vector(&raw).expect("vector_extract parses");
    assert_eq!(parsed, embedding);
}

// ---------------------------------------------------------------------------
// W4.2 Step 1 — migration v2 (soft delete + orphan tracking)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_upgrade_v1_to_v2_adds_soft_delete_columns() {
    let dir = TestDir::new("migration-v1-to-v2");
    let raw = common::open_raw(dir.path(), EPISODIC_DB_NAME).await;
    apply_v1_only(raw.as_ref(), &dir.path().join(EPISODIC_DB_NAME)).await;

    // A pre-W4.2 db really is at v1 with ten columns.
    let v1_version = query_one(raw.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("row");
    assert_eq!(v1_version.i64("user_version"), Some(1));
    let v1_columns = table_columns(raw.as_ref()).await;
    assert_eq!(v1_columns.len(), 10, "v1 columns: {v1_columns:?}");
    assert!(!v1_columns.contains(&"deleted_at".to_string()));
    assert!(!v1_columns.contains(&"orphaned".to_string()));
    drop(raw);

    // The store upgrades it in place: v2 columns, the v3 milestone table, v4 capture_id, the v6 retention schema.
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    // A v1→v6 upgrade takes a destructive backup: the store needs a live proof
    // for its exact storage path (issue 46.4).
    let _proof = common::exclusive_root(dir.path());
    let db = store.ready().await.expect("ready");

    let version = query_one(db.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("row");
    assert_eq!(version.i64("user_version"), Some(6));

    let columns = table_columns(db.as_ref()).await;
    assert_eq!(
        columns,
        V2_COLUMNS
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    );

    // `deleted_at` is nullable; `orphaned` is NOT NULL DEFAULT 0.
    let table_info = db
        .query("PRAGMA table_info(agent_sessions)", Vec::new())
        .await
        .expect("table_info");
    let deleted_at = table_info
        .iter()
        .find(|row| row.text("name").as_deref() == Some("deleted_at"))
        .expect("deleted_at column");
    assert_eq!(deleted_at.i64("notnull"), Some(0), "{deleted_at:?}");
    let orphaned = table_info
        .iter()
        .find(|row| row.text("name").as_deref() == Some("orphaned"))
        .expect("orphaned column");
    assert_eq!(orphaned.i64("notnull"), Some(1), "{orphaned:?}");

    // The v2 index landed as well.
    let v2_index_rows = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_agent_sessions_project_deleted_created'",
            Vec::new(),
        )
        .await
        .expect("sqlite_master");
    assert_eq!(v2_index_rows.len(), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v2_defaults_existing_rows_to_null_deleted_and_not_orphaned() {
    let dir = TestDir::new("migration-v2-defaults");
    let raw = common::open_raw(dir.path(), EPISODIC_DB_NAME).await;
    apply_v1_only(raw.as_ref(), &dir.path().join(EPISODIC_DB_NAME)).await;
    raw_insert(
        raw.as_ref(),
        "row-legacy",
        "project-a",
        "session-1",
        "agent-1",
        None,
        "legacy row",
        r#"{"timestamp":1,"agentName":"agent-1"}"#,
        &unit_vector(0),
        5,
        5,
    )
    .await
    .expect("insert legacy row");
    drop(raw);

    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    // A v1→v6 upgrade takes a destructive backup: the store needs a live proof
    // for its exact storage path (issue 46.4).
    let _proof = common::exclusive_root(dir.path());
    let db = store.ready().await.expect("ready");

    let row = query_one(
        db.as_ref(),
        "SELECT deleted_at, orphaned FROM agent_sessions WHERE id = ?",
        vec![Value::Text("row-legacy".to_string())],
    )
    .await
    .expect("select")
    .expect("row");
    assert!(row.is_null("deleted_at"), "existing rows default to NULL");
    assert_eq!(row.i64("orphaned"), Some(0));

    // The record surface agrees with the columns.
    let session = store
        .get_session("row-legacy")
        .await
        .expect("get")
        .expect("row still visible");
    assert!(session.deleted_at.is_none());
    assert!(!session.orphaned);
}

// ---------------------------------------------------------------------------
// W4.3 — migration v3 (consolidated milestone records)
// ---------------------------------------------------------------------------

/// The v3 milestone column set, declaration order preserved.
const V3_MILESTONE_COLUMNS: [&str; 10] = [
    "id",
    "project_id",
    "agent_id",
    "summary",
    "contributors",
    "source_count",
    "window_start",
    "window_end",
    "embedding",
    "created_at",
];

/// `PRAGMA table_info` column names for the milestone table.
async fn milestone_columns(db: &dyn Db) -> Vec<String> {
    db.query("PRAGMA table_info(episodic_milestones)", Vec::new())
        .await
        .expect("table_info")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect()
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v3_migration_creates_episodic_milestones_table_and_index() {
    let dir = TestDir::new("migration-v3-milestones");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    // A v1→v6 upgrade takes a destructive backup: the store needs a live proof
    // for its exact storage path (issue 46.4).
    let _proof = common::exclusive_root(dir.path());
    let db = store.ready().await.expect("ready");

    let version = query_one(db.as_ref(), "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma user_version")
        .expect("user_version row");
    assert_eq!(version.i64("user_version"), Some(6));

    let columns = milestone_columns(db.as_ref()).await;
    assert_eq!(
        columns,
        V3_MILESTONE_COLUMNS
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    );

    // The v3 index landed exactly once.
    let index_rows = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_episodic_milestones_project_agent_created'",
            Vec::new(),
        )
        .await
        .expect("sqlite_master");
    assert_eq!(index_rows.len(), 1);

    // The embedding column is dimension-pinned like the session table.
    let table_info = db
        .query("PRAGMA table_info(episodic_milestones)", Vec::new())
        .await
        .expect("table_info");
    let embedding = table_info
        .iter()
        .find(|row| row.text("name").as_deref() == Some("embedding"))
        .expect("embedding column");
    assert_eq!(embedding.i64("notnull"), Some(1), "{embedding:?}");
    assert_eq!(embedding.text("type").as_deref(), Some("F32_BLOB"));
}

#[test]
fn test_v3_migration_leaves_v1_v2_statements_unchanged() {
    let migrations = episodic_migrations(DIMS);
    let v1 = migrations
        .iter()
        .find(|migration| migration.version == 1)
        .expect("v1 migration");
    let v2 = migrations
        .iter()
        .find(|migration| migration.version == 2)
        .expect("v2 migration");
    let v3 = migrations
        .iter()
        .find(|migration| migration.version == 3)
        .expect("v3 migration");

    // v1: the baseline DDL plus its six indexes, byte-unchanged.
    assert_eq!(v1.statements.len(), EPISODIC_INDEXES.len() + 1);
    assert_eq!(
        v1.statements[0].sql,
        mem_store::episodic_sql::agent_sessions_ddl(DIMS)
    );
    for (offset, index) in EPISODIC_INDEXES.iter().enumerate() {
        assert_eq!(v1.statements[1 + offset].sql, *index);
    }

    // v2: the two alters plus the one index, byte-unchanged.
    assert_eq!(v2.statements.len(), 3);
    assert_eq!(
        v2.statements[0].sql,
        mem_store::episodic_sql::EPISODIC_V2_ALTERS[0]
    );
    assert_eq!(
        v2.statements[1].sql,
        mem_store::episodic_sql::EPISODIC_V2_ALTERS[1]
    );
    assert_eq!(v2.statements[2].sql, EPISODIC_V2_INDEXES[0]);

    // v3: the milestone DDL plus its index.
    assert_eq!(v3.statements.len(), 2);
    assert_eq!(
        v3.statements[0].sql,
        mem_store::episodic_sql::episodic_milestones_ddl(DIMS)
    );
    assert_eq!(
        v3.statements[1].sql,
        mem_store::episodic_sql::EPISODIC_V3_INDEXES[0]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v2_migration_is_idempotent() {
    let dir = TestDir::new("migration-v2-idempotent");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    let db = store.ready().await.expect("first ready");

    let version = apply_schema_migrations(
        db.as_ref(),
        &episodic_migrations(DIMS),
        &migration_options(&store),
        // v6 is already applied: pending is empty, so no backup arises.
        None,
    )
    .await
    .expect("re-apply");
    assert_eq!(version, 6);

    // Re-applying never duplicates columns or the v2 index.
    let columns = table_columns(db.as_ref()).await;
    assert_eq!(
        columns,
        V2_COLUMNS
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    );
    let v2_index_rows = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_agent_sessions_project_deleted_created'",
            Vec::new(),
        )
        .await
        .expect("sqlite_master");
    assert_eq!(v2_index_rows.len(), 1);
}
