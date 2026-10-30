// Test helpers for mem-store integration tests.
//
//! House law (`docs/agents/testing.md`): temp dirs under `std::env::temp_dir()`
//! only, injected seams instead of env mutation, no dev-dependencies
//! (this file relies on the crate's normal dependencies), no sleeps for
//! synchronization. Helpers grow per wave; the W1.2 first cut ships only
//! `temp_dir`.

#![allow(dead_code)]

pub mod capture;

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
        allow_unguarded_open: false,
    }
}

/// A live exclusivity proof for `storage_root`, memoised per **exact** path.
///
/// The engine lock is one per root per process (a second acquire answers
/// `Lost`), so every test in this binary that touches a claimed mutation shares
/// the one lock. Acquire with the **same path** the `ConnectionManager` was
/// built with — `ExclusiveRoot::current` matches `storage_root()` exactly, so
/// never `canonicalize` here.
///
/// The proof is deliberately kept alive for the whole test binary: a test that
/// needs to observe `None` (the restore-without-lock arm) must acquire and drop
/// its own manual `Arc<EngineLock>` instead.
pub fn exclusive_root(
    storage_root: &Path,
) -> std::sync::Arc<mem_store::delete_guard::ExclusiveRoot> {
    static ROOTS: std::sync::Mutex<
        Vec<(
            PathBuf,
            std::sync::Arc<mem_store::delete_guard::ExclusiveRoot>,
        )>,
    > = std::sync::Mutex::new(Vec::new());
    let mut roots = ROOTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, proof)) = roots.iter().find(|(root, _)| root == storage_root) {
        return proof.clone();
    }
    let outcome = mem_store::engine_lock::acquire_engine_lock(
        storage_root,
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
    )
    .expect("acquire the engine lock for a test root");
    let lock = match outcome {
        mem_store::engine_lock::EngineLockOutcome::Acquired(lock) => lock,
        mem_store::engine_lock::EngineLockOutcome::Lost { holder_pid } => {
            panic!(
                "test root {} is already locked by pid {holder_pid}",
                storage_root.display()
            )
        }
    };
    let proof = std::sync::Arc::new(mem_store::delete_guard::ExclusiveRoot::new(lock));
    roots.push((storage_root.to_path_buf(), proof.clone()));
    proof
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
    components(config, probe, None).await
}

/// Wired real components carrying an explicit embedding identity (W2's
/// fingerprint tests need a manager that can claim).
pub async fn real_components_with_identity(
    config: StoreConfig,
    identity: Option<mem_core::embedding::EmbeddingIdentity>,
) -> (
    StoreConfig,
    std::sync::Arc<mem_store::connection::ConnectionManager>,
    mem_store::shard_manager::ShardManager,
) {
    components(
        config,
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
        identity,
    )
    .await
}

async fn components(
    config: StoreConfig,
    probe: std::sync::Arc<dyn mem_store::platform::ProcessProbe>,
    identity: Option<mem_core::embedding::EmbeddingIdentity>,
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
        identity,
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
// W2 recording connection factory (AC13/AC15)
// ---------------------------------------------------------------------------

/// One recorded `BatchMode::Write` batch: the opened path and its statements.
#[derive(Clone)]
pub struct RecordedBatch {
    pub path: std::path::PathBuf,
    pub statements: Vec<Statement>,
}

#[derive(Default)]
struct RecordingState {
    opens: std::sync::Mutex<Vec<std::path::PathBuf>>,
    batches: std::sync::Mutex<Vec<RecordedBatch>>,
}

/// A `ConnectFactory` that wraps the real one and records every open path and
/// every write batch. AC13 uses it to prove no `.reembed-*.tmp` was opened or
/// written; AC15 uses it to identify the staged vector batch by content.
#[derive(Clone, Default)]
pub struct RecordingConnectFactory {
    state: Arc<RecordingState>,
}

impl RecordingConnectFactory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every path handed to `connect`, in order.
    pub fn opens(&self) -> Vec<std::path::PathBuf> {
        self.state.opens.lock().unwrap().clone()
    }

    /// Every write batch, in order.
    pub fn write_batches(&self) -> Vec<RecordedBatch> {
        self.state.batches.lock().unwrap().clone()
    }
}

impl ConnectFactory for RecordingConnectFactory {
    fn connect(
        &self,
        path: std::path::PathBuf,
        opts: ConnectOptions,
    ) -> BoxFuture<'static, Result<DbHandle, StoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            state.opens.lock().unwrap().push(path.clone());
            let inner = mem_store::db::turso_connect_factory()
                .connect(path.clone(), opts)
                .await?;
            Ok(Arc::new(RecordingDb { inner, path, state }) as DbHandle)
        })
    }
}

struct RecordingDb {
    inner: DbHandle,
    path: std::path::PathBuf,
    state: Arc<RecordingState>,
}

impl Db for RecordingDb {
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
        if mode == BatchMode::Write {
            self.state.batches.lock().unwrap().push(RecordedBatch {
                path: self.path.clone(),
                statements: statements.clone(),
            });
        }
        self.inner.batch(statements, mode)
    }
}

/// Wired real components with an explicit identity and connection factory.
pub async fn real_components_with_factory(
    config: StoreConfig,
    identity: Option<mem_core::embedding::EmbeddingIdentity>,
    factory: std::sync::Arc<dyn ConnectFactory>,
) -> (
    StoreConfig,
    std::sync::Arc<mem_store::connection::ConnectionManager>,
    mem_store::shard_manager::ShardManager,
) {
    use mem_store::connection::ConnectionManager;
    use mem_store::platform::Platform;
    use mem_store::shard_manager::ShardManager;

    let connections = std::sync::Arc::new(ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        factory,
    ));
    let manager = ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
        identity,
    );
    (config, connections, manager)
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

// ---------------------------------------------------------------------------
// user_prompts schema fixtures (issue 43 W5)
// ---------------------------------------------------------------------------

/// Builds the `user_prompts` fixture for the schema-v2 battery.
///
/// The fixture table is the single authority for shape, `db_path` and `root`
/// (`proof` is `exclusive_root(&config.storage_path)` bound to a `let` local in
/// each test; never call `.as_ref()` on the call's own temporary):
///
/// | fixture | seed / build | table shape | `MigrationOptions.db_path` | `root` | extra assertions |
/// |---|---|---|---|---|---|
/// | fresh | empty file, no seed | none | `Some(dir/user-prompts.db)` | `Some(proof.as_ref())` | 12 columns, `Ok(2)` |
/// | v0-legacy | `seed_user_prompts(0, false, true)` | 9 cols, 1 row | `Some(…)` | `Some(proof.as_ref())` | row intact, defaults 0/NULL |
/// | v1-without | `seed_user_prompts(1, false, false)` | 9 cols, **no row** | `Some(…)` | `Some(proof.as_ref())` | types/defaults, INSERT roundtrip, **no `.schema-v*`** |
/// | v1-with | `seed_user_prompts(1, true, false)` | 12 cols, **no row** | `Some(…)` | `Some(proof.as_ref())` | 12 cols after, `Ok(2)` twice |
/// | v2-noop | `seed_user_prompts(2, true, false)` | 12 cols, **no row** | `Some(…)` | `Some(proof.as_ref())` | `USER_PROMPTS_MAX_KNOWN == 2`, `Ok(2)` |
/// | v3 | `seed_user_prompts(3, true, true)` | 12 cols, 1 row | `Some(…)` | `Some(proof.as_ref())` | typed refusal, version/columns/row unchanged |
/// | fail-surface arm (a) | empty file + `PRAGMA user_version = 1` by hand | **no table** | `Some(…)` | `Some(proof.as_ref())` | `Err`, **no `.schema-v*`** |
/// | fail-surface arm (b) | `PromptsDb`, `version = 0`, `probe_error = "disk I/O error"` | — | `None` | `None` | `Err` propagates, no ALTER batch |
/// | no-write | `PromptsDb`, `version = 2`, 12 columns | — | `None` | `None` | `Ok(2)`, SQL allowlist `["PRAGMA user_version"]` |
///
/// The 9-column legacy shape is created with `CREATE TABLE IF NOT EXISTS`;
/// the three v2 columns are appended only when `with_v2_columns`. Exactly one
/// row (`prompt_legacy`, content `preserve this prompt`, `captured = 1`, no
/// `project_path`) is inserted only when `with_row`, and the file is stamped
/// `PRAGMA user_version = {version}` only when `version > 0`.
pub async fn seed_user_prompts(
    db: &dyn Db,
    version: i64,
    with_v2_columns: bool,
    with_row: bool,
) -> Result<(), StoreError> {
    let mut ddl = String::from(
        "CREATE TABLE IF NOT EXISTS user_prompts (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            project_path TEXT,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            captured INTEGER DEFAULT 0,
            user_learning_captured BOOLEAN DEFAULT 0,
            linked_memory_id TEXT",
    );
    if with_v2_columns {
        ddl.push_str(", capture_attempts INTEGER DEFAULT 0, provider_id TEXT, model_id TEXT");
    }
    ddl.push(')');
    db.execute(&ddl, Vec::new()).await?;
    if with_row {
        db.execute(
            "INSERT INTO user_prompts (id, session_id, message_id, content, created_at, captured)
             VALUES (?, ?, ?, ?, ?, ?)",
            vec![
                Value::Text("prompt_legacy".to_string()),
                Value::Text("session_legacy".to_string()),
                Value::Text("message_legacy".to_string()),
                Value::Text("preserve this prompt".to_string()),
                Value::Integer(123),
                Value::Integer(1),
            ],
        )
        .await?;
    }
    if version > 0 {
        db.execute(&format!("PRAGMA user_version = {version}"), Vec::new())
            .await?;
    }
    Ok(())
}

/// Column names of `user_prompts` in definition order (`PRAGMA table_info`).
pub async fn user_prompt_columns(db: &dyn Db) -> Vec<String> {
    db.query("PRAGMA table_info(user_prompts)", Vec::new())
        .await
        .expect("table_info(user_prompts)")
        .into_iter()
        .filter_map(|row| row.text("name"))
        .collect()
}
