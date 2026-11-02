//! Issue 110 — episodic v7: the capture inbox's nullable `not_before`
//! scheduling column, and the access path of the drain statement.
//!
//! What v7 adds, and what it deliberately does NOT add:
//! - `capture_inbox.not_before INTEGER` — nullable, NO default, so a NULL
//!   means "immediately eligible" and every pre-v7 row keeps its behaviour;
//! - no second backup: the `ALTER` is additive and one transactional batch
//!   plus one pragma converge under the probe (D-110-1);
//! - no index change: the drain keeps the `(status, created_at)` seek and
//!   filters `not_before` as a residual on the rows that seek already
//!   returns (D-110-4).
//!
//! The state this suite exists for is the CRASH WINDOW between the v7 `ALTER`
//! commit and `PRAGMA user_version = 7`: a v6 file whose column exists but
//! whose stamp does not. `ALTER TABLE … ADD COLUMN` is not re-runnable, so the
//! re-apply must converge through a `PRAGMA table_info` probe instead of
//! failing on a duplicate column. An unprobed `db.batch(EPISODIC_V7_ALTERS)`
//! passes every other criterion here and fails only on that file.

mod common;

use common::{open_real_db, store_config};
use mem_store::config::StoreConfig;
use mem_store::db::{Db, DbHandle, Value, query_one};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{MigrationOptions, apply_episodic_schema};

const DIMS: u32 = 2;

/// The crash fixture's own literal. Deliberately NOT the production
/// constant: this file must compile against a tree where
/// `EPISODIC_V7_ALTERS` does not exist yet. Step 2 pins the production
/// constant to this string inside the convergence test.
const V7_ALTER: &str = "ALTER TABLE capture_inbox ADD COLUMN not_before INTEGER";

/// The memoised root proof (a v5→v6 upgrade needs one).
fn root_proof(config: &StoreConfig) -> std::sync::Arc<mem_store::delete_guard::ExclusiveRoot> {
    common::exclusive_root(&config.storage_path)
}

/// Apply-time options with an EXPLICIT `max_known`.
///
/// The v6 suites' helper reads [`mem_store::schema_migrations::EPISODIC_MAX_KNOWN`],
/// which cannot build a genuine v6 file once the build knows v7; passing the
/// value is what makes the v6 fixture and the six-only refusal expressible.
fn options(config: &StoreConfig, max_known: i64) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("episodic.db")),
        label: "episodic-v7",
        max_known,
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

/// Builds a GENUINE v6 file: the whole fold under a build that knows only 6,
/// so the v7 leg never runs.
async fn v6_database(config: &StoreConfig) -> DbHandle {
    let db = open_real_db(config, "episodic.db").await.expect("open");
    apply_episodic_schema(
        &*db,
        DIMS,
        &options(config, 6),
        Some(root_proof(config).as_ref()),
    )
    .await
    .expect("build a v6 file under max_known: 6");
    assert_eq!(user_version(&*db).await, 6, "the fixture is a v6 file");
    db
}

/// Applies the v7 leg to an existing file.
async fn apply_v7(db: &dyn Db, config: &StoreConfig) -> i64 {
    let outcome = apply_episodic_schema(
        db,
        DIMS,
        &options(config, 7),
        Some(root_proof(config).as_ref()),
    )
    .await
    .expect("v7 apply");
    outcome.version
}

/// The v7 column reaches disk NULLABLE with no default, and the file is
/// stamped 7: `ADD COLUMN not_before INTEGER NOT NULL DEFAULT 0` would make
/// `notnull` 1 (or give the column a default) and is the natural mutation.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v7_adds_nullable_not_before_and_stamps_seven() {
    let config = store_config("v7-fresh");
    let db = open_real_db(&config, "episodic.db").await.expect("open");

    assert_eq!(
        apply_v7(&*db, &config).await,
        7,
        "a fresh root reaches v7 in one open"
    );
    assert_eq!(user_version(&*db).await, 7);

    let info = db
        .query("PRAGMA table_info(capture_inbox)", Vec::new())
        .await
        .expect("table_info(capture_inbox)");
    let column = info
        .iter()
        .find(|row| row.text("name").as_deref() == Some("not_before"))
        .expect("the not_before column exists");
    assert_eq!(column.i64("notnull"), Some(0), "the column is NULLABLE");
    assert!(
        column.is_null("dflt_value"),
        "the column has NO default: {column:?}"
    );
    assert_eq!(
        info.iter()
            .filter(|row| row.text("name").as_deref() == Some("not_before"))
            .count(),
        1,
        "exactly one not_before column"
    );
}

/// The upgrade runs on a POPULATED inbox: a nullable no-default `ADD COLUMN`
/// succeeds and every pre-v7 row survives with `not_before IS NULL` (which is
/// what keeps it immediately eligible — the drain-statement half of that is
/// `test_inbox_batch_select_searches_the_status_created_index`).
///
/// Target mutant: `ADD COLUMN … NOT NULL` (the `ALTER` refuses on a populated
/// table) or a rebuild-and-copy that drops rows.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v7_upgrades_a_v6_database_preserving_its_rows() {
    let config = store_config("v7-upgrade");
    let db = v6_database(&config).await;
    db.execute(
        "INSERT INTO capture_inbox (capture_id, project_id, session_id, agent_id, payload, \
         created_at, attempts, last_attempt_at, status) \
         VALUES (?, ?, ?, ?, ?, ?, 0, NULL, 'queued')",
        vec![
            Value::Text("cap-legacy".to_string()),
            Value::Text("/srv/legacy".to_string()),
            Value::Text("sess-legacy".to_string()),
            Value::Text("agent-legacy".to_string()),
            Value::Text(r#"{"transcript":"the legacy capture"}"#.to_string()),
            Value::Integer(11),
        ],
    )
    .await
    .expect("seed one pre-v7 queued row");

    assert_eq!(
        apply_v7(&*db, &config).await,
        7,
        "the nullable ADD COLUMN succeeds on a populated table"
    );
    assert_eq!(user_version(&*db).await, 7);

    let row = query_one(
        &*db,
        "SELECT capture_id, payload, created_at, attempts, status, not_before \
         FROM capture_inbox WHERE capture_id = 'cap-legacy'",
        Vec::new(),
    )
    .await
    .expect("select the legacy row")
    .expect("the legacy row survives the upgrade");
    assert_eq!(row.text("capture_id").as_deref(), Some("cap-legacy"));
    assert_eq!(
        row.text("payload").as_deref(),
        Some(r#"{"transcript":"the legacy capture"}"#),
        "the payload is not rewritten"
    );
    assert_eq!(row.i64("created_at"), Some(11));
    assert_eq!(row.i64("attempts"), Some(0));
    assert_eq!(row.text("status").as_deref(), Some("queued"));
    assert!(
        row.is_null("not_before"),
        "a pre-v7 row is immediately eligible"
    );
}

/// The bricking state: the v7 `ALTER` committed and the pragma did not. The
/// probed re-apply must converge to `Ok`, stamp 7, and leave the column list
/// byte-identical.
///
/// Target mutants: an unprobed `db.batch(EPISODIC_V7_ALTERS.to_vec(), …)`
/// returns `duplicate column name: not_before` instead of `Ok`; a renamed
/// production column changes the snapshot even though the fixture supplied
/// `not_before`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v7_converges_after_a_crash_between_the_alter_and_the_pragma() {
    assert_eq!(
        mem_store::episodic_sql::EPISODIC_V7_ALTERS,
        [V7_ALTER],
        "the production literal must stay the nullable, no-default ALTER"
    );

    let config = store_config("v7-crash-window");
    let db = v6_database(&config).await;
    db.execute(V7_ALTER, Vec::new())
        .await
        .expect("run the v7 ALTER the way the crashed process did");
    assert_eq!(
        user_version(&*db).await,
        6,
        "a raw ALTER leaves the stamp at 6"
    );
    let before = table_columns(&*db, "capture_inbox").await;
    assert!(
        before.iter().any(|name| name == "not_before"),
        "the fixture's column exists: {before:?}"
    );

    assert_eq!(
        apply_v7(&*db, &config).await,
        7,
        "the probed re-apply converges instead of a duplicate column"
    );
    assert_eq!(user_version(&*db).await, 7);

    let after = table_columns(&*db, "capture_inbox").await;
    assert_eq!(
        after
            .iter()
            .filter(|name| name.as_str() == "not_before")
            .count(),
        1,
        "exactly one not_before column: {after:?}"
    );
    assert_eq!(after, before, "the probed re-apply alters nothing");
}

/// v6→v7 takes NO backup: the change is additive and the batch is atomic, so
/// a restore would have nothing to roll back (D-110-1). The version assertion
/// comes FIRST so this criterion is RED before the leg exists instead of
/// passing on a v6 file that simply never got there.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v7_takes_no_second_backup() {
    let config = store_config("v7-no-backup");
    let db = v6_database(&config).await;

    assert_eq!(
        apply_v7(&*db, &config).await,
        7,
        "the file is at v7 before any backup claim"
    );
    assert_eq!(user_version(&*db).await, 7);

    let backups: Vec<String> = std::fs::read_dir(&config.storage_path)
        .expect("read storage root")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("episodic.db.schema-v6-") && name.ends_with(".bak"))
        .collect();
    assert!(
        backups.is_empty(),
        "the v7 leg takes no schema-v6 backup: {backups:?}"
    );
}

/// No index change: the drain keeps the `(status, created_at)` index. The
/// version assertion comes FIRST for the same reason as the backup criterion.
///
/// Target mutants: an index swap to `(status, not_before, created_at)`, or a
/// dropped index.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v7_leaves_the_status_created_index_unchanged() {
    let config = store_config("v7-index");
    let db = v6_database(&config).await;

    assert_eq!(
        apply_v7(&*db, &config).await,
        7,
        "the file is at v7 before the index claim"
    );

    let row = query_one(
        &*db,
        "SELECT sql FROM sqlite_master WHERE type = 'index' \
         AND name = 'idx_capture_inbox_status_created'",
        Vec::new(),
    )
    .await
    .expect("sqlite_master")
    .expect("the status_created index exists");
    let ddl = row.text("sql").unwrap_or_default();
    assert!(
        ddl.contains("(status, created_at)"),
        "the index keeps its columns: {ddl}"
    );
    assert!(
        !ddl.contains("not_before"),
        "the index gained no column: {ddl}"
    );
}

/// The access path of the PRODUCTION drain statement, measured on this
/// engine: the `status = 'queued'` equality is the index seek and
/// `(not_before IS NULL OR not_before <= ?)` is a residual on the rows that
/// seek returns, so `not_before` never removes the seek and the `capture_id`
/// tie-break keeps the sorter that was already there at v6.
///
/// Target mutant: dropping the `status = 'queued'` equality (or the index)
/// turns the seek into `SCAN capture_inbox`.
///
/// The statement is the production constant, never a local copy: a local copy
/// would mask the Step-5 dependency that the drain carries two parameters.
/// Until that step the constant has ONE placeholder, so this call fails with
/// `bind index 2 is out of bounds` — an `Err` is a test FAILURE here, recorded
/// verbatim, never a fallback.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inbox_batch_select_searches_the_status_created_index() {
    let config = store_config("v7-plan");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    apply_v7(&*db, &config).await;

    let plan = format!(
        "EXPLAIN QUERY PLAN {}",
        mem_store::episodic_sql::SELECT_CAPTURE_INBOX_BATCH
    );
    let rows = db
        .query(&plan, vec![Value::Integer(0), Value::Integer(1)])
        .await
        .expect("EXPLAIN QUERY PLAN over the drain statement");
    let details: Vec<String> = rows.iter().filter_map(|row| row.text("detail")).collect();
    assert_eq!(
        details,
        [
            "SEARCH capture_inbox USING INDEX idx_capture_inbox_status_created (status=?)",
            "USE SORTER FOR ORDER BY",
        ]
        .map(str::to_string)
        .to_vec(),
        "the measured plan of the drain statement"
    );
    assert!(
        !details
            .iter()
            .any(|detail| detail.contains("SCAN capture_inbox")),
        "the status equality keeps the seek: {details:?}"
    );
}

/// A build that knows only 6 must refuse a file this build stamped 7 — the
/// no-downgrade promise (ADR-0021 D-21-3), and the proof that
/// `EPISODIC_MAX_KNOWN` moved WITH the leg.
///
/// Target mutant: `EPISODIC_MAX_KNOWN` left at 6 while the leg stamps 7 —
/// the build then refuses its own file on the third open.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v7_stamp_is_refused_by_a_six_only_build() {
    let config = store_config("v7-six-only");
    let db = v6_database(&config).await;
    assert_eq!(apply_v7(&*db, &config).await, 7);

    let refused = apply_episodic_schema(
        &*db,
        DIMS,
        &options(&config, 6),
        Some(root_proof(&config).as_ref()),
    )
    .await
    .expect_err("a six-only build refuses a v7 file");
    match refused {
        StoreError::SchemaNewerThanBuild {
            found, supported, ..
        } => {
            assert_eq!(found, 7, "the file's version is 7");
            assert_eq!(supported, 6, "the build knows 6");
        }
        other => panic!("expected SchemaNewerThanBuild, got {other:?}"),
    }

    assert_eq!(
        apply_v7(&*db, &config).await,
        7,
        "the build that stamped the file still opens it"
    );
}
