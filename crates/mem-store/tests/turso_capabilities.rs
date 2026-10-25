//! W0 capability pins for the database-base hardening (issue 43).
//!
//! This file pins **engine** facts about turso 0.8.3 — it is **not** an
//! upstream port and has no upstream oracle. Each test turns one fact the
//! hardening design depends on into a falsifiable assertion about an
//! observable: rows returned, a `StoreError` shape, or the bytes on disk.
//!
//! **Controls.** Every test named `...does_not...`, `...is_refused...` or
//! `...is_unimplemented...` is a *control*: it passes only while the engine
//! lacks the capability, and it must fail the moment the engine gains it. A
//! capability test that passes under the opposite behaviour is worthless, so
//! no test here asserts a bare `is_ok()` on a pragma or an `ALTER`.
//!
//! **Option set.** AC11–AC15, AC18, AC19, AC21 and AC22 open through the
//! shipping option set — `ConnectOptions::for_config(config, Platform::current())`,
//! i.e. multiprocess WAL on non-Windows and the encryption flag off when
//! encryption is disabled. AC1's second, explicitly labelled engine-baseline
//! handle goes through `turso::Builder` directly, because that claim is about
//! the engine default rather than about the shipping open — and since issue 45
//! the factory refuses a legacy option set (`UnsupportedOpenMode`, owner
//! decision O3), so no seam-based way to open one remains. AC16/AC17
//! necessarily reach `turso::Builder` directly too: `ConnectOptions` cannot
//! express `experimental_vacuum`, and this wave may not extend it. AC16 is the
//! explicitly non-production capability probe, labelled as such in its doc;
//! AC17 is the configuration W6's remedy would create, and its refusal is the
//! finding.
//!
//! **Non-vacuity.** (a) An absence assertion only counts immediately after
//! the same helper proved the byte pattern present on disk and the engine was
//! given the operation that should remove it. (b) A rejection assertion names
//! the one mechanism able to produce the `Err` and builds the fixture so no
//! other mechanism in the schema can produce it — each such test's doc
//! comment states that mechanism. Assertions are on variants, classifiers and
//! file bytes only; message text is never asserted.
//!
//! **Green on first run is not proof.** There is no implementation to drive
//! (this wave is test-only), so the cycle is expected-fact-first rather than
//! red-green; the bound on first-run green is the two-arm inversion proof
//! recorded in the wave ledger (invert AC13's presence assertion → FAILED;
//! invert AC6's active set to the non-transactional value → FAILED; both
//! restored → PASSED).
//!
//! No production source, manifest, `sql.rs` or `golden_sql.rs` is touched.

mod common;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use mem_store::config::StoreConfig;
use mem_store::db::{
    BatchMode, ConnectFactory, ConnectOptions, Db, DbHandle, EncryptionOptions, ExecResult, Row,
    Statement, Value, turso_connect_factory,
};
use mem_store::error::StoreError;
use mem_store::platform::Platform;

// ---------------------------------------------------------------------------
// Pinned fixtures and marker
// ---------------------------------------------------------------------------

/// Unique ASCII marker written into a table cell and then deleted; its byte
/// pattern is the observable for every erasure control. Deliberately long and
/// non-repeating so a chance collision in a freshly created database is
/// impossible.
const CANARY: &str = "CANARY-W0-r1-8f4c1d2e-do-not-survive";

/// The `shards` fixture, verbatim from the W0 probe: the table carries a
/// row-level `UNIQUE(scope, scope_hash, shard_index)`, so the active-row tests
/// must use *distinct* `shard_index` values for the partial unique index to be
/// the only possible rejecting mechanism.
const CREATE_SHARDS: &str = concat!(
    "CREATE TABLE shards (",
    "id INTEGER PRIMARY KEY AUTOINCREMENT, ",
    "scope TEXT NOT NULL, ",
    "scope_hash TEXT NOT NULL, ",
    "shard_index INTEGER NOT NULL, ",
    "is_active INTEGER DEFAULT 1, ",
    "UNIQUE(scope, scope_hash, shard_index))",
);

/// The one-active-shard partial unique index (issue 43 / W4's mechanism).
const CREATE_ONE_ACTIVE_SHARD_INDEX: &str = concat!(
    "CREATE UNIQUE INDEX one_active_shard ON shards(scope, scope_hash) ",
    "WHERE is_active = 1"
);

/// The `user_prompts` table the erasure helpers seed. `IF NOT EXISTS` because
/// the upgrade probe creates it itself: its kept row must be written BEFORE the
/// canary, so the canary's `DELETE` is the last write to the table (see
/// [`seed_probe_database`]).
const CREATE_USER_PROMPTS: &str = concat!(
    "CREATE TABLE IF NOT EXISTS user_prompts (",
    "id TEXT PRIMARY KEY, content TEXT NOT NULL)"
);

// ---------------------------------------------------------------------------
// Local helpers (no change to tests/common)
// ---------------------------------------------------------------------------

/// Hermetic store config over a fresh temp dir (encryption disabled).
fn production_config(prefix: &str) -> StoreConfig {
    common::store_config(prefix)
}

/// A handle opened through the **shipping** option set:
/// `ConnectOptions::for_config` (multiprocess WAL on non-Windows, encryption
/// feature flag off for an unencrypted config). This is the configuration
/// production actually uses, so a control that claims "production" must use
/// this handle.
async fn open_production(config: &StoreConfig, name: &str) -> DbHandle {
    turso_connect_factory()
        .connect(
            config.storage_path.join(name),
            ConnectOptions::for_config(config, Platform::current()),
        )
        .await
        .expect("open database through the production option set")
}

/// A handle opened with the **plain builder** — no experimental feature at all,
/// i.e. turso's single-process (legacy) WAL mode. Used **only** by AC1 as a
/// labelled engine-baseline handle (the claim there is about the engine's default
/// journal mode, not about the shipping open).
///
/// Reaches `turso::Builder` directly by design: since issue 45 the production
/// factory **refuses** a legacy option set with `UnsupportedOpenMode` (owner
/// decision O3), and production code must not grow a test-only escape hatch
/// through the seam. This is the same class as AC16/AC17's raw handles.
async fn open_default(dir: &Path, name: &str) -> DbHandle {
    let database = turso::Builder::new_local(&dir.join(name).to_string_lossy())
        .build()
        .await
        .expect("open the engine-default database");
    let connection = database
        .connect()
        .expect("connect to the engine-default db");
    let db: DbHandle = std::sync::Arc::new(RawDb {
        database,
        connection,
    });
    db
}

/// A `Db` over a raw `turso::Connection` — the test-local mirror of
/// `mem_store::db::TursoDb` for the handles that must bypass the guard.
///
/// Why it exists: after issue 45 three tests that measure the *engine* on a
/// flag-off handle cannot open through `TursoConnectFactory`, because that seam
/// refuses a legacy option set by construction. Rather than add a production
/// backdoor, they build the same builder AC16/AC17 already use and wrap it here.
struct RawDb {
    /// Retained so the file resources live as long as the connection (turso has
    /// no `close`; release is Drop).
    #[allow(dead_code)]
    database: turso::Database,
    connection: turso::Connection,
}

fn raw_value_into(value: Value) -> turso::Value {
    match value {
        Value::Null => turso::Value::Null,
        Value::Integer(value) => turso::Value::Integer(value),
        Value::Real(value) => turso::Value::Real(value),
        Value::Text(text) => turso::Value::Text(text),
        Value::Blob(bytes) => turso::Value::Blob(bytes),
    }
}

fn raw_value_from(value: turso::Value) -> Value {
    match value {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(value) => Value::Integer(value),
        turso::Value::Real(value) => Value::Real(value),
        turso::Value::Text(text) => Value::Text(text),
        turso::Value::Blob(bytes) => Value::Blob(bytes),
    }
}

async fn raw_rows(mut rows: turso::Rows) -> Result<Vec<Row>, StoreError> {
    let columns = std::sync::Arc::new(rows.column_names());
    let mut collected = Vec::new();
    while let Some(raw) = rows.next().await? {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            values.push(raw_value_from(raw.get_value(index)?));
        }
        collected.push(Row::new(columns.clone(), values));
    }
    Ok(collected)
}

impl Db for RawDb {
    fn execute(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> mem_store::db::BoxFuture<'_, Result<ExecResult, StoreError>> {
        let params: Vec<turso::Value> = params.into_iter().map(raw_value_into).collect();
        let sql = sql.to_string();
        Box::pin(async move {
            let changes = self
                .connection
                .execute(sql.as_str(), params)
                .await
                .map_err(StoreError::from)?;
            Ok(ExecResult {
                changes,
                last_insert_rowid: self.connection.last_insert_rowid(),
            })
        })
    }

    fn query(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> mem_store::db::BoxFuture<'_, Result<Vec<Row>, StoreError>> {
        let params: Vec<turso::Value> = params.into_iter().map(raw_value_into).collect();
        let sql = sql.to_string();
        Box::pin(async move {
            let rows = self
                .connection
                .query(sql.as_str(), params)
                .await
                .map_err(StoreError::from)?;
            raw_rows(rows).await
        })
    }

    fn batch(
        &self,
        statements: Vec<Statement>,
        mode: BatchMode,
    ) -> mem_store::db::BoxFuture<'_, Result<Vec<ExecResult>, StoreError>> {
        let behavior = match mode {
            BatchMode::Write => turso::transaction::TransactionBehavior::Immediate,
            BatchMode::Read => turso::transaction::TransactionBehavior::Deferred,
        };
        let batched: Vec<(String, Vec<turso::Value>)> = statements
            .into_iter()
            .map(|statement| {
                (
                    statement.sql,
                    statement.params.into_iter().map(raw_value_into).collect(),
                )
            })
            .collect();
        Box::pin(async move {
            let results = self
                .connection
                .transactional_batch(batched, behavior)
                .await
                .map_err(StoreError::from)?;
            Ok(results
                .into_iter()
                .map(|result| ExecResult {
                    changes: result.rows_affected(),
                    last_insert_rowid: result.last_insert_rowid().unwrap_or(0),
                })
                .collect())
        })
    }
}

/// Raw builder handles for the `experimental_vacuum` probes. `ConnectOptions`
/// cannot express this flag and this wave must not extend it, so AC16/AC17
/// reach `turso::Builder` directly. The caller keeps the `Database` alive next
/// to the `Connection` (dropping the database releases the file resources).
async fn open_with_vacuum(
    path: &Path,
    multiprocess_wal: bool,
) -> (turso::Database, turso::Connection) {
    try_open_with_vacuum(path, multiprocess_wal)
        .await
        .unwrap_or_else(|error| panic!("build vacuum database: {error}"))
}

/// The non-panicking twin of [`open_with_vacuum`], one builder chain, two
/// contracts. The Step 8 second-wave probe's **question** is whether this open
/// succeeds, so it cannot share a helper that panics on the answer it is
/// measuring; the callers that require success keep [`open_with_vacuum`].
async fn try_open_with_vacuum(
    path: &Path,
    multiprocess_wal: bool,
) -> Result<(turso::Database, turso::Connection), String> {
    let mut builder = turso::Builder::new_local(&path.to_string_lossy()).experimental_vacuum(true);
    if multiprocess_wal {
        builder = builder.experimental_multiprocess_wal(true);
    }
    let database = builder.build().await.map_err(|error| error.to_string())?;
    let connection = database.connect().map_err(|error| error.to_string())?;
    Ok((database, connection))
}

/// Byte-pattern search with an explicit empty/short guard (a naive
/// `windows(0)` scan would report a hit for an empty needle).
fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Does the file exist and contain the pattern? A missing file is `false`.
fn file_contains(path: &Path, needle: &str) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => bytes_contain(&bytes, needle.as_bytes()),
        Err(_) => false,
    }
}

/// File length, or `None` when the file does not exist. The `None`/`Some(0)`
/// distinction is load-bearing: an absent `-wal` must FAIL a truncation
/// assertion rather than pass it vacuously.
fn file_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

/// Walks **every** file under the per-test directory (recursively) looking for
/// the marker. Scanning the directory instead of a fixed sidecar list means
/// `.db`, `-wal`, `-shm`, `-tshm` and `.bak` are all covered by construction —
/// under multiprocess WAL a `-tshm` sidecar really exists (measured 86 016
/// bytes while `-shm` stayed 0), so a fixed list would miss bytes.
fn dir_contains_canary(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if dir_contains_canary(&path) {
                return true;
            }
        } else if file_contains(&path, CANARY) {
            return true;
        }
    }
    false
}

/// `<path><suffix>`, the sidecar-path shape turso uses (`-wal`, `-shm`,
/// `-tshm`).
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

/// Text of the row's OWN first column. Single-value pragmas
/// (`PRAGMA journal_mode`) have engine-defined column names, so hard-coding
/// one would be a false pin.
fn first_column_text(row: &Row) -> Option<String> {
    let name = row.column_names().first()?;
    row.text(name)
}

/// `StoreError::Turso { is_constraint: true }` — the classifier `StoreError`
/// derives from a *top-level* `turso::Error::Constraint`.
fn is_constraint(err: &StoreError) -> bool {
    matches!(
        err,
        StoreError::Turso {
            is_constraint: true,
            ..
        }
    )
}

/// `StoreError::Turso { is_constraint: false }` — the plain engine-error shape
/// (a parse error, a batch-wrapped statement error, a batch rollback error).
fn is_plain_turso_error(err: &StoreError) -> bool {
    matches!(
        err,
        StoreError::Turso {
            is_constraint: false,
            ..
        }
    )
}

/// First-column integers of a query (counts, shard indexes, scalar selects).
async fn query_integers(db: &dyn Db, sql: &str) -> Vec<i64> {
    let rows = db.query(sql, vec![]).await.expect("probe query");
    rows.iter()
        .filter_map(|row| row.column_names().first().and_then(|name| row.i64(name)))
        .collect()
}

/// `SELECT COUNT(*)` over one fixture table.
async fn count_rows(db: &dyn Db, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let counts = query_integers(db, &sql).await;
    counts
        .first()
        .copied()
        .expect("COUNT(*) returns exactly one row")
}

/// `PRAGMA wal_checkpoint(TRUNCATE)` through the `Db` seam. The returned rows
/// are engine-defined and are not asserted; the observable is the `-wal`
/// length measured by the caller.
async fn checkpoint_truncate(db: &dyn Db) {
    db.query("PRAGMA wal_checkpoint(TRUNCATE)", vec![])
        .await
        .expect("PRAGMA wal_checkpoint(TRUNCATE)");
}

/// `(shard_index, is_active)` for every fixture row, ordered by
/// `shard_index`; the SELECT aliases are ours, so naming the columns here is
/// safe.
async fn shard_state(db: &dyn Db) -> Vec<(i64, i64)> {
    let rows = db
        .query(
            "SELECT shard_index, is_active FROM shards ORDER BY shard_index",
            vec![],
        )
        .await
        .expect("read shard state");
    rows.iter()
        .map(|row| {
            let shard_index = row.i64("shard_index").expect("shard_index is an integer");
            let is_active = row.i64("is_active").expect("is_active is an integer");
            (shard_index, is_active)
        })
        .collect()
}

/// Active `shard_index` values, ascending.
async fn active_shard_indexes(db: &dyn Db) -> Vec<i64> {
    query_integers(
        db,
        "SELECT shard_index FROM shards WHERE is_active = 1 ORDER BY shard_index",
    )
    .await
}

/// Creates the `shards` fixture table. The index is installed separately so
/// each test can assert (or knowingly depend on) its installation.
async fn shards_table(prefix: &str) -> (StoreConfig, DbHandle) {
    let config = production_config(prefix);
    let db = open_production(&config, "shards.db").await;
    db.execute(CREATE_SHARDS, vec![])
        .await
        .expect("create the shards fixture table");
    (config, db)
}

/// Installs the one-active-shard partial unique index and returns the raw
/// result so the caller decides whether an `Err` is a failure (it always is —
/// a missing index would make the active-row assertions vacuous).
async fn install_one_active_shard_index(db: &dyn Db) -> Result<ExecResult, StoreError> {
    db.execute(CREATE_ONE_ACTIVE_SHARD_INDEX, vec![]).await
}

/// `shards_table` + an **asserted** index installation.
async fn shards_fixture(prefix: &str) -> (StoreConfig, DbHandle) {
    let (config, db) = shards_table(prefix).await;
    let index = install_one_active_shard_index(db.as_ref()).await;
    assert!(
        index.is_ok(),
        "the one_active_shard partial unique index must install: {index:?}"
    );
    (config, db)
}

/// One `shards` row for the fixture scope.
async fn seed_shard(db: &dyn Db, shard_index: i64, is_active: i64) {
    db.execute(
        "INSERT INTO shards (scope, scope_hash, shard_index, is_active) VALUES ('project', 'abc', ?, ?)",
        vec![Value::Integer(shard_index), Value::Integer(is_active)],
    )
    .await
    .expect("seed shard row");
}

/// Production-side erasure fixture: create `user_prompts` (unless the caller
/// already did), insert the canary, prove the marker reached disk (what stops
/// the later absence from being vacuous), then delete the row. The bytes stay
/// in the WAL; the caller checkpoints.
async fn seed_and_delete_canary(db: &dyn Db, storage: &Path) {
    db.execute(CREATE_USER_PROMPTS, vec![])
        .await
        .expect("create user_prompts");
    db.execute(
        "INSERT INTO user_prompts (id, content) VALUES ('p1', ?)",
        vec![Value::Text(CANARY.to_string())],
    )
    .await
    .expect("insert the canary row");
    assert!(
        dir_contains_canary(storage),
        "the canary must reach disk before the delete, otherwise the later absence assertion is vacuous"
    );
    db.execute("DELETE FROM user_prompts WHERE id = 'p1'", vec![])
        .await
        .expect("delete the canary row");
}

/// Raw-connection twin of [`seed_and_delete_canary`] (AC16/AC17 hold a
/// `turso::Connection`, not a `Db`).
async fn raw_seed_and_delete_canary(connection: &turso::Connection, storage: &Path) {
    connection
        .execute(CREATE_USER_PROMPTS, ())
        .await
        .expect("create user_prompts");
    let insert = format!("INSERT INTO user_prompts (id, content) VALUES ('p1', '{CANARY}')");
    connection
        .execute(insert.as_str(), ())
        .await
        .expect("insert the canary row");
    assert!(
        dir_contains_canary(storage),
        "the canary must reach disk before the delete, otherwise the later absence assertion is vacuous"
    );
    connection
        .execute("DELETE FROM user_prompts WHERE id = 'p1'", ())
        .await
        .expect("delete the canary row");
}

/// Raw-connection `PRAGMA wal_checkpoint(TRUNCATE)`, draining the
/// engine-defined result rows.
async fn raw_checkpoint(connection: &turso::Connection) {
    let mut rows = connection
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await
        .expect("PRAGMA wal_checkpoint(TRUNCATE)");
    while rows
        .next()
        .await
        .expect("read a wal_checkpoint result row")
        .is_some()
    {}
}

/// Raw-connection `SELECT COUNT(*)`.
async fn raw_count(connection: &turso::Connection, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let mut rows = connection
        .query(sql.as_str(), ())
        .await
        .expect("raw COUNT(*) query");
    let row = rows
        .next()
        .await
        .expect("read the raw COUNT(*) row")
        .expect("COUNT(*) yields a row");
    match row.get_value(0).expect("raw COUNT(*) first column") {
        turso::Value::Integer(value) => value,
        other => panic!("raw COUNT(*) must be an INTEGER, got {other:?}"),
    }
}

/// Text values of one named column across a query's rows, sorted (set
/// semantics). `table_info`'s documented `name` column is passed by the
/// caller; the ordering of rows is not asserted.
async fn column_names(db: &dyn Db, sql: &str, column: &str) -> Vec<String> {
    let rows = db.query(sql, vec![]).await.expect("column probe query");
    let mut names: Vec<String> = rows.iter().filter_map(|row| row.text(column)).collect();
    names.sort();
    names
}

// ===========================================================================
// Step 1 — engine default journal mode (tracer)
// ===========================================================================

/// AC1: a brand-new database is WAL, both for the engine default and for the
/// shipping open.
///
/// The premise every "deleted text lives in the WAL" argument rests on. Two
/// handles, deliberately labelled: `production.db` through the shipping option
/// set, `default.db` through the plain raw builder ([`open_default`]), because
/// the claim is about the engine's default, not about the shipping open. The
/// factory cannot open a legacy handle any more (issue 45 / O3).
#[tokio::test]
async fn test_new_database_defaults_to_wal_journal_mode() {
    let production_db_config = production_config("cap-journal-production");
    let default_db_config = production_config("cap-journal-default");

    let handles = [
        (
            "production",
            open_production(&production_db_config, "production.db").await,
        ),
        (
            "default",
            open_default(&default_db_config.storage_path, "default.db").await,
        ),
    ];

    for (label, db) in &handles {
        let rows = db
            .query("PRAGMA journal_mode", vec![])
            .await
            .expect("read PRAGMA journal_mode");
        assert_eq!(
            rows.len(),
            1,
            "{label}: PRAGMA journal_mode must return exactly one row"
        );
        let mode =
            first_column_text(&rows[0]).expect("journal_mode row has text in its first column");
        assert!(
            mode.eq_ignore_ascii_case("wal"),
            "{label}: a new database must be in WAL journal mode, got {mode}"
        );
    }

    drop(handles);
    common::cleanup_temp_dir(&production_db_config.storage_path)
        .expect("cleanup production temp dir");
    common::cleanup_temp_dir(&default_db_config.storage_path).expect("cleanup default temp dir");
}

// ===========================================================================
// Step 2 — the `execute` contract, P1, and batch semantics (AC2–AC8)
// ===========================================================================

/// AC2: `execute` applies only the **first** statement of a multi-statement
/// string.
///
/// This pins the trap that made the original P1 rotation probe vacuous:
/// `Connection::execute` compiles the first parsed command and drops the rest,
/// returning `Ok`. `changes` is the first statement's own count (INSERT → 1,
/// BEGIN → 0), never a sum. The true discriminators are `changes == 1` with a
/// row count of 1, `changes == 0` with the value unchanged, and a still-usable
/// connection after the dangling `BEGIN`.
#[tokio::test]
async fn test_execute_applies_only_the_first_statement_of_a_multi_statement_string() {
    let config = production_config("cap-execute-first");
    let db = open_production(&config, "probe.db").await;
    db.execute(
        "CREATE TABLE probe_execute (id TEXT PRIMARY KEY, v INTEGER)",
        vec![],
    )
    .await
    .expect("create probe_execute");

    let inserted = db
        .execute(
            "INSERT INTO probe_execute VALUES ('a',1); INSERT INTO probe_execute VALUES ('b',2);",
            vec![],
        )
        .await
        .expect("a two-INSERT string is accepted (only the first statement compiles)");
    assert_eq!(
        inserted.changes, 1,
        "changes counts the FIRST statement only, so exactly one row was inserted"
    );
    assert_eq!(
        count_rows(db.as_ref(), "probe_execute").await,
        1,
        "the second INSERT must have been dropped — a two-row table means multi-statement execute applied both"
    );

    let begun = db
        .execute(
            "BEGIN; UPDATE probe_execute SET v = 9 WHERE id = 'a'; COMMIT;",
            vec![],
        )
        .await
        .expect("a BEGIN-led string is accepted (only BEGIN compiles)");
    assert_eq!(
        begun.changes, 0,
        "BEGIN changes no rows; the UPDATE/COMMIT inside the string never compiled"
    );

    let values = query_integers(db.as_ref(), "SELECT v FROM probe_execute WHERE id = 'a'").await;
    assert_eq!(
        values,
        vec![1],
        "the UPDATE inside the string must not have applied"
    );

    let counts = query_integers(db.as_ref(), "SELECT COUNT(*) FROM probe_execute").await;
    assert_eq!(
        counts,
        vec![1],
        "the connection still answers after the dangling BEGIN"
    );
    let single = db
        .execute("INSERT INTO probe_execute VALUES ('c',3)", vec![])
        .await
        .expect("a single-statement insert still works after the dangling BEGIN");
    assert_eq!(
        single.changes, 1,
        "a further single-statement insert applies exactly one row"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC3: the partial unique index installs and enforces one active row.
///
/// Rejection mechanism: the partial unique index `one_active_shard` only. The
/// two rows carry **distinct** `shard_index` values (0 and 1), so the table's
/// own `UNIQUE(scope, scope_hash, shard_index)` cannot produce the `Err`; and
/// the second row differs from the first only in `shard_index`, so nothing but
/// the partial index can reject it.
#[tokio::test]
async fn test_partial_unique_index_installs_and_rejects_a_second_active_row() {
    let (config, db) = shards_fixture("cap-partial-unique").await;

    seed_shard(db.as_ref(), 0, 1).await;
    let second = db
        .execute(
            "INSERT INTO shards (scope, scope_hash, shard_index, is_active) VALUES ('project', 'abc', 1, 1)",
            vec![],
        )
        .await;
    let error = second.expect_err(
        "a second active row for the same (scope, scope_hash) must be rejected by the partial unique index",
    );
    assert!(
        is_constraint(&error),
        "the rejection must be constraint-classified, got {error:?}"
    );
    assert_eq!(
        count_rows(
            db.as_ref(),
            "shards WHERE is_active = 1 AND scope = 'project' AND scope_hash = 'abc'"
        )
        .await,
        1,
        "exactly one active shard row may exist for the scope"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC4: the partial predicate lets inactive duplicates coexist.
///
/// Rejection mechanism (inverted): the same index must NOT fire here. Both rows
/// are `is_active = 0` at **distinct** `shard_index` values (0 and 1), so no
/// other constraint in the schema can reject them either — the only way this
/// test can fail is over-enforcement (the predicate being ignored), which would
/// falsify W4's "retired shards coexist" model.
#[tokio::test]
async fn test_partial_unique_index_allows_inactive_duplicates() {
    let (config, db) = shards_fixture("cap-partial-inactive").await;

    seed_shard(db.as_ref(), 0, 0).await;
    seed_shard(db.as_ref(), 1, 0).await;

    let inactive = count_rows(
        db.as_ref(),
        "shards WHERE is_active = 0 AND scope = 'project' AND scope_hash = 'abc'",
    )
    .await;
    assert_eq!(
        inactive, 2,
        "two inactive rows with the same (scope, scope_hash) must coexist"
    );
    assert_eq!(
        active_shard_indexes(db.as_ref()).await,
        Vec::<i64>::new(),
        "no row may be active in this fixture"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC5: the rotation shape works inside one write transaction.
///
/// `Db::batch(…, BatchMode::Write)` is `transactional_batch(Immediate)` — the
/// same call the production write path uses — so deactivating the old shard and
/// activating the new one must be accepted as one transaction: the index never
/// sees two active rows, because the intermediate state is inside the
/// transaction.
#[tokio::test]
async fn test_partial_unique_index_allows_the_rotation_sequence_in_one_transaction() {
    let (config, db) = shards_fixture("cap-rotation").await;

    seed_shard(db.as_ref(), 0, 1).await;
    seed_shard(db.as_ref(), 2, 0).await;

    let batched = db
        .batch(
            vec![
                Statement::new(
                    "UPDATE shards SET is_active = 0 WHERE shard_index = 0",
                    vec![],
                ),
                Statement::new(
                    "UPDATE shards SET is_active = 1 WHERE shard_index = 2",
                    vec![],
                ),
            ],
            BatchMode::Write,
        )
        .await;
    let results = batched.expect("the rotation sequence must be accepted as one write transaction");
    assert_eq!(results.len(), 2, "both rotation statements must execute");

    assert_eq!(
        active_shard_indexes(db.as_ref()).await,
        vec![2],
        "after the rotation exactly shard 2 is active"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC6: `BatchMode::Write` is all-or-nothing.
///
/// Rejection mechanism: only statement 2 (the second activation) can fail, and
/// the partial unique index is the mechanism. The fixture guarantees at least
/// one statement succeeds before the failing one — statement 0 deactivates an
/// existing row and statement 1 activates a row while no other active row
/// exists, so neither can violate anything in this schema. After the failure
/// the active set must be `[0]`: a **non-transactional** batch would leave
/// `[2]` behind, which is exactly the discriminator this test exists for.
#[tokio::test]
async fn test_transactional_write_batch_rolls_back_every_statement_on_a_constraint_failure() {
    let (config, db) = shards_fixture("cap-batch-rollback").await;

    seed_shard(db.as_ref(), 0, 1).await;
    seed_shard(db.as_ref(), 2, 0).await;
    seed_shard(db.as_ref(), 3, 0).await;

    let batched = db
        .batch(
            vec![
                Statement::new(
                    "UPDATE shards SET is_active = 0 WHERE shard_index = 0",
                    vec![],
                ),
                Statement::new(
                    "UPDATE shards SET is_active = 1 WHERE shard_index = 2",
                    vec![],
                ),
                Statement::new(
                    "UPDATE shards SET is_active = 1 WHERE shard_index = 3",
                    vec![],
                ),
            ],
            BatchMode::Write,
        )
        .await;
    let error = batched.expect_err(
        "the third statement activates a second row and must be rejected by the partial unique index",
    );
    assert!(
        matches!(error, StoreError::Turso { .. }),
        "the batch failure surfaces as an engine error, got {error:?}"
    );

    assert_eq!(
        active_shard_indexes(db.as_ref()).await,
        vec![0],
        "a transactional batch must roll back statements 0 and 1; [2] here would mean the batch was not transactional"
    );
    assert_eq!(
        shard_state(db.as_ref()).await,
        vec![(0, 1), (2, 0), (3, 0)],
        "every fixture row must be back at its pre-batch state"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC7: a constraint raised inside a write batch loses `is_constraint`.
///
/// `transactional_batch` wraps each statement error as
/// `turso::Error::BatchStatementFailed { index, error, .. }` (or
/// `BatchRollbackFailed { error, rollback_error }` when the rollback itself
/// fails). `StoreError::from` matches only a *top-level*
/// `turso::Error::Constraint`, so both wrappers yield `is_constraint = false`.
/// The test asserts only the classifier, so either wrapper shape passes. A
/// single-statement `execute` unique violation on the same table keeps
/// `is_constraint = true` — that is the contrast, and it is why W4's rider is
/// sound on `execute` while W2's `shard_stats` CHECK (which fires inside a
/// batch) is not.
#[tokio::test]
async fn test_constraint_inside_a_write_batch_loses_the_is_constraint_flag() {
    let (config, db) = shards_fixture("cap-batch-classifier").await;

    seed_shard(db.as_ref(), 0, 1).await;
    seed_shard(db.as_ref(), 2, 0).await;
    seed_shard(db.as_ref(), 3, 0).await;

    let batched = db
        .batch(
            vec![
                Statement::new(
                    "UPDATE shards SET is_active = 0 WHERE shard_index = 0",
                    vec![],
                ),
                Statement::new(
                    "UPDATE shards SET is_active = 1 WHERE shard_index = 2",
                    vec![],
                ),
                Statement::new(
                    "UPDATE shards SET is_active = 1 WHERE shard_index = 3",
                    vec![],
                ),
            ],
            BatchMode::Write,
        )
        .await;
    let wrapped = batched.expect_err(
        "the batch must fail on the second activation (the partial unique index rejects it)",
    );
    assert!(
        is_plain_turso_error(&wrapped),
        "a batch-wrapped constraint must lose the classifier (is_constraint = false), got {wrapped:?}"
    );

    // The same constraint through `execute` keeps its classifier: insert the
    // table-UNIQUE duplicate at shard_index 9 (inactive, so the partial index
    // is not involved).
    db.execute(
        "INSERT INTO shards (scope, scope_hash, shard_index, is_active) VALUES ('project', 'abc', 9, 0)",
        vec![],
    )
    .await
    .expect("seed the row used for the single-statement violation");
    let single = db
        .execute(
            "INSERT INTO shards (scope, scope_hash, shard_index, is_active) VALUES ('project', 'abc', 9, 0)",
            vec![],
        )
        .await
        .expect_err("the table UNIQUE(scope, scope_hash, shard_index) must reject the duplicate");
    assert!(
        is_constraint(&single),
        "a top-level execute constraint keeps is_constraint = true, got {single:?}"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC8: a CHECK violation inside a write batch errors and rolls back.
///
/// Rejection mechanism: `CHECK (vector_count >= 0)` on `probe_stats`, and only
/// that — the positive control inserts `(1, 0)` first, which proves the table
/// exists and the CHECK admits a valid row, so a "no such table" setup error
/// (which also arrives as `BatchStatementFailed`) cannot masquerade as the
/// asserted failure. The batch's first statement inserts into a different,
/// unconstrained table; after the rollback that row must be gone.
#[tokio::test]
async fn test_check_violation_inside_a_write_batch_is_an_error_and_rolls_back() {
    let config = production_config("cap-batch-check");
    let db = open_production(&config, "probe.db").await;

    db.execute(
        concat!(
            "CREATE TABLE probe_stats (id INTEGER PRIMARY KEY CHECK(id = 1), ",
            "vector_count INTEGER NOT NULL CHECK (vector_count >= 0))"
        ),
        vec![],
    )
    .await
    .expect("create probe_stats");
    db.execute("CREATE TABLE probe_memories (id TEXT PRIMARY KEY)", vec![])
        .await
        .expect("create probe_memories");

    // Positive control: the fixture can reach the CHECK and the CHECK admits a
    // valid row.
    db.execute(
        "INSERT INTO probe_stats (id, vector_count) VALUES (1, 0)",
        vec![],
    )
    .await
    .expect("a valid probe_stats row must be accepted");
    db.execute("DELETE FROM probe_stats", vec![])
        .await
        .expect("clear the positive-control row");

    let batched = db
        .batch(
            vec![
                Statement::new("INSERT INTO probe_memories (id) VALUES ('ok')", vec![]),
                Statement::new(
                    "INSERT INTO probe_stats (id, vector_count) VALUES (1, -1)",
                    vec![],
                ),
            ],
            BatchMode::Write,
        )
        .await;
    let error = batched.expect_err("vector_count = -1 must be rejected by the CHECK constraint");
    assert!(
        !is_constraint(&error),
        "a CHECK violation raised inside a batch is not constraint-classified, got {error:?}"
    );

    assert_eq!(
        count_rows(db.as_ref(), "probe_memories").await,
        0,
        "statement 0 must be rolled back with the failed transaction"
    );
    assert_eq!(
        count_rows(db.as_ref(), "probe_stats").await,
        0,
        "the rejected row must not exist"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

// ===========================================================================
// Step 3 — P2 + P3 (AC9–AC10)
// ===========================================================================

/// AC9: `CHECK` constraints, including `json_valid`, are enforced.
///
/// Rejection mechanism: each rejected insert violates exactly one of the three
/// CHECKs and is otherwise well-formed (unique id, nullable metadata, integer
/// count), so the specific CHECK is the only possible source of the `Err`. The
/// final valid insert proves the table does not reject everything — a table
/// that rejects all rows would make the three rejections meaningless.
#[tokio::test]
async fn test_check_constraints_are_enforced_including_json_valid() {
    let config = production_config("cap-check-constraints");
    let db = open_production(&config, "probe.db").await;

    db.execute(
        concat!(
            "CREATE TABLE probe_checks (",
            "id TEXT PRIMARY KEY, ",
            "is_pinned INTEGER DEFAULT 0 CHECK (is_pinned IN (0,1)), ",
            "metadata TEXT CHECK (metadata IS NULL OR json_valid(metadata)), ",
            "vector_count INTEGER NOT NULL CHECK (vector_count >= 0))"
        ),
        vec![],
    )
    .await
    .expect("create probe_checks");

    let bad_json = db
        .execute(
            "INSERT INTO probe_checks (id, is_pinned, metadata, vector_count) VALUES ('bad-json', 0, 'not json', 0)",
            vec![],
        )
        .await
        .expect_err("json_valid(metadata) must reject a non-JSON string");
    assert!(
        is_constraint(&bad_json),
        "the json_valid CHECK rejection must be constraint-classified, got {bad_json:?}"
    );

    let negative = db
        .execute(
            "INSERT INTO probe_checks (id, is_pinned, metadata, vector_count) VALUES ('negative', 0, NULL, -1)",
            vec![],
        )
        .await
        .expect_err("vector_count >= 0 must reject a negative count");
    assert!(
        is_constraint(&negative),
        "the vector_count CHECK rejection must be constraint-classified, got {negative:?}"
    );

    let bad_pin = db
        .execute(
            "INSERT INTO probe_checks (id, is_pinned, metadata, vector_count) VALUES ('bad-pin', 7, NULL, 0)",
            vec![],
        )
        .await
        .expect_err("is_pinned IN (0,1) must reject 7");
    assert!(
        is_constraint(&bad_pin),
        "the is_pinned CHECK rejection must be constraint-classified, got {bad_pin:?}"
    );

    db.execute(
        "INSERT INTO probe_checks (id, is_pinned, metadata, vector_count) VALUES ('valid', 1, '{\"k\":1}', 3)",
        vec![],
    )
    .await
    .expect("a valid row must be accepted (the table does not reject everything)");
    assert_eq!(
        count_rows(db.as_ref(), "probe_checks").await,
        1,
        "exactly the valid row exists"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC10: `DELETE … RETURNING id` reports the deleted ids.
///
/// `Db::query` executes the DELETE (a statement with a result set) and yields
/// the deleted ids, which retention uses to drive counts. The remaining count
/// separates "returned the right ids" from "deleted the right rows".
#[tokio::test]
async fn test_delete_returning_lists_the_deleted_ids() {
    let config = production_config("cap-delete-returning");
    let db = open_production(&config, "probe.db").await;

    db.execute("CREATE TABLE r (id TEXT PRIMARY KEY, v INTEGER)", vec![])
        .await
        .expect("create r");
    db.execute("INSERT INTO r VALUES ('a',1), ('b',2), ('c',3)", vec![])
        .await
        .expect("seed r");

    let rows = db
        .query("DELETE FROM r WHERE v >= 2 RETURNING id", vec![])
        .await
        .expect("DELETE … RETURNING must execute through query");
    let mut deleted: Vec<String> = rows.iter().filter_map(first_column_text).collect();
    deleted.sort();
    assert_eq!(
        deleted,
        vec!["b".to_string(), "c".to_string()],
        "only the rows with v >= 2 may be reported as deleted"
    );
    assert_eq!(
        count_rows(db.as_ref(), "r").await,
        1,
        "exactly the non-matching row remains"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

// ===========================================================================
// Step 4 — P4 secure_delete in the production option set (AC11–AC14)
// ===========================================================================

/// AC11: `PRAGMA secure_delete` is unimplemented — the set is accepted and the
/// read returns no rows.
///
/// This is a control: `Ok` from a pragma **set** is not support. The read's
/// zero rows are the observable; a row here would mean the pragma exists and
/// W6's erasure posture must be revisited.
#[tokio::test]
async fn test_secure_delete_pragma_is_unimplemented_set_ok_read_no_rows() {
    let config = production_config("cap-secure-delete-pragma");
    let db = open_production(&config, "probe.db").await;

    let set = db.execute("PRAGMA secure_delete = ON", vec![]).await;
    assert!(
        set.is_ok(),
        "the engine accepts an unknown pragma set (this `Ok` carries no information): {set:?}"
    );

    let rows = db
        .query("PRAGMA secure_delete", vec![])
        .await
        .expect("reading an unimplemented pragma is not an error");
    assert!(
        rows.is_empty(),
        "PRAGMA secure_delete is unimplemented, so it must return ZERO rows, got {} row(s)",
        rows.len()
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC12: "`Ok` + no rows" carries no information — an unknown pragma has the
/// identical observable shape as `secure_delete`.
///
/// The assertion is the *equality of shapes*, re-observed in one test. If
/// turso starts rejecting unknown pragmas, `Ok` becomes evidence and the
/// diagnostic method can be simplified; this test is the control that notices.
#[tokio::test]
async fn test_unknown_pragma_has_the_same_observable_shape_as_secure_delete() {
    let config = production_config("cap-unknown-pragma");
    let db = open_production(&config, "probe.db").await;

    let bogus_set = db.execute("PRAGMA w0_bogus_pragma = 1", vec![]).await;
    let bogus_rows = db
        .query("PRAGMA w0_bogus_pragma", vec![])
        .await
        .expect("reading an unknown pragma is not an error");
    let secure_set = db.execute("PRAGMA secure_delete = ON", vec![]).await;
    let secure_rows = db
        .query("PRAGMA secure_delete", vec![])
        .await
        .expect("reading an unimplemented pragma is not an error");

    assert!(
        bogus_set.is_ok(),
        "an unknown pragma set is accepted: {bogus_set:?}"
    );
    assert!(
        secure_set.is_ok(),
        "the secure_delete set is accepted: {secure_set:?}"
    );
    assert!(
        bogus_rows.is_empty(),
        "an unknown pragma read returns zero rows, got {} row(s)",
        bogus_rows.len()
    );
    assert_eq!(
        bogus_rows.len(),
        secure_rows.len(),
        "secure_delete and an unknown pragma must be observably identical: Ok + zero rows for both"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC13: the pragma does NOT erase — **control, production option set**.
///
/// This asserts the engine does NOT erase: if it starts erasing this test
/// fails and the erasure design must be revisited.
///
/// Shape of the non-vacuity proof: insert → the canary is on disk; delete +
/// `wal_checkpoint(TRUNCATE)` → the `-wal` is present and 0 bytes while the
/// canary is still in the main file. This test also pins, as a side effect,
/// that `wal_checkpoint(TRUNCATE)` truncates under multiprocess WAL (measured
/// 238 992 → 0 bytes with rows intact), which is why that fact has no 23rd
/// test.
#[tokio::test]
async fn test_secure_delete_pragma_does_not_erase_the_canary_before_vacuum() {
    let config = production_config("cap-secure-delete-erase");
    let db = open_production(&config, "canary.db").await;
    let db_path = config.storage_path.join("canary.db");
    let wal = sidecar(&db_path, "-wal");

    db.execute("PRAGMA secure_delete = ON", vec![])
        .await
        .expect("set secure_delete (accepted, unimplemented)");

    seed_and_delete_canary(db.as_ref(), &config.storage_path).await;
    checkpoint_truncate(db.as_ref()).await;

    assert_eq!(
        file_len(&wal),
        Some(0),
        "the -wal must be PRESENT and truncated to 0 bytes: an absent -wal means the WAL never existed and the assertion would be vacuous"
    );
    assert!(
        file_contains(&db_path, CANARY),
        "PRAGMA secure_delete must not erase: the canary has to survive in the main file after the checkpoint"
    );
    assert_eq!(
        count_rows(db.as_ref(), "user_prompts").await,
        0,
        "the row is deleted — only its bytes remain"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC14: the shipping open does NOT erase — **control, production option set,
/// no pragma at all**.
///
/// Same shape as AC13 without any pragma: this is the configuration that
/// actually ships, so if production were already erasing, this test fails.
/// This asserts the engine does NOT erase: if it starts erasing, this test
/// fails and the erasure design must be revisited.
#[tokio::test]
async fn test_production_open_leaves_the_canary_in_the_main_file_after_checkpoint() {
    let config = production_config("cap-production-erase");
    let db = open_production(&config, "canary.db").await;
    let db_path = config.storage_path.join("canary.db");
    let wal = sidecar(&db_path, "-wal");

    seed_and_delete_canary(db.as_ref(), &config.storage_path).await;
    checkpoint_truncate(db.as_ref()).await;

    assert_eq!(
        file_len(&wal),
        Some(0),
        "the -wal must be PRESENT and truncated to 0 bytes: an absent -wal means the WAL never existed and the assertion would be vacuous"
    );
    assert!(
        file_contains(&db_path, CANARY),
        "the shipping open must not erase: the canary has to survive in the main file after the checkpoint"
    );
    assert_eq!(
        count_rows(db.as_ref(), "user_prompts").await,
        0,
        "the row is deleted — only its bytes remain"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

// ===========================================================================
// Step 5 — VACUUM and the WAL (AC15–AC19)
// ===========================================================================

/// AC15: `VACUUM` is refused in the production configuration — **control**.
///
/// The shipping option set enables multiprocess WAL on non-Windows, and the
/// vacuum feature is not enabled at all, so `VACUUM` is refused at translation
/// with a plain (non-constraint) engine error — the same `StoreError` shape as
/// the `SELEC 1` control. This is why `prompt.rs`'s best-effort VACUUM can
/// never have run. If VACUUM ever succeeds here, that finding is void and this
/// test fails.
#[tokio::test]
async fn test_vacuum_is_refused_in_the_production_configuration() {
    let config = production_config("cap-vacuum-production");
    let db = open_production(&config, "canary.db").await;
    let db_path = config.storage_path.join("canary.db");

    seed_and_delete_canary(db.as_ref(), &config.storage_path).await;
    checkpoint_truncate(db.as_ref()).await;
    assert!(
        file_contains(&db_path, CANARY),
        "the canary must be in the main file before VACUUM, otherwise the survival assertion is vacuous"
    );

    let control = db
        .execute("SELEC 1", vec![])
        .await
        .expect_err("SELEC 1 is a parse error (the shape control)");
    assert!(
        is_plain_turso_error(&control),
        "the parse-error control must be a plain Turso error, got {control:?}"
    );

    let vacuum = db
        .execute("VACUUM", vec![])
        .await
        .expect_err("VACUUM must be refused in the shipping configuration");
    assert!(
        is_plain_turso_error(&vacuum),
        "the VACUUM refusal must have the same StoreError shape as SELEC 1, got {vacuum:?}"
    );

    assert!(
        file_contains(&db_path, CANARY),
        "a refused VACUUM must not touch the file: the canary is still in the main file"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC16: `experimental_vacuum(true)` VACUUM erases the canary —
/// **non-production** handle, deliberate capability probe.
///
/// With the vacuum flag and **no** multiprocess WAL, VACUUM runs and rewrites
/// the file, so the marker is gone from every file under the directory
/// (measured: a compacted 12 288-byte image, a 0-byte `-wal`, and no canary in
/// any file). The follow-up `SELECT` proves the rewrite left a readable
/// database rather than a corrupt one. This handle is explicitly not the
/// shipping configuration: `ConnectOptions` cannot express the vacuum flag and
/// this wave must not change that.
#[tokio::test]
async fn test_vacuum_with_the_experimental_flag_erases_the_canary() {
    let dir = common::temp_dir("cap-vacuum-flag");
    let path = dir.join("vacuum.db");
    let (database, connection) = open_with_vacuum(&path, false).await;

    raw_seed_and_delete_canary(&connection, &dir).await;
    raw_checkpoint(&connection).await;

    let wal = sidecar(&path, "-wal");
    assert!(
        file_contains(&path, CANARY),
        "the canary must be in the main file before VACUUM, otherwise the absence assertion is vacuous"
    );
    assert_eq!(
        file_len(&wal),
        Some(0),
        "the -wal must be present and truncated before VACUUM"
    );

    let vacuum = connection.execute("VACUUM", ()).await;
    assert!(
        vacuum.is_ok(),
        "with experimental_vacuum(true) and no multiprocess WAL, VACUUM must run: {vacuum:?}"
    );

    assert!(
        !dir_contains_canary(&dir),
        "VACUUM must have removed the marker's bytes from every file under the directory"
    );
    assert_eq!(
        raw_count(&connection, "user_prompts").await,
        0,
        "the database must still answer after VACUUM (the file is not corrupt)"
    );

    drop(connection);
    drop(database);
    common::cleanup_temp_dir(&dir).expect("cleanup temp dir");
}

/// AC17: the experimental flag does NOT make VACUUM work under multiprocess
/// WAL — **the exact shape W6's "one flag" remedy would create**.
///
/// Measured refusal: `VACUUM is incompatible with experimental multiprocess
/// WAL`, surfaced as a plain (non-constraint) engine error with the same
/// `StoreError` shape as the `SELEC 1` control; the canary is still present
/// afterwards. Production always enables multiprocess WAL on non-Windows, so
/// the design's W6 remedy cannot work until the owner decides whether
/// multiprocess WAL is dropped. This test is the evidence, not the fix.
#[tokio::test]
async fn test_vacuum_with_the_flag_is_refused_when_multiprocess_wal_is_enabled() {
    let dir = common::temp_dir("cap-vacuum-mpwal");
    let path = dir.join("vacuum.db");
    let (database, connection) = open_with_vacuum(&path, true).await;

    raw_seed_and_delete_canary(&connection, &dir).await;
    raw_checkpoint(&connection).await;
    assert!(
        file_contains(&path, CANARY),
        "the canary must be in the main file before VACUUM, otherwise the survival assertion is vacuous"
    );

    let control = connection
        .execute("SELEC 1", ())
        .await
        .expect_err("SELEC 1 is a parse error (the shape control)");
    let control = StoreError::from(control);
    assert!(
        is_plain_turso_error(&control),
        "the parse-error control must be a plain Turso error, got {control:?}"
    );

    let vacuum = connection
        .execute("VACUUM", ())
        .await
        .expect_err("VACUUM must be refused when the vacuum flag meets multiprocess WAL");
    let vacuum = StoreError::from(vacuum);
    assert!(
        is_plain_turso_error(&vacuum),
        "the VACUUM refusal must have the same StoreError shape as SELEC 1, got {vacuum:?}"
    );

    assert!(
        file_contains(&path, CANARY),
        "a refused VACUUM must not touch the file: the canary is still in the main file"
    );

    drop(connection);
    drop(database);
    common::cleanup_temp_dir(&dir).expect("cleanup temp dir");
}

/// AC18: `wal_checkpoint(TRUNCATE)` truncates the WAL and keeps the rows —
/// production handle.
///
/// The pre-condition (`-wal` exists with a non-zero length) is what makes the
/// post-condition meaningful; the row count guards against a "truncation" that
/// lost data.
#[tokio::test]
async fn test_wal_checkpoint_truncate_zeroes_the_wal_and_keeps_the_rows() {
    let config = production_config("cap-checkpoint-truncate");
    let db = open_production(&config, "pad.db").await;
    let db_path = config.storage_path.join("pad.db");
    let wal = sidecar(&db_path, "-wal");

    db.execute(
        "CREATE TABLE probe_pad (id INTEGER PRIMARY KEY, v TEXT)",
        vec![],
    )
    .await
    .expect("create probe_pad");
    for index in 0..50_i64 {
        db.execute(
            "INSERT INTO probe_pad (id, v) VALUES (?, ?)",
            vec![Value::Integer(index), Value::Text("p".repeat(200))],
        )
        .await
        .expect("insert a pad row");
    }

    let before = file_len(&wal);
    assert!(
        before.is_some_and(|len| len > 0),
        "the -wal must exist with a non-zero length before the checkpoint, got {before:?}"
    );

    checkpoint_truncate(db.as_ref()).await;

    assert_eq!(
        file_len(&wal),
        Some(0),
        "wal_checkpoint(TRUNCATE) must truncate the -wal to 0 bytes (an absent -wal is a failure, not a pass)"
    );
    assert_eq!(
        count_rows(db.as_ref(), "probe_pad").await,
        50,
        "the checkpoint must keep every inserted row"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC19: `PRAGMA wal_autocheckpoint` is unimplemented — **control**, zero rows.
///
/// If rows ever appear, the engine checkpoints automatically and the WAL-sizing
/// premise in the design changes.
#[tokio::test]
async fn test_wal_autocheckpoint_pragma_returns_no_rows() {
    let config = production_config("cap-autocheckpoint");
    let db = open_production(&config, "probe.db").await;

    let rows = db
        .query("PRAGMA wal_autocheckpoint", vec![])
        .await
        .expect("reading an unimplemented pragma is not an error");
    assert!(
        rows.is_empty(),
        "PRAGMA wal_autocheckpoint is unimplemented, so it must return ZERO rows, got {} row(s)",
        rows.len()
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

// ===========================================================================
// Step 6 — P7 + P8 (AC20–AC21)
// ===========================================================================

/// AC20: `PRAGMA table_info` is a discriminating column probe, and the
/// table-valued spelling agrees with it.
///
/// Both forms must report exactly `{id, content}` **before** the ALTER and not
/// `provider_id` — the present-assertion is what makes the absent-assertion
/// non-vacuous (a probe that returned nothing would otherwise "prove" absence).
/// After `ALTER TABLE … ADD COLUMN provider_id TEXT` both forms must report
/// `{id, content, provider_id}` and still agree.
#[tokio::test]
async fn test_table_info_lists_existing_columns_and_omits_missing_ones() {
    let config = production_config("cap-table-info");
    let db = open_production(&config, "probe.db").await;

    db.execute(
        "CREATE TABLE probe_table (id TEXT PRIMARY KEY, content TEXT NOT NULL)",
        vec![],
    )
    .await
    .expect("create probe_table");

    let before_pragma = column_names(db.as_ref(), "PRAGMA table_info(probe_table)", "name").await;
    let before_function = column_names(
        db.as_ref(),
        "SELECT name FROM pragma_table_info('probe_table')",
        "name",
    )
    .await;
    assert_eq!(
        before_pragma,
        vec!["content".to_string(), "id".to_string()],
        "PRAGMA table_info must report exactly the two existing columns"
    );
    assert_eq!(
        before_pragma, before_function,
        "both table_info spellings must agree"
    );
    assert!(
        !before_pragma.iter().any(|name| name == "provider_id"),
        "provider_id does not exist before the ALTER"
    );

    db.execute(
        "ALTER TABLE probe_table ADD COLUMN provider_id TEXT",
        vec![],
    )
    .await
    .expect("ALTER TABLE ADD COLUMN must succeed on a fresh column");

    let after_pragma = column_names(db.as_ref(), "PRAGMA table_info(probe_table)", "name").await;
    let after_function = column_names(
        db.as_ref(),
        "SELECT name FROM pragma_table_info('probe_table')",
        "name",
    )
    .await;
    assert_eq!(
        after_pragma,
        vec![
            "content".to_string(),
            "id".to_string(),
            "provider_id".to_string()
        ],
        "PRAGMA table_info must report the added column"
    );
    assert_eq!(
        after_pragma, after_function,
        "both table_info spellings must still agree after the ALTER"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// AC21: a duplicate ADD COLUMN is variant-identical to a syntax error.
///
/// Rejection mechanisms, bracketed on both sides: `SELEC 1` is the parse-error
/// control, a duplicate primary-key insert is the constraint control, and the
/// second `ALTER TABLE … ADD COLUMN provider_id` is the case under test. The
/// duplicate and the syntax error must both be
/// `StoreError::Turso { is_constraint: false, .. }`; only the unique violation
/// may carry `is_constraint = true`. No variant or field separates a
/// duplicate-column error from a syntax error, and this file never asserts
/// message text, so the only remaining discriminator is the message — which the
/// design forbids; hence `PRAGMA table_info` (AC20).
#[tokio::test]
async fn test_duplicate_add_column_error_is_indistinguishable_from_a_parse_error() {
    let config = production_config("cap-duplicate-column");
    let db = open_production(&config, "probe.db").await;

    db.execute(
        "CREATE TABLE probe_table (id TEXT PRIMARY KEY, content TEXT NOT NULL)",
        vec![],
    )
    .await
    .expect("create probe_table");

    let first = db
        .execute(
            "ALTER TABLE probe_table ADD COLUMN provider_id TEXT",
            vec![],
        )
        .await;
    assert!(
        first.is_ok(),
        "the first ADD COLUMN must succeed: {first:?}"
    );

    let duplicate = db
        .execute(
            "ALTER TABLE probe_table ADD COLUMN provider_id TEXT",
            vec![],
        )
        .await
        .expect_err("a second ADD COLUMN of the same name must be rejected");
    let syntax = db
        .execute("SELEC 1", vec![])
        .await
        .expect_err("SELEC 1 is a parse error (the bracket control)");

    assert!(
        is_plain_turso_error(&duplicate),
        "the duplicate-column error must be a plain Turso error, got {duplicate:?}"
    );
    assert!(
        is_plain_turso_error(&syntax),
        "the syntax error must be a plain Turso error, got {syntax:?}"
    );

    db.execute(
        "INSERT INTO probe_table (id, content, provider_id) VALUES ('dup', 'x', NULL)",
        vec![],
    )
    .await
    .expect("seed the row used for the constraint control");
    let constraint = db
        .execute(
            "INSERT INTO probe_table (id, content, provider_id) VALUES ('dup', 'y', NULL)",
            vec![],
        )
        .await
        .expect_err("the duplicate primary key must be rejected");
    assert!(
        is_constraint(&constraint),
        "the unique violation must be the only constraint-classified error here, got {constraint:?}"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

// ===========================================================================
// Step 7 — P9 ATTACH (AC22)
// ===========================================================================

/// AC22: `ATTACH` is refused — **control, production option set**.
///
/// This asserts the engine does NOT support ATTACH in the shipping
/// configuration, hence no cross-file transaction — which is why ADR-0020's
/// reverse-order backup and ADR-0018's in-shard `shard_stats` are the available
/// mechanisms. The attach target is a **real on-disk path** (not `:memory:`, so
/// a memory-specific refusal cannot make the control pass while file attach
/// would work) and the post-condition pins that the refused ATTACH created no
/// file. The `SELEC 1` control brackets the shape.
#[tokio::test]
async fn test_attach_is_refused() {
    let config = production_config("cap-attach");
    let db = open_production(&config, "main.db").await;
    let target = config.storage_path.join("attached.db");

    let attach_sql = format!("ATTACH DATABASE '{}' AS other", target.display());
    let control = db
        .execute("SELEC 1", vec![])
        .await
        .expect_err("SELEC 1 is a parse error (the shape control)");
    assert!(
        is_plain_turso_error(&control),
        "the parse-error control must be a plain Turso error, got {control:?}"
    );

    let attach = db
        .execute(&attach_sql, vec![])
        .await
        .expect_err("ATTACH must be refused in the production configuration");
    assert!(
        is_plain_turso_error(&attach),
        "the ATTACH refusal must have the same StoreError shape as SELEC 1, got {attach:?}"
    );
    assert!(
        !target.exists(),
        "a refused ATTACH must not create the target file at {}",
        target.display()
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

// ===========================================================================
// Step 8 — W6 probe: the single-process path (multiprocess WAL OFF)
// ===========================================================================
//
// This section answers one question the erasure decision is waiting on: can
// the port run with `experimental_multiprocess_wal` disabled? It is a
// **measurement**, not a port: no production file is touched, and the only
// new code is test-local plumbing that mirrors production minus that one
// feature.
//
// Why the flag is the question at all: `VACUUM` is refused at translation
// while multiprocess WAL is on (AC17), and `VACUUM` is the only mechanism the
// port has that erases deleted bytes. The erasure capability itself is already
// proven (AC16). What is unproven is *ship safety*: every existing user's
// storage root currently holds databases written WITH the flag, so a flag-off
// build must reopen them.

/// Production's builder, minus the multiprocess-WAL feature.
///
/// Fidelity, spelled out because it is the whole point:
/// - `experimental_encryption` is set exactly as `TursoConnectFactory` sets it
///   (on iff key material was resolved, i.e. iff `encryption.is_some()`);
/// - `experimental_multiprocess_wal(true)` is set ONLY when `multiprocess_wal`
///   says so — the flag-off arm is the shipping candidate;
/// - `experimental_vacuum` is NEVER set, because production never sets it.
///   [`open_with_vacuum`] would answer a different question.
///
/// The caller keeps the `Database` alive next to the `Connection`.
async fn open_single_process(
    path: &Path,
    multiprocess_wal: bool,
    encryption: Option<&EncryptionOptions>,
) -> (turso::Database, turso::Connection) {
    try_open_single_process(path, multiprocess_wal, encryption)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "open {} with multiprocess_wal={multiprocess_wal}: {error}",
                path.display()
            )
        })
}

/// The non-panicking twin of [`open_single_process`] (same builder, one
/// definition). Probe B's cells measure whether a second process's open is
/// *refused*, and the refusal text is the datum, so the opener child must be
/// able to report it instead of panicking.
async fn try_open_single_process(
    path: &Path,
    multiprocess_wal: bool,
    encryption: Option<&EncryptionOptions>,
) -> Result<(turso::Database, turso::Connection), String> {
    let mut builder = turso::Builder::new_local(&path.to_string_lossy())
        .experimental_encryption(encryption.is_some());
    if multiprocess_wal {
        builder = builder.experimental_multiprocess_wal(true);
    }
    if let Some(encryption) = encryption {
        builder = builder.with_encryption(turso::EncryptionOpts {
            cipher: encryption.cipher.clone(),
            hexkey: encryption.hexkey.clone(),
        });
    }
    let database = builder.build().await.map_err(|error| error.to_string())?;
    let connection = database.connect().map_err(|error| error.to_string())?;
    Ok((database, connection))
}

/// Sorted `(path relative to `dir`, byte length)` inventory of every regular
/// file under `dir`. This is the probe's file-set observable: which sidecars
/// exist at each phase, and whether a stale one survives a reopen.
fn file_inventory(dir: &Path) -> Vec<(String, u64)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(metadata) = std::fs::metadata(&path) {
                let name = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                out.push((name, metadata.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Text values of one named column from a raw-connection query, in result
/// order. Positional access needs the engine's own column list, so the index
/// is looked up rather than hard-coded.
async fn raw_text_column(connection: &turso::Connection, sql: &str, column: &str) -> Vec<String> {
    let mut rows = connection
        .query(sql, ())
        .await
        .unwrap_or_else(|error| panic!("raw query `{sql}`: {error}"));
    let names = rows.column_names();
    let index = names
        .iter()
        .position(|name| name == column)
        .unwrap_or_else(|| panic!("`{sql}` has no column `{column}`, columns: {names:?}"));
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.expect("read a raw result row") {
        match row.get_value(index).expect("read a raw column value") {
            turso::Value::Text(text) => out.push(text),
            other => panic!("column `{column}` of `{sql}` is not TEXT: {other:?}"),
        }
    }
    out
}

/// Integer values of one named column from a raw-connection query, in result
/// order (the raw twin of [`query_integers`]).
async fn raw_integer_column(connection: &turso::Connection, sql: &str, column: &str) -> Vec<i64> {
    let mut rows = connection
        .query(sql, ())
        .await
        .unwrap_or_else(|error| panic!("raw query `{sql}`: {error}"));
    let names = rows.column_names();
    let index = names
        .iter()
        .position(|name| name == column)
        .unwrap_or_else(|| panic!("`{sql}` has no column `{column}`, columns: {names:?}"));
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.expect("read a raw result row") {
        match row.get_value(index).expect("read a raw column value") {
            turso::Value::Integer(value) => out.push(value),
            other => panic!("column `{column}` of `{sql}` is not an INTEGER: {other:?}"),
        }
    }
    out
}

/// Text of the raw row's OWN first column — the raw-connection twin of
/// [`first_column_text`], for single-value pragmas whose column name is
/// engine-defined (`PRAGMA journal_mode`, `PRAGMA integrity_check`).
async fn raw_first_column_text(connection: &turso::Connection, sql: &str) -> Option<String> {
    let mut rows = connection
        .query(sql, ())
        .await
        .unwrap_or_else(|error| panic!("raw query `{sql}`: {error}"));
    let row = rows.next().await.expect("read a raw result row")?;
    match row.get_value(0).expect("read a raw first column") {
        turso::Value::Text(text) => Some(text),
        other => panic!("the first column of `{sql}` is not TEXT: {other:?}"),
    }
}

/// The fixture written by every probe arm: three shards (one active), a kept
/// prompt row, and a deleted canary. Under the flag-on option set this is what
/// a real storage root looks like.
async fn seed_probe_database(db: &dyn Db, storage: &Path) {
    db.execute(CREATE_SHARDS, vec![])
        .await
        .expect("create the shards fixture table");
    let index = install_one_active_shard_index(db).await;
    assert!(
        index.is_ok(),
        "the one_active_shard partial unique index must install: {index:?}"
    );
    seed_shard(db, 0, 1).await;
    seed_shard(db, 1, 0).await;
    seed_shard(db, 2, 0).await;
    // Order is load-bearing: the kept row is written FIRST, so the canary's
    // `DELETE` is the last write to `user_prompts`. Writing `keep` after the
    // delete makes the insert reuse the cell the delete just freed, overwrite
    // the marker's bytes, and erase the canary from the page image — which
    // leaves the flag-off probe's "not yet in the main file" assertion green
    // even after a checkpoint, i.e. under exactly the regression it names.
    db.execute(CREATE_USER_PROMPTS, vec![])
        .await
        .expect("create user_prompts");
    db.execute(
        "INSERT INTO user_prompts (id, content) VALUES ('keep', 'survives-the-upgrade')",
        vec![],
    )
    .await
    .expect("insert the row that must survive the reopen");
    seed_and_delete_canary(db, storage).await;
}

/// Item 1 — the stale-`-tshm` upgrade path. **The go/no-go for dropping the
/// flag.**
///
/// Shape: write a database through the SHIPPING option set (multiprocess WAL
/// on, i.e. exactly what every current user's storage root holds), close it
/// cleanly, then reopen the SAME file through
/// [`open_single_process`]`(.., false, None)` — production minus the flag.
///
/// Assertions are on data, not on absence of a panic: every row readable, the
/// schema and its index intact, the deleted canary still present (proving the
/// reopen neither erased nor silently rebuilt the file), and the file set
/// coherent at each phase. The inventories are printed so the report can quote
/// them. The closed shape is pinned as a *triple* (main file still ~one page,
/// `-wal` still ~107 KB, the canary in the `-wal` and not in the main file)
/// rather than as a sidecar list: the list is a measurement, but the triple is
/// the premise the erasure recipe rests on, and it must fail if turso ever
/// starts checkpointing on clean close. The third leg only holds because
/// [`seed_probe_database`] writes its kept row *before* the canary, so the
/// delete is the last write and the marker's bytes are still in the page image.
#[tokio::test]
async fn test_flag_on_database_reopens_with_the_flag_off() {
    let config = production_config("cap-single-process-upgrade");
    let db_path = config.storage_path.join("carry.db");
    let tshm = sidecar(&db_path, "-tshm");
    let wal = sidecar(&db_path, "-wal");

    // -- Phase 1: written under the shipping (flag-ON) option set.
    {
        let db = open_production(&config, "carry.db").await;
        seed_probe_database(db.as_ref(), &config.storage_path).await;
        println!(
            "[phase 1 flag-on, open] {:?}",
            file_inventory(&config.storage_path)
        );
        drop(db); // clean close: no crash, no kill
    }
    let closed_inventory = file_inventory(&config.storage_path);
    println!("[phase 1 flag-on, closed] {closed_inventory:?}");
    assert!(
        dir_contains_canary(&config.storage_path),
        "the fixture must be on disk after the clean close, otherwise the reopen assertions are vacuous; files: {closed_inventory:?}"
    );

    // The CLOSED SHAPE, pinned — not merely printed. Without this triple the
    // rest of this test stays green even if turso starts checkpointing on
    // clean close: the reopen would still read every row, but from a main file
    // that already holds the whole database, i.e. the probe would silently be
    // measuring a different (easier) shape and claim 2 — "the deleted bytes
    // live in the `-wal`, and a checkpoint is what moves them into the main
    // file" — would be false while every assertion still passed.
    //
    // Measured on turso 0.8.3: main `carry.db` 4096 (one header page), `-wal`
    // 107 152, `-tshm` 86 016. The bound `< 8192` allows for a page-size or
    // header-page change but not for the fixture: the seeded database cannot
    // fit in one page.
    let main_len =
        file_len(&db_path).expect("the main database file must exist after the clean close");
    assert!(
        main_len < 8192,
        "the main file must still be ~one page after the clean close, i.e. NOTHING was checkpointed into it; got {main_len} bytes, files: {closed_inventory:?}"
    );
    let closed_wal_len = file_len(&wal).expect(
        "the -wal must still exist after the clean close: an absent -wal means the database was checkpointed and the erasure recipe's premise is void",
    );
    assert!(
        closed_wal_len > 100_000,
        "the whole seeded database must still be in the -wal after the clean close (measured 107 152); got {closed_wal_len} bytes, files: {closed_inventory:?}"
    );
    assert!(
        file_contains(&wal, CANARY),
        "the deleted canary's bytes must be IN THE -WAL after the clean close: that is the fact the erasure recipe acts on; files: {closed_inventory:?}"
    );
    assert!(
        !file_contains(&db_path, CANARY),
        "the canary's bytes must NOT yet be in the main file: if a clean close checkpointed, the delete's last-write cell image would have been copied here and this probe would be measuring the post-checkpoint shape without saying so; files: {closed_inventory:?}"
    );

    // -- Phase 2: reopen the SAME file with the flag OFF.
    let (database, connection) = open_single_process(&db_path, false, None).await;
    let open_inventory = file_inventory(&config.storage_path);
    println!("[phase 2 flag-off, open] {open_inventory:?}");

    let journal_mode = raw_first_column_text(&connection, "PRAGMA journal_mode").await;
    println!("[phase 2 flag-off] journal_mode={journal_mode:?}");
    assert_eq!(
        journal_mode.as_deref(),
        Some("wal"),
        "the flag-off open must still be WAL: the erasure recipe is a WAL checkpoint, and a rollback-journal mode would change it"
    );

    // The rows are all readable — the exact fixture, not a count that a
    // partial recovery could also satisfy.
    assert_eq!(
        raw_integer_column(
            &connection,
            "SELECT shard_index, is_active FROM shards ORDER BY shard_index",
            "shard_index"
        )
        .await,
        vec![0, 1, 2],
        "every shard row must be readable with the flag off"
    );
    assert_eq!(
        raw_integer_column(
            &connection,
            "SELECT shard_index, is_active FROM shards ORDER BY shard_index",
            "is_active"
        )
        .await,
        vec![1, 0, 0],
        "every shard's active flag must survive the reopen"
    );
    assert_eq!(
        raw_count(&connection, "user_prompts").await,
        1,
        "the kept prompt row must be readable (the deleted canary row is gone as a row)"
    );
    assert_eq!(
        raw_text_column(
            &connection,
            "SELECT id, content FROM user_prompts",
            "content"
        )
        .await,
        vec!["survives-the-upgrade".to_string()],
        "the kept row's content must be intact, not just its count"
    );

    // The schema is intact: columns and the partial index, both read through
    // the engine's own catalogue.
    assert_eq!(
        raw_text_column(&connection, "PRAGMA table_info(shards)", "name").await,
        vec![
            "id".to_string(),
            "scope".to_string(),
            "scope_hash".to_string(),
            "shard_index".to_string(),
            "is_active".to_string(),
        ],
        "the shards schema must be intact after the flag-off reopen"
    );
    assert!(
        raw_text_column(
            &connection,
            "SELECT name FROM sqlite_master WHERE type = 'index'",
            "name"
        )
        .await
        .contains(&"one_active_shard".to_string()),
        "the one_active_shard partial unique index must survive the reopen"
    );

    // The engine's own corruption check. Hard-asserted, like every other
    // arm in this file: a zero-row answer is a FAILURE here, not a recorded
    // non-answer, because a pragma that stops answering would otherwise make
    // the check vacuous while the test stayed green.
    let integrity = raw_first_column_text(&connection, "PRAGMA integrity_check").await;
    assert_eq!(
        integrity.as_deref(),
        Some("ok"),
        "PRAGMA integrity_check must report the reopened file as ok, got {integrity:?}"
    );

    // The delete is still only a logical delete: the reopen must not have
    // rebuilt the file behind our back.
    assert!(
        dir_contains_canary(&config.storage_path),
        "the deleted canary's bytes must still be present after the flag-off reopen; files: {open_inventory:?}"
    );

    // The reopen must also accept WRITES, not just reads: an upgrade path that
    // only reads would be useless to a running engine.
    connection
        .execute(
            "INSERT INTO shards (scope, scope_hash, shard_index, is_active) VALUES ('project', 'abc', 3, 0)",
            (),
        )
        .await
        .expect("write a shard row through the flag-off reopen");
    connection
        .execute(
            "INSERT INTO user_prompts (id, content) VALUES ('after-upgrade', 'written-with-the-flag-off')",
            (),
        )
        .await
        .expect("write a prompt row through the flag-off reopen");

    drop(connection);
    drop(database);
    let closed_again = file_inventory(&config.storage_path);
    println!("[phase 2 flag-off, closed] {closed_again:?}");

    // Coherence: a rollback journal must never coexist with the WAL, and a
    // `-tshm` left behind must not stop the file reopening (asserted below by
    // the third open).
    assert!(
        !sidecar(&db_path, "-journal").exists(),
        "no rollback journal may coexist with the WAL; files: {closed_again:?}"
    );
    println!(
        "[phase 2] tshm_exists={} wal_len={:?}",
        tshm.exists(),
        file_len(&wal)
    );

    // -- Phase 3: the file set the flag-off close left must still be a valid
    // database for the flag-off build (a stale sidecar that poisoned the next
    // open would show up here), and the writes made through the reopened
    // handle must be durable.
    let (database, connection) = open_single_process(&db_path, false, None).await;
    assert_eq!(
        raw_count(&connection, "shards").await,
        4,
        "a second flag-off open must read the fixture PLUS the shard written through the reopened handle"
    );
    assert_eq!(
        raw_text_column(
            &connection,
            "SELECT id, content FROM user_prompts ORDER BY id",
            "id"
        )
        .await,
        vec!["after-upgrade".to_string(), "keep".to_string()],
        "both the pre-upgrade and the post-upgrade prompt rows must be readable"
    );
    drop(connection);
    drop(database);

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// The `Db` seam opened through the **raw builder** with the multiprocess-WAL
/// feature removed — the handle every `Db`-layer probe (`wal_checkpoint` through
/// the seam, concurrent tasks) must use.
///
/// Not the factory: since issue 45 `TursoConnectFactory::connect` refuses a legacy
/// option set with `UnsupportedOpenMode` (owner decision O3), and these tests
/// measure the *engine* on a flag-off database, so they build the same builder
/// AC16/AC17 use and wrap it in [`RawDb`].
async fn open_single_process_db(config: &StoreConfig, name: &str) -> DbHandle {
    let encryption =
        mem_store::db::resolve_database_encryption(config).expect("resolve encryption options");
    let (database, connection) =
        open_single_process(&config.storage_path.join(name), false, encryption.as_ref()).await;
    let db: DbHandle = std::sync::Arc::new(RawDb {
        database,
        connection,
    });
    db
}

/// The `experimental_vacuum` handle **with encryption on and the flag off** —
/// the option-3 combination. Deliberately raw (`ConnectOptions` cannot express
/// the vacuum flag), deliberately NOT the shipping handle, and named apart from
/// [`open_single_process`] so the vacuum-free fidelity of that helper is not
/// diluted.
async fn open_encrypted_vacuum(
    path: &Path,
    encryption: &EncryptionOptions,
    multiprocess_wal: bool,
) -> (turso::Database, turso::Connection) {
    let mut builder = turso::Builder::new_local(&path.to_string_lossy())
        .experimental_encryption(true)
        .with_encryption(turso::EncryptionOpts {
            cipher: encryption.cipher.clone(),
            hexkey: encryption.hexkey.clone(),
        })
        .experimental_vacuum(true);
    if multiprocess_wal {
        builder = builder.experimental_multiprocess_wal(true);
    }
    let database = builder.build().await.unwrap_or_else(|error| {
        panic!(
            "open encrypted-vacuum {} (multiprocess_wal={multiprocess_wal}): {error}",
            path.display()
        )
    });
    let connection = database
        .connect()
        .expect("connect encrypted-vacuum database");
    (database, connection)
}

/// The row the crash-probe child writes inside an open, never-committed
/// transaction. Distinct from [`CRASH_POST_UPGRADE_ID`] so the absence half and
/// the post-upgrade write can never be confused for one another, and offset far
/// past the committed ids so the whole uncommitted band is one range test.
const CRASH_UNCOMMITTED_ID: i64 = 10_000;

/// The uncommitted row's `v` value. Its presence **on disk** is asserted by the
/// parent while the child is still blocked, which is what makes the later
/// absence assertion non-vacuous (this file's doctrine: an absence assertion
/// only counts after the same helper proved the byte pattern present).
const CRASH_UNCOMMITTED_VALUE: &str = "uncommitted-row-10000-must-not-survive";

/// Filler rows inside the same uncommitted transaction. A small uncommitted
/// write stays in the pager's page cache, where "absent after the crash" proves
/// nothing; enough of them spill frames into the `-wal`, so recovery has
/// uncommitted frames to (wrongly) apply.
///
/// Measured on turso 0.8.3 (this machine, 4096-byte pages): 128 × 8 KiB filler
/// is NOT enough — the marker never reaches a file and the parent's
/// `uncommitted_on_disk` assertion fails, which is the loud signal that the
/// absence half has gone vacuous. 1024 × 8 KiB spills: `-wal` 4 251 872 bytes
/// with the marker present while the child is still blocked on its barrier.
const CRASH_UNCOMMITTED_FILLER_ROWS: i64 = 1024;

/// Payload size of each filler row: comfortably above any plausible page-cache
/// entry, so the spill is dominated by volume rather than by row count.
const CRASH_UNCOMMITTED_FILLER_BYTES: usize = 8192;

/// The row the flag-off parent writes into the crashed flag-on file, proving the
/// upgraded database takes writes. Its id is past every id the child reaches, so
/// a reopened file containing it is unambiguous.
const CRASH_POST_UPGRADE_ID: i64 = 1_000_000;

/// Item 2's writer child. It is INERT unless the parent hands it a database
/// path through `MEM_STORE_CRASH_PROBE_DB`, so a normal suite run pays one
/// no-op test. The protocol is deliberate and has **two** phases:
///
/// 1. `MEM_STORE_CRASH_PROBE_COMMITS` autocommit inserts, announcing each with
///    `COMMITTED <id>` on stdout — the parent's only per-commit synchronisation.
/// 2. `BEGIN; INSERT <uncommitted id>; <filler rows>;` then `OPEN <id>` on
///    stdout, and then it **blocks on stdin**. That is the absence half: at kill
///    time the child holds a row that was written but never committed, and the
///    filler has pushed its frames out to the file set, so a recovery that
///    resurrected uncommitted state is caught. No sleeps anywhere; both barriers
///    are pipes.
///
/// The stdin barrier is only ever released by a bug (the parent kills instead):
/// it then prints `BARRIER-RELEASED` and leaves with code 3, which the parent
/// asserts against, so "the child exited by itself" can never be mistaken for a
/// successful kill.
#[test]
fn test_crash_probe_writer_child() {
    let Some(path) = std::env::var_os("MEM_STORE_CRASH_PROBE_DB") else {
        return;
    };
    let path = PathBuf::from(path);
    let commits: i64 = std::env::var("MEM_STORE_CRASH_PROBE_COMMITS")
        .ok()
        .and_then(|value| value.parse().ok())
        .expect(
            "MEM_STORE_CRASH_PROBE_COMMITS must be set alongside MEM_STORE_CRASH_PROBE_DB: the parent, not the child, decides where the kill lands",
        );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    // `MEM_STORE_CRASH_PROBE_MULTIPROCESS=1` makes the child write under the
    // SHIPPING (flag-on) option set, which is how the upgrade-after-crash case
    // gets its dirty WAL and its possibly-inconsistent `-tshm`.
    let multiprocess_wal =
        std::env::var("MEM_STORE_CRASH_PROBE_MULTIPROCESS").as_deref() == Ok("1");
    runtime.block_on(async move {
        let (_database, connection) = open_single_process(&path, multiprocess_wal, None).await;
        connection
            .execute(
                "CREATE TABLE crash_rows (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
                (),
            )
            .await
            .expect("create crash_rows");
        for id in 0..commits {
            let insert = format!("INSERT INTO crash_rows (id, v) VALUES ({id}, 'row-{id}')");
            connection
                .execute(insert.as_str(), ())
                .await
                .expect("insert a crash-probe row");
            println!("COMMITTED {id}");
        }

        // -- The absence half: written, NOT committed, at the moment of death.
        // The transaction is deliberately large enough to spill into the `-wal`
        // (the parent asserts the marker really is on disk while this read is
        // still pending), because uncommitted frames that reached the file are
        // exactly what a recovery bug would apply.
        connection
            .execute("BEGIN", ())
            .await
            .expect("begin the uncommitted transaction");
        connection
            .execute(
                format!(
                    "INSERT INTO crash_rows (id, v) VALUES ({CRASH_UNCOMMITTED_ID}, '{CRASH_UNCOMMITTED_VALUE}')"
                )
                .as_str(),
                (),
            )
            .await
            .expect("insert the uncommitted row");
        let filler = "u".repeat(CRASH_UNCOMMITTED_FILLER_BYTES);
        for offset in 1..=CRASH_UNCOMMITTED_FILLER_ROWS {
            connection
                .execute(
                    "INSERT INTO crash_rows (id, v) VALUES (?, ?)",
                    vec![
                        turso::Value::Integer(CRASH_UNCOMMITTED_ID + offset),
                        turso::Value::Text(filler.clone()),
                    ],
                )
                .await
                .expect("insert an uncommitted filler row");
        }
        println!("OPEN {CRASH_UNCOMMITTED_ID}");

        // The barrier: the parent kills us while this read is pending.
        let mut released = String::new();
        let _ = std::io::stdin().read_line(&mut released);
        println!("BARRIER-RELEASED");
        std::process::exit(3);
    });
}

/// What [`kill_writer_child_with_an_open_transaction`] observed about the child
/// it killed. A struct rather than a tuple because the last assertion in the
/// upgrade probe needs two of these fields at once and a bare 4-tuple is where
/// "which `i64` was that" bugs come from.
struct CrashedWriter {
    /// Rows `0..acknowledged` are exactly the ones the child committed and
    /// announced — exact because the child stops committing before it opens its
    /// transaction. Item 2 asserts on that exact set. The upgrade probe still
    /// derives its post-write count from the first reopen's own row set
    /// (`recovered.len() + 1`) rather than from this number, so that assertion
    /// does not depend on the pipe protocol at all — even though the barrier
    /// now makes `acknowledged` exact.
    acknowledged: i64,
    /// The id the child inserted inside an open, never-committed transaction.
    uncommitted: i64,
    /// The killed child's exit status.
    status: std::process::ExitStatus,
    /// The file set captured while the child was still alive.
    before_kill: Vec<(String, u64)>,
}

/// Runs [`test_crash_probe_writer_child`] as a separate process over `path`,
/// lets it commit `commits` rows, waits for it to announce its open uncommitted
/// transaction, then `SIGKILL`s it.
///
/// Synchronisation is the child's own two-line protocol on stdout: the parent
/// blocks on `read_line` for `OPEN <id>`, so there is no sleep and no timeout
/// race, and the kill lands on a child that provably holds an uncommitted row.
/// `acknowledged` is therefore exact (the child stops committing before it
/// opens the transaction), which is why item 2's row-set assertion can be an
/// equality rather than a containment.
async fn kill_writer_child_with_an_open_transaction(
    path: &Path,
    multiprocess_wal: bool,
    commits: i64,
) -> CrashedWriter {
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("the running test binary path"));
    command
        .arg("--exact")
        .arg("test_crash_probe_writer_child")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("MEM_STORE_CRASH_PROBE_DB", path)
        .env("MEM_STORE_CRASH_PROBE_COMMITS", commits.to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    if multiprocess_wal {
        command.env("MEM_STORE_CRASH_PROBE_MULTIPROCESS", "1");
    }
    let mut child = command.spawn().expect("spawn the crash-probe writer child");

    let stdout = child.stdout.take().expect("piped child stdout");
    let mut lines = std::io::BufReader::new(stdout).lines();
    let mut acknowledged = 0_i64;
    let mut uncommitted = None;
    for line in lines.by_ref() {
        let line = line.expect("read a child protocol line");
        if let Some(rest) = line.strip_prefix("COMMITTED ") {
            let id: i64 = rest.trim().parse().expect("a commit id");
            acknowledged = id + 1;
        } else if let Some(rest) = line.strip_prefix("OPEN ") {
            uncommitted = Some(rest.trim().parse().expect("an uncommitted row id"));
            break;
        }
    }
    let uncommitted = uncommitted.expect(
        "the child must announce its open uncommitted transaction (`OPEN <id>`) before the parent kills it; without that line there is no absence half and the kill point is a race",
    );
    let dir = path.parent().expect("the database has a parent directory");
    let before_kill = file_inventory(dir);
    // The absence half's non-vacuity proof: the marker's bytes must really be on
    // disk while the child is still blocked inside the open transaction, so the
    // later "the row is absent" assertion is about bytes recovery had to discard
    // rather than about bytes that never left the process.
    let uncommitted_on_disk = before_kill
        .iter()
        .any(|(name, _)| file_contains(&dir.join(name), CRASH_UNCOMMITTED_VALUE));
    child.kill().expect("SIGKILL the writer child");
    let status = child.wait().expect("reap the killed child");
    // A released barrier means the child left on its own and this was not a
    // kill; the parent's assertions would then pass for the wrong reason.
    let released = lines.any(|line| line.map(|line| line == "BARRIER-RELEASED").unwrap_or(false));
    assert!(
        !released,
        "the child released its stdin barrier instead of being SIGKILLed; the kill did not land and this arm measures nothing"
    );
    println!(
        "[crash] acknowledged={acknowledged} uncommitted={uncommitted} uncommitted_on_disk={uncommitted_on_disk} status={status:?} before_kill={before_kill:?}"
    );
    // Asserted after the kill so a failure cannot leave a blocked child behind.
    assert!(
        uncommitted_on_disk,
        "the uncommitted transaction's marker must have reached the file set while the child was still blocked (measured on disk? {uncommitted_on_disk}); if it did not, the absence half of this probe would be about bytes that never left the process. files: {before_kill:?}"
    );
    CrashedWriter {
        acknowledged,
        uncommitted,
        status,
        before_kill,
    }
}

/// Item 2 — committed rows survive a `SIGKILL` on the flag-off path, and the
/// uncommitted row does not.
///
/// Shape: a child process (this test binary re-executed, filtered to
/// [`test_crash_probe_writer_child`]) opens the database with the flag off,
/// commits six rows announcing each on the pipe, then opens a transaction,
/// inserts one row plus [`CRASH_UNCOMMITTED_FILLER_ROWS`] filler rows whose
/// frames the parent proves reached the file set, announces `OPEN` and blocks on
/// stdin. The parent `SIGKILL`s it on that line — never after a coordinated
/// shutdown — while the written-but-uncommitted rows are held open. Then the
/// parent reopens the same file with the flag off.
///
/// Which rows may be asserted on, precisely: the child commits exactly
/// `acknowledged` rows and then stops committing, so the recovered row set must
/// be **exactly** `0..acknowledged`. That is stronger than "the prefix is
/// present" in both directions — a lost commit fails it, and a recovery that
/// resurrected the open transaction's row ([`CRASH_UNCOMMITTED_ID`]) fails it
/// too. The upgrade probe still derives its post-write count from the reopen's
/// own row set, so that assertion does not depend on the pipe protocol at all —
/// even though the barrier now makes `acknowledged` exact there as well.
///
/// Scope of the claim, stated honestly: `SIGKILL` kills the process, not the
/// kernel, so this measures *process-crash* durability (the frames reached the
/// OS), not power-loss durability (fsync). The comment in the assertion says so
/// rather than implying more.
#[tokio::test]
async fn test_committed_rows_survive_a_sigkill_with_the_flag_off() {
    let dir = common::temp_dir("cap-crash-flag-off");
    let path = dir.join("crash.db");

    let crashed = kill_writer_child_with_an_open_transaction(&path, false, 6).await;
    let CrashedWriter {
        acknowledged,
        uncommitted,
        status,
        before_kill,
    } = crashed;
    assert!(
        acknowledged >= 6,
        "the child must have acknowledged at least 6 commits before the kill, got {acknowledged}"
    );
    assert_eq!(
        uncommitted, CRASH_UNCOMMITTED_ID,
        "the killed child must have announced the uncommitted row's id, otherwise this arm has no absence half"
    );
    assert!(
        !status.success(),
        "a SIGKILLed child must not exit successfully, got {status:?}"
    );
    let after_kill = file_inventory(&dir);
    println!("[crash] after_kill={after_kill:?}");

    let (database, connection) = open_single_process(&path, false, None).await;
    println!("[crash] reopened={:?}", file_inventory(&dir));
    let recovered =
        raw_integer_column(&connection, "SELECT id FROM crash_rows ORDER BY id", "id").await;
    println!(
        "[crash] recovered={} rows, max={:?}",
        recovered.len(),
        recovered.last()
    );
    // The presence half: every committed row, against the durability claim.
    for id in 0..acknowledged {
        assert!(
            recovered.contains(&id),
            "committed row {id} must survive a process kill (process-crash durability, not power loss); recovered: {recovered:?}, files before kill: {before_kill:?}"
        );
    }
    // The absence half: the row that was in an OPEN transaction when the
    // process died is not a committed row, so recovery must discard it.
    assert!(
        !recovered.contains(&uncommitted),
        "row {uncommitted} was written inside an uncommitted transaction, so recovering it would mean the reopen applied uncommitted state; recovered: {recovered:?}"
    );
    // …and nothing else at all, which is the strongest form of both halves: the
    // child stops committing before it opens the transaction, so any extra row
    // here is either a resurrected uncommitted write or an invention.
    assert_eq!(
        recovered,
        (0..acknowledged).collect::<Vec<i64>>(),
        "the recovered rows must be EXACTLY the committed prefix 0..{acknowledged}: a missing row is lost durability, an extra row is resurrected uncommitted state"
    );
    let integrity = raw_first_column_text(&connection, "PRAGMA integrity_check").await;
    assert_eq!(
        integrity.as_deref(),
        Some("ok"),
        "the SIGKILLed file must reopen clean, integrity_check said {integrity:?}"
    );

    drop(connection);
    drop(database);
    common::cleanup_temp_dir(&dir).expect("cleanup temp dir");
}

/// Item 3 — the exact sidecar file set per phase, flag on versus flag off.
///
/// Two arms, because "what a flag-off close leaves behind" and "what a
/// flag-on-built file looks like to a flag-off build" are different questions:
/// - arm A: fresh database created and closed through the FLAG-OFF builder;
/// - arm B: fresh database created and closed through the SHIPPING (flag-on)
///   builder, then reopened and closed with the flag off.
///
/// The inventories are printed for the report; the assertions pin the
/// discrimination that matters (`-tshm` vs `-shm`) and that a stale `-tshm`
/// neither breaks the flag-off open nor gets reaped behind the reader's back.
#[tokio::test]
async fn test_sidecar_inventory_per_phase() {
    // -- Arm A: fresh, flag off, from creation to close.
    let dir_a = common::temp_dir("cap-sidecars-flag-off");
    let path_a = dir_a.join("fresh.db");
    {
        let (_database, connection) = open_single_process(&path_a, false, None).await;
        connection
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", ())
            .await
            .expect("create t");
        connection
            .execute("INSERT INTO t (v) VALUES ('x')", ())
            .await
            .expect("insert into t");
        println!("[A flag-off, open+written] {:?}", file_inventory(&dir_a));
    }
    println!("[A flag-off, closed] {:?}", file_inventory(&dir_a));
    let a_closed = file_inventory(&dir_a);
    // Measured: the flag-off path creates NO shared-memory sidecar at all
    // (neither `-shm` nor `-tshm`) — only the main file and `-wal`. Asserting
    // the absence of both is the pin: if a future turso ever mmaps a `-shm`
    // here, this probe's file-set story changes and this test says so.
    assert!(
        !sidecar(&path_a, "-tshm").exists(),
        "a database created with the flag off must not grow a -tshm: {a_closed:?}"
    );
    assert!(
        !sidecar(&path_a, "-shm").exists(),
        "the flag-off path was measured to need no -shm sidecar; if it grew one, the sidecar story changed: {a_closed:?}"
    );
    assert!(
        a_closed.iter().any(|(name, _)| name.ends_with("-wal")),
        "a flag-off database must still be WAL-journalled: {a_closed:?}"
    );
    // …and that `-wal` must be NON-ZERO: existence alone would also be
    // satisfied by a clean close that checkpointed everything into the main
    // file, which is the shape the erasure recipe depends on NOT happening.
    // Measured: main 4096, `-wal` 12 392.
    let a_wal =
        file_len(&sidecar(&path_a, "-wal")).expect("the flag-off -wal exists: asserted above");
    assert!(
        a_wal > 0,
        "the flag-off -wal must be non-empty after a clean close (measured 12 392 bytes): a 0-byte -wal means the close checkpointed, and the recipe's premise changed; {a_closed:?}"
    );

    // -- Arm B: fresh, flag on, then the flag-off upgrade transition.
    let dir_b = common::temp_dir("cap-sidecars-flag-on");
    let path_b = dir_b.join("upgraded.db");
    let config_b = common::store_config("cap-sidecars-flag-on-config");
    // The shipping builder writes straight into arm B's directory.
    {
        let db = turso_connect_factory()
            .connect(
                path_b.clone(),
                ConnectOptions::for_config(&config_b, Platform::current()),
            )
            .await
            .expect("open the shipping handle");
        db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", vec![])
            .await
            .expect("create t");
        db.execute("INSERT INTO t (v) VALUES ('x')", vec![])
            .await
            .expect("insert into t");
        println!("[B flag-on, open+written] {:?}", file_inventory(&dir_b));
        drop(db);
    }
    let b_closed = file_inventory(&dir_b);
    println!("[B flag-on, closed] {b_closed:?}");
    assert!(
        b_closed.iter().any(|(name, _)| name.ends_with("-tshm")),
        "a database written under the flag must carry a -tshm: {b_closed:?}"
    );

    let tshm_before = std::fs::read(sidecar(&path_b, "-tshm")).expect("read the stale -tshm");
    {
        let (_database, connection) = open_single_process(&path_b, false, None).await;
        println!("[B flag-off, open] {:?}", file_inventory(&dir_b));
        assert_eq!(
            raw_count(&connection, "t").await,
            1,
            "the flag-off open must read the row written under the flag"
        );
        assert!(
            sidecar(&path_b, "-tshm").exists(),
            "the stale -tshm must still be there while the flag-off handle is open: {:?}",
            file_inventory(&dir_b)
        );
    }
    let b_upgraded = file_inventory(&dir_b);
    println!("[B flag-off, closed] {b_upgraded:?}");
    assert_eq!(
        std::fs::read(sidecar(&path_b, "-tshm")).ok(),
        Some(tshm_before),
        "closing the flag-off handle must not truncate or rewrite the stale -tshm: {b_upgraded:?}"
    );
    assert_eq!(
        b_upgraded
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
        b_closed
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
        "the flag-off open+close of an upgraded database must leave the SAME file set: it must not add a sidecar (there is no -shm on this path) nor reap the stale -tshm"
    );

    // Can a one-time migration simply DELETE the stale -tshm? Under D-39's
    // engine lock no flag-on process is running, so this is the reaping a
    // migration would do. Measured, not assumed: the file must reopen with all
    // rows readable afterwards.
    std::fs::remove_file(sidecar(&path_b, "-tshm")).expect("reap the stale -tshm");
    {
        let (_database, connection) = open_single_process(&path_b, false, None).await;
        assert_eq!(
            raw_count(&connection, "t").await,
            1,
            "after reaping the stale -tshm, the flag-off build must still read every row"
        );
        assert!(
            !sidecar(&path_b, "-tshm").exists(),
            "the flag-off build must not recreate a -tshm: {:?}",
            file_inventory(&dir_b)
        );
    }
    println!("[B flag-off, tshm reaped] {:?}", file_inventory(&dir_b));

    common::cleanup_temp_dir(&dir_a).expect("cleanup arm A");
    common::cleanup_temp_dir(&dir_b).expect("cleanup arm B");
    common::cleanup_temp_dir(&config_b.storage_path).expect("cleanup arm B config");
}

/// Item 4 — `wal_checkpoint(TRUNCATE)` on the flag-off path, through the `Db`
/// seam (not raw turso): it is step one of the erasure recipe.
///
/// Mirrors AC18's shape so the two are directly comparable: a non-empty `-wal`
/// first (otherwise truncation passes vacuously), rows intact afterwards, and
/// the deleted canary still in the main file (checkpoint alone never erases —
/// that is VACUUM's job, and it is what makes the two steps distinguishable).
#[tokio::test]
async fn test_checkpoint_truncate_on_the_flag_off_db_seam() {
    let config = production_config("cap-single-process-checkpoint");
    let db = open_single_process_db(&config, "pad.db").await;
    let db_path = config.storage_path.join("pad.db");
    let wal = sidecar(&db_path, "-wal");

    db.execute(
        "CREATE TABLE probe_pad (id INTEGER PRIMARY KEY, v TEXT)",
        vec![],
    )
    .await
    .expect("create probe_pad");
    for index in 0..50_i64 {
        db.execute(
            "INSERT INTO probe_pad (id, v) VALUES (?, ?)",
            vec![Value::Integer(index), Value::Text("p".repeat(200))],
        )
        .await
        .expect("insert a pad row");
    }
    seed_and_delete_canary(db.as_ref(), &config.storage_path).await;
    println!(
        "[checkpoint flag-off, written] {:?}",
        file_inventory(&config.storage_path)
    );

    let before = file_len(&wal);
    assert!(
        before.is_some_and(|len| len > 0),
        "the -wal must exist with a non-zero length before the checkpoint, got {before:?}"
    );

    checkpoint_truncate(db.as_ref()).await;

    assert_eq!(
        file_len(&wal),
        Some(0),
        "wal_checkpoint(TRUNCATE) must truncate the -wal to 0 bytes with the flag off (an absent -wal is a failure, not a pass)"
    );
    assert_eq!(
        count_rows(db.as_ref(), "probe_pad").await,
        50,
        "the checkpoint must keep every inserted row"
    );
    assert_eq!(
        count_rows(db.as_ref(), "user_prompts").await,
        0,
        "the canary row stays deleted"
    );
    assert!(
        file_contains(&db_path, CANARY),
        "a checkpoint alone must not erase: the deleted canary's bytes are now in the main file, which is exactly why VACUUM is the second step"
    );
    println!(
        "[checkpoint flag-off, after] {:?}",
        file_inventory(&config.storage_path)
    );

    // Scope question this closes: AC15 refuses VACUUM in the production option
    // set, but that set has BOTH the multiprocess flag on AND
    // `experimental_vacuum` off. Which one does the refusing? Measured here on
    // the flag-off handle through the same `Db` seam.
    //
    // The classifier alone is NOT specific — `is_plain_turso_error` is the
    // shape of any translation-level refusal, identical to the `SELEC 1`
    // parse-error control — so the conclusion is carried by the four-arm
    // matrix, not by the error shape:
    //
    //   | arm            | multiprocess WAL | experimental_vacuum | VACUUM      |
    //   |----------------|------------------|---------------------|-------------|
    //   | AC15           | ON               | OFF                 | refused     |
    //   | AC17           | ON               | ON                  | refused     |
    //   | AC16           | OFF              | ON                  | runs, erases|
    //   | this test      | OFF              | OFF                 | refused     |
    //
    // Read down the last two rows: with multiprocess WAL off, the vacuum flag
    // is the only difference between a working VACUUM and this refusal. Read
    // across the first two: enabling the vacuum flag does not help while
    // multiprocess WAL is on. Both facts together are what says "the flag-off
    // open still needs `experimental_vacuum(true)` for erasure, so dropping
    // the multiprocess flag alone rejects the one-flag remedy" — the shape
    // assert only pins that the refusal is a plain engine error rather than a
    // constraint error.
    let control = db
        .execute("SELEC 1", vec![])
        .await
        .expect_err("SELEC 1 is a parse error (the shape control)");
    assert!(
        is_plain_turso_error(&control),
        "the parse-error control must be a plain Turso error, got {control:?}"
    );

    let vacuum = db.execute("VACUUM", vec![]).await.expect_err(
        "VACUUM must be refused on the flag-off path while the experimental_vacuum feature is off: dropping the multiprocess flag alone does NOT enable erasure, and if this assertion starts failing then it does and the erasure recipe no longer needs the per-open vacuum flag",
    );
    assert!(
        is_plain_turso_error(&vacuum),
        "the flag-off VACUUM refusal must have the same StoreError shape as SELEC 1, got {vacuum:?}"
    );
    println!("[checkpoint flag-off] vacuum_refusal={vacuum:?}");
    assert!(
        file_contains(&db_path, CANARY),
        "a refused VACUUM must not touch the file: the canary is still in the main file"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// Item 5a — encryption on, flag off: open, write, read, reopen.
///
/// Production always enables the encryption *feature*
/// (`ConnectOptions::for_config` pushes `ExperimentalFeature::Encryption`
/// unconditionally) and passes key material whenever the config resolves a
/// key. This arm uses the real resolver (`resolve_database_encryption`) with an
/// explicit hermetic hex key, so no production key path is touched and no key
/// material is faked.
#[tokio::test]
async fn test_encrypted_database_opens_writes_and_reads_with_the_flag_off() {
    let key_hex = common::hex_key_64();
    let config = common::encrypted_store_config("cap-single-process-enc", &key_hex);
    let db_path = config.storage_path.join("sealed.db");
    let encryption =
        mem_store::db::resolve_database_encryption(&config).expect("resolve the encryption key");
    assert!(
        encryption.is_some(),
        "the encrypted fixture must resolve key material, otherwise this test measures nothing"
    );
    let encryption = encryption.expect("encryption options");

    {
        let (database, connection) = open_single_process(&db_path, false, Some(&encryption)).await;
        connection
            .execute(
                "CREATE TABLE prompts (id TEXT PRIMARY KEY, content TEXT NOT NULL)",
                (),
            )
            .await
            .expect("create prompts");
        connection
            .execute(
                "INSERT INTO prompts (id, content) VALUES ('p1', ?)",
                vec![turso::Value::Text(CANARY.to_string())],
            )
            .await
            .expect("insert the canary as an encrypted payload");
        assert_eq!(
            raw_text_column(&connection, "SELECT id, content FROM prompts", "content").await,
            vec![CANARY.to_string()],
            "the encrypted flag-off handle must read back its own write"
        );
        drop(connection);
        drop(database);
    }

    assert!(
        !dir_contains_canary(&config.storage_path),
        "the payload must not be readable in ANY file: encryption is on and the flag-off path must not bypass it"
    );

    let (database, connection) = open_single_process(&db_path, false, Some(&encryption)).await;
    assert_eq!(
        raw_text_column(&connection, "SELECT id, content FROM prompts", "content").await,
        vec![CANARY.to_string()],
        "a second flag-off open with the same key must read the row back"
    );
    let integrity = raw_first_column_text(&connection, "PRAGMA integrity_check").await;
    assert_eq!(
        integrity.as_deref(),
        Some("ok"),
        "the encrypted flag-off file must be intact, integrity_check said {integrity:?}"
    );
    println!(
        "[encryption flag-off, closed] {:?}",
        file_inventory(&config.storage_path)
    );

    drop(connection);
    drop(database);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// Item 5b — VACUUM runs under **encryption on and the flag off**: the
/// option-3 combination, i.e. the erasure recipe as it would ship for an
/// encrypted storage root.
///
/// Observable, stated honestly: on an encrypted database the plaintext marker
/// is unsearchable (that is the point of encryption, and it is what forced the
/// shape below). The byte-level erasure claim is already pinned by AC16 on the
/// unencrypted path; what is *new* here is whether the rewrite even runs with a
/// key. So the observable is the physical image: a large payload is written and
/// checkpointed (file grows), deleted and checkpointed (file does NOT shrink —
/// `secure_delete` is unimplemented, so the pages are retained), then VACUUM
/// must rewrite the file to materially fewer bytes and the database must still
/// answer. A refused VACUUM, or a VACUUM that left the payload's pages in
/// place, both fail.
///
/// Non-production handle by construction (`experimental_vacuum(true)`, which
/// `ConnectOptions` cannot express), labelled exactly like AC16.
#[tokio::test]
async fn test_encrypted_vacuum_rewrites_the_file_with_the_flag_off() {
    let key_hex = common::hex_key_64();
    let config = common::encrypted_store_config("cap-single-process-enc-vacuum", &key_hex);
    let dir = config.storage_path.clone();
    let path = dir.join("sealed.db");
    let encryption = mem_store::db::resolve_database_encryption(&config)
        .expect("resolve the encryption key")
        .expect("the encrypted fixture resolves key material");

    let (database, connection) = open_encrypted_vacuum(&path, &encryption, false).await;
    connection
        .execute(
            "CREATE TABLE payload (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
            (),
        )
        .await
        .expect("create payload");
    raw_checkpoint(&connection).await;
    let base = file_len(&path).expect("the main file exists after the DDL");

    // 40 x 8 KiB rows: comfortably more than the 4 KiB page size, so a
    // rewritten image differs from the deleted one by hundreds of KiB.
    let chunk = "q".repeat(8192);
    for id in 0..40_i64 {
        connection
            .execute(
                "INSERT INTO payload (id, v) VALUES (?, ?)",
                vec![turso::Value::Integer(id), turso::Value::Text(chunk.clone())],
            )
            .await
            .expect("insert a payload row");
    }
    raw_checkpoint(&connection).await;
    let filled = file_len(&path).expect("the main file exists after the writes");
    assert!(
        filled > base + 200_000,
        "the payload must reach the main file before the delete, otherwise the erasure measurement is vacuous: base={base} filled={filled}"
    );

    connection
        .execute("DELETE FROM payload", ())
        .await
        .expect("delete every payload row");
    raw_checkpoint(&connection).await;
    let after_delete = file_len(&path).expect("the main file exists after the delete");
    assert!(
        after_delete + 4096 >= filled,
        "the delete alone must not shrink the file (secure_delete is unimplemented): filled={filled} after_delete={after_delete}"
    );

    let vacuum = connection.execute("VACUUM", ()).await;
    assert!(
        vacuum.is_ok(),
        "VACUUM must run with encryption on and the flag off: {vacuum:?}"
    );
    let vacuumed = file_len(&path).expect("the main file exists after VACUUM");
    println!(
        "[encrypted vacuum] base={base} filled={filled} after_delete={after_delete} vacuumed={vacuumed} files={:?}",
        file_inventory(&dir)
    );
    assert!(
        after_delete.saturating_sub(vacuumed) > 200_000,
        "VACUUM must rewrite the encrypted image without the deleted payload's pages: after_delete={after_delete} vacuumed={vacuumed}"
    );
    assert_eq!(
        raw_count(&connection, "payload").await,
        0,
        "the encrypted database must still answer after VACUUM"
    );

    drop(connection);
    drop(database);
    common::cleanup_temp_dir(&dir).expect("cleanup temp dir");
}

/// Item 6 — concurrent tasks on ONE flag-off pool, on a `current_thread`
/// runtime (the engine's shape: `mem-bridge`'s single runtime).
///
/// Every task shares the same `Arc<dyn Db>`, exactly as the production services
/// do; no task owns a private connection. The assertions are the total row
/// count after every task joined and the absence of any join failure — a
/// serialisation bug shows up as a lost row or a task panic, not as a timing
/// difference, so nothing here is timing-dependent.
#[tokio::test(flavor = "current_thread")]
async fn test_concurrent_tasks_share_one_flag_off_connection() {
    assert_eq!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::CurrentThread,
        "the engine runs a current_thread runtime; this probe must measure that shape"
    );

    let config = production_config("cap-single-process-concurrency");
    let db = open_single_process_db(&config, "shared.db").await;
    db.execute(
        "CREATE TABLE concurrent_rows (id INTEGER PRIMARY KEY AUTOINCREMENT, worker INTEGER NOT NULL, seq INTEGER NOT NULL)",
        vec![],
    )
    .await
    .expect("create concurrent_rows");

    const WORKERS: i64 = 8;
    const ROWS_PER_WORKER: i64 = 20;
    let mut handles = Vec::new();
    for worker in 0..WORKERS {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            for seq in 0..ROWS_PER_WORKER {
                db.execute(
                    "INSERT INTO concurrent_rows (worker, seq) VALUES (?, ?)",
                    vec![Value::Integer(worker), Value::Integer(seq)],
                )
                .await
                .unwrap_or_else(|error| {
                    panic!("worker {worker} insert {seq} failed on the shared pool: {error:?}")
                });
                let rows = db
                    .query(
                        "SELECT COUNT(*) AS written FROM concurrent_rows WHERE worker = ?",
                        vec![Value::Integer(worker)],
                    )
                    .await
                    .unwrap_or_else(|error| {
                        panic!("worker {worker} read {seq} failed on the shared pool: {error:?}")
                    });
                let written = rows
                    .first()
                    .and_then(|row| row.i64("written"))
                    .expect("COUNT(*) yields a row");
                assert_eq!(
                    written,
                    seq + 1,
                    "worker {worker} must see exactly its own committed rows"
                );
            }
        }));
    }
    for (index, handle) in handles.into_iter().enumerate() {
        handle
            .await
            .unwrap_or_else(|error| panic!("worker task {index} panicked: {error:?}"));
    }

    assert_eq!(
        count_rows(db.as_ref(), "concurrent_rows").await,
        WORKERS * ROWS_PER_WORKER,
        "every concurrently inserted row must be present exactly once"
    );
    let per_worker = query_integers(
        db.as_ref(),
        "SELECT COUNT(*) FROM concurrent_rows GROUP BY worker ORDER BY worker",
    )
    .await;
    assert_eq!(
        per_worker,
        vec![ROWS_PER_WORKER; WORKERS as usize],
        "each worker's rows must be complete"
    );

    // The shared pool must also leave a coherent file behind: the same
    // checkpoint the erasure recipe would run, then the engine's own check.
    checkpoint_truncate(db.as_ref()).await;
    let integrity = db
        .query("PRAGMA integrity_check", vec![])
        .await
        .expect("integrity_check is not an error");
    let verdict = integrity.first().and_then(first_column_text);
    assert_eq!(
        verdict.as_deref(),
        Some("ok"),
        "the file after 160 concurrent inserts must be intact, integrity_check said {verdict:?}"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// Extra probe (not one of the seven) — the **reverse** direction: an old
/// flag-ON build opening a database that a flag-OFF build has written, with the
/// stale `-tshm` still lying next to it.
///
/// Nothing in the brief asks for this, but it is the failure mode a user hits
/// by rolling back a release: upgrade (flag off), write, then downgrade (flag
/// on). If the two WAL bookkeeping eras do not interoperate, dropping the flag
/// makes downgrade destructive — a ship risk of the same family as item 1, in
/// the direction nobody would test by accident.
///
/// The cycle is run twice so a one-way fluke cannot pass: flag on → flag off →
/// flag on → flag off, with a row written in each era and every earlier row
/// re-asserted after each transition.
#[tokio::test]
async fn test_round_trip_between_the_flag_on_and_flag_off_paths() {
    let config = production_config("cap-single-process-round-trip");
    let db_path = config.storage_path.join("round-trip.db");

    // -- Era 1: the shipping (flag-on) build creates the database.
    {
        let db = open_production(&config, "round-trip.db").await;
        db.execute(
            "CREATE TABLE rounds (id INTEGER PRIMARY KEY, era TEXT NOT NULL)",
            vec![],
        )
        .await
        .expect("create rounds");
        db.execute(
            "INSERT INTO rounds (id, era) VALUES (1, 'flag-on-1')",
            vec![],
        )
        .await
        .expect("write era 1");
        drop(db);
    }
    println!(
        "[round trip, era 1 closed] {:?}",
        file_inventory(&config.storage_path)
    );

    // -- Era 2: the flag-off build reads era 1 and writes its own row.
    {
        let (_database, connection) = open_single_process(&db_path, false, None).await;
        assert_eq!(
            raw_integer_column(&connection, "SELECT id FROM rounds ORDER BY id", "id").await,
            vec![1],
            "the flag-off build must read the row written by the flag-on build"
        );
        connection
            .execute("INSERT INTO rounds (id, era) VALUES (2, 'flag-off')", ())
            .await
            .expect("write era 2");
        drop(connection);
        drop(_database);
    }
    println!(
        "[round trip, era 2 closed] {:?}",
        file_inventory(&config.storage_path)
    );

    // -- Era 3: a DOWNGRADE to the flag-on build must see both rows.
    {
        let db = open_production(&config, "round-trip.db").await;
        assert_eq!(
            count_rows(db.as_ref(), "rounds").await,
            2,
            "the downgraded flag-on build must read the row the flag-off build committed"
        );
        db.execute(
            "INSERT INTO rounds (id, era) VALUES (3, 'flag-on-2')",
            vec![],
        )
        .await
        .expect("write era 3");
        drop(db);
    }
    println!(
        "[round trip, era 3 closed] {:?}",
        file_inventory(&config.storage_path)
    );

    // -- Era 4: back to flag off; every row from every era is readable.
    let (database, connection) = open_single_process(&db_path, false, None).await;
    assert_eq!(
        raw_text_column(&connection, "SELECT id, era FROM rounds ORDER BY id", "era").await,
        vec![
            "flag-on-1".to_string(),
            "flag-off".to_string(),
            "flag-on-2".to_string(),
        ],
        "every era's row must survive every transition"
    );
    let integrity = raw_first_column_text(&connection, "PRAGMA integrity_check").await;
    assert_eq!(
        integrity.as_deref(),
        Some("ok"),
        "the round-tripped file must be intact, integrity_check said {integrity:?}"
    );
    drop(connection);
    drop(database);
    println!(
        "[round trip, era 4 closed] {:?}",
        file_inventory(&config.storage_path)
    );

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup temp dir");
}

/// Extra probe — **the upgrade as it will actually be experienced**: a flag-ON
/// process crashes (dirty WAL plus a `-tshm` whose shared-memory state was
/// never closed), and the next build opens that file with the flag OFF.
///
/// Item 1 tests the clean-close transition; this tests the dirty one, which is
/// strictly harder: the flag-off engine must replay a WAL produced by the
/// multiprocess writer *and* ignore a shared-memory file that was never
/// coherently unmapped. The child is killed by the same pipe protocol as item 2
/// (no sleeps) while it holds an uncommitted row open, so `before_kill` really
/// is the file set of a live multiprocess writer and the absence half is
/// measured here too.
///
/// Bound discipline: the count after the post-upgrade write is pinned to
/// `recovered.len() + 1`, where `recovered` is the row set the first flag-off
/// reopen actually returned on this run. The bound is derived from the reopen's
/// own row set, so the assertion does not depend on the pipe protocol at all —
/// even though the barrier now makes `acknowledged` exact (the child stops
/// committing before it opens its transaction, so no commit can land between
/// the last line the parent read and the kill).
#[tokio::test]
async fn test_flag_on_crash_then_flag_off_upgrade() {
    let dir = common::temp_dir("cap-crash-then-upgrade");
    let path = dir.join("upgraded.db");

    let crashed = kill_writer_child_with_an_open_transaction(&path, true, 6).await;
    let CrashedWriter {
        acknowledged,
        uncommitted,
        status,
        before_kill,
    } = crashed;
    println!("[upgrade after crash] before_kill={before_kill:?}");
    assert!(
        acknowledged >= 6,
        "the flag-on child must have acknowledged at least 6 commits, got {acknowledged}"
    );
    assert_eq!(
        uncommitted, CRASH_UNCOMMITTED_ID,
        "the killed flag-on child must have announced the uncommitted row's id"
    );
    assert!(
        !status.success(),
        "a SIGKILLed flag-on child must not exit successfully, got {status:?}"
    );
    let after_kill = file_inventory(&dir);
    println!("[upgrade after crash] after_kill={after_kill:?}");
    assert!(
        after_kill.iter().any(|(name, _)| name.ends_with("-tshm")),
        "a flag-on child must have left its -tshm behind; without it this is not the dirty-upgrade case: {after_kill:?}"
    );

    // The upgrade itself: flag off, over the dirty file.
    let (database, connection) = open_single_process(&path, false, None).await;
    println!("[upgrade after crash] reopened={:?}", file_inventory(&dir));
    let recovered =
        raw_integer_column(&connection, "SELECT id FROM crash_rows ORDER BY id", "id").await;
    // The presence half: every commit the child announced must have survived.
    for id in 0..acknowledged {
        assert!(
            recovered.contains(&id),
            "row {id} committed by the flag-on engine must survive the crash AND the flag-off reopen; recovered={recovered:?}"
        );
    }
    // The absence half: the rows held in an open transaction survive the crash
    // only if recovery applies uncommitted WAL frames, which it must not. The
    // band covers all of them — the announced row plus the filler whose frames
    // the parent proved were on disk — and the post-upgrade id is a million, so
    // "inside the band" can never be confused with the later write.
    let uncommitted_band =
        CRASH_UNCOMMITTED_ID..=CRASH_UNCOMMITTED_ID + CRASH_UNCOMMITTED_FILLER_ROWS;
    assert!(
        recovered.iter().all(|id| !uncommitted_band.contains(id)),
        "no row of the open transaction (ids {uncommitted_band:?}, announced {uncommitted}) may be recovered: they were uncommitted at the kill; recovered={recovered:?}"
    );
    let integrity = raw_first_column_text(&connection, "PRAGMA integrity_check").await;
    assert_eq!(
        integrity.as_deref(),
        Some("ok"),
        "the crashed flag-on file must open clean under the flag-off build, integrity_check said {integrity:?}"
    );

    // And the upgraded file must still take writes from the flag-off engine.
    // `CRASH_POST_UPGRADE_ID` is past every id the child ever reaches, so the row
    // is genuinely new rather than a PK collision.
    connection
        .execute(
            format!(
                "INSERT INTO crash_rows (id, v) VALUES ({CRASH_POST_UPGRADE_ID}, 'post-upgrade')"
            )
            .as_str(),
            (),
        )
        .await
        .expect("the flag-off build must be able to write over the crashed flag-on file");
    drop(connection);
    drop(database);

    let (database, connection) = open_single_process(&path, false, None).await;
    let after =
        raw_integer_column(&connection, "SELECT id FROM crash_rows ORDER BY id", "id").await;
    // `recovered.len()` — measured on THIS run by the first reopen — is the
    // baseline for "plus one": the bound is derived from the reopen's own row
    // set, so the assertion does not depend on the pipe protocol at all, even
    // though the barrier now makes `acknowledged` exact.
    assert_eq!(
        after.len(),
        recovered.len() + 1,
        "the post-upgrade write must be durable: expected the {} rows the first reopen actually returned, plus one; got {after:?}",
        recovered.len()
    );
    assert!(
        after.contains(&CRASH_POST_UPGRADE_ID),
        "the post-upgrade row {CRASH_POST_UPGRADE_ID} must be readable after the reopen; got {after:?}"
    );
    assert!(
        !after.contains(&uncommitted),
        "the uncommitted row {uncommitted} must still be absent after the post-upgrade write; got {after:?}"
    );
    drop(connection);
    drop(database);
    println!("[upgrade after crash] final={:?}", file_inventory(&dir));

    common::cleanup_temp_dir(&dir).expect("cleanup temp dir");
}

// ===========================================================================
// Step 8, second wave — the two questions the erasure ADR must settle first
// ===========================================================================
//
// Both probes are measurements, not ports: no production file is touched, and
// both are hermetic (per-test temp dir, no sleeps — every barrier is a pipe, a
// completed engine call, or a handle the caller closes on purpose).
//
// **A.** Can ONE process hold a second `Database` on a live path with
// *different* open options, so the erasure sweep could run beside the engine's
// normal handle? `experimental_vacuum` is a `Builder`-level option
// (`turso::Builder::new_local(..).experimental_vacuum(true)`), so "enable it
// only on the erasure handle" implies exactly that shape. If the engine refuses
// it, or silently hands back the existing database, the erasure path becomes
// close main handle → open with vacuum → sweep → reopen, which changes
// availability during a sweep and must be known before the ADR is written.
//
// **B.** After the multiprocess-WAL flag is dropped, is a flag-off open
// exclusive against a live flag-off holder in another process? `purge-session`
// is a mode of the same binary; today it coordinates with a live engine through
// the operation lock while both hold the file open. If a flag-off open is
// exclusive, that concurrency model changes with no mixed versions involved.
//
// **Non-vacuity, stated per arm.**
// - A's refusal arm is only meaningful because a CONTROL arm runs the same
//   `VACUUM` on a single vacuum-enabled handle in the same process: if the
//   control fails, the refusal is VACUUM's fault, not the second open's.
// - A's DECISIVE arm is the reverse direction: a vacuum-enabled FIRST handle
//   plus a plain second open whose `VACUUM` still runs. The only mechanism that
//   produces that is the engine returning the first `Database` for the path, so
//   the second open's requested option set was never in effect.
// - B's cells are externally controlled: the holder's row set and integrity
//   verdict are asserted after the opener acted, so "the opener was refused"
//   cannot be confused with "the holder quietly broke".

/// How long a probe waits for one engine call or one child protocol line
/// before calling it *blocked*. Not a synchronisation sleep: a successful wait
/// is woken by the pipe line the caller's own last action caused. This only
/// exists so "the engine blocked" becomes a named failure instead of a hung
/// suite — which is itself one of the answers these probes ask for.
const PROBE_GUARD: std::time::Duration = std::time::Duration::from_secs(10);

/// Runs `future` under [`PROBE_GUARD`]; `Err` means "did not return", i.e. the
/// blocked answer.
async fn within_guard<F: std::future::Future>(what: &str, future: F) -> Result<F::Output, String> {
    tokio::time::timeout(PROBE_GUARD, future)
        .await
        .map_err(|_| format!("{what} did not return within {PROBE_GUARD:?} (blocked)"))
}

/// The budget for the one leg that claims an engine call returns *fast*, not
/// merely that it returns: `VACUUM` against a live writer must REFUSE rather
/// than block ([`test_vacuum_is_refused_while_another_connection_holds_a_write_transaction`]).
///
/// Deliberately distinct from [`PROBE_GUARD`]. The guard only turns a hang into a
/// named failure, so a 9-second busy-handler wait would satisfy it while
/// "refuses rather than blocks" became false — the claim would be unfalsifiable.
/// This constant IS the claim: a refusal that takes longer than this fails the
/// leg, and the leg prints the elapsed time it actually measured.
const VACUUM_REFUSAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Probe A — a second `Database` on a live path with different open options
// ---------------------------------------------------------------------------
//
// **Not a statement about lock state.** The inherited-option finding below is
// about the engine's `Database` registry and the option set it hands back, never
// about file locks: a transient second in-process open of a live path releases
// the primary's lock on the `.db` file — the second open takes its own
// descriptor before the registry is consulted and `turso_core`'s
// `impl Drop for UnixFile` unlocks unconditionally, POSIX `fcntl` locks being
// per-process — so after one such open, cross-process exclusivity survives only
// through the `-wal` lock.

/// Probe A, arms 1 and 4: **a second same-path open does not get its own option
/// set**, so there is no vacuum-enabled handle to be had beside a live primary.
///
/// Arm 1 (the ADR's question, forward direction): a normal flag-off handle
/// writes the canary fixture, then the SAME path is opened with
/// `experimental_vacuum(true)` while the primary is still live. Recorded: does
/// the open succeed, and can the second handle run `VACUUM`?
///
/// Arm 2 (baseline): the same path opened a third time with IDENTICAL options
/// to the primary. Without this, arm 1's `Ok` could be blamed on the option
/// difference or on double-opening, and the two conclusions are opposite.
///
/// Arm 3 (control): the same `VACUUM` on a single vacuum-enabled handle — the
/// counterfactual that stops a refusal in arm 1 from being VACUUM's fault.
///
/// Arm 4 (decisive): the REVERSE direction. The vacuum-enabled handle opens
/// first; a plain (vacuum-free) open comes second; `VACUUM` through that plain
/// handle is attempted. If it runs, the second open inherited the FIRST
/// database's option set — the engine handed back the existing instance and the
/// requested options were ignored. That is the only mechanism that can produce
/// it, and it is what makes "not an independent handle" a measurement rather
/// than an inference from the refusal alone.
///
/// Measured on turso 0.8.3, Linux: the second open is `Ok` in both directions,
/// `VACUUM` is refused in arm 1 and RUNS in arm 4, and arm 4 erases the canary.
#[tokio::test]
async fn test_second_handle_with_the_vacuum_flag_on_a_live_path_inherits_the_first_option_set() {
    // -- Arm 1: normal handle first, vacuum-enabled second (the forward case).
    let dir = common::temp_dir("cap-second-handle-forward");
    let path = dir.join("p.db");
    let (primary_database, primary) = open_single_process(&path, false, None).await;
    primary
        .execute(
            "CREATE TABLE probe_rows (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
            (),
        )
        .await
        .expect("create probe_rows");
    primary
        .execute("INSERT INTO probe_rows (id, v) VALUES (1, 'primary')", ())
        .await
        .expect("write through the primary handle");
    raw_seed_and_delete_canary(&primary, &dir).await;
    raw_checkpoint(&primary).await;
    assert!(
        file_contains(&path, CANARY),
        "the deleted canary must be in the main file before the second open, otherwise the later survival assertion is vacuous; files: {:?}",
        file_inventory(&dir)
    );
    println!("[A arm 1, primary live] {:?}", file_inventory(&dir));

    let second = try_open_with_vacuum(&path, false).await;
    println!("[A arm 1] second_open_error={:?}", second.as_ref().err());
    let (second_database, second) = second.unwrap_or_else(|error| {
        panic!(
            "the second open of a LIVE path with `experimental_vacuum(true)` must be measured, not assumed; it returned the error {error:?} instead of a handle. A refusal here is also an ADR answer (no second handle at all), so read this failure as a decision point rather than a test fix; files: {:?}",
            file_inventory(&dir)
        )
    });

    // The second handle's own answer: can it vacuum while the primary is live?
    let vacuum = second.execute("VACUUM", ()).await;
    println!(
        "[A arm 1] vacuum_on_second_error={:?}",
        vacuum.as_ref().err()
    );
    let vacuum_error = vacuum.expect_err(
        "the second handle asked the engine for `experimental_vacuum(true)` and VACUUM was REFUSED here. If this assertion starts failing, VACUUM ran on a second handle while the primary was still live: a same-process second open then carries its own option set, the erasure sweep can keep the engine's handle open, and the ADR's close-reopen branch disappears — an ADR-level change.",
    );
    println!("[A arm 1] vacuum_refusal_text={vacuum_error}");
    assert!(
        file_contains(&path, CANARY),
        "a refused VACUUM must not have touched the file: the canary is still in the main file; files: {:?}",
        file_inventory(&dir)
    );

    // The second open IS a live handle on the same database (reads the
    // primary's row, its writes are visible to the primary) — the point of the
    // arm is that it is a handle WITHOUT the options it asked for.
    assert_eq!(
        raw_count(&second, "probe_rows").await,
        1,
        "the second open must at least read the primary's committed row, otherwise 'the options were ignored' and 'the open is useless' would be indistinguishable"
    );
    second
        .execute("INSERT INTO probe_rows (id, v) VALUES (2, 'second')", ())
        .await
        .expect("write through the second handle");
    assert_eq!(
        raw_count(&primary, "probe_rows").await,
        2,
        "a write through the second handle must be visible to the primary handle: that is what 'one database' looks like, and it is why the missing option set is the finding rather than a failed open"
    );

    // The primary must still read AND write after a refused VACUUM (nothing
    // ran, so this is the "not left in a broken state" leg, not an erasure leg).
    primary
        .execute(
            "INSERT INTO probe_rows (id, v) VALUES (3, 'primary-after')",
            (),
        )
        .await
        .expect("the primary must still take writes after the refused VACUUM");
    assert_eq!(
        raw_integer_column(&primary, "SELECT id FROM probe_rows ORDER BY id", "id").await,
        vec![1, 2, 3],
        "the primary's view must be complete after the second handle's refused VACUUM"
    );
    assert_eq!(
        raw_first_column_text(&primary, "PRAGMA integrity_check")
            .await
            .as_deref(),
        Some("ok"),
        "the file must still be intact after the second open and the refused VACUUM"
    );

    // -- Arm 2: the baseline. Same path, IDENTICAL options.
    let baseline = try_open_single_process(&path, false, None).await;
    println!(
        "[A arm 2] baseline_second_open_error={:?}",
        baseline.as_ref().err()
    );
    assert!(
        baseline.is_ok(),
        "a second open of the SAME path with IDENTICAL options must be Ok, otherwise arm 1's Ok is attributable to the option difference rather than to double-opening (the opposite conclusion); got {:?}",
        baseline.as_ref().err()
    );
    drop(baseline);

    // -- Arm 3: the control. One handle, vacuum enabled, nothing else live.
    let control_dir = common::temp_dir("cap-second-handle-control");
    let control_path = control_dir.join("c.db");
    let (control_database, control) = open_with_vacuum(&control_path, false).await;
    control
        .execute(
            "CREATE TABLE probe_rows (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
            (),
        )
        .await
        .expect("create the control's probe_rows");
    control
        .execute("INSERT INTO probe_rows (id, v) VALUES (1, 'control')", ())
        .await
        .expect("write the control's row");
    let control_vacuum = control.execute("VACUUM", ()).await;
    assert!(
        control_vacuum.is_ok(),
        "CONTROL: the same VACUUM on a single handle opened with `experimental_vacuum(true)` must run on this build; if it does not, arm 1's refusal is VACUUM's fault, not the second open's: {control_vacuum:?}"
    );
    drop(control);
    drop(control_database);
    common::cleanup_temp_dir(&control_dir).expect("cleanup the control arm");

    // -- Arm 4: the decisive reverse direction.
    let reverse_dir = common::temp_dir("cap-second-handle-reverse");
    let reverse_path = reverse_dir.join("r.db");
    let (vacuum_database, vacuum_first) = open_with_vacuum(&reverse_path, false).await;
    vacuum_first
        .execute(
            "CREATE TABLE probe_rows (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
            (),
        )
        .await
        .expect("create the reverse arm's probe_rows");
    vacuum_first
        .execute("INSERT INTO probe_rows (id, v) VALUES (1, 'vacuum')", ())
        .await
        .expect("write through the vacuum-enabled first handle");
    raw_seed_and_delete_canary(&vacuum_first, &reverse_dir).await;
    raw_checkpoint(&vacuum_first).await;
    assert!(
        file_contains(&reverse_path, CANARY),
        "the canary must be in the reverse arm's main file before the second open"
    );

    let plain = try_open_single_process(&reverse_path, false, None).await;
    println!(
        "[A arm 4] plain_second_open_error={:?}",
        plain.as_ref().err()
    );
    let (_plain_database, plain) = plain.unwrap_or_else(|error| {
        panic!(
            "the second, vacuum-FREE open of a live path must return a handle (arm 1 already showed the same-path open is accepted): {error:?}"
        )
    });
    let inherited = plain.execute("VACUUM", ()).await;
    println!(
        "[A arm 4] vacuum_on_the_vacuum_free_second_handle_error={:?}",
        inherited.as_ref().err()
    );
    assert!(
        inherited.is_ok(),
        "DECISIVE ARM: this second open asked for NO vacuum feature, yet VACUUM ran through it. The only mechanism that produces that is the engine handing back the FIRST (vacuum-enabled) Database for this path: the requested option set was never in effect, so a same-process second open is not an independent handle. If this ever fails, same-path second opens have become independent and the erasure design can keep the primary open."
    );
    assert!(
        !dir_contains_canary(&reverse_dir),
        "the VACUUM that ran through the second connection must have erased the deleted canary from every file under the directory — which is what says the inherited handle is a WORKING vacuum handle, not merely one that accepted the statement; files: {:?}",
        file_inventory(&reverse_dir)
    );
    assert_eq!(
        raw_count(&vacuum_first, "user_prompts").await,
        0,
        "the vacuum-enabled first handle must still answer after the VACUUM its second connection ran"
    );
    assert_eq!(
        raw_integer_column(&vacuum_first, "SELECT id FROM probe_rows", "id").await,
        vec![1],
        "the first handle must read the REWRITTEN image, not a stale pre-VACUUM page cache: its kept row is still there"
    );
    assert_eq!(
        raw_first_column_text(&vacuum_first, "PRAGMA integrity_check")
            .await
            .as_deref(),
        Some("ok"),
        "the file must be intact after the inherited-handle VACUUM"
    );

    drop(plain);
    drop(_plain_database);
    drop(second);
    drop(second_database);
    drop(primary);
    drop(primary_database);
    drop(vacuum_first);
    drop(vacuum_database);
    common::cleanup_temp_dir(&reverse_dir).expect("cleanup the reverse arm");
    common::cleanup_temp_dir(&dir).expect("cleanup the forward arm");
}

/// Probe A, arm 5 — the fallback the ADR would have to adopt: **close the
/// primary, open with the vacuum flag, sweep, reopen.**
///
/// This is the design's cost made measurable: the sweep is only reachable with
/// the engine's handle closed, so availability during a sweep is a design
/// decision, not an implementation detail. The arm therefore asserts the whole
/// cycle, not just that VACUUM ran: canary present in the main file before,
/// erased from every file after, every kept row readable through the reopened
/// flag-off handle, and the engine's own integrity verdict.
#[tokio::test]
async fn test_close_then_reopen_with_the_vacuum_flag_erases_and_reopens_clean() {
    let dir = common::temp_dir("cap-erasure-close-reopen");
    let path = dir.join("p.db");

    {
        let (database, connection) = open_single_process(&path, false, None).await;
        connection
            .execute(
                "CREATE TABLE probe_rows (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
                (),
            )
            .await
            .expect("create probe_rows");
        for id in 0..8_i64 {
            connection
                .execute(
                    "INSERT INTO probe_rows (id, v) VALUES (?, ?)",
                    vec![
                        turso::Value::Integer(id),
                        turso::Value::Text(format!("kept-row-{id}")),
                    ],
                )
                .await
                .expect("write a kept row");
        }
        raw_seed_and_delete_canary(&connection, &dir).await;
        raw_checkpoint(&connection).await;
        drop(connection);
        drop(database);
    }
    println!("[erase close-reopen, closed] {:?}", file_inventory(&dir));
    assert!(
        file_contains(&path, CANARY),
        "the deleted canary's bytes must be in the main file after the close, otherwise the erase assertion is vacuous; files: {:?}",
        file_inventory(&dir)
    );

    // The sweep itself: a fresh, single vacuum-enabled handle on the closed
    // file — the only shape Probe A's arm 4 showed is reachable.
    {
        let (database, connection) = open_with_vacuum(&path, false).await;
        connection
            .execute("VACUUM", ())
            .await
            .expect("VACUUM must run on a closed-then-reopened file with the vacuum flag");
        drop(connection);
        drop(database);
    }
    println!("[erase close-reopen, vacuumed] {:?}", file_inventory(&dir));
    assert!(
        !dir_contains_canary(&dir),
        "VACUUM must have removed the marker's bytes from every file under the directory"
    );

    let (database, connection) = open_single_process(&path, false, None).await;
    assert_eq!(
        raw_integer_column(&connection, "SELECT id FROM probe_rows ORDER BY id", "id").await,
        (0..8).collect::<Vec<i64>>(),
        "every kept row must be readable through the reopened flag-off handle"
    );
    assert_eq!(
        raw_count(&connection, "user_prompts").await,
        0,
        "the deleted canary row stays deleted"
    );
    assert_eq!(
        raw_first_column_text(&connection, "PRAGMA integrity_check")
            .await
            .as_deref(),
        Some("ok"),
        "the rewritten file must be intact for the flag-off build"
    );
    drop(connection);
    drop(database);

    common::cleanup_temp_dir(&dir).expect("cleanup temp dir");
}

// ---------------------------------------------------------------------------
// Probe B — is a flag-off open exclusive against a live flag-off holder?
// ---------------------------------------------------------------------------
//
// Orientation: the HOLDER is this test process (it must be, so its row set and
// integrity verdict can be read directly), and the OPENER is a child process
// re-executing this test binary, filtered to [`test_lock_probe_opener_child`].
// The opener is the child because a mixed-mode second open is the call most
// likely to fail *inside* the engine, and a panic there must be one cell's
// datum, not the whole suite's.

const LOCK_PROBE_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS lock_probe (id INTEGER PRIMARY KEY, writer TEXT NOT NULL)";

/// The holder's committed seed row: the row that must still be readable after
/// the opener has done whatever it does.
const LOCK_SEED_ID: i64 = 1;

/// The row the holder writes (and leaves UNCOMMITTED) in the mid-write cells.
const LOCK_MID_WRITE_ID: i64 = 2;

/// The row the holder writes AFTER the opener's attempt — the "does the holder
/// still take writes" leg, and the row the opener's second read looks for.
const LOCK_HOLDER_AFTER_ID: i64 = 3;

/// The row the opener writes. Distinct from every holder row so "the holder
/// sees the opener's data" and "the opener sees the holder's data" are two
/// different assertions rather than one count.
const LOCK_OPENER_ID: i64 = 100;

/// `ids` as the child protocol carries them (`1,2,100`).
fn join_ids(ids: &[i64]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Ids in `lock_probe`, or the engine's error text. Deliberately tolerant: the
/// mixed cells must be able to REPORT a failed read (that is the finding), so
/// this twin of [`raw_integer_column`] returns the error instead of panicking.
async fn lock_probe_ids(connection: &turso::Connection) -> Result<Vec<i64>, String> {
    let mut rows = connection
        .query("SELECT id FROM lock_probe ORDER BY id", ())
        .await
        .map_err(|error| error.to_string())?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        match row.get_value(0).map_err(|error| error.to_string())? {
            turso::Value::Integer(id) => ids.push(id),
            other => return Err(format!("lock_probe.id is not an INTEGER: {other:?}")),
        }
    }
    Ok(ids)
}

/// The engine's own verdict, tolerant for the same reason as
/// [`lock_probe_ids`]. A `None`/zero-row answer is an `Err`, not a silent pass.
async fn lock_probe_integrity(connection: &turso::Connection) -> Result<String, String> {
    let mut rows = connection
        .query("PRAGMA integrity_check", ())
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.next().await.map_err(|error| error.to_string())? else {
        return Err("PRAGMA integrity_check returned no row".to_string());
    };
    match row.get_value(0).map_err(|error| error.to_string())? {
        turso::Value::Text(verdict) => Ok(verdict),
        other => Err(format!(
            "integrity_check's first column is not TEXT: {other:?}"
        )),
    }
}

/// Probe B's opener child. It is INERT unless `MEM_STORE_LOCK_PROBE_DB` is set,
/// so it is `#[ignore]`d: a normal suite run then reports it as one IGNORED test
/// rather than as a green pass that asserted nothing. [`ProbeChild::spawn_opener`]
/// re-executes this binary with `--ignored` and the environment set — that spawn
/// is the only way this entry point is meant to run, and when it does it performs
/// the whole protocol below.
///
/// Protocol (stdout, one line each):
/// - `OPEN-OK` — the open returned a live handle;
/// - `OPEN-ERR <engine text>` — the open was refused, text verbatim;
/// - `OPEN-BLOCKED <text>` — the open did not return inside [`PROBE_GUARD`];
/// - `OPENER-WRITE ok` / `OPENER-WRITE err <text>`;
/// - `OPENER-IDS <csv>` / `OPENER-READ-ERR <text>` — what the opener sees after
///   its own write;
/// - then it blocks on stdin (the parent's barrier: the holder writes while the
///   opener is still live);
/// - `OPENER-IDS2 <csv>` / `OPENER-READ-ERR <text>` — what it sees after;
/// - `OPENER-DONE`.
///
/// No sleep anywhere: the parent's `OPEN-OK` read is woken by this child's
/// open, and `OPENER-IDS2` by the parent's own release line.
#[test]
#[ignore = "re-exec'd worker: ProbeChild::spawn_opener runs it with --ignored; a suite-run of it asserts nothing"]
fn test_lock_probe_opener_child() {
    let Some(path) = std::env::var_os("MEM_STORE_LOCK_PROBE_DB") else {
        return;
    };
    let path = PathBuf::from(path);
    let multiprocess_wal =
        std::env::var("MEM_STORE_LOCK_PROBE_OPENER_MULTIPROCESS").as_deref() == Ok("1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("opener child runtime");
    runtime.block_on(async move {
        let opened = within_guard(
            "the opener's build",
            try_open_single_process(&path, multiprocess_wal, None),
        )
        .await;
        let (_database, connection) = match opened {
            Ok(Ok(handles)) => handles,
            Ok(Err(error)) => {
                println!("OPEN-ERR {error}");
                println!("OPENER-DONE");
                return;
            }
            Err(blocked) => {
                println!("OPEN-BLOCKED {blocked}");
                println!("OPENER-DONE");
                return;
            }
        };
        println!("OPEN-OK");

        let write = connection
            .execute(
                "INSERT INTO lock_probe (id, writer) VALUES (?, 'opener')",
                vec![turso::Value::Integer(LOCK_OPENER_ID)],
            )
            .await;
        println!(
            "OPENER-WRITE {}",
            match &write {
                Ok(_) => "ok".to_string(),
                Err(error) => format!("err {error}"),
            }
        );
        match lock_probe_ids(&connection).await {
            Ok(ids) => println!("OPENER-IDS {}", join_ids(&ids)),
            Err(error) => println!("OPENER-READ-ERR {error}"),
        }

        // The barrier: the holder writes while this handle is still open.
        let mut release = String::new();
        let _ = std::io::stdin().read_line(&mut release);

        match lock_probe_ids(&connection).await {
            Ok(ids) => println!("OPENER-IDS2 {}", join_ids(&ids)),
            Err(error) => println!("OPENER-READ-ERR {error}"),
        }
        println!("OPENER-DONE");
        drop(connection);
        drop(_database);
    });
}

/// The protocol tags the opener child emits, in the order they appear. Used to
/// separate the child's lines from libtest's chatter on the same stdout.
const OPENER_PROTOCOL_TAGS: [&str; 8] = [
    "OPEN-OK",
    "OPEN-ERR",
    "OPEN-BLOCKED",
    "OPENER-WRITE",
    "OPENER-IDS",
    "OPENER-IDS2",
    "OPENER-READ-ERR",
    "OPENER-DONE",
];

/// The protocol part of a child stdout line, or `None` when the line is libtest
/// chatter. `find`, not `strip_prefix`: the harness's `test <name> ... ` line has
/// no trailing newline, so the first protocol line is glued to it.
fn opener_protocol_line(line: &str) -> Option<&str> {
    let at = OPENER_PROTOCOL_TAGS
        .iter()
        .filter_map(|tag| line.find(tag))
        .min()?;
    Some(&line[at..])
}

/// A spawned probe child plus its line protocol.
///
/// The reader runs on its own OS thread so a child that stops talking becomes a
/// NAMED failure instead of a hung suite: a blocking `read_line` inside a
/// `current_thread` test runtime would also stop the timer that is supposed to
/// bound it. The guard is a hang detector — every successful line arrives when
/// the parent's own last action caused the child to produce it.
struct ProbeChild {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
    finished: bool,
}

impl ProbeChild {
    fn spawn_opener(path: &Path, multiprocess_wal: bool) -> Self {
        let mut command = std::process::Command::new(
            std::env::current_exe().expect("the running test binary path"),
        );
        command
            .arg("--exact")
            .arg("test_lock_probe_opener_child")
            .arg("--nocapture")
            .arg("--test-threads=1")
            // The worker is `#[ignore]`d so the recorded suite counts it as
            // ignored rather than as a green no-op; `--ignored` is what still
            // lets the suite SPAWN it. Without this flag the child would run
            // zero tests and close its stdout immediately.
            .arg("--ignored")
            .env("MEM_STORE_LOCK_PROBE_DB", path)
            .env(
                "MEM_STORE_LOCK_PROBE_OPENER_MULTIPROCESS",
                if multiprocess_wal { "1" } else { "0" },
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = command.spawn().expect("spawn the lock-probe opener child");
        let stdin = child.stdin.take().expect("piped child stdin");
        let stdout = child.stdout.take().expect("piped child stdout");
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            lines,
            finished: false,
        }
    }

    /// The child's next protocol line. Every line is echoed into the test
    /// output so a run's report can quote the child verbatim.
    ///
    /// libtest's own chatter shares the child's stdout (`running 1 test`, the
    /// `test <name> ...` line, a blank line, `ok`), and the harness prints the
    /// test-name line WITHOUT a trailing newline — so the child's first protocol
    /// line can arrive glued to it. Lines are therefore scanned for a protocol
    /// tag and trimmed to it, and chatter is skipped rather than mistaken for a
    /// verdict.
    fn next_line(&mut self) -> String {
        loop {
            match self.lines.recv_timeout(PROBE_GUARD) {
                Ok(line) => {
                    println!("[opener child] {line}");
                    if let Some(protocol) = opener_protocol_line(&line) {
                        return protocol.to_string();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!(
                    "the opener child produced no protocol line for {PROBE_GUARD:?}: the engine call that should have produced it did not return, so this cell measured a BLOCK, not a verdict"
                ),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!(
                    "the opener child closed its stdout without finishing its protocol (it exited early); its stderr is INHERITED, not piped or captured, so the reason went straight to this test binary's own stderr rather than into this message — and that exit is a finding rather than a pass"
                ),
            }
        }
    }

    /// Releases the child's stdin barrier. Dropping the pipe also means a child
    /// that ever asks for a second line sees EOF instead of blocking.
    fn release(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"go\n");
            let _ = stdin.flush();
        }
    }

    /// Reads the child's last protocol line, reaps it, and requires a successful
    /// exit — a panic inside the opener is one cell's datum, never a green cell.
    ///
    /// The reap is bounded for the same reason the line read is: a child that
    /// prints `OPENER-DONE` and then wedges inside `drop(connection)` or
    /// `drop(database)` would otherwise hang `wait()` forever, which is exactly
    /// the unnamed hang [`Self::next_line`] exists to prevent — an unbounded
    /// `wait()` here would make that guard decorative.
    fn finish(mut self) {
        let done = self.next_line();
        assert_eq!(
            done, "OPENER-DONE",
            "the opener child must end its protocol with OPENER-DONE"
        );
        let deadline = std::time::Instant::now() + PROBE_GUARD;
        let status = loop {
            match self.child.try_wait().expect("poll the opener child") {
                Some(status) => break status,
                None if std::time::Instant::now() >= deadline => panic!(
                    "the opener child printed OPENER-DONE but had still not exited {PROBE_GUARD:?} later: it is wedged AFTER its protocol (a handle drop inside the engine, not the test protocol), so this cell measured a HANG rather than a verdict"
                ),
                // A poll interval for the bounded reap, not a synchronisation
                // barrier: the child's own exit is the event, and no assertion
                // depends on this thread's timing.
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        };
        self.finished = true;
        assert!(
            status.success(),
            "the opener child must exit successfully (a panic inside the engine is a finding, not a pass); got {status:?}"
        );
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// One cell of Probe B's matrix.
struct LockCellSpec {
    /// Temp-dir label, also printed as the cell's name.
    label: &'static str,
    /// The HOLDER's (this process's) option set: `experimental_multiprocess_wal`.
    holder_multiprocess_wal: bool,
    /// Whether the holder is mid-write (an open `BEGIN IMMEDIATE` transaction
    /// with an uncommitted row) at the moment of the second open.
    holder_mid_write: bool,
    /// The OPENER's (child process's) option set.
    opener_multiprocess_wal: bool,
    /// Whether the holder writes [`LOCK_HOLDER_AFTER_ID`] after the opener's
    /// attempt. `false` isolates "the mixed open itself" from "the holder's next
    /// commit" — the difference between a poisoned WAL and a clobbered frame.
    holder_writes_after: bool,
}

/// What one cell observed. Every field is a measurement, not a verdict: the
/// per-cell tests do the pinning, so one cell's answer can be quoted without
/// re-running the others.
struct LockCell {
    /// `Ok` = the opener got a live handle; `Err` = the engine's refusal text
    /// (or the blocked text). This is the cell's primary answer.
    opener: Result<(), String>,
    opener_write: Option<String>,
    /// What the opener reads after its own write (before the holder's write).
    opener_ids: Option<Result<Vec<i64>, String>>,
    /// What the opener reads after the holder's write — the "does the opener
    /// see the holder's later data" leg.
    opener_ids_after: Option<Result<Vec<i64>, String>>,
    /// The holder's view while the opener is still live.
    holder_ids: Result<Vec<i64>, String>,
    holder_integrity: Result<String, String>,
    /// The holder's post-open write result, or `None` when the cell was built
    /// not to write (the isolation variant).
    holder_write: Option<Result<(), String>>,
    holder_commit: Option<Result<(), String>>,
    /// The file set while the holder was live and after everything closed.
    held_files: Vec<(String, u64)>,
    after_files: Vec<(String, u64)>,
    /// A FRESH handle opened with the opener's option set once the holder is
    /// gone: is the file still a database both option sets can read?
    final_ids: Result<Vec<i64>, String>,
    final_integrity: Result<String, String>,
}

/// Runs one Probe B cell: holder here, opener in a child, holder still live for
/// the whole second open. The order is deliberate — the opener's verdict and
/// its write happen while the holder is live, the holder then reads, commits and
/// writes, the opener reads again on the parent's release and exits (its handles
/// close with the child process, before the holder's), and only then does the
/// HOLDER close, so no assertion is about a state that has already moved on.
async fn run_lock_cell(spec: LockCellSpec) -> LockCell {
    let LockCellSpec {
        label,
        holder_multiprocess_wal,
        holder_mid_write,
        opener_multiprocess_wal,
        holder_writes_after,
    } = spec;
    let dir = common::temp_dir(label);
    let path = dir.join("lock.db");

    let (holder_database, holder) = open_single_process(&path, holder_multiprocess_wal, None).await;
    holder
        .execute(LOCK_PROBE_TABLE, ())
        .await
        .expect("create lock_probe");
    holder
        .execute(
            format!("INSERT INTO lock_probe (id, writer) VALUES ({LOCK_SEED_ID}, 'holder-seed')")
                .as_str(),
            (),
        )
        .await
        .expect("write the holder's committed seed row");
    if holder_mid_write {
        holder
            .execute("BEGIN IMMEDIATE", ())
            .await
            .expect("open the holder's write transaction");
        holder
            .execute(
                format!(
                    "INSERT INTO lock_probe (id, writer) VALUES ({LOCK_MID_WRITE_ID}, 'holder-mid-write')"
                )
                .as_str(),
                (),
            )
            .await
            .expect("write inside the holder's open transaction");
    }
    let held_files = file_inventory(&dir);
    println!(
        "[cell {label}] holder_live multiprocess_wal={holder_multiprocess_wal} mid_write={holder_mid_write} files={held_files:?}"
    );
    if holder_mid_write {
        println!(
            "[cell {label}] holder_view_mid_write={:?}",
            lock_probe_ids(&holder).await
        );
    }

    // -- The second open, in another process, while the holder is live.
    let mut child = ProbeChild::spawn_opener(&path, opener_multiprocess_wal);
    let verdict = child.next_line();
    let opener = if verdict == "OPEN-OK" {
        Ok(())
    } else if let Some(error) = verdict.strip_prefix("OPEN-ERR ") {
        Err(error.to_string())
    } else if let Some(blocked) = verdict.strip_prefix("OPEN-BLOCKED ") {
        Err(blocked.to_string())
    } else {
        panic!(
            "the opener child's first line must be OPEN-OK / OPEN-ERR / OPEN-BLOCKED, got {verdict:?}"
        )
    };
    println!("[cell {label}] opener={opener:?}");

    let mut opener_write = None;
    let mut opener_ids = None;
    let mut opener_ids_after = None;
    if opener.is_ok() {
        let write = child.next_line();
        opener_write = Some(
            write
                .strip_prefix("OPENER-WRITE ")
                .unwrap_or_else(|| panic!("unexpected opener write line {write:?}"))
                .to_string(),
        );
        opener_ids = Some(parse_opener_ids(&child.next_line()));
    }

    // -- The holder's view while the opener is STILL live.
    let holder_ids = lock_probe_ids(&holder).await;
    let holder_integrity = lock_probe_integrity(&holder).await;
    println!("[cell {label}] holder_ids_while_opener_live={holder_ids:?}");

    let holder_commit = if holder_mid_write {
        let commit = holder
            .execute("COMMIT", ())
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        println!("[cell {label}] holder_commit={commit:?}");
        Some(commit)
    } else {
        None
    };
    let holder_write = if holder_writes_after {
        let write = holder
            .execute(
                format!(
                    "INSERT INTO lock_probe (id, writer) VALUES ({LOCK_HOLDER_AFTER_ID}, 'holder-after')"
                )
                .as_str(),
                (),
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        println!("[cell {label}] holder_write={write:?}");
        Some(write)
    } else {
        println!("[cell {label}] holder_write=skipped (isolation variant)");
        None
    };

    // -- Release the opener so it can re-read; its second read is the
    // "opener sees the holder's later data" leg.
    child.release();
    if opener.is_ok() {
        opener_ids_after = Some(parse_opener_ids(&child.next_line()));
    }
    child.finish();

    let after_files = file_inventory(&dir);
    println!("[cell {label}] files_after={after_files:?}");
    drop(holder);
    drop(holder_database);

    // -- The quiet file must still be a database for BOTH option sets: this is
    // what distinguishes "the second open succeeded harmlessly" from "the two
    // bookkeepings diverged and one side's writes are now unreadable".
    let (final_database, final_connection) =
        try_open_single_process(&path, opener_multiprocess_wal, None)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "[cell {label}] after every handle closed, the file must reopen with the opener's option set (multiprocess_wal={opener_multiprocess_wal}): {error}; files while held: {held_files:?}; files after: {after_files:?}"
                )
            });
    let final_ids = lock_probe_ids(&final_connection).await;
    let final_integrity = lock_probe_integrity(&final_connection).await;
    println!("[cell {label}] final_ids={final_ids:?} final_integrity={final_integrity:?}");
    drop(final_connection);
    drop(final_database);

    common::cleanup_temp_dir(&dir).expect("cleanup the cell's temp dir");

    LockCell {
        opener,
        opener_write,
        opener_ids,
        opener_ids_after,
        holder_ids,
        holder_integrity,
        holder_write,
        holder_commit,
        held_files,
        after_files,
        final_ids,
        final_integrity,
    }
}

/// Parses `OPENER-IDS <csv>` / `OPENER-IDS2 <csv>` / `OPENER-READ-ERR <text>`.
fn parse_opener_ids(line: &str) -> Result<Vec<i64>, String> {
    if let Some(csv) = line
        .strip_prefix("OPENER-IDS ")
        .or_else(|| line.strip_prefix("OPENER-IDS2 "))
    {
        let ids = csv
            .split(',')
            .map(|id| {
                id.trim()
                    .parse::<i64>()
                    .unwrap_or_else(|_| panic!("the opener sent an unparsable id in {line:?}"))
            })
            .collect();
        return Ok(ids);
    }
    if let Some(error) = line.strip_prefix("OPENER-READ-ERR ") {
        return Err(error.to_string());
    }
    panic!("unexpected opener id line {line:?}");
}

/// Probe B, cell 1 — a live, **idle** flag-off holder versus a second flag-off
/// opener in another process.
///
/// The cell the earlier scratch probe reported but never pinned. Expected on
/// turso 0.8.3: the holder took the database file's exclusive `fcntl` lock at
/// open time (the legacy, non-multiprocess path locks the file), so the second
/// opener is REFUSED with the engine's locking error — the holder is untouched.
///
/// This is the cell that matters for `purge-session` after the flag is dropped:
/// a flag-off `purge-session` could not open a database a flag-off engine
/// holds, with no mixed versions involved.
#[tokio::test]
async fn test_flag_off_holder_refuses_a_flag_off_opener() {
    let cell = run_lock_cell(LockCellSpec {
        label: "cap-lock-cell-1-flag-off-idle",
        holder_multiprocess_wal: false,
        holder_mid_write: false,
        opener_multiprocess_wal: false,
        holder_writes_after: true,
    })
    .await;

    let refusal = match &cell.opener {
        Ok(()) => panic!(
            "a live flag-off holder must REFUSE a second flag-off opener in another process, but the opener got a handle (it saw {:?}); holder files: {:?}",
            cell.opener_ids, cell.held_files
        ),
        Err(error) => error.clone(),
    };
    assert!(
        refusal.contains("Locking error"),
        "the refusal must be the engine's locking error (a classifier, not a text pin: `LimboError::LockingError` reaches the crate as a plain `Error(String)`, so its prefix is the only machine-recognisable carrier); got {refusal:?}"
    );
    assert!(
        refusal.contains("locked by another process"),
        "the refusal must name the other holder; got {refusal:?}"
    );
    assert_eq!(
        cell.holder_ids,
        Ok(vec![LOCK_SEED_ID]),
        "while the opener is refused, the holder must see exactly its own seed row — no opener row, nothing replayed"
    );
    assert_eq!(
        cell.holder_integrity,
        Ok("ok".to_string()),
        "the refused open must leave the holder's file intact"
    );
    assert_eq!(
        cell.holder_write,
        Some(Ok(())),
        "the holder must still take writes"
    );
    assert_eq!(
        cell.final_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_HOLDER_AFTER_ID]),
        "after both handles closed, the file must hold exactly the holder's two rows"
    );
    assert_eq!(
        cell.final_integrity,
        Ok("ok".to_string()),
        "the file must be intact for a fresh handle"
    );
}

/// Probe B, cell 2 — a live **mid-write** flag-off holder versus a second
/// flag-off opener.
///
/// The difference from cell 1 is the holder's state, not its option set: an
/// open `BEGIN IMMEDIATE` transaction with one uncommitted row. The expected
/// answer is the same refusal (the lock is taken at OPEN time, before any
/// transaction exists), which is the useful part: `purge-session` cannot get a
/// handle even while the engine is mid-transaction, so the refusal is not a
/// momentary write-lock artefact. The holder then commits, and its post state
/// (both rows present) proves the refused open did not disturb the transaction.
#[tokio::test]
async fn test_flag_off_holder_mid_write_refuses_a_flag_off_opener() {
    let cell = run_lock_cell(LockCellSpec {
        label: "cap-lock-cell-2-flag-off-mid-write",
        holder_multiprocess_wal: false,
        holder_mid_write: true,
        opener_multiprocess_wal: false,
        holder_writes_after: true,
    })
    .await;

    let refusal = match &cell.opener {
        Ok(()) => panic!(
            "a live flag-off holder must REFUSE a second flag-off opener even mid-transaction, but the opener got a handle (it saw {:?}); holder files: {:?}",
            cell.opener_ids, cell.held_files
        ),
        Err(error) => error.clone(),
    };
    assert!(
        refusal.contains("Locking error"),
        "the mid-write refusal must be the same locking error as cell 1, got {refusal:?}"
    );
    assert_eq!(
        cell.holder_commit,
        Some(Ok(())),
        "the holder's open transaction must still commit after the refused open"
    );
    assert_eq!(
        cell.holder_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_MID_WRITE_ID]),
        "while the opener is refused the holder must see its seed row AND the row its own open transaction holds (same connection), which is what proves the cell really was measured mid-write"
    );
    assert_eq!(cell.holder_integrity, Ok("ok".to_string()));
    assert_eq!(
        cell.final_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_MID_WRITE_ID, LOCK_HOLDER_AFTER_ID]),
        "the committed transaction and the post-open write must be durable"
    );
}

/// Probe B, cell 3 — a live **flag-on** holder versus a flag-off opener: the
/// mixed-version direction the earlier scratch probe reported as *not* refused.
///
/// Measured on turso 0.8.3, Linux — **it is not refused, and it is not
/// harmless**:
/// - the flag-on holder opened the database file with `NoLock` (multiprocess
///   WAL coordinates through `-tshm` instead), so the flag-off opener's
///   exclusive `fcntl` lock is uncontended and the open is ACCEPTED;
/// - the opener commits a row and never sees the holder's later commit, and
///   the holder never sees the opener's row: two live handles, two WAL
///   bookkeepings, one file;
/// - after both handles close, the opener's COMMITTED row is gone and only the
///   holder's rows are readable. No error was reported on either side.
///
/// The loss is pinned as the difference between what the opener was told it
/// committed and what the quiet file holds. It is a data-loss hazard, not a
/// theory about frames: `final_ids` is read through a FRESH handle after
/// everything closed. Cell 6 isolates the cause on the mirrored direction: with
/// no later write from the other side, the opener's row survives.
#[tokio::test]
async fn test_mixed_flag_on_holder_and_flag_off_opener_succeed_and_lose_the_opener_row() {
    let cell = run_lock_cell(LockCellSpec {
        label: "cap-lock-cell-3-flag-on-vs-flag-off",
        holder_multiprocess_wal: true,
        holder_mid_write: false,
        opener_multiprocess_wal: false,
        holder_writes_after: true,
    })
    .await;

    assert!(
        cell.opener.is_ok(),
        "MEASURED: a flag-on holder takes no OS lock on the database file, so a flag-off opener in another process is ACCEPTED. If this starts failing, the engine has grown a cross-mode guard and purge-session's coexistence with a flag-on engine is safe by refusal — an ADR-level change. Got {:?}; holder files: {:?}",
        cell.opener,
        cell.held_files
    );
    assert_eq!(
        cell.opener_write.as_deref(),
        Some("ok"),
        "the accepted opener's write must report success — that is what makes the later loss silent"
    );
    assert_eq!(
        cell.opener_ids,
        Some(Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID])),
        "the opener must read the holder's pre-existing row plus its own"
    );
    assert_eq!(
        cell.opener_ids_after,
        Some(Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID])),
        "the opener must NOT see the row the flag-on holder commits while both are live: the two bookkeepings do not share a view"
    );
    assert_eq!(
        cell.holder_ids,
        Ok(vec![LOCK_SEED_ID]),
        "the flag-on holder must NOT see the flag-off opener's committed row"
    );
    assert_eq!(
        cell.holder_write,
        Some(Ok(())),
        "the holder must still take writes"
    );
    assert_eq!(cell.holder_integrity, Ok("ok".to_string()));
    assert_eq!(
        cell.final_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_HOLDER_AFTER_ID]),
        "after both handles closed, the file must hold the holder's rows only: the opener's committed row {LOCK_OPENER_ID} is GONE. If this ever starts including it, mixed-mode coexistence has become lossless and the ADR can treat a second process as safe; until then it cannot."
    );
    assert_eq!(
        cell.final_integrity,
        Ok("ok".to_string()),
        "the loss is silent: the surviving file still reports a clean integrity verdict"
    );
}

/// Probe B, cell 4 — a live flag-on holder versus a flag-on opener: the
/// baseline contrast for cell 3, and the configuration the engine is designed
/// for (`-tshm` coordination, `NoLock` on the database file).
///
/// Both handles must be live AND coherent: each side's write must be readable
/// by the other, and the file must still be a database with one integrity
/// verdict of `ok` after both close. "Succeeds" alone would not say that.
#[tokio::test]
async fn test_flag_on_holder_and_a_flag_on_opener_share_the_database() {
    let cell = run_lock_cell(LockCellSpec {
        label: "cap-lock-cell-4-flag-on-vs-flag-on",
        holder_multiprocess_wal: true,
        holder_mid_write: false,
        opener_multiprocess_wal: true,
        holder_writes_after: true,
    })
    .await;

    assert!(
        cell.opener.is_ok(),
        "the supported multiprocess configuration must accept a second process's flag-on open; got {:?}; holder files: {:?}",
        cell.opener,
        cell.held_files
    );
    assert_eq!(
        cell.opener_write.as_deref(),
        Some("ok"),
        "the opener's write must succeed on a live multiprocess database"
    );
    assert_eq!(
        cell.opener_ids,
        Some(Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID])),
        "the opener must see the holder's seed row plus its own row"
    );
    assert_eq!(
        cell.opener_ids_after,
        Some(Ok(vec![LOCK_SEED_ID, LOCK_HOLDER_AFTER_ID, LOCK_OPENER_ID])),
        "the opener's SECOND read must include the row the holder wrote while both were live: two live handles, one database"
    );
    assert_eq!(
        cell.holder_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID]),
        "the holder must see the opener's committed row while the opener is still live (the holder's own later write has not happened yet at this read)"
    );
    assert_eq!(cell.holder_integrity, Ok("ok".to_string()));
    assert_eq!(
        cell.final_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_HOLDER_AFTER_ID, LOCK_OPENER_ID]),
        "after both close, exactly the three committed rows must remain — no replayed or invented row"
    );
    assert_eq!(cell.final_integrity, Ok("ok".to_string()));
}

/// Probe B, cell 5 — a live flag-off holder versus a **flag-on** opener: the
/// reverse mixed direction, which the earlier scratch probe never pinned.
///
/// Measured on turso 0.8.3, Linux — the same shape as cell 3, mirrored: the
/// flag-on opener is ACCEPTED (it bypasses the database-file lock by design),
/// commits a row, loses the holder's later commit from its own view, is itself
/// invisible to the holder, and after both close its committed row is gone
/// while the holder's rows survive. Neither side reports an error. Cell 6 shows
/// the loss needs the other side's next COMMIT: the mixed open alone does not
/// destroy the row.
#[tokio::test]
async fn test_mixed_flag_off_holder_and_flag_on_opener_succeed_and_lose_the_opener_row() {
    let cell = run_lock_cell(LockCellSpec {
        label: "cap-lock-cell-5-flag-off-vs-flag-on",
        holder_multiprocess_wal: false,
        holder_mid_write: false,
        opener_multiprocess_wal: true,
        holder_writes_after: true,
    })
    .await;

    assert!(
        cell.opener.is_ok(),
        "MEASURED: a flag-on opener bypasses the database-file lock, so a flag-off holder does not refuse it. If this starts failing, a cross-mode guard exists and the reverse mixed direction is safe by refusal — an ADR-level change. Got {:?}; holder files: {:?}",
        cell.opener,
        cell.held_files
    );
    assert_eq!(
        cell.opener_write.as_deref(),
        Some("ok"),
        "the accepted opener's write must report success"
    );
    assert_eq!(
        cell.opener_ids,
        Some(Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID])),
        "the opener must read the holder's pre-existing row plus its own"
    );
    assert_eq!(
        cell.opener_ids_after,
        Some(Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID])),
        "the opener must NOT see the row the flag-off holder commits while both are live"
    );
    assert_eq!(
        cell.holder_ids,
        Ok(vec![LOCK_SEED_ID]),
        "the flag-off holder must NOT see the flag-on opener's committed row"
    );
    assert_eq!(
        cell.holder_write,
        Some(Ok(())),
        "the holder must still take writes"
    );
    assert_eq!(cell.holder_integrity, Ok("ok".to_string()));
    assert_eq!(
        cell.final_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_HOLDER_AFTER_ID]),
        "after both handles closed, the opener's committed row {LOCK_OPENER_ID} is GONE and only the holder's rows remain"
    );
    assert_eq!(
        cell.final_integrity,
        Ok("ok".to_string()),
        "the loss is silent: the surviving file reports a clean integrity verdict"
    );
    assert!(
        cell.after_files
            .iter()
            .any(|(name, _)| name.ends_with("-tshm")),
        "the flag-on opener must have created its coordination sidecar next to the flag-off holder's database — the two bookkeepings really did coexist; files: {:?}",
        cell.after_files
    );
}

/// Probe B, isolation variant of cell 5 — the same mixed open, but the holder
/// does **no** write of its own after the opener's attempt.
///
/// Cell 5 cannot say whether the opener's row is lost because the holder's next
/// commit clobbered frames or because the mixed open itself poisons the WAL.
/// This cell removes the holder's write, so the only later mutation is the
/// holder's clean close: whatever happens to the opener's committed row here is
/// attributable to the coexistence itself.
#[tokio::test]
async fn test_mixed_flag_off_holder_and_flag_on_opener_without_a_later_holder_write() {
    let cell = run_lock_cell(LockCellSpec {
        label: "cap-lock-cell-6-flag-off-vs-flag-on-no-holder-write",
        holder_multiprocess_wal: false,
        holder_mid_write: false,
        opener_multiprocess_wal: true,
        holder_writes_after: false,
    })
    .await;

    println!(
        "[cell 6] opener={:?} opener_write={:?} opener_ids={:?} opener_ids_after={:?}",
        cell.opener, cell.opener_write, cell.opener_ids, cell.opener_ids_after
    );
    println!(
        "[cell 6] holder_ids={:?} holder_integrity={:?} final_ids={:?} final_integrity={:?} files_after={:?}",
        cell.holder_ids,
        cell.holder_integrity,
        cell.final_ids,
        cell.final_integrity,
        cell.after_files
    );
    assert!(
        cell.opener.is_ok(),
        "the isolation variant only means something if the mixed open is still accepted; got {:?}",
        cell.opener
    );
    assert_eq!(cell.opener_write.as_deref(), Some("ok"));
    assert_eq!(
        cell.opener_ids,
        Some(Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID])),
        "the opener's row must commit while the holder is live"
    );
    assert_eq!(
        cell.holder_ids,
        Ok(vec![LOCK_SEED_ID]),
        "the holder still must not see the opener's row while both are live"
    );
    // THE POINT OF THIS CELL: with no later holder write, the opener's
    // committed row SURVIVES the coexistence and the holder's clean close.
    assert_eq!(
        cell.final_ids,
        Ok(vec![LOCK_SEED_ID, LOCK_OPENER_ID]),
        "MEASURED: without a later holder write, the flag-on opener's committed row survives. Cell 5 loses it only when the flag-off holder commits again — so the mixed OPEN is not what destroys the row, the next commit on the other side of the bookkeeping split is. If this ever starts losing the row, the divergence is deeper than a clobbered frame and the ADR's hazard statement widens."
    );
    assert_eq!(cell.final_integrity, Ok("ok".to_string()));
}

/// Probe A, side-check — does `VACUUM` block against a live writer, or refuse?
///
/// The ADR's other availability question: if a sweep ever ran beside a live
/// engine handle, would it wait for writers or fail? Three legs on ONE
/// vacuum-enabled handle, so the answer cannot be confused with "VACUUM is
/// broken":
/// 1. control: with no transaction open, `VACUUM` runs (this is what makes the
///    refusal below attributable to the writer);
/// 2. a second connection holds `BEGIN IMMEDIATE` + an uncommitted row, and
///    `VACUUM` on the first connection must return as `Busy("database is
///    locked")` — refused, never waited out. This leg carries its OWN budget,
///    [`VACUUM_REFUSAL_BUDGET`] (1 s), separate from the [`PROBE_GUARD`] hang
///    guard: the guard alone would accept a 9-second busy-handler wait while
///    "refuses rather than blocks" quietly became false. The leg prints the
///    elapsed time it measured, so a regression shows the number that moved;
/// 3. after `ROLLBACK`, the same `VACUUM` runs again, so the busy was the
///    writer's lock and not a poisoned connection.
///
/// Not a stress harness: the writer's transaction is opened BEFORE the VACUUM,
/// so there is no race to hit and no sleep anywhere.
#[tokio::test]
async fn test_vacuum_is_refused_while_another_connection_holds_a_write_transaction() {
    let dir = common::temp_dir("cap-vacuum-lock-shape");
    let path = dir.join("v.db");
    let (database, writer) = open_with_vacuum(&path, false).await;
    writer
        .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", ())
        .await
        .expect("create t");
    writer
        .execute("INSERT INTO t (id, v) VALUES (1, 'a')", ())
        .await
        .expect("write a row");
    let reader = database.connect().expect("the handle's second connection");

    // Leg 1 — the control: no transaction open, VACUUM runs.
    let control = reader.execute("VACUUM", ()).await;
    assert!(
        control.is_ok(),
        "CONTROL: VACUUM must run on this handle while no transaction is open, otherwise the refusal below is VACUUM's fault: {control:?}"
    );

    // Leg 2 — a live write transaction on the other connection.
    writer
        .execute("BEGIN IMMEDIATE", ())
        .await
        .expect("open the writer's transaction");
    writer
        .execute("INSERT INTO t (id, v) VALUES (2, 'b')", ())
        .await
        .expect("write inside the writer's transaction");
    // The refusal is the claim, so it is measured against its own budget rather
    // than the hang guard: `started`/`elapsed` are the measurement, printed
    // below. See [`VACUUM_REFUSAL_BUDGET`].
    let started = std::time::Instant::now();
    let busy = tokio::time::timeout(VACUUM_REFUSAL_BUDGET, reader.execute("VACUUM", ()))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "VACUUM must REFUSE rather than block against a live writer, but it did not return within the refusal budget {VACUUM_REFUSAL_BUDGET:?} — this leg is deliberately tighter than the {PROBE_GUARD:?} hang guard, precisely so a multi-second busy-handler wait fails here instead of passing"
            )
        });
    let elapsed = started.elapsed();
    println!("[vacuum lock shape] against_a_live_writer={busy:?} elapsed={elapsed:?}");
    let busy = busy.expect_err(
        "VACUUM must not succeed while another connection holds an uncommitted write transaction",
    );
    assert!(
        matches!(busy, turso::Error::Busy(_)),
        "the refusal must be the engine's Busy variant (the same-connection 'database is locked' class), not a parse/generic error: {busy:?}"
    );

    // Leg 3 — release, and the same VACUUM runs.
    writer
        .execute("ROLLBACK", ())
        .await
        .expect("roll back the writer's transaction");
    let after = reader.execute("VACUUM", ()).await;
    assert!(
        after.is_ok(),
        "once the writer's transaction is gone, the same VACUUM must run: that is what proves the Busy was the lock, not a broken connection: {after:?}"
    );
    assert_eq!(
        raw_integer_column(&writer, "SELECT id FROM t ORDER BY id", "id").await,
        vec![1],
        "the rolled-back row stays rolled back and the kept row stays readable after the sweep"
    );

    drop(reader);
    drop(writer);
    drop(database);
    common::cleanup_temp_dir(&dir).expect("cleanup temp dir");
}
