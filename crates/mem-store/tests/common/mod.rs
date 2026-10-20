// Test helpers for mem-store integration tests.
//
//! House law (`docs/agents/testing.md`): temp dirs under `std::env::temp_dir()`
//! only, injected seams instead of env mutation, no dev-dependencies
//! (this file relies on the crate's normal dependencies), no sleeps for
//! synchronization. Helpers grow per wave; the W1.2 first cut ships only
//! `temp_dir`.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory under `std::env::temp_dir()` (never the user home,
/// never a shared path): `{temp}/mem-store-w12-{pid}-{counter}-{nanos}`.
pub fn temp_dir(prefix: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "mem-store-w12-{prefix}-{}-{counter}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// ---------------------------------------------------------------------------
// Cleanup (upstream `cleanupTursoTestDirectory`)
// ---------------------------------------------------------------------------

use std::path::Path;

/// Upstream `cleanupTursoTestDirectory` minus the Bun/GC pass and the
/// process-wide cache close (Rust: handles release on Drop; the manager is
/// owned per-test). `rmSync(recursive, force)` ≙ `remove_dir_all`; Windows
/// libsql handles can outlive a rename, so the removal runs under the
/// system retry with the upstream 1-retry budget. A lock error with the
/// budget exhausted is best-effort hygiene (warn + tolerate, like upstream
/// `isExhaustedWindowsLock`); any other error propagates.
pub fn cleanup_temp_dir(dir: &Path) -> std::io::Result<()> {
    let ctx = mem_store::sqlite_files::system_retry_context();
    mem_store::sqlite_files::with_sqlite_file_lock_retry(|| std::fs::remove_dir_all(dir), 1, &ctx)
}

// ---------------------------------------------------------------------------
// Shared connection-manager fixtures (blueprint step 4)
// ---------------------------------------------------------------------------

use mem_store::config::StoreConfig;
use mem_store::db::{
    BatchMode, BoxFuture, ConnectFactory, ConnectOptions, Db, DbHandle, ExecResult, Row, Statement,
    Value,
};
use mem_store::error::StoreError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64};

/// Hermetic store config over a fresh temp dir (encryption disabled).
pub fn store_config(prefix: &str) -> StoreConfig {
    StoreConfig {
        storage_path: temp_dir(prefix),
        embedding_dimensions: 2,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

// ---------------------------------------------------------------------------
// FakeDb (blueprint step 4)
// ---------------------------------------------------------------------------

/// Implements `Db`, records executed SQL, flips `on_drop` when the LAST
/// handle is released (the `Arc<dyn Db>` refcount reaching zero drops it).
#[derive(Debug)]
pub struct FakeDb {
    core: Arc<FakeCore>,
    pub on_drop: Arc<AtomicBool>,
}

#[derive(Default, Debug)]
pub struct FakeCore {
    pub sqls: std::sync::Mutex<Vec<String>>,
}

impl FakeDb {
    pub fn new() -> Self {
        Self::attach(Arc::default(), Arc::new(AtomicBool::new(false)))
    }

    pub fn attach(core: Arc<FakeCore>, on_drop: Arc<AtomicBool>) -> Self {
        Self { core, on_drop }
    }
}

impl Drop for FakeDb {
    fn drop(&mut self) {
        self.on_drop.store(true, Ordering::SeqCst);
    }
}

impl Db for FakeDb {
    fn execute(
        &self,
        sql: &str,
        _params: Vec<Value>,
    ) -> BoxFuture<'_, Result<ExecResult, StoreError>> {
        self.core.sqls.lock().unwrap().push(sql.to_string());
        Box::pin(async move {
            Ok(ExecResult {
                changes: 0,
                last_insert_rowid: 0,
            })
        })
    }

    fn query(&self, sql: &str, _params: Vec<Value>) -> BoxFuture<'_, Result<Vec<Row>, StoreError>> {
        self.core.sqls.lock().unwrap().push(sql.to_string());
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn batch(
        &self,
        _statements: Vec<Statement>,
        _mode: BatchMode,
    ) -> BoxFuture<'_, Result<Vec<ExecResult>, StoreError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

// ---------------------------------------------------------------------------
// FakeConnectFactory (blueprint step 4)
// ---------------------------------------------------------------------------

/// Counts opens, captures options, optionally gates opens behind oneshots,
/// keeps an event log, can fail opens with a fixed message.
#[derive(Default)]
pub struct FakeConnectFactory {
    pub opens: Arc<AtomicI64>,
    pub captured: Arc<std::sync::Mutex<Vec<ConnectOptions>>>,
    /// Armed open receivers consumed by `connect` in order; each `arm_open`
    /// queues `(tx, rx)` and the caller releases via the senders.
    pub gate:
        Arc<tokio::sync::Mutex<std::collections::VecDeque<tokio::sync::oneshot::Receiver<()>>>>,
    pub gate_senders: Arc<std::sync::Mutex<Vec<tokio::sync::oneshot::Sender<()>>>>,
    pub events: Arc<std::sync::Mutex<Vec<String>>>,
    pub fail: Arc<std::sync::Mutex<Option<String>>>,
}

impl FakeConnectFactory {
    /// Arms ONE future open behind a gate.
    pub async fn arm_open(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.gate.lock().await.push_back(rx);
        self.gate_senders.lock().unwrap().push(tx);
    }

    /// Releases the oldest armed open.
    pub fn release_next(&self) {
        if let Some(tx) = self.gate_senders.lock().unwrap().pop() {
            let _ = tx.send(());
        }
    }

    /// Releases every armed open.
    pub fn release_all(&self) {
        loop {
            let next = self.gate_senders.lock().unwrap().pop();
            match next {
                Some(tx) => {
                    let _ = tx.send(());
                }
                None => break,
            }
        }
    }

    /// Removes all gates (subsequent opens run free).
    pub async fn ungate(&self) {
        self.gate.lock().await.clear();
        self.gate_senders.lock().unwrap().clear();
    }

    pub fn event(&self, message: impl Into<String>) {
        self.events.lock().unwrap().push(message.into());
    }

    pub fn event_log(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl ConnectFactory for FakeConnectFactory {
    fn connect(
        &self,
        _path: std::path::PathBuf,
        opts: ConnectOptions,
    ) -> BoxFuture<'static, Result<DbHandle, StoreError>> {
        let opens = self.opens.clone();
        let captured = self.captured.clone();
        let gate = self.gate.clone();
        let events = self.events.clone();
        let fail = self.fail.clone();
        Box::pin(async move {
            captured.lock().unwrap().push(opts.clone());
            let sequence = opens.fetch_add(1, Ordering::SeqCst);
            events.lock().unwrap().push(format!("open{sequence}:start"));
            // Optional gate: await the armed receiver before finishing.
            let armed = {
                let mut queue = gate.lock().await;
                queue.pop_front()
            };
            if let Some(receiver) = armed {
                let _ = receiver.await;
                events
                    .lock()
                    .unwrap()
                    .push(format!("open{sequence}:released"));
            }
            if let Some(message) = fail.lock().unwrap().clone() {
                events
                    .lock()
                    .unwrap()
                    .push(format!("open{sequence}:failed"));
                return Err(StoreError::Turso {
                    message,
                    is_constraint: false,
                });
            }
            events.lock().unwrap().push(format!("open{sequence}:done"));
            Ok(Arc::new(FakeDb::new()) as DbHandle)
        })
    }
}

/// Untested convenience for later steps opening through the real factory.
pub async fn open_real_db(config: &StoreConfig, name: &str) -> Result<DbHandle, StoreError> {
    let opts = ConnectOptions::for_config(config, mem_store::platform::Platform::current());
    mem_store::db::turso_connect_factory()
        .connect(config.storage_path.join(name), opts)
        .await
}

// ---------------------------------------------------------------------------
// Shard-manager fixtures (blueprint step 8)
// ---------------------------------------------------------------------------

/// Hermetic store config with explicit dims/max for rotation batteries.
pub fn test_store_config(prefix: &str, dims: u32, max: i64) -> StoreConfig {
    let mut config = store_config(prefix);
    config.embedding_dimensions = dims;
    config.max_vectors_per_shard = u32::try_from(max).unwrap_or(u32::MAX);
    config
}

/// Probe controllable from tests (stale-lock detection paths).
pub struct FakeProbe {
    pub alive: std::sync::atomic::AtomicBool,
}

impl Default for FakeProbe {
    fn default() -> Self {
        Self {
            alive: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

impl mem_store::platform::ProcessProbe for FakeProbe {
    fn is_alive(&self, _pid: u32) -> bool {
        self.alive.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Wired real components over a temp storage root (TursoConnectFactory +
/// SystemProcessProbe). Returns the config (with the temp dir) for reuse.
pub async fn real_components(
    config: StoreConfig,
) -> (
    StoreConfig,
    std::sync::Arc<mem_store::connection::ConnectionManager>,
    mem_store::shard_manager::ShardManager,
) {
    use mem_store::platform::SystemProcessProbe;
    real_components_with_probe(config, std::sync::Arc::new(SystemProcessProbe)).await
}

/// Wired real components over a temp storage root with an INJECTED process
/// probe.
///
/// The probe decides whether a lock file's owner is alive, so a fake probe
/// makes a FOREIGN pid report "live" without a real process (the seam the
/// own-lock-ownership contract needs).
pub async fn real_components_with_probe(
    config: StoreConfig,
    probe: std::sync::Arc<dyn mem_store::platform::ProcessProbe>,
) -> (
    StoreConfig,
    std::sync::Arc<mem_store::connection::ConnectionManager>,
    mem_store::shard_manager::ShardManager,
) {
    use mem_store::connection::ConnectionManager;
    use mem_store::db::TursoConnectFactory;
    use mem_store::platform::Platform;
    use mem_store::shard_manager::ShardManager;

    let connections = std::sync::Arc::new(ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        std::sync::Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        probe,
    );
    (config, connections, manager)
}

/// Writes a lock-state JSON file into a storage root.
pub fn write_lock_state(
    storage: &std::path::Path,
    file: &str,
    pid: u32,
    operation: Option<&str>,
) -> std::path::PathBuf {
    let path = storage.join(file);
    let body = format!(
        r#"{{"pid":{},"timestamp":"2026-01-01T00:00:00.000Z","operation":{}}}"#,
        pid,
        match operation {
            Some(op) => format!(r#""{op}""#),
            None => "null".to_string(),
        }
    );
    std::fs::write(&path, body).expect("lock write");
    path
}

/// Step-9/12 fixture: writes a `path-migrate` swap journal into a storage
/// root (camelCase, byte-shaped like the TS-written journal).
pub fn write_swap_fixture(
    storage: &std::path::Path,
    state: &mem_store::shard_path_migration::PathMigrateSwapState,
) -> std::path::PathBuf {
    let path = mem_store::shard_path_migration::swap_state_path(storage);
    let body = serde_json::to_string(state).expect("journal serialize");
    std::fs::write(&path, body).expect("journal write");
    path
}

// ---------------------------------------------------------------------------
// FakeTagSource (blueprint step 11+) — hermetic canned TagInfo per directory.
// ---------------------------------------------------------------------------

/// Canned `TagInfo` per directory (no git spawns in tests).
pub struct FakeTagSource {
    pub map: std::sync::Mutex<std::collections::HashMap<String, mem_core::tags::TagInfo>>,
    pub fallback: Option<mem_core::tags::TagInfo>,
}

impl FakeTagSource {
    /// One directory -> canned info entry.
    pub fn canned(
        directory: std::path::PathBuf,
        tag: &str,
        display_name: &str,
        project_path: Option<std::path::PathBuf>,
        project_name: Option<&str>,
    ) -> Self {
        let mut map = std::collections::HashMap::new();
        let _ = project_name;
        map.insert(
            directory.to_string_lossy().to_string(),
            mem_core::tags::TagInfo {
                tag: tag.to_string(),
                display_name: display_name.to_string(),
                user_name: None,
                user_email: None,
                project_path,
                project_name: project_name.map(str::to_string),
                git_repo_url: None,
            },
        );
        Self {
            map: std::sync::Mutex::new(map),
            fallback: None,
        }
    }
}

impl mem_store::project_tags::ProjectTagSource for FakeTagSource {
    fn project_tag_info(&self, directory: &std::path::Path) -> mem_core::tags::TagInfo {
        let key = directory.to_string_lossy().to_string();
        if let Some(info) = self.map.lock().unwrap().get(&key) {
            return info.clone();
        }
        self.fallback
            .clone()
            .unwrap_or_else(|| mem_core::tags::TagInfo {
                tag: "opencode_project_ffffffffffffffff".to_string(),
                display_name: key.clone(),
                user_name: None,
                user_email: None,
                project_path: Some(directory.to_path_buf()),
                project_name: None,
                git_repo_url: None,
            })
    }
}

// ---------------------------------------------------------------------------
// Step-12 instrumentation: FaultingRegistry + RecordingPromptUpdater
// ---------------------------------------------------------------------------

/// Decorator that fails the Nth FORWARD `reassign_shard_scope` (the target
/// hash differs from the recorded source hash) so the swap fault path is
/// exercised; restore-reassigns (`new_hash == old_hash`) pass through.
pub struct FaultingRegistry {
    pub inner: Arc<dyn mem_store::shard_manager::ShardRegistry>,
    /// The source hash: reassigns restoring TO this hash never fault.
    pub old_hash: String,
    /// 1-based ordinal of the forward reassign that must fail.
    pub fail_nth: std::sync::atomic::AtomicUsize,
    pub forward_calls: std::sync::atomic::AtomicUsize,
}

impl mem_store::shard_manager::ShardRegistry for FaultingRegistry {
    fn get_all_shards<'a>(
        &'a self,
        scope: mem_store::scope::Scope,
        scope_hash: Option<&'a str>,
    ) -> mem_store::db::BoxFuture<
        'a,
        Result<Vec<mem_store::shard_manager::ShardInfo>, mem_store::error::StoreError>,
    > {
        self.inner.get_all_shards(scope, scope_hash)
    }

    fn register_existing_shard<'a>(
        &'a self,
        scope: mem_store::scope::Scope,
        scope_hash: &'a str,
        shard_index: u32,
        db_path: &'a std::path::Path,
        vector_count: u64,
        is_active: bool,
    ) -> mem_store::db::BoxFuture<
        'a,
        Result<mem_store::shard_manager::ShardInfo, mem_store::error::StoreError>,
    > {
        self.inner.register_existing_shard(
            scope,
            scope_hash,
            shard_index,
            db_path,
            vector_count,
            is_active,
        )
    }

    fn archive_shard<'a>(
        &'a self,
        shard_id: i64,
        reason: &'a str,
        archive_path_override: Option<&'a std::path::Path>,
    ) -> mem_store::db::BoxFuture<
        'a,
        Result<Option<std::path::PathBuf>, mem_store::error::StoreError>,
    > {
        self.inner
            .archive_shard(shard_id, reason, archive_path_override)
    }

    fn reassign_shard_scope<'a>(
        &'a self,
        shard_id: i64,
        new_scope_hash: &'a str,
        new_db_path: &'a std::path::Path,
        vector_count: u64,
        is_active: bool,
    ) -> mem_store::db::BoxFuture<
        'a,
        Result<mem_store::shard_manager::ShardInfo, mem_store::error::StoreError>,
    > {
        use std::sync::atomic::Ordering;
        if new_scope_hash != self.old_hash {
            let call = self.forward_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let fail_nth = self.fail_nth.load(Ordering::SeqCst);
            if call == fail_nth {
                return Box::pin(async {
                    Err(mem_store::error::StoreError::Internal {
                        message: "forced reassign fault".to_string(),
                    })
                });
            }
        }
        self.inner.reassign_shard_scope(
            shard_id,
            new_scope_hash,
            new_db_path,
            vector_count,
            is_active,
        )
    }

    fn delete_shard<'a>(
        &'a self,
        shard_id: i64,
    ) -> mem_store::db::BoxFuture<'a, Result<(), mem_store::error::StoreError>> {
        self.inner.delete_shard(shard_id)
    }

    fn get_shard_by_id<'a>(
        &'a self,
        shard_id: i64,
    ) -> mem_store::db::BoxFuture<
        'a,
        Result<Option<mem_store::shard_manager::ShardInfo>, mem_store::error::StoreError>,
    > {
        self.inner.get_shard_by_id(shard_id)
    }
}

/// Recording `PromptPathUpdater` — either records successes or fails.
#[derive(Default)]
pub struct RecordingPromptUpdater {
    pub calls: std::sync::Mutex<Vec<(String, String)>>,
    pub fail: std::sync::atomic::AtomicBool,
}

impl mem_store::prompts::PromptPathUpdater for RecordingPromptUpdater {
    fn update_project_paths(
        &self,
        old_path: &str,
        new_path: &str,
    ) -> mem_store::db::BoxFuture<'_, Result<u64, mem_store::error::StoreError>> {
        let record = (old_path.to_string(), new_path.to_string());
        let fail = self.fail.load(std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            self.calls.lock().unwrap().push(record);
            if fail {
                Err(mem_store::error::StoreError::Internal {
                    message: "prompt update fault".to_string(),
                })
            } else {
                Ok(1)
            }
        })
    }
}

// ---------------------------------------------------------------------------
// W1.3 migration fixtures: FakeReEmbedder + RecordingSleeper + DiskANN
// fixture helpers (blueprint step 2)
// ---------------------------------------------------------------------------

use std::sync::atomic::AtomicUsize;
use std::time::Duration;

/// Injected `ReEmbedder` (upstream `embeddingService` seam): deterministic
/// `[1, 0, 0, 0]` vectors, optional fail-on-substring, call recording.
pub struct FakeReEmbedder {
    pub vector: Vec<f32>,
    pub fail_on: std::sync::Mutex<Option<String>>,
    pub calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub warmups: AtomicUsize,
    pub clears: AtomicUsize,
}

impl FakeReEmbedder {
    pub fn new() -> Self {
        Self {
            vector: vec![1.0, 0.0, 0.0, 0.0],
            fail_on: std::sync::Mutex::new(None),
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            warmups: AtomicUsize::new(0),
            clears: AtomicUsize::new(0),
        }
    }

    /// The synthetic error text (upstream test stub message).
    pub fn failure_message() -> &'static str {
        "synthetic embedding failure"
    }
}

impl mem_store::migration_service::ReEmbedder for FakeReEmbedder {
    fn warmup(&self) -> mem_store::db::BoxFuture<'_, Result<(), StoreError>> {
        self.warmups.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(()) })
    }

    fn clear_cache(&self) {
        self.clears.fetch_add(1, Ordering::SeqCst);
    }

    fn embed(&self, input: &str) -> mem_store::db::BoxFuture<'_, Result<Vec<f32>, StoreError>> {
        let fail_on = self.fail_on.lock().unwrap().clone();
        let calls = std::sync::Arc::clone(&self.calls);
        let vector = self.vector.clone();
        let input = input.to_string();
        Box::pin(async move {
            if let Some(needle) = fail_on
                && input.contains(&needle)
            {
                return Err(StoreError::Internal {
                    message: Self::failure_message().to_string(),
                });
            }
            calls.lock().unwrap().push(input);
            Ok(vector)
        })
    }
}

/// Recording sleeper: captures requested retry delays instead of wall-clock
/// sleeps (upstream `delay(ms)` — `setTimeout` inside the lock retry).
pub struct RecordingSleeper {
    pub delays: std::sync::Arc<std::sync::Mutex<Vec<Duration>>>,
}

impl RecordingSleeper {
    pub fn new() -> Self {
        Self {
            delays: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn recorded(&self) -> Vec<Duration> {
        self.delays.lock().unwrap().clone()
    }

    /// The injected `SleepFn` (production uses `platform::system_sleep`).
    pub fn as_sleep_fn(&self) -> mem_store::platform::SleepFn {
        let delays = std::sync::Arc::clone(&self.delays);
        std::sync::Arc::new(move |delay| {
            delays.lock().unwrap().push(delay);
        })
    }
}

/// Path of the shipped DiskANN fixture (pure data; never mutated in place).
pub fn diskann_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("legacy_diskann_shard.db")
}

/// Copies the DiskANN fixture (main + sidecars) into `dir` as `name`.
pub fn copy_diskann_fixture(dir: &Path, name: &str) -> PathBuf {
    let source = diskann_fixture_path();
    let target = dir.join(name);
    for suffix in ["", "-wal", "-shm", "-tshm", "-journal"] {
        // The fixture is a fixed filename; sidecars are `<fixture><suffix>`.
        let from = {
            let mut text = source.as_os_str().to_os_string();
            if !suffix.is_empty() {
                text.push(suffix);
            }
            PathBuf::from(text)
        };
        if from.exists() {
            let mut to_text = target.as_os_str().to_os_string();
            if !suffix.is_empty() {
                to_text.push(suffix);
            }
            std::fs::copy(&from, PathBuf::from(to_text)).expect("fixture sidecar copy");
        }
    }
    target
}

// ---------------------------------------------------------------------------
// W1.5 encryption fixtures (blueprint step 2, captain scaffold)
// ---------------------------------------------------------------------------

/// The TS-pinned aes256gcm key from the read-compat spike
/// (`/tmp/turso-probe/ts-encrypted.db`): 64 hex chars. Reused by every
/// encryption test so the cipher/key pair stays a single pinned literal.
pub fn hex_key_64() -> String {
    "b1bbfda4f589dc9daaf004fe21111e00dc00c98237102f5c7002a5669fc76327".to_string()
}

/// A store config over a fresh temp dir with encryption enabled and an
/// explicit hex key (never the real home; hermetic).
pub fn encrypted_store_config(prefix: &str, key_hex: &str) -> StoreConfig {
    let mut config = store_config(prefix);
    config.database_encryption_enabled = true;
    config.database_encryption_cipher = "aes256gcm".to_string();
    config.database_encryption_key = Some(key_hex.to_string());
    config
}

/// The default key-file path *inside* a storage dir — tests never touch the
/// real `~/.config` path; production resolution is asserted separately by
/// pinning the config key to this hermetic location via `file://`.
pub fn default_key_path(dir: &Path) -> PathBuf {
    dir.join("opencode-mem-db.key")
}

/// Alias of [`default_key_path`] (the blueprint's file-structure summary
/// names the helper `key_file`; step 2 names it `default_key_path`).
pub fn key_file(dir: &Path) -> PathBuf {
    default_key_path(dir)
}

// ---------------------------------------------------------------------------
// W1.5 cross-process lock-holder fixtures (blueprint step 2, captain scaffold)
// ---------------------------------------------------------------------------

/// Resolves the compiled `lock_holder` example binary next to the running
/// test executable (`<target>/<profile>/examples/lock_holder`). Fails loudly
/// when the example was not built: a plain `cargo test -p mem-store` and
/// `cargo test --workspace` build examples, but a scoped
/// `cargo test -p mem-store --test <file>` does NOT — run
/// `cargo build -p mem-store --example lock_holder` first.
pub fn lock_holder_binary_path() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let profile_dir = exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target profile directory")
        .to_path_buf();
    let name = if cfg!(windows) {
        "lock_holder.exe"
    } else {
        "lock_holder"
    };
    let candidate = profile_dir.join("examples").join(name);
    assert!(
        candidate.exists(),
        "lock_holder example not built at {} — run `cargo build -p mem-store --example lock_holder`",
        candidate.display()
    );
    candidate
}

/// A spawned `lock_holder` with stdout/stderr redirected to files (never
/// pipes — a chatty child must not deadlock the parent).
pub struct LockHolderRun {
    pub child: std::process::Child,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

impl LockHolderRun {
    /// Bounded wait: polls `try_wait()` until the deadline, then kills and
    /// reaps (never leaves an orphan).
    pub fn wait(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return None;
                }
            }
        }
    }

    pub fn captured_stderr(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    pub fn captured_stdout(&self) -> String {
        std::fs::read_to_string(&self.stdout_path).unwrap_or_default()
    }
}

/// Spawns `lock_holder --storage <dir> --hold-ms <n> --wait-ms <n>`.
pub fn spawn_lock_holder(storage: &Path, hold_ms: u64, wait_ms: u64) -> LockHolderRun {
    let binary = lock_holder_binary_path();
    let capture_dir = temp_dir("lock-holder-capture");
    let stdout_path = capture_dir.join("stdout.txt");
    let stderr_path = capture_dir.join("stderr.txt");
    let child = std::process::Command::new(binary)
        .arg("--storage")
        .arg(storage)
        .arg("--hold-ms")
        .arg(hold_ms.to_string())
        .arg("--wait-ms")
        .arg(wait_ms.to_string())
        .stdout(std::fs::File::create(&stdout_path).expect("stdout capture file"))
        .stderr(std::fs::File::create(&stderr_path).expect("stderr capture file"))
        .spawn()
        .expect("spawn lock_holder");
    LockHolderRun {
        child,
        stdout_path,
        stderr_path,
    }
}

/// Polls until `path` exists or the deadline elapses (bounded barrier wait).
pub fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if path.exists() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Writes a re-embed swap journal (camelCase JSON, byte-shaped like the
/// TS-written journal) and returns its path.
pub fn write_reembed_swap_state(db_path: &Path, staged: &Path, backup: &Path) -> PathBuf {
    let state_path = PathBuf::from(format!("{}.reembed-swap.json", db_path.display()));
    let body = format!(
        r#"{{"dbPath":"{}","stagedPath":"{}","backupPath":"{}"}}"#,
        db_path.display(),
        staged.display(),
        backup.display()
    );
    std::fs::write(&state_path, body).expect("swap state write");
    state_path
}
