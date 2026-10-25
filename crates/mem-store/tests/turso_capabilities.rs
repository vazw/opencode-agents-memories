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
//! encryption is disabled. `ConnectOptions::default()` survives only in AC1,
//! as an explicitly labelled engine-baseline handle, because that claim is
//! about the engine default rather than about the shipping open. AC16/AC17
//! necessarily reach `turso::Builder` directly: `ConnectOptions` cannot
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

use std::path::{Path, PathBuf};

use mem_store::config::StoreConfig;
use mem_store::db::{
    BatchMode, ConnectFactory, ConnectOptions, Db, DbHandle, ExecResult, Row, Statement, Value,
    turso_connect_factory,
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

/// A handle opened with `ConnectOptions::default()` — the plain builder, with
/// no experimental feature at all. Used **only** by AC1 as a labelled
/// engine-baseline handle (the claim there is about the engine's default
/// journal mode, not about the shipping open).
async fn open_default(dir: &Path, name: &str) -> DbHandle {
    turso_connect_factory()
        .connect(dir.join(name), ConnectOptions::default())
        .await
        .expect("open database with default connect options")
}

/// Raw builder handles for the `experimental_vacuum` probes. `ConnectOptions`
/// cannot express this flag and this wave must not extend it, so AC16/AC17
/// reach `turso::Builder` directly. The caller keeps the `Database` alive next
/// to the `Connection` (dropping the database releases the file resources).
async fn open_with_vacuum(
    path: &Path,
    multiprocess_wal: bool,
) -> (turso::Database, turso::Connection) {
    let mut builder = turso::Builder::new_local(&path.to_string_lossy()).experimental_vacuum(true);
    if multiprocess_wal {
        builder = builder.experimental_multiprocess_wal(true);
    }
    let database = builder.build().await.expect("build vacuum database");
    let connection = database.connect().expect("connect vacuum database");
    (database, connection)
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

/// Production-side erasure fixture: create `user_prompts`, insert the canary,
/// prove the marker reached disk (what stops the later absence from being
/// vacuous), then delete the row. The bytes stay in the WAL; the caller
/// checkpoints.
async fn seed_and_delete_canary(db: &dyn Db, storage: &Path) {
    db.execute(
        "CREATE TABLE user_prompts (id TEXT PRIMARY KEY, content TEXT NOT NULL)",
        vec![],
    )
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
        .execute(
            "CREATE TABLE user_prompts (id TEXT PRIMARY KEY, content TEXT NOT NULL)",
            (),
        )
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
/// set, `default.db` through the plain builder (this is the only place
/// `ConnectOptions::default()` appears, because the claim is about the engine's
/// default, not about the shipping open).
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
