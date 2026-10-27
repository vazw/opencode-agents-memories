//! Issue 53 measurement probe — built-in encryption × experimental multiprocess WAL.
//!
//! **This file is a measurement harness, not a suite addition.** Every test here is
//! `#[ignore]`d and is meant to be run explicitly:
//!
//! ```sh
//! cargo test -p mem-store --test issue53_probe -- --ignored --nocapture --test-threads=1
//! ```
//!
//! It answers the four cells of issue 53's gate:
//!
//! | probe | encryption | multiprocess WAL | expectation |
//! |---|---|---|---|
//! | G1 | on | on | the combination our tree ships on Unix |
//! | G1B | on | on | G1 with the writer cleanly closed before the second opener (`-tshm` backfill) |
//! | G1C | on | on (from a flag-off file) | the flag-off → flag-on upgrade, i.e. the forced backfill |
//! | G3 | off | on | the control: the flag alone |
//! | G3B / G3C | off | on | the two backfill shapes, plaintext |
//! | inversion | on | off | the encryption half alone |
//!
//! G2 (the vault surface) lives in `crates/mem-vault/tests/issue53_vault_probe.rs`:
//! `mem-store` cannot dev-depend on `mem-vault` (ADR-0001 edge), and the vault is
//! the surface where the pair is *forced*, not opt-in.
//!
//! **Protocol.** The parent opens a fresh database through the production option
//! set (`ConnectOptions::for_config` + `resolve_database_encryption`, i.e. exactly
//! `ConnectionManager::open_database`'s seam), commits three rows and keeps the
//! handle open. Then it spawns this same test binary (`current_exe()`, the
//! `turso_capabilities.rs` / `open_guard.rs` precedent) twice: once read-only, once
//! writable. The writable child commits a fourth row while the parent's writer is
//! still live. The parent then commits its own row, runs `PRAGMA integrity_check`,
//! drops, reopens the same file and runs `integrity_check` again. Every step prints
//! a literal line, and the probe ends with `PROBE G<n> RESULT: PASS` or
//! `... FAIL side=<side> error=<text>`.
//!
//! **The inversion arm is engine-level, by necessity.** Since issue 45 (owner
//! decision O3) `TursoConnectFactory` refuses a legacy (multiprocess-WAL-off) option
//! set with `UnsupportedOpenMode`, and `ConnectOptions::allow_unguarded_open` is
//! `pub(crate)`. The arm therefore builds the same `turso::Builder` tuple the
//! factory builds, minus the MP-WAL token — the precedent is
//! `turso_capabilities.rs::open_single_process`, which exists for the same reason.
//! The open under test is still the production tuple in every other respect
//! (`resolve_database_encryption`, the same cipher and key, the same flags).
//!
//! **No assertions on a measured failure.** A FAIL is the datum, not a test crash;
//! only a broken harness (a child that cannot start, an unwritable temp dir) panics.
//! The evidence README quotes these lines verbatim.

mod common;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mem_store::config::StoreConfig;
use mem_store::db::{
    BatchMode, ConnectFactory, ConnectOptions, Db, DbHandle, ExecResult, Row, Statement, Value,
    turso_connect_factory,
};
use mem_store::error::StoreError;
use mem_store::platform::Platform;

/// How long a child may take to produce its next protocol line. Generous for a
/// cold page cache; short enough that a wedge is a recorded finding, not a hung run.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// The row ids: the parent writes `1,2,3`, the writable child `100`, the parent's
/// post-child commit `4`. Distinct bands so "the child's write is visible to the
/// parent" and "the parent's write is visible to the child" can never be confused.
const PARENT_ROWS: [i64; 3] = [1, 2, 3];
const CHILD_ROW: i64 = 100;
const PARENT_AFTER_ROW: i64 = 4;

const CREATE_PROBE: &str = "CREATE TABLE probe_rows (id INTEGER PRIMARY KEY, v TEXT NOT NULL)";

/// A second table with fat rows. Three tiny rows fit one page, which would make the
/// `-tshm` backfill trivial; the archived ingest's `database header magic mismatch`
/// is about *many* frames being replayed into the shared-memory index, so the
/// probe carries a WAL that spans hundreds of pages.
const CREATE_FILLER: &str =
    "CREATE TABLE probe_filler (id INTEGER PRIMARY KEY, payload TEXT NOT NULL)";

const FILLER_ROWS: i64 = 128;
const FILLER_BYTES: usize = 4096;

// ---------------------------------------------------------------------------
// Cells and configuration
// ---------------------------------------------------------------------------

/// One gate cell: the two flags this issue is about, plus writability for the
/// second opener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    encryption: bool,
    multiprocess_wal: bool,
    read_only: bool,
}

impl Cell {
    fn with_read_only(self, read_only: bool) -> Self {
        Self { read_only, ..self }
    }

    fn label(self) -> String {
        format!(
            "encryption={} multiprocess_wal={} read_only={}",
            self.encryption, self.multiprocess_wal, self.read_only
        )
    }
}

fn probe_config(root: &Path, encryption: bool, key: &str) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: 2,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: encryption,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: encryption.then(|| key.to_string()),
        allow_unguarded_open: false,
    }
}

// ---------------------------------------------------------------------------
// The open under test
// ---------------------------------------------------------------------------

/// Opens `path` with the production tuple for `cell`.
///
/// - multiprocess WAL **on** ⇒ the production factory, i.e. the exact open every
///   non-Windows shard/prompt/profile/episodic/facts/vault open performs, guard
///   included. A guard refusal is a finding and is returned verbatim.
/// - multiprocess WAL **off** ⇒ the same `turso::Builder` calls the factory makes,
///   without the factory, because issue 45's O3 refuses that option set by design
///   (`unsupported-open-mode`) and the inversion arm measures the engine, not the
///   guard. Precedent and rationale: `turso_capabilities.rs::open_single_process`.
async fn open_cell(config: &StoreConfig, path: &Path, cell: Cell) -> Result<DbHandle, String> {
    let encryption = mem_store::db::resolve_database_encryption(config)
        .map_err(|error| format!("resolve_database_encryption: {error}"))?;
    if cell.multiprocess_wal {
        let mut options = ConnectOptions::for_config(config, Platform::current());
        options.encryption = encryption;
        options.read_only = cell.read_only;
        turso_connect_factory()
            .connect(path.to_path_buf(), options)
            .await
            .map_err(|error| format!("factory open ({}): {error}", cell.label()))
    } else {
        let mut builder = turso::Builder::new_local(&path.to_string_lossy())
            .experimental_encryption(encryption.is_some());
        if cell.read_only {
            builder = builder.read_only(true);
        }
        if let Some(encryption) = encryption {
            builder = builder.with_encryption(turso::EncryptionOpts {
                cipher: encryption.cipher,
                hexkey: encryption.hexkey,
            });
        }
        let database = builder
            .build()
            .await
            .map_err(|error| format!("raw builder open ({}): {error}", cell.label()))?;
        let connection = database
            .connect()
            .map_err(|error| format!("raw builder connect ({}): {error}", cell.label()))?;
        Ok(Arc::new(RawDb {
            database,
            connection,
        }))
    }
}

// ---------------------------------------------------------------------------
// SQL helpers (tolerant: a failed query is a reported cell, never a panic)
// ---------------------------------------------------------------------------

async fn insert_row(db: &dyn Db, id: i64, value: &str) -> Result<(), String> {
    db.execute(
        "INSERT INTO probe_rows (id, v) VALUES (?, ?)",
        vec![Value::Integer(id), Value::Text(value.to_string())],
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn insert_filler(db: &dyn Db, id: i64) -> Result<(), String> {
    db.execute(
        "INSERT INTO probe_filler (id, payload) VALUES (?, ?)",
        vec![Value::Integer(id), Value::Text("f".repeat(FILLER_BYTES))],
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// `(count, length(payload) of the first row)` — a corruption that loses or
/// truncates filler pages fails the first number, and one that mangles a page's
/// payload fails the second.
async fn filler_shape(db: &dyn Db) -> Result<(i64, i64), String> {
    let rows = db
        .query(
            "SELECT count(*) AS n, max(length(payload)) AS len FROM probe_filler",
            Vec::new(),
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.first() else {
        return Err("filler query returned no row".to_string());
    };
    Ok((row.i64("n").unwrap_or(-1), row.i64("len").unwrap_or(-1)))
}

async fn row_ids(db: &dyn Db) -> Result<Vec<i64>, String> {
    let rows = db
        .query("SELECT id FROM probe_rows ORDER BY id", Vec::new())
        .await
        .map_err(|error| error.to_string())?;
    Ok(rows.iter().filter_map(|row| row.i64("id")).collect())
}

async fn integrity(db: &dyn Db) -> Result<String, String> {
    let rows = db
        .query("PRAGMA integrity_check", Vec::new())
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.first() else {
        return Err("PRAGMA integrity_check returned no row".to_string());
    };
    let Some(column) = row.column_names().first().cloned() else {
        return Err("PRAGMA integrity_check returned a row with no columns".to_string());
    };
    row.text(&column)
        .ok_or_else(|| format!("PRAGMA integrity_check's first column is not TEXT: {row:?}"))
}

// ---------------------------------------------------------------------------
// RawDb — the `Db` seam over a hand-built connection (the inversion arm only).
// Copied from `turso_capabilities.rs` rather than shared, because integration
// test binaries cannot import one another's modules.
// ---------------------------------------------------------------------------

struct RawDb {
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
    let columns = Arc::new(rows.column_names());
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

// ---------------------------------------------------------------------------
// The re-exec'd child
// ---------------------------------------------------------------------------

const ENV_CHILD: &str = "MEM53_CHILD";
const ENV_ROOT: &str = "MEM53_ROOT";
const ENV_DB: &str = "MEM53_DB";
const ENV_KEY: &str = "MEM53_KEY";
const ENV_ENCRYPTION: &str = "MEM53_ENCRYPTION";
const ENV_MP: &str = "MEM53_MP";
const ENV_MODE: &str = "MEM53_MODE";

/// The child entry point. INERT unless `MEM53_CHILD` is set, so a normal suite run
/// (even with `--ignored`) cannot do anything through it by accident.
///
/// Protocol (stdout, one line each), every line literal:
/// - `CHILD-OPEN-OK` or `CHILD-OPEN-ERR <text>`;
/// - `CHILD-IDS <csv>` or `CHILD-READ-ERR <text>`;
/// - `CHILD-INTEGRITY <verdict>` or `CHILD-INTEGRITY-ERR <text>`;
/// - writable mode only: `CHILD-WRITE ok` / `CHILD-WRITE-ERR <text>`, then a second
///   integrity line;
/// - `CHILD-DONE` last, always.
///
/// The child exits 0 even when the engine refused it: the refusal is the datum.
#[test]
#[ignore = "issue 53 re-exec'd measurement child; spawned by the parent probe with MEM53_CHILD set"]
fn issue53_child_entry() {
    if std::env::var(ENV_CHILD).as_deref() != Ok("1") {
        return;
    }
    let db_path = PathBuf::from(std::env::var(ENV_DB).expect("MEM53_DB"));
    let root = PathBuf::from(std::env::var(ENV_ROOT).expect("MEM53_ROOT"));
    let key = std::env::var(ENV_KEY).expect("MEM53_KEY");
    let encryption = std::env::var(ENV_ENCRYPTION).as_deref() == Ok("1");
    let multiprocess_wal = std::env::var(ENV_MP).as_deref() == Ok("1");
    let mode = std::env::var(ENV_MODE).expect("MEM53_MODE");
    let cell = Cell {
        encryption,
        multiprocess_wal,
        read_only: mode == "ro",
    };
    let config = probe_config(&root, encryption, &key);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    runtime.block_on(async move {
        println!("CHILD-CELL {}", cell.label());
        let db = match open_cell(&config, &db_path, cell).await {
            Ok(db) => {
                println!("CHILD-OPEN-OK");
                db
            }
            Err(error) => {
                println!("CHILD-OPEN-ERR {error}");
                println!("CHILD-DONE");
                return;
            }
        };

        match row_ids(db.as_ref()).await {
            Ok(ids) => println!(
                "CHILD-IDS {}",
                ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
            ),
            Err(error) => println!("CHILD-READ-ERR {error}"),
        }
        match integrity(db.as_ref()).await {
            Ok(verdict) => println!("CHILD-INTEGRITY {verdict}"),
            Err(error) => println!("CHILD-INTEGRITY-ERR {error}"),
        }

        if !cell.read_only {
            match insert_row(db.as_ref(), CHILD_ROW, "written-by-child-rw").await {
                Ok(()) => {
                    println!("CHILD-WRITE ok");
                    match row_ids(db.as_ref()).await {
                        Ok(ids) => println!(
                            "CHILD-IDS-AFTER {}",
                            ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
                        ),
                        Err(error) => println!("CHILD-READ-ERR {error}"),
                    }
                    match integrity(db.as_ref()).await {
                        Ok(verdict) => println!("CHILD-INTEGRITY-AFTER {verdict}"),
                        Err(error) => println!("CHILD-INTEGRITY-ERR {error}"),
                    }
                }
                Err(error) => println!("CHILD-WRITE-ERR {error}"),
            }
        }
        println!("CHILD-DONE");
    });
}

// ---------------------------------------------------------------------------
// Child process driver (bounded reads, kill+reap on every path)
// ---------------------------------------------------------------------------

/// A spawned child plus its line channel. Mirrors `open_guard.rs::LegacyHolder`:
/// the reader runs on its own thread so a child that stops talking becomes a
/// named, bounded failure rather than a hung suite, and `Drop` kills + reaps.
struct ProbeChild {
    child: Option<std::process::Child>,
    lines: std::sync::mpsc::Receiver<String>,
}

impl ProbeChild {
    fn spawn(test: &str, envs: &[(&str, String)]) -> Self {
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("the running test binary"));
        command
            .arg("--exact")
            .arg(test)
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(ENV_CHILD, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            // stderr is INHERITED: a panic inside the child lands in this binary's
            // own stderr (and therefore in the captured log) instead of being lost.
            .stderr(std::process::Stdio::inherit());
        for (name, value) in envs {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("spawn the issue-53 probe child");
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
            child: Some(child),
            lines,
        }
    }

    /// The child's next stdout line, echoed into the parent's own log. `Err` is a
    /// timeout or an early EOF — both are recorded as the probe's failure, never a
    /// silent pass.
    fn next_line(&self, side: &str) -> Result<String, String> {
        match self.lines.recv_timeout(PROBE_TIMEOUT) {
            Ok(line) => {
                println!("[{side}] {line}");
                Ok(line)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "the {side} child produced no line for {PROBE_TIMEOUT:?}: measured a BLOCK, not a verdict"
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(format!(
                "the {side} child closed stdout without finishing its protocol: it exited early"
            )),
        }
    }

    /// Runs the child to `CHILD-DONE`, then reaps it and requires a successful
    /// exit. Returns every protocol line, or the first failure.
    fn finish(mut self, side: &str) -> Result<Vec<String>, String> {
        let mut collected = Vec::new();
        loop {
            let line = self.next_line(side)?;
            let done = line.ends_with("CHILD-DONE");
            collected.push(line);
            if done {
                break;
            }
        }
        let mut child = self.child.take().expect("child handle");
        let status = child.wait().expect("reap the issue-53 probe child");
        if !status.success() {
            return Err(format!(
                "the {side} child must exit successfully; got {status:?} (a panic inside the engine is a finding, and its text is on this binary's stderr)"
            ));
        }
        Ok(collected)
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn child_envs(root: &Path, db: &Path, key: &str, cell: Cell) -> Vec<(&'static str, String)> {
    vec![
        (ENV_ROOT, root.display().to_string()),
        (ENV_DB, db.display().to_string()),
        (ENV_KEY, key.to_string()),
        (
            ENV_ENCRYPTION,
            if cell.encryption { "1" } else { "0" }.to_string(),
        ),
        (
            ENV_MP,
            if cell.multiprocess_wal { "1" } else { "0" }.to_string(),
        ),
        (
            ENV_MODE,
            if cell.read_only { "ro" } else { "rw" }.to_string(),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The probe body
// ---------------------------------------------------------------------------

/// The literal per-probe record plus its first failure (if any).
struct ProbeRecord {
    name: String,
    lines: Vec<String>,
    failure: Option<(String, String)>,
}

impl ProbeRecord {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            lines: Vec::new(),
            failure: None,
        }
    }

    fn ok(&mut self, line: impl Into<String>) {
        let line = line.into();
        println!("[{}] {line}", self.name);
        self.lines.push(line);
    }

    fn fail(&mut self, side: &str, error: impl Into<String>) {
        let error = error.into();
        self.ok(format!("FAIL side={side} error={error}"));
        if self.failure.is_none() {
            self.failure = Some((side.to_string(), error));
        }
    }

    fn verdict_line(&self) -> String {
        match &self.failure {
            None => format!("PROBE {} RESULT: PASS", self.name),
            Some((side, error)) => {
                format!("PROBE {} RESULT: FAIL side={side} error={error}", self.name)
            }
        }
    }

    /// Prints the record and returns the one-line verdict (also written to
    /// `MEM53_REPORT` when the runner sets it, so the evidence file does not depend
    /// on scraping cargo output).
    fn finish(self) -> String {
        let verdict = self.verdict_line();
        println!("{verdict}");
        if let Some(path) = std::env::var_os("MEM53_REPORT")
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = writeln!(file, "{verdict}");
        }
        verdict
    }
}

/// When the first writer's handle is released relative to the second process's
/// open.
///
/// The gate's G1 shape keeps the writer live (the real mixed-process window). The
/// `ClosedBeforeSecondOpen` shape is the one the archived ingest's `nbackfills > 0`
/// record points at: a writer that cleanly closed can leave a `-tshm` a later
/// opener must backfill from the (encrypted) `-wal`, and that backfill is the
/// hypothesised corruption path. Both are measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriterLifetime {
    LiveDuringSecondOpen,
    ClosedBeforeSecondOpen,
}

/// Whether the probe's first writer creates the database, or inherits one written
/// and cleanly closed by a *different* option tuple.
///
/// `After(cell)` is the upgrade path: a database created by `cell` (multiprocess
/// WAL **off**, encryption on, in `G1C`) is reopened by the production tuple
/// (multiprocess WAL **on**). That open has no `-tshm` yet, so the engine must
/// create and **backfill** it from the existing `-wal` — the exact `nbackfills > 0`
/// shape the archived ingest record points at, and the shape every existing user
/// would take if issue 43's W6 ever dropped the flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opening {
    Fresh,
    After(Cell),
}

/// The file set a probe step left behind: only the database and its sidecars, so a
/// log line is short enough to quote. Sizes are the point — a `-tshm` that grows or
/// a `-wal` that was truncated says which path ran.
fn inventory(root: &Path) -> String {
    let mut entries = Vec::new();
    if let Ok(dir) = std::fs::read_dir(root) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("probe.db") || name == ".wal-mode" {
                let len = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                entries.push(format!("{name}={len}"));
            }
        }
    }
    entries.sort();
    entries.join(" ")
}

/// The whole gate sequence for one cell.
async fn run_store_probe(
    name: &str,
    cell: Cell,
    lifetime: WriterLifetime,
    opening: Opening,
) -> String {
    let root = common::temp_dir(&format!("issue53-{name}"));
    let key = common::hex_key_64();
    let config = probe_config(&root, cell.encryption, &key);
    let db_path = root.join("probe.db");
    let mut record = ProbeRecord::new(name);
    record.ok(format!("CELL {}", cell.label()));
    record.ok(format!("LIFETIME {lifetime:?}"));
    record.ok(format!("OPENING {opening:?}"));
    record.ok(format!("ROOT {}", root.display()));

    // -- 0. Optional: a different option tuple creates the database and closes it
    // cleanly, so the probe's open is an **upgrade** that must backfill a `-tshm`.
    let previous_writer = if let Opening::After(previous) = opening {
        record.ok(format!("PRIOR-CELL {}", previous.label()));
        match open_cell(&config, &db_path, previous).await {
            Ok(prior) => {
                record.ok("PRIOR-OPEN ok");
                match prior.execute(CREATE_PROBE, Vec::new()).await {
                    Ok(_) => record.ok("PRIOR-CREATE ok"),
                    Err(error) => {
                        record.fail("prior-create", error.to_string());
                        return record.finish();
                    }
                }
                if let Err(error) = prior.execute(CREATE_FILLER, Vec::new()).await {
                    record.fail("prior-create-filler", error.to_string());
                    return record.finish();
                }
                for id in PARENT_ROWS {
                    if let Err(error) = insert_row(prior.as_ref(), id, "written-by-prior").await {
                        record.fail("prior-commit", error);
                        return record.finish();
                    }
                }
                for id in 0..FILLER_ROWS {
                    if let Err(error) = insert_filler(prior.as_ref(), id).await {
                        record.fail("prior-filler", error);
                        return record.finish();
                    }
                }
                record.ok(format!("INVENTORY-PRIOR {}", inventory(&root)));
                drop(prior);
                record.ok("PRIOR-CLOSED clean");
                true
            }
            Err(error) => {
                record.fail("prior-open", error);
                return record.finish();
            }
        }
    } else {
        false
    };

    // -- 1. The writer opens and commits.
    let mut writer = match open_cell(&config, &db_path, cell).await {
        Ok(db) => {
            record.ok("WRITER-OPEN ok");
            Some(db)
        }
        Err(error) => {
            record.fail("writer-open", error);
            return record.finish();
        }
    };
    {
        let writer_ref = writer.as_ref().expect("writer handle");
        if previous_writer {
            record.ok("WRITER-OPENED-EXISTING ok");
        } else {
            match writer_ref.execute(CREATE_PROBE, Vec::new()).await {
                Ok(_) => record.ok("WRITER-CREATE ok"),
                Err(error) => {
                    record.fail("writer-create", error.to_string());
                    return record.finish();
                }
            }
            if let Err(error) = writer_ref.execute(CREATE_FILLER, Vec::new()).await {
                record.fail("writer-create-filler", error.to_string());
                return record.finish();
            }
            for id in PARENT_ROWS {
                if let Err(error) = insert_row(writer_ref.as_ref(), id, "written-by-parent").await {
                    record.fail("writer-commit", error);
                    return record.finish();
                }
            }
            for id in 0..FILLER_ROWS {
                if let Err(error) = insert_filler(writer_ref.as_ref(), id).await {
                    record.fail("writer-filler", error);
                    return record.finish();
                }
            }
        }
        match row_ids(writer_ref.as_ref()).await {
            Ok(ids) => record.ok(format!("WRITER-IDS {ids:?}")),
            Err(error) => {
                record.fail("writer-read", error);
                return record.finish();
            }
        }
        match filler_shape(writer_ref.as_ref()).await {
            Ok((count, len)) => record.ok(format!("WRITER-FILLER rows={count} max_len={len}")),
            Err(error) => record.fail("writer-filler-read", error),
        }
        match integrity(writer_ref.as_ref()).await {
            Ok(verdict) => record.ok(format!("WRITER-INTEGRITY {verdict}")),
            Err(error) => record.fail("writer-integrity", error),
        }
        record.ok(format!("INVENTORY-WRITTEN {}", inventory(&root)));
    }

    if lifetime == WriterLifetime::ClosedBeforeSecondOpen {
        // The backfill shape: a clean close, so a later opener finds a `-tshm` that
        // may be behind the `-wal` and must backfill it.
        drop(writer.take());
        record.ok("WRITER-CLOSED clean");
        record.ok(format!("INVENTORY-CLOSED {}", inventory(&root)));
    }

    // -- 2. A second process opens read-only, then a third opens writable. With
    // `LiveDuringSecondOpen` both do so while the writer is still live.
    for read_only in [true, false] {
        let child_cell = cell.with_read_only(read_only);
        let side = if read_only { "child-ro" } else { "child-rw" };
        let child = ProbeChild::spawn(
            "issue53_child_entry",
            &child_envs(&root, &db_path, &key, child_cell),
        );
        match child.finish(side) {
            Ok(lines) => {
                record.lines.extend(lines.iter().cloned());
                for line in &lines {
                    if line.contains("OPEN-ERR")
                        || line.contains("READ-ERR")
                        || line.contains("INTEGRITY-ERR")
                        || line.contains("WRITE-ERR")
                    {
                        record.fail(side, line.trim().to_string());
                    }
                }
                if let Some(line) = lines.iter().find(|line| line.starts_with("CHILD-OPEN-OK")) {
                    record.ok(format!("{side}: {line}"));
                }
            }
            Err(error) => record.fail(side, error),
        }
    }

    // -- 3. The parent commits after the second process has been and gone, then
    // checks integrity on its own handle. In the closed-writer shape this is a
    // *fresh open* of the same file (the third process's turn).
    let after = match lifetime {
        WriterLifetime::LiveDuringSecondOpen => writer.take(),
        WriterLifetime::ClosedBeforeSecondOpen => match open_cell(&config, &db_path, cell).await {
            Ok(db) => {
                record.ok("POST-CHILD-OPEN ok");
                Some(db)
            }
            Err(error) => {
                record.fail("post-child-open", error);
                None
            }
        },
    };
    match after {
        None => {}
        Some(after) => {
            match insert_row(after.as_ref(), PARENT_AFTER_ROW, "written-by-parent-after").await {
                Ok(()) => record.ok(format!("POST-CHILD-COMMIT ok (id {PARENT_AFTER_ROW})")),
                Err(error) => record.fail("parent-commit", error),
            }
            match integrity(after.as_ref()).await {
                Ok(verdict) => record.ok(format!("POST-CHILD-INTEGRITY {verdict}")),
                Err(error) => record.fail("parent-integrity", error),
            }
            record.ok(format!("INVENTORY-POST-CHILD {}", inventory(&root)));
            drop(after);
        }
    }

    // -- 4. Reopen the same file and ask the engine for its verdict.
    match open_cell(&config, &db_path, cell).await {
        Ok(reopened) => {
            record.ok("REOPEN ok");
            match row_ids(reopened.as_ref()).await {
                Ok(ids) => {
                    let mut expected = PARENT_ROWS.to_vec();
                    expected.push(CHILD_ROW);
                    expected.push(PARENT_AFTER_ROW);
                    expected.sort_unstable();
                    if ids == expected {
                        record.ok(format!("REOPEN-IDS {ids:?} (exact)"));
                    } else {
                        record.fail("reopen-ids", format!("expected {expected:?}, got {ids:?}"));
                    }
                }
                Err(error) => record.fail("reopen-read", error),
            }
            match filler_shape(reopened.as_ref()).await {
                Ok((count, len)) if count == FILLER_ROWS && len == FILLER_BYTES as i64 => {
                    record.ok(format!("REOPEN-FILLER rows={count} max_len={len}"))
                }
                Ok((count, len)) => record.fail(
                    "reopen-filler",
                    format!(
                        "expected {FILLER_ROWS} rows of {FILLER_BYTES} bytes, got {count} rows, max_len {len}"
                    ),
                ),
                Err(error) => record.fail("reopen-filler", error),
            }
            match integrity(reopened.as_ref()).await {
                Ok(verdict) => record.ok(format!("REOPEN-INTEGRITY {verdict}")),
                Err(error) => record.fail("reopen-integrity", error),
            }
        }
        Err(error) => record.fail("reopen-open", error),
    }

    // -- 5. Non-vacuity for the encryption half. A cell that says
    // `encryption=true` only means something if a keyless read of the same file is
    // refused; otherwise the whole probe measured a plaintext database while
    // claiming the encrypted cell. The mirror check runs for the plaintext control:
    // it must allow the keyless read.
    let keyless_config = probe_config(&root, false, "");
    let keyless_cell = cell.with_read_only(true);
    let keyless_refused = match open_cell(&keyless_config, &db_path, keyless_cell).await {
        Ok(db) => match row_ids(db.as_ref()).await {
            Ok(ids) => {
                record.ok(format!("KEYLESS-READ ok ids={ids:?}"));
                false
            }
            Err(error) => {
                record.ok(format!("KEYLESS-READ ERR {error}"));
                true
            }
        },
        Err(error) => {
            record.ok(format!("KEYLESS-OPEN ERR {error}"));
            true
        }
    };
    if cell.encryption && !keyless_refused {
        record.fail(
            "non-vacuity",
            "a keyless read of the encrypted database succeeded: this cell did not measure encryption",
        );
    }
    if !cell.encryption && keyless_refused {
        record.fail(
            "non-vacuity",
            "the plaintext control refused a keyless read: the control is not plaintext",
        );
    }
    record.ok(format!("INVENTORY-FINAL {}", inventory(&root)));

    record.finish()
}

// ---------------------------------------------------------------------------
// The probes
// ---------------------------------------------------------------------------

fn run_probe(name: &str, cell: Cell, lifetime: WriterLifetime, opening: Opening) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("probe runtime");
    runtime.block_on(run_store_probe(name, cell, lifetime, opening))
}

const ENCRYPTED_MULTIPROCESS: Cell = Cell {
    encryption: true,
    multiprocess_wal: true,
    read_only: false,
};

const PLAINTEXT_MULTIPROCESS: Cell = Cell {
    encryption: false,
    multiprocess_wal: true,
    read_only: false,
};

const ENCRYPTED_SINGLE_PROCESS: Cell = Cell {
    encryption: true,
    multiprocess_wal: false,
    read_only: false,
};

const PLAINTEXT_SINGLE_PROCESS: Cell = Cell {
    encryption: false,
    multiprocess_wal: false,
    read_only: false,
};

/// G1 — a **fresh** encrypted database with multiprocess WAL, writer live during
/// the second process's open. The cell our tree ships on every non-Windows platform
/// when `database_encryption_enabled` is on, and the cell the vault is forced into.
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_g1_fresh_encrypted_multiprocess_wal() {
    run_probe(
        "G1",
        ENCRYPTED_MULTIPROCESS,
        WriterLifetime::LiveDuringSecondOpen,
        Opening::Fresh,
    );
}

/// G1C — the **upgrade** shape: a database written and cleanly closed by
/// multiprocess-WAL-**off** encryption (the inversion arm's tuple, which is also
/// what a Windows run or an older build produces), then opened by the production
/// tuple with multiprocess WAL **on**. That open has no `-tshm` and must create and
/// backfill one from the existing encrypted `-wal` — the `nbackfills > 0` shape the
/// issue's only harm evidence names. Not one of the gate's four cells; recorded as
/// an extension because it is the one reachable history that forces a backfill.
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_g1c_upgrade_from_single_process_encrypted() {
    run_probe(
        "G1C",
        ENCRYPTED_MULTIPROCESS,
        WriterLifetime::LiveDuringSecondOpen,
        Opening::After(ENCRYPTED_SINGLE_PROCESS),
    );
}

/// G3C — the plaintext control for G1C: created and closed with multiprocess WAL
/// off, then opened with it on. Separates "the backfill itself is broken" from
/// "the backfill of *encrypted* pages is broken".
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_g3c_upgrade_from_single_process_plaintext() {
    run_probe(
        "G3C",
        PLAINTEXT_MULTIPROCESS,
        WriterLifetime::LiveDuringSecondOpen,
        Opening::After(PLAINTEXT_SINGLE_PROCESS),
    );
}

/// G1B — the same cell, but the writer **cleanly closes** before the second process
/// opens. This is the `nbackfills > 0` shape the archived ingest points at: the
/// `-tshm` is behind the `-wal` and the next opener must backfill it from pages the
/// cipher produced.
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_g1b_backfill_after_clean_close() {
    run_probe(
        "G1B",
        ENCRYPTED_MULTIPROCESS,
        WriterLifetime::ClosedBeforeSecondOpen,
        Opening::Fresh,
    );
}

/// G3 — the control: identical probe, encryption **off**, multiprocess WAL on. This
/// is the shipping default for the five store families. If it fails too, the flag
/// is the problem and issue 53 collapses into issue 45.
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_g3_plaintext_multiprocess_wal() {
    run_probe(
        "G3",
        PLAINTEXT_MULTIPROCESS,
        WriterLifetime::LiveDuringSecondOpen,
        Opening::Fresh,
    );
}

/// G3B — the control's backfill shape, paired with G1B.
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_g3b_plaintext_backfill_after_clean_close() {
    run_probe(
        "G3B",
        PLAINTEXT_MULTIPROCESS,
        WriterLifetime::ClosedBeforeSecondOpen,
        Opening::Fresh,
    );
}

/// The inversion arm: encryption **on**, multiprocess WAL **off**. Must pass. If it
/// fails too, the encryption half is the problem and the issue is mistitled.
///
/// The writer closes before the second process opens **by necessity, not by
/// choice**: without the flag a writable opener takes an exclusive file lock for
/// the life of its handle, so a concurrent writable second process is refused by
/// the engine (`File is locked by another process`) — the very serialisation the
/// flag removes, and the reason this cell cannot be run concurrently. The
/// concurrent attempt is recorded separately by
/// [`issue53_observation_writable_second_opener_without_the_flag`].
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_inversion_encrypted_single_process() {
    run_probe(
        "INVERSION",
        ENCRYPTED_SINGLE_PROCESS,
        WriterLifetime::ClosedBeforeSecondOpen,
        Opening::Fresh,
    );
}

/// Not an arm: the observation that makes the inversion arm's sequential shape
/// honest. With multiprocess WAL off and a live writer, a writable second process
/// is refused by the engine's own single-process lock. Printed, never asserted — a
/// different answer here would itself be the finding.
#[test]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
fn issue53_observation_writable_second_opener_without_the_flag() {
    let root = common::temp_dir("issue53-INVERSION-CONCURRENT");
    let key = common::hex_key_64();
    let config = probe_config(&root, true, &key);
    let db_path = root.join("probe.db");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("probe runtime");
    runtime.block_on(async {
        let writer = open_cell(&config, &db_path, ENCRYPTED_SINGLE_PROCESS)
            .await
            .expect("the single-process writer opens");
        writer
            .execute(CREATE_PROBE, Vec::new())
            .await
            .expect("create probe_rows");
        insert_row(writer.as_ref(), 1, "written-by-parent")
            .await
            .expect("commit");
        let child = ProbeChild::spawn(
            "issue53_child_entry",
            &child_envs(&root, &db_path, &key, ENCRYPTED_SINGLE_PROCESS),
        );
        match child.finish("child-rw-concurrent") {
            Ok(lines) => {
                for line in lines {
                    if line.contains("CHILD-OPEN-ERR") {
                        println!(
                            "OBSERVATION INVERSION-CONCURRENT: writable second opener refused \
                             while the first handle is live (single-process semantics): {line}"
                        );
                    }
                }
            }
            Err(error) => println!("OBSERVATION INVERSION-CONCURRENT: harness failure: {error}"),
        }
        drop(writer);
    });
}
