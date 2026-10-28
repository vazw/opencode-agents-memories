//! Schema-migration runner battery (blueprint W1.2 step 6).
//!
//! Port of upstream `src/services/turso/schema-migrations.ts` apply order,
//! backup/restore semantics, and `tests/schema-migrations.test.ts` cases.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use mem_store::config::StoreConfig;
use mem_store::db::{BatchMode, BoxFuture, Db, DbHandle, ExecResult, Row, Statement, Value};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    MigrationOptions, SchemaMigration, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::sql;

// ---------------------------------------------------------------------------
// A `Db` test double that tracks user_version + statement order
// ---------------------------------------------------------------------------

#[derive(Default)]
struct VersionedCore {
    version: AtomicI64,
    sqls: Mutex<Vec<String>>,
}

struct VersionedDb {
    core: Arc<VersionedCore>,
}

impl VersionedDb {
    fn new() -> Self {
        Self {
            core: Arc::default(),
        }
    }
}

impl Db for VersionedDb {
    fn execute(
        &self,
        sql: &str,
        _params: Vec<Value>,
    ) -> BoxFuture<'_, Result<ExecResult, StoreError>> {
        let core = self.core.clone();
        let sql = sql.to_string();
        Box::pin(async move {
            let lower = sql.to_ascii_lowercase();
            if lower.starts_with("pragma user_version =") {
                let version: i64 = sql.rsplit('=').next().unwrap().trim().parse().unwrap_or(0);
                core.version.store(version, Ordering::SeqCst);
            }
            core.sqls.lock().unwrap().push(sql);
            Ok(ExecResult {
                changes: 0,
                last_insert_rowid: 0,
            })
        })
    }

    fn query(
        &self,
        sqlq: &str,
        _params: Vec<Value>,
    ) -> BoxFuture<'_, Result<Vec<Row>, StoreError>> {
        let core = self.core.clone();
        let sql = sqlq.to_string();
        Box::pin(async move {
            if sql.to_ascii_lowercase().starts_with("pragma user_version") {
                let version = core.version.load(Ordering::SeqCst);
                return Ok(vec![Row::new(
                    Arc::new(vec!["user_version".to_string()]),
                    vec![Value::Integer(version)],
                )]);
            }
            Ok(Vec::new())
        })
    }

    fn batch(
        &self,
        statements: Vec<Statement>,
        _mode: BatchMode,
    ) -> BoxFuture<'_, Result<Vec<ExecResult>, StoreError>> {
        let core = self.core.clone();
        Box::pin(async move {
            for statement in statements {
                core.sqls.lock().unwrap().push(statement.sql);
            }
            Ok(Vec::new())
        })
    }
}

fn options_for(config: &StoreConfig, file: &str) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join(file)),
        label: "test",
    }
}

/// The memoised root proof (a destructive upgrade needs one).
fn root_proof(config: &StoreConfig) -> Arc<mem_store::delete_guard::ExclusiveRoot> {
    common::exclusive_root(&config.storage_path)
}

fn migration(version: i64, sql: &str) -> SchemaMigration {
    SchemaMigration {
        version,
        description: "test",
        statements: vec![Statement::new(sql, Vec::new())],
    }
}

// ---------------------------------------------------------------------------
// 1/6
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_applies_pending_migrations_in_ascending_version_order_and_records_user_version() {
    let _config = common::store_config("mig-order");
    let db = VersionedDb::new();
    // Input deliberately out of order.
    let migrations = vec![
        migration(2, "CREATE TABLE t_two (id INTEGER)"),
        migration(1, "CREATE TABLE t_one (id INTEGER)"),
    ];
    let target = apply_schema_migrations(
        &db,
        &migrations,
        &MigrationOptions {
            db_path: None,
            label: "test",
        },
        None,
    )
    .await
    .expect("applies");
    assert_eq!(target, 2);
    assert_eq!(
        db.core.version.load(Ordering::SeqCst),
        2,
        "PRAGMA user_version records the last applied version"
    );
    let sqls = db.core.sqls.lock().unwrap().clone();
    let one = sqls
        .iter()
        .position(|s| s.contains("t_one"))
        .expect("v1 applied");
    let two = sqls
        .iter()
        .position(|s| s.contains("t_two"))
        .expect("v2 applied");
    assert!(one < two, "ascending apply order: {sqls:?}");
    let set = sqls
        .iter()
        .position(|s| s.contains("PRAGMA user_version = 1"))
        .expect("version bump for v1");
    assert!(set < two, "user_version set right after each batch apply");
}

// ---------------------------------------------------------------------------
// 2/6
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_returns_current_version_when_nothing_is_pending() {
    let _config = common::store_config("mig-noop");
    let db = VersionedDb::new();
    db.core.version.store(7, Ordering::SeqCst);
    let migrations = vec![migration(5, "CREATE TABLE never (id INTEGER)")];
    let version = apply_schema_migrations(
        &db,
        &migrations,
        &MigrationOptions {
            db_path: None,
            label: "test",
        },
        None,
    )
    .await
    .expect("noop");
    assert_eq!(version, 7, "current version wins when nothing pending");
    let sqls = db.core.sqls.lock().unwrap().clone();
    assert!(
        sqls.iter()
            .all(|s| s.to_ascii_lowercase().starts_with("pragma user_version")),
        "no statements ran beyond the version probe: {sqls:?}"
    );
}

// ---------------------------------------------------------------------------
// 3/6 backup before destructive upgrade
// ---------------------------------------------------------------------------

/// Seeds the v1 schema + one row through an already-open handle.
async fn seed_v1(db: &dyn Db) {
    db.batch(
        vec![
            Statement::new(sql::shard_metadata_ddl(), Vec::new()),
            Statement::new(sql::memories_ddl(2), Vec::new()),
        ],
        BatchMode::Write,
    )
    .await
    .expect("v1 schema");
    db.execute(&sql::set_user_version_sql(1), Vec::new())
        .await
        .expect("seed version");
    // v1-era table (no session_id column yet): golden lib INSERTs are
    // v2-shaped and only valid after the chain — same 16-column fixture
    // as the golden battery.
    const PRE_V2_INSERT: &str = r#"
      INSERT INTO memories (
        id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
        metadata, display_name, user_name, user_email, project_path, project_name, git_repo_url
      ) VALUES (?, ?, vector32(?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    "#;
    db.execute(
        PRE_V2_INSERT,
        vec![
            Value::Text("row1".to_string()),
            Value::Text("keep".to_string()),
            Value::Text("[1.0,0.0]".to_string()),
            Value::Null,
            Value::Text("opencode_project_a1b2c3d4e5f60001".to_string()),
            Value::Null,
            Value::Null,
            Value::Integer(1),
            Value::Integer(1),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ],
    )
    .await
    .expect("seed row");
}

async fn seed_real_shard_v1(config: &StoreConfig, file: &str) -> DbHandle {
    let db = common::open_real_db(config, file).await.expect("open");
    seed_v1(db.as_ref()).await;
    db
}

#[tokio::test]
async fn test_creates_backup_before_destructive_upgrade_when_version_positive() {
    let config = common::store_config("mig-backup");
    let db = seed_real_shard_v1(&config, "shard.db").await;
    let options = &options_for(&config, "shard.db");
    let version = apply_schema_migrations(
        db.as_ref(),
        &memory_shard_migrations(2),
        options,
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("applies v2");
    assert_eq!(version, 2);

    let backups: Vec<PathBuf> = std::fs::read_dir(&config.storage_path)
        .expect("storage dir")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.to_string_lossy().ends_with(".bak")
                && path.to_string_lossy().contains(".schema-v1-")
        })
        .collect();
    assert_eq!(backups.len(), 1, "one backup file: {:?}", backups);
    assert!(
        config.storage_path.join("shard.db").exists(),
        "original kept"
    );
}

// ---------------------------------------------------------------------------
// 4/6 restore on failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_failure_restores_backup_and_reraises() {
    let config = common::store_config("mig-restore");
    let (_config, connections, _shards) = common::real_components(config.clone()).await;
    let db_path = config.storage_path.join("shard.db");
    let db = connections
        .get_connection(&db_path)
        .await
        .expect("open through the manager");
    seed_v1(db.as_ref()).await;
    let failing = vec![
        migration(1, "CREATE TABLE hero (id INTEGER)"),
        migration(2, "SELECTX certainly not sql"),
    ];
    let options = &options_for(&config, "shard.db");
    let error = apply_schema_migrations(
        db.as_ref(),
        &failing,
        options,
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect_err("migration failure propagates");

    // 46.4: the runner never unlinks the file it has open. It hands the
    // restore to the caller, which drops its handle and settles the pending.
    let pending = match error {
        StoreError::MigrationRestorePending(pending) => pending,
        other => panic!("expected MigrationRestorePending, got {other:?}"),
    };
    assert_eq!(pending.db_path(), db_path.as_path());
    assert!(
        pending.backup_path().exists(),
        "the backup must still exist while the pending is unsettled"
    );
    assert!(
        !pending.original().to_string().is_empty(),
        "the original failure travels with the pending value"
    );
    pending
        .restore(&connections, db)
        .await
        .expect("the caller settles the restore");

    let reopened = connections
        .get_connection(&db_path)
        .await
        .expect("reopen through the same manager");
    let rows = reopened
        .query(sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version probe");
    assert_eq!(rows[0].i64("user_version"), Some(1), "restored to v1");
    let kept = reopened
        .query("SELECT id FROM memories", Vec::new())
        .await
        .expect("rows probe");
    assert_eq!(kept.len(), 1, "seeded row restored");
}

// ---------------------------------------------------------------------------
// 5/6 version-zero failures never touch the original
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_failure_at_version_zero_leaves_original_untouched_and_reraises() {
    let config = common::store_config("mig-vzero");
    let db = common::open_real_db(&config, "fresh.db")
        .await
        .expect("open");
    let failing = vec![migration(1, "SELECTX certainly not sql")];
    let options = &options_for(&config, "fresh.db");
    let error = apply_schema_migrations(
        db.as_ref(),
        &failing,
        options,
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect_err("failure re-raised");
    assert!(
        !error.to_string().is_empty(),
        "original error carries through: {error}"
    );
    // no backup was created (current == 0)
    let backups: Vec<PathBuf> = std::fs::read_dir(&config.storage_path)
        .expect("storage dir")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.to_string_lossy().ends_with(".bak"))
        .collect();
    assert!(backups.is_empty(), "no backup at version 0");
    let reopened = common::open_real_db(&config, "fresh.db")
        .await
        .expect("reopen");
    let rows = reopened
        .query(sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version probe");
    assert_eq!(rows[0].i64("user_version"), Some(0));
}

// ---------------------------------------------------------------------------
// 6/6 memory shard migrations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_memory_shard_migrations_apply_v1_and_v2() {
    let config = common::store_config("mig-mem");
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open");
    let options = &options_for(&config, "shard.db");
    let version = apply_schema_migrations(
        db.as_ref(),
        &memory_shard_migrations(4),
        options,
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect("applies");
    assert_eq!(version, 2);

    let columns = db
        .query("PRAGMA table_info(memories)", Vec::new())
        .await
        .expect("table_info");
    assert_eq!(columns.len(), 18, "v1 17 columns + session_id");
    assert!(
        columns
            .iter()
            .any(|row| row.text("name").as_deref() == Some("session_id"))
    );
    let indexes = db
        .query(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='memories' ORDER BY name",
            Vec::new(),
        )
        .await
        .expect("indexes");
    let names: Vec<String> = indexes
        .iter()
        .map(|row| row.text("name").unwrap())
        .collect();
    for expected in [
        "idx_container_tag",
        "idx_created_at",
        "idx_is_pinned",
        "idx_session_id",
        "idx_type",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "index {expected} present"
        );
    }
}
