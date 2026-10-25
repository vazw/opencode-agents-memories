#![allow(dead_code)]

//! Std-only helpers shared by mem-profile integration tests (house law:
//! `docs/agents/testing.md`) — temp dirs under `std::env::temp_dir()` only,
//! injected seams instead of env mutation, no dev-dependencies, no sleeps for
//! synchronization.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mem_store::config::StoreConfig;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory under `std::env::temp_dir()`:
/// `mem-profile-w31-{prefix}-{pid}-{counter}-{nanos}`.
pub fn temp_dir(prefix: &str) -> PathBuf {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "mem-profile-w31-{prefix}-{}-{counter}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Best-effort recursive removal (restore an owner-writable mode first in
/// case a test left a hostile mode behind).
pub fn cleanup_temp_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let _ = fs::remove_dir_all(dir);
}

/// Hermetic store config over a fresh temp dir (encryption disabled).
pub fn store_config(storage_path: PathBuf) -> StoreConfig {
    StoreConfig {
        storage_path,
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// Home/XDG isolation for child processes (upstream service-worker parity):
/// a child must never reach the real `~/.opencode-mem`.
pub fn isolated_env(storage: &Path) -> Vec<(String, String)> {
    vec![
        ("HOME".to_string(), storage.display().to_string()),
        (
            "XDG_DATA_HOME".to_string(),
            storage.join("xdg").display().to_string(),
        ),
        ("USERPROFILE".to_string(), storage.display().to_string()),
    ]
}

// ---------------------------------------------------------------------------
// Fixture-child recipe (docs/agents/testing.md)
// ---------------------------------------------------------------------------

pub const FIXTURE_CHILD_ENV: &str = "MEM_FIXTURE_CHILD";
pub const FIXTURE_OUT_ENV: &str = "MEM_FIXTURE_OUT";
pub const FIXTURE_CHILD_TEST_NAME: &str = "fixture_child_entry";

/// Spawns this test binary via self re-exec
/// (`current_exe()` + `--exact fixture_child_entry --ignored`), selects the
/// child mode with [`FIXTURE_CHILD_ENV`], redirects stdout/stderr to files
/// (never pipes) and returns the live [`std::process::Child`] for handshakes.
pub fn spawn_fixture_child(
    mode: &str,
    out_path: &Path,
    extra_env: &[(&str, &str)],
) -> std::io::Result<std::process::Child> {
    let stdout_path = out_path.with_extension("stdout");
    let stderr_path = out_path.with_extension("stderr");
    Command::new(std::env::current_exe()?)
        .args(["--exact", FIXTURE_CHILD_TEST_NAME, "--ignored"])
        .env(FIXTURE_CHILD_ENV, mode)
        .env(FIXTURE_OUT_ENV, out_path)
        .envs(extra_env.iter().copied())
        .stdout(Stdio::from(fs::File::create(&stdout_path)?))
        .stderr(Stdio::from(fs::File::create(&stderr_path)?))
        .spawn()
}

/// Spawns a child and bounded-reaps it (poll every 5 ms, kill + reap on the
/// deadline). Returns `(status, stdout, stderr)`.
pub fn run_fixture_child(
    mode: &str,
    out_path: &Path,
    extra_env: &[(&str, &str)],
    timeout: Duration,
) -> std::io::Result<(std::process::ExitStatus, String, String)> {
    let (status, stdout, stderr) = run_fixture_child_status(mode, out_path, extra_env, timeout)?;
    Ok((status, stdout, stderr))
}

fn run_fixture_child_status(
    mode: &str,
    out_path: &Path,
    extra_env: &[(&str, &str)],
    timeout: Duration,
) -> std::io::Result<(std::process::ExitStatus, String, String)> {
    let mut child = spawn_fixture_child(mode, out_path, extra_env)?;
    let status = wait_bounded(&mut child, timeout)?;
    Ok((
        status,
        read_optional(&out_path.with_extension("stdout")),
        read_optional(&out_path.with_extension("stderr")),
    ))
}

/// Bounded reap: poll `try_wait()` every 5 ms; on the deadline `kill()` then
/// `wait()` (never an orphan) and surface a `TimedOut` error carrying the
/// captured stderr text.
pub fn wait_bounded(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "fixture child exceeded the bounded deadline and was killed",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn read_optional(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Writes `contents` to `<dir>/<name>`, creating missing parents.
pub fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(&path, contents).expect("write test file");
    path
}

// ---------------------------------------------------------------------------
// Barrier files (bounded, never sleep-synchronized)
// ---------------------------------------------------------------------------

/// Path of the handshake barrier `<dir>/hs.<name>`.
pub fn barrier_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("hs.{name}"))
}

/// Spawns a background thread that waits (bounded, 5 ms poll) for a barrier
/// file to appear. Returns a join handle resolving to `Ok(())` on appearance
/// or `Err` on timeout.
pub fn expect_barrier(
    dir: PathBuf,
    name: String,
    timeout: Duration,
) -> std::thread::JoinHandle<bool> {
    std::thread::spawn(move || wait_for_barrier(&dir, &name, timeout))
}

/// Bounded barrier wait (5 ms poll, ≤ 30 s by convention).
pub fn wait_for_barrier(dir: &Path, name: &str, timeout: Duration) -> bool {
    let path = barrier_path(dir, name);
    let started = Instant::now();
    while !path.exists() {
        if started.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// Clears every `hs.*` barrier file in a storage dir.
pub fn clear_barriers(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("hs.") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NDJSON event files (children append one JSON object per line)
// ---------------------------------------------------------------------------

/// Parses a child-written NDJSON event file into its JSON objects. An
/// unparseable line is a test failure (never a silent skip).
pub fn read_events(path: &Path) -> Vec<serde_json::Value> {
    let raw = fs::read_to_string(path).unwrap_or_default();
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|error| panic!("unparseable child event line {line:?}: {error}"))
        })
        .collect()
}

/// Appends one NDJSON line to an event file in a SINGLE write syscall.
///
/// The parent polls these files while a child appends; a formatted
/// `write_fmt` can split the JSON and its newline into separate writes, which
/// a concurrent reader then sees as a torn line. One `write_all` of the
/// preformatted line ("same write-fault discipline as production: bounded,
/// appended per event") removes that window.
pub fn append_event(path: &Path, value: &serde_json::Value) {
    use std::io::Write;
    let line = format!("{value}\n");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open event file");
    file.write_all(line.as_bytes()).expect("append event");
    file.flush().expect("flush event");
}

// ---------------------------------------------------------------------------
// Coordination-DB helpers (W3.1 step 10) — open the DB DIRECTLY via the
// factory (never the caching ConnectionManager) so a parent can observe the
// lock row while a child is parked.
// ---------------------------------------------------------------------------

pub use mem_profile::learning_lock::PROFILE_LEARNING_COORDINATION_DB;

/// Opens the coordination DB directly for fixture setup / assertions.
pub async fn with_coordination_db<T, F, Fut>(dir: &Path, operation: F) -> T
where
    F: FnOnce(mem_store::db::DbHandle) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let mut config = store_config(dir.to_path_buf());
    config.database_encryption_enabled = false;
    let options = mem_store::db::ConnectOptions::for_config(
        &config,
        mem_store::platform::Platform::current(),
    );
    use mem_store::db::ConnectFactory as _;
    let path = dir.join(PROFILE_LEARNING_COORDINATION_DB);
    let db = mem_store::db::turso_connect_factory()
        .connect(path, options)
        .await
        .expect("open coordination db");
    db.execute("PRAGMA busy_timeout = 5000", Vec::new())
        .await
        .expect("busy timeout");
    let result = operation(db.clone()).await;
    drop(db);
    result
}

/// A planted coordination row (fixture setup).
#[derive(Debug, Clone)]
pub struct PlantedOwner {
    pub token: String,
    pub pid: u32,
    pub boot_id: Option<String>,
    pub starttime: Option<String>,
    pub acquired_at: i64,
}

impl Default for PlantedOwner {
    fn default() -> Self {
        Self {
            token: "planted-owner-token".to_string(),
            pid: 4_194_304,
            boot_id: None,
            starttime: None,
            acquired_at: mem_profile::ids::now_millis(),
        }
    }
}

/// Upstream `plantOwner`: `INSERT OR REPLACE` then a truncating WAL checkpoint
/// so a freshly spawned child always sees the row.
pub async fn plant_coordination_owner(dir: &Path, owner: &PlantedOwner) {
    with_coordination_db(dir, |db| async move {
        db.execute(mem_profile::learning_lock::LOCK_TABLE_DDL, Vec::new())
            .await
            .expect("create lock table");
        db.execute(
            "INSERT OR REPLACE INTO profile_learning_lock
              (name, owner_token, pid, boot_id, starttime, acquired_at)
            VALUES ('profile-learning', ?, ?, ?, ?, ?)",
            vec![
                mem_store::db::Value::Text(owner.token.clone()),
                mem_store::db::Value::Integer(i64::from(owner.pid)),
                owner.boot_id.clone().into(),
                owner.starttime.clone().into(),
                mem_store::db::Value::Integer(owner.acquired_at),
            ],
        )
        .await
        .expect("plant owner row");
        db.query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
            .await
            .expect("wal checkpoint");
    })
    .await;
}

/// The current `owner_token`, or `None` when no row exists.
pub async fn current_owner_token(dir: &Path) -> Option<String> {
    with_coordination_db(dir, |db| async move {
        // A fresh coordination DB has no lock table until an acquire creates
        // it; creating it here makes the observation well-defined.
        let _ = db
            .execute(mem_profile::learning_lock::LOCK_TABLE_DDL, Vec::new())
            .await;
        let row = mem_store::db::query_one(
            db.as_ref(),
            "SELECT owner_token FROM profile_learning_lock WHERE name = 'profile-learning'",
            Vec::new(),
        )
        .await
        .expect("select owner token");
        row.and_then(|row| row.text("owner_token"))
    })
    .await
}

/// Truncating WAL checkpoint (fixture hygiene between phases).
pub async fn wal_checkpoint(dir: &Path) {
    with_coordination_db(dir, |db| async move {
        db.query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
            .await
            .expect("wal checkpoint");
    })
    .await;
}

/// Writes one of the child's NDJSON-adjacent JSON report files
/// (`<out>.json`).
pub fn write_child_report(out_path: &Path, report: &serde_json::Value) {
    fs::write(
        out_path.with_extension("json"),
        serde_json::to_vec(report).expect("serialize child report"),
    )
    .expect("write child report");
}

/// Reads a child's JSON report (`<out>.json`).
pub fn read_child_report(out_path: &Path) -> serde_json::Value {
    let path = out_path.with_extension("json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|error| {
        let stderr = fs::read_to_string(out_path.with_extension("stderr")).unwrap_or_default();
        panic!(
            "child produced no report at {} ({error}); stderr: {stderr}",
            path.display()
        )
    });
    serde_json::from_str(&raw).expect("child report is JSON")
}
