//! W7.1 — convergent v6 apply: the partial-application states, the measured
//! pragma path, the forced fallback branch and the reopen path (M3, M14,
//! M28).
//!
//! The bricking state this suite exists for: a kill between the v6 DDL
//! commit and `PRAGMA user_version = 6` leaves a v5 file that already has
//! the v6 objects. `ALTER TABLE … ADD COLUMN` is not re-runnable, so the
//! retry must converge via a pre-flight `PRAGMA table_info` guard instead of
//! failing on a duplicate column.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{open_real_db, store_config};
use mem_store::db::{
    BatchMode, BoxFuture, Db, DbHandle, ExecResult, Row, Statement, Value, query_one,
};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    EpisodicSchemaOutcome, MigrationOptions, PragmaOutcome, apply_episodic_schema,
    apply_schema_migrations, episodic_migrations, episodic_v6_migration,
};

const DIMS: u32 = 2;

/// The pragma path MEASURED on this workspace's turso (`turso 0.8.3`,
/// turso_core 0.8.3) with
/// `CARGO_TARGET_DIR=$PWD/tmp/cargo-target TMPDIR=$PWD/.tmp/test-tmp
/// cargo test -p mem-store --test episodic_v6_convergence`.
///
/// M14: the first-apply test pins what the engine actually did, never
/// "either branch". The forced-refusal test below proves the OTHER branch.
const V6_PRAGMA_PATH_MEASURED: PragmaOutcome = PragmaOutcome::InBatch;

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

/// The memoised root proof (a destructive upgrade needs one).
fn root_proof(
    config: &mem_store::config::StoreConfig,
) -> std::sync::Arc<mem_store::delete_guard::ExclusiveRoot> {
    common::exclusive_root(&config.storage_path)
}

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
    assert_eq!(version, 5);
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

/// Applies the v6 DDL but deliberately does NOT stamp the version — the
/// kill-between-batch-and-pragma state. `tables` selects which retention
/// tables the partial state already has.
async fn half_applied_v6(db: &dyn Db, raw_context: bool, segments: bool) {
    let mut statements: Vec<Statement> = vec![Statement::new(
        mem_store::episodic_sql::EPISODIC_V6_ALTERS[0],
        Vec::new(),
    )];
    if raw_context {
        statements.push(Statement::new(
            mem_store::episodic_sql::RAW_CONTEXT_DDL,
            Vec::new(),
        ));
    }
    if segments {
        statements.push(Statement::new(
            mem_store::episodic_sql::SEGMENTS_DDL,
            Vec::new(),
        ));
        statements.push(Statement::new(
            mem_store::episodic_sql::SEGMENTS_INDEX,
            Vec::new(),
        ));
    }
    db.batch(statements, BatchMode::Write)
        .await
        .expect("half-apply the v6 DDL");
    assert_eq!(user_version(db).await, 5, "the version was NOT stamped");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_rerun_after_a_partial_application_converges() {
    let config = store_config("v6-partial-full");
    let db = v5_database(&config).await;
    insert_v5_row(&*db, "row-v5").await;
    half_applied_v6(&*db, true, true).await;

    // Already at v5 + the v6 objects: this is the state a retry meets.
    let outcome = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("the retry converges instead of a duplicate-column error");
    assert_eq!(outcome.version, 6);
    assert_eq!(user_version(&*db).await, 6);

    let columns = table_columns(&*db, "agent_sessions").await;
    assert_eq!(
        columns
            .iter()
            .filter(|name| name.as_str() == "agent_role")
            .count(),
        1,
        "exactly one agent_role column: {columns:?}"
    );
    for table in ["episodic_raw_context", "episodic_segments"] {
        assert!(
            !table_columns(&*db, table).await.is_empty(),
            "{table} intact"
        );
    }
    let row = query_one(
        &*db,
        "SELECT summary, agent_role FROM agent_sessions WHERE id = 'row-v5'",
        Vec::new(),
    )
    .await
    .expect("select")
    .expect("the pre-existing row survives");
    assert_eq!(row.text("summary").as_deref(), Some("the summary"));
    assert_eq!(row.text("agent_role").as_deref(), Some("main"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_partial_application_with_a_missing_table_converges() {
    let config = store_config("v6-partial-missing");
    let db = v5_database(&config).await;
    // The `agent_role` column and the raw table landed; the segment table did
    // not.
    half_applied_v6(&*db, true, false).await;

    let outcome = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("the retry converges and creates the missing table");
    assert_eq!(outcome.version, 6);
    assert_eq!(user_version(&*db).await, 6);
    assert_eq!(
        table_columns(&*db, "episodic_segments").await,
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
    let index = db
        .query(
            "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_episodic_segments_session'",
            Vec::new(),
        )
        .await
        .expect("sqlite_master");
    assert_eq!(index.len(), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_pragma_path_is_the_measured_constant() {
    let config = store_config("v6-pragma-measured");
    let db = v5_database(&config).await;
    let outcome: EpisodicSchemaOutcome = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("first apply");
    assert_eq!(outcome.version, 6);
    assert_eq!(
        outcome.pragma, V6_PRAGMA_PATH_MEASURED,
        "the first apply must take the MEASURED pragma path"
    );
    assert_eq!(user_version(&*db).await, 6);
}

/// Wraps the real handle and refuses the FIRST batch containing the version
/// pragma, forcing the fallback branch (the other branch is not vacuous).
struct PragmaRefusingDb {
    inner: DbHandle,
    refused: AtomicBool,
}

impl PragmaRefusingDb {
    fn new(inner: DbHandle) -> Self {
        Self {
            inner,
            refused: AtomicBool::new(false),
        }
    }

    fn should_refuse(&self, statements: &[Statement]) -> bool {
        let contains_pragma = statements
            .iter()
            .any(|statement| statement.sql.contains("PRAGMA user_version"));
        contains_pragma && !self.refused.swap(true, Ordering::SeqCst)
    }
}

impl Db for PragmaRefusingDb {
    fn execute(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> BoxFuture<'_, Result<ExecResult, StoreError>> {
        self.inner.execute(sql, params)
    }

    fn query(&self, sql: &str, params: Vec<Value>) -> BoxFuture<'_, Result<Vec<Row>, StoreError>> {
        self.inner.query(sql, params)
    }

    fn batch(
        &self,
        statements: Vec<Statement>,
        mode: BatchMode,
    ) -> BoxFuture<'_, Result<Vec<ExecResult>, StoreError>> {
        if self.should_refuse(&statements) {
            return Box::pin(async {
                Err(StoreError::Internal {
                    message: "forced refusal of PRAGMA user_version inside a batch".to_string(),
                })
            });
        }
        self.inner.batch(statements, mode)
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_pragma_refusal_falls_back_and_converges() {
    let config = store_config("v6-pragma-refusal");
    let inner = v5_database(&config).await;
    let db = Arc::new(PragmaRefusingDb::new(inner.clone()));

    let outcome = apply_episodic_schema(
        db.as_ref(),
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("the fallback path converges");
    assert_eq!(outcome.pragma, PragmaOutcome::Fallback);
    assert_eq!(outcome.version, 6);
    assert_eq!(user_version(db.as_ref()).await, 6);

    for table in ["episodic_raw_context", "episodic_segments"] {
        assert!(
            !table_columns(db.as_ref(), table).await.is_empty(),
            "{table} present"
        );
    }
    let row = query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM agent_sessions",
        Vec::new(),
    )
    .await
    .expect("count")
    .expect("row");
    assert_eq!(row.i64("count"), Some(0), "a readable table");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_reopen_reports_not_attempted() {
    let config = store_config("v6-reopen");
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

    let reopened = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("reopen");
    assert_eq!(
        reopened,
        EpisodicSchemaOutcome {
            version: 6,
            pragma: PragmaOutcome::NotAttempted,
        },
        "the reopen path never claims a pragma path it did not run"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v6_apply_takes_one_backup_of_an_existing_database() {
    let config = store_config("v6-backup");
    let db = v5_database(&config).await;
    apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("apply");

    let backups: Vec<String> = std::fs::read_dir(&config.storage_path)
        .expect("read root")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("episodic.db.schema-v5-") && name.ends_with(".bak"))
        .collect();
    assert_eq!(backups.len(), 1, "exactly one v5 backup: {backups:?}");

    // A fresh root takes none.
    let fresh = store_config("v6-backup-fresh");
    let fresh_db = open_real_db(&fresh, "episodic.db").await.expect("open");
    apply_episodic_schema(
        &*fresh_db,
        DIMS,
        &options(&fresh),
        Some(root_proof(&fresh).as_ref()),
    )
    .await
    .expect("fresh apply");
    let fresh_backups: Vec<String> = std::fs::read_dir(&fresh.storage_path)
        .expect("read root")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".bak"))
        .collect();
    assert!(fresh_backups.is_empty(), "fresh root: {fresh_backups:?}");
}

/// `episodic_v6_migration` is the only holder of the v6 statements: the
/// applied set is EXACTLY it (minus the non-re-runnable `ALTER` when
/// `agent_role` already exists) plus the version pragma.
#[test]
fn test_v6_migration_is_not_part_of_the_direct_set() {
    let migration = episodic_v6_migration(DIMS);
    assert_eq!(migration.version, 6);
    assert_eq!(migration.statements.len(), 4);
    let direct = episodic_migrations(DIMS);
    assert!(
        direct.iter().all(|migration| migration.version <= 5),
        "episodic_migrations() stays v1..v5"
    );
    assert_eq!(
        direct.last().map(|migration| migration.version),
        Some(5),
        "M13: direct callers keep observing v5"
    );
}
