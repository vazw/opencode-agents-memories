//! `user_prompts` schema v2 battery (issue 43 W5): the probe-then-ALTER applier
//! over real files plus a scripted double.
//!
//! Every real-file fixture supplies a real `db_path` and an exclusivity proof
//! (`common::exclusive_root(..)` bound to a `let` local, never borrowed from the
//! call's temporary) so the no-backup rule is testable: the unmutated applier
//! never calls `backup_before_destructive`, while a mutant that does would
//! succeed and leave a `.schema-v1-*.bak` the scans can see.

mod common;

use std::path::Path;

use mem_store::db::{BatchMode, BoxFuture, Db, ExecResult, Row, Statement, Value, query_one};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{MigrationOptions, USER_PROMPTS_MAX_KNOWN};

const PROMPTS_DB: &str = "user-prompts.db";

fn migration_options(db_path: &Path) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(db_path.to_path_buf()),
        label: "test",
        max_known: USER_PROMPTS_MAX_KNOWN,
    }
}

async fn read_user_version(db: &dyn Db) -> i64 {
    let row = query_one(db, mem_store::sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("read user_version")
        .expect("pragma row");
    row.i64("user_version").expect("user_version column")
}

/// Every `.schema-v*.bak` left in `dir` (the backup the v2 step must not take).
fn schema_backups(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read storage root")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains(".schema-v") && name.ends_with(".bak"))
        .collect()
}

/// `(declared type, default expression)` from `PRAGMA table_info(user_prompts)`.
async fn column_shape(db: &dyn Db, column: &str) -> (String, Option<String>) {
    let rows = db
        .query("PRAGMA table_info(user_prompts)", Vec::new())
        .await
        .expect("table_info");
    let row = rows
        .into_iter()
        .find(|row| row.text("name").as_deref() == Some(column))
        .unwrap_or_else(|| panic!("column {column} present"));
    let declared_type = row.text("type").unwrap_or_default();
    let default = row.get("dflt_value").and_then(|value| match value {
        Value::Text(text) => Some(text.clone()),
        Value::Integer(number) => Some(number.to_string()),
        Value::Real(number) => Some(number.to_string()),
        Value::Null | Value::Blob(_) => None,
    });
    (declared_type, default)
}

// ---------------------------------------------------------------------------
// 1/8 — a fresh file: v1 runs first, then the probe sees the columns it made
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fresh_file_converges_to_v2() {
    let config = common::store_config("prompts-fresh");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&db_path);

    let version = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await
    .expect("a fresh file converges");

    assert_eq!(version, 2, "the applier reports v2");
    assert_eq!(read_user_version(db.as_ref()).await, 2, "stamped 2");
    let columns = common::user_prompt_columns(db.as_ref()).await;
    assert_eq!(columns.len(), 12, "v1's DDL supplied the three v2 columns");
    for expected in [
        "id",
        "session_id",
        "message_id",
        "project_path",
        "content",
        "created_at",
        "captured",
        "user_learning_captured",
        "linked_memory_id",
        "capture_attempts",
        "provider_id",
        "model_id",
    ] {
        assert!(
            columns.iter().any(|name| name == expected),
            "column {expected} present in {columns:?}"
        );
    }

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}

// ---------------------------------------------------------------------------
// 2/8 — a pre-version file keeps its row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_v0_legacy_table_without_columns_converges_to_v2() {
    let config = common::store_config("prompts-v0");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    common::seed_user_prompts(db.as_ref(), 0, false, true)
        .await
        .expect("seed a v0 legacy file");
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&db_path);

    let version = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await
    .expect("a v0 file converges");

    assert_eq!(version, 2);
    assert_eq!(read_user_version(db.as_ref()).await, 2);
    let columns = common::user_prompt_columns(db.as_ref()).await;
    assert_eq!(columns.len(), 12);
    let row = query_one(
        db.as_ref(),
        "SELECT content, captured, capture_attempts, provider_id, model_id \
         FROM user_prompts WHERE id = 'prompt_legacy'",
        Vec::new(),
    )
    .await
    .expect("query the legacy row")
    .expect("the legacy row survives the upgrade");
    assert_eq!(row.text("content").as_deref(), Some("preserve this prompt"));
    assert_eq!(row.i64("captured"), Some(1));
    assert_eq!(row.i64("capture_attempts"), Some(0));
    assert!(row.is_null("provider_id"));
    assert!(row.is_null("model_id"));

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}

// ---------------------------------------------------------------------------
// 3/8 — a 9-column v1 file: add-if-missing, declarations, no backup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_v1_without_columns_converges_to_v2() {
    let config = common::store_config("prompts-v1-without");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    common::seed_user_prompts(db.as_ref(), 1, false, false)
        .await
        .expect("seed a v1 file without the v2 columns");
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&db_path);

    let version = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await
    .expect("a v1 file upgrades");

    assert_eq!(version, 2);
    assert_eq!(read_user_version(db.as_ref()).await, 2);
    let columns = common::user_prompt_columns(db.as_ref()).await;
    assert_eq!(columns.len(), 12);
    assert_eq!(
        column_shape(db.as_ref(), "capture_attempts").await,
        ("INTEGER".to_string(), Some("0".to_string()))
    );
    assert_eq!(
        column_shape(db.as_ref(), "provider_id").await,
        ("TEXT".to_string(), None)
    );
    assert_eq!(
        column_shape(db.as_ref(), "model_id").await,
        ("TEXT".to_string(), None)
    );

    db.execute(
        "INSERT INTO user_prompts \
         (id, session_id, message_id, content, created_at, capture_attempts, provider_id, model_id) \
         VALUES ('p_roundtrip', 's', 'm', 'roundtrip', 1, 4, 'prov', 'model')",
        Vec::new(),
    )
    .await
    .expect("insert naming all three v2 columns");
    let row = query_one(
        db.as_ref(),
        "SELECT capture_attempts, provider_id, model_id FROM user_prompts WHERE id = 'p_roundtrip'",
        Vec::new(),
    )
    .await
    .expect("read the roundtrip row")
    .expect("roundtrip row exists");
    assert_eq!(row.i64("capture_attempts"), Some(4));
    assert_eq!(row.text("provider_id").as_deref(), Some("prov"));
    assert_eq!(row.text("model_id").as_deref(), Some("model"));

    assert!(
        schema_backups(&config.storage_path).is_empty(),
        "the v2 step takes no backup: {:?}",
        schema_backups(&config.storage_path)
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}

// ---------------------------------------------------------------------------
// 4/8 — the crash-window file: columns already there, converge silently
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_v1_with_columns_converges_to_v2() {
    let config = common::store_config("prompts-v1-with");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    common::seed_user_prompts(db.as_ref(), 1, true, false)
        .await
        .expect("seed a v1 file already carrying the v2 columns");
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&db_path);

    let first = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await
    .expect("first apply converges");
    let second = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await
    .expect("second apply is a no-op");

    assert_eq!(first, 2);
    assert_eq!(second, 2);
    assert_eq!(read_user_version(db.as_ref()).await, 2);
    let columns = common::user_prompt_columns(db.as_ref()).await;
    assert_eq!(columns.len(), 12, "no duplicate ALTER: {columns:?}");

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}

// ---------------------------------------------------------------------------
// 5/8 — a v2 file is a no-op and the maximum is 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_v2_file_is_a_noop_and_stays_2() {
    let config = common::store_config("prompts-v2");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    common::seed_user_prompts(db.as_ref(), 2, true, false)
        .await
        .expect("seed a v2 file");
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&db_path);

    assert_eq!(USER_PROMPTS_MAX_KNOWN, 2, "the prompts set's maximum is v2");
    let first = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await
    .expect("a v2 file is not refused");
    let second = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await
    .expect("a second open is still a no-op");

    assert_eq!(first, 2);
    assert_eq!(second, 2);
    assert_eq!(read_user_version(db.as_ref()).await, 2);

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}

// ---------------------------------------------------------------------------
// 6/8 — reopening a v2 file costs no write statement and still succeeds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_v2_file_runs_no_write() {
    let db = PromptsDb::new(2, v2_columns());
    let options = MigrationOptions {
        db_path: None,
        label: "test",
        max_known: USER_PROMPTS_MAX_KNOWN,
    };

    let version = mem_store::schema_migrations::apply_user_prompts_schema(&db, &options, None)
        .await
        .expect("a v2 file opens without a write");

    assert_eq!(version, 2);
    assert_eq!(
        db.recorded(),
        vec!["PRAGMA user_version".to_string()],
        "exactly one read: no probe, no ALTER, no pragma write"
    );
}

// ---------------------------------------------------------------------------
// 7/8 — a v3 file is refused before the pending check, shape untouched
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_v3_file_is_refused_before_the_pending_check() {
    let config = common::store_config("prompts-v3");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    common::seed_user_prompts(db.as_ref(), 3, true, true)
        .await
        .expect("seed a v3 file");
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&db_path);

    let outcome = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await;

    match outcome {
        Err(StoreError::SchemaNewerThanBuild {
            label,
            found,
            supported,
        }) => {
            assert_eq!(label, "test");
            assert_eq!(found, 3);
            assert_eq!(supported, USER_PROMPTS_MAX_KNOWN);
        }
        other => panic!("expected a typed refusal before the pending check, got {other:?}"),
    }

    // The claim is "no DDL, no row change", not "byte-identical": opening the
    // file creates -wal/-tshm sidecars, so a byte hash would be vacuous.
    assert_eq!(read_user_version(db.as_ref()).await, 3, "version unchanged");
    let columns = common::user_prompt_columns(db.as_ref()).await;
    assert_eq!(columns.len(), 12, "columns unchanged");
    let rows = db
        .query("SELECT content FROM user_prompts", Vec::new())
        .await
        .expect("read rows");
    assert_eq!(rows.len(), 1, "row unchanged");
    assert_eq!(
        rows[0].text("content").as_deref(),
        Some("preserve this prompt")
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();
}

// ---------------------------------------------------------------------------
// 8/8 — a DDL failure (arm a) and a probe failure (arm b) both surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_missing_table_at_v1_is_surfaced_not_swallowed() {
    // Arm (a): a real file stamped v1 with NO table. The v2 phase is reached
    // (nothing pending) and its ALTER batch must fail the open loudly.
    let config = common::store_config("prompts-fail-table");
    let db_path = config.storage_path.join(PROMPTS_DB);
    let db = common::open_real_db(&config, PROMPTS_DB)
        .await
        .expect("open");
    db.execute("PRAGMA user_version = 1", Vec::new())
        .await
        .expect("stamp v1 by hand");
    let proof = common::exclusive_root(&config.storage_path);
    let options = migration_options(&db_path);

    let outcome = mem_store::schema_migrations::apply_user_prompts_schema(
        db.as_ref(),
        &options,
        Some(proof.as_ref()),
    )
    .await;
    assert!(
        outcome.is_err(),
        "a missing-table DDL failure must surface, got {outcome:?}"
    );
    assert!(
        schema_backups(&config.storage_path).is_empty(),
        "no backup before the failed batch: {:?}",
        schema_backups(&config.storage_path)
    );
    drop(db);
    common::cleanup_temp_dir(&config.storage_path).ok();

    // Arm (b): the probe itself fails. The runner first applies v1 into the
    // double (version 0 avoids the pre-runner `current == 1` shortcut mutants
    // target), then the probe error must propagate unchanged and no ALTER
    // batch may be issued.
    let double = PromptsDb::with_probe_error(0, v2_columns(), "disk I/O error");
    let options = MigrationOptions {
        db_path: None,
        label: "test",
        max_known: USER_PROMPTS_MAX_KNOWN,
    };
    let outcome =
        mem_store::schema_migrations::apply_user_prompts_schema(&double, &options, None).await;
    match outcome {
        Err(StoreError::Turso {
            message,
            is_constraint,
        }) => {
            assert_eq!(message, "disk I/O error", "probe error preserved");
            assert!(!is_constraint);
        }
        other => panic!("expected the probe error to propagate, got {other:?}"),
    }
    assert!(
        !double
            .recorded()
            .iter()
            .any(|sql| sql.starts_with("ALTER TABLE")),
        "no ALTER batch after a failed probe: {:?}",
        double.recorded()
    );
}

fn v2_columns() -> Vec<&'static str> {
    vec![
        "id",
        "session_id",
        "message_id",
        "project_path",
        "content",
        "created_at",
        "captured",
        "user_learning_captured",
        "linked_memory_id",
        "capture_attempts",
        "provider_id",
        "model_id",
    ]
}

// ---------------------------------------------------------------------------
// PromptsDb — the scripted double (versions 6 and 8's second arm)
// ---------------------------------------------------------------------------

/// A scripted [`Db`] for the prompts applier: `version`, `columns` and an
/// optional `probe_error` are set by the test; every statement the applier
/// issues is recorded in order.
///
/// Limit: it proves the applier issues no write **statement**, not that the
/// engine performs no write.
struct PromptsDb {
    version: std::sync::Mutex<i64>,
    columns: Vec<&'static str>,
    probe_error: Option<&'static str>,
    sql: std::sync::Mutex<Vec<String>>,
}

impl PromptsDb {
    fn new(version: i64, columns: Vec<&'static str>) -> Self {
        Self {
            version: std::sync::Mutex::new(version),
            columns,
            probe_error: None,
            sql: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn with_probe_error(version: i64, columns: Vec<&'static str>, message: &'static str) -> Self {
        Self {
            probe_error: Some(message),
            ..Self::new(version, columns)
        }
    }

    fn recorded(&self) -> Vec<String> {
        self.sql.lock().unwrap().clone()
    }

    fn record(&self, sql: &str) {
        self.sql.lock().unwrap().push(sql.to_string());
    }
}

impl Db for PromptsDb {
    fn execute(
        &self,
        sql: &str,
        _params: Vec<Value>,
    ) -> BoxFuture<'_, Result<ExecResult, StoreError>> {
        self.record(sql);
        // Mirror the runner's own pragma so a version-0 double advances to v1.
        if let Some(rest) = sql.strip_prefix("PRAGMA user_version =")
            && let Ok(version) = rest.trim().parse::<i64>()
        {
            *self.version.lock().unwrap() = version;
        }
        Box::pin(async {
            Ok(ExecResult {
                changes: 0,
                last_insert_rowid: 0,
            })
        })
    }

    fn query(&self, sql: &str, _params: Vec<Value>) -> BoxFuture<'_, Result<Vec<Row>, StoreError>> {
        self.record(sql);
        if sql == "PRAGMA user_version" {
            let version = *self.version.lock().unwrap();
            return Box::pin(async move {
                Ok(vec![Row::new(
                    std::sync::Arc::new(vec!["user_version".to_string()]),
                    vec![Value::Integer(version)],
                )])
            });
        }
        if sql.starts_with("PRAGMA table_info(user_prompts)") {
            if let Some(message) = self.probe_error {
                return Box::pin(async move { Err(StoreError::turso_message(message)) });
            }
            let columns = self.columns.clone();
            return Box::pin(async move {
                Ok(columns
                    .into_iter()
                    .map(|name| {
                        Row::new(
                            std::sync::Arc::new(vec!["name".to_string()]),
                            vec![Value::Text(name.to_string())],
                        )
                    })
                    .collect())
            });
        }
        Box::pin(async { Ok(Vec::new()) })
    }

    fn batch(
        &self,
        statements: Vec<Statement>,
        _mode: BatchMode,
    ) -> BoxFuture<'_, Result<Vec<ExecResult>, StoreError>> {
        for statement in &statements {
            self.record(&statement.sql);
        }
        Box::pin(async { Ok(Vec::new()) })
    }
}
