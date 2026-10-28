#![allow(dead_code)]

//! Std-only helpers shared by mem-episodic integration tests (house law:
//! `docs/agents/testing.md`) — temp dirs under `std::env::temp_dir()` only,
//! injected seams instead of env mutation, no dev-dependencies.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use mem_ai::provider::{AiProvider, CompletionStream};
use mem_embed::embedder::{BoxFuture, EmbedTask, Embedder};
use mem_embed::error::EmbedError;
use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory as _, ConnectOptions, DbHandle, turso_connect_factory};
use mem_store::platform::Platform;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory under `std::env::temp_dir()`:
/// `mem-episodic-w41-{prefix}-{pid}-{counter}-{nanos}`.
pub fn temp_dir(prefix: &str) -> PathBuf {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "mem-episodic-w41-{prefix}-{}-{counter}-{nanos}",
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

/// A live exclusivity proof for `storage_root`, memoised per **exact** path.
///
/// Mirrors `crates/mem-store/tests/common/mod.rs`: the engine lock is one per
/// root per process, so the proof is shared by every test in this binary that
/// exercises a claimed mutation. Acquire with the same path the store was built
/// with (`ExclusiveRoot::current` matches `storage_root()` exactly).
pub fn exclusive_root(
    storage_root: &Path,
) -> std::sync::Arc<mem_store::delete_guard::ExclusiveRoot> {
    static ROOTS: Mutex<
        Vec<(
            PathBuf,
            std::sync::Arc<mem_store::delete_guard::ExclusiveRoot>,
        )>,
    > = Mutex::new(Vec::new());
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

/// Owns a temp dir and removes it on drop (also on test panic).
pub struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub fn new(prefix: &str) -> Self {
        Self {
            path: temp_dir(prefix),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.path.clone()
    }

    /// Hermetic config over this directory (encryption disabled, 8 dims).
    pub fn store_config(&self) -> StoreConfig {
        store_config(self.path.clone())
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        cleanup_temp_dir(&self.path);
    }
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

/// Opens a database file inside `dir` directly through the production factory
/// (bypassing the caching `ConnectionManager`) — fixture setup for pre-existing
/// on-disk state.
pub async fn open_raw(dir: &Path, name: &str) -> DbHandle {
    let config = store_config(dir.to_path_buf());
    let options = ConnectOptions::for_config(&config, Platform::current());
    turso_connect_factory()
        .connect(dir.join(name), options)
        .await
        .expect("open raw database")
}

/// A wired shard manager over the given storage root (real turso factory +
/// the production process probe).
pub fn shard_manager(
    config: mem_store::config::StoreConfig,
) -> Arc<mem_store::shard_manager::ShardManager> {
    shard_manager_with_probe(config, Arc::new(mem_store::platform::SystemProcessProbe))
}

/// A wired shard manager over the given storage root with an INJECTED
/// process probe.
///
/// The probe decides whether a lock file's owner is alive, so a fake probe
/// makes a FOREIGN pid report "live" without a real process (the seam the
/// own-lock-bypass tests need).
pub fn shard_manager_with_probe(
    config: mem_store::config::StoreConfig,
    probe: Arc<dyn mem_store::platform::ProcessProbe>,
) -> Arc<mem_store::shard_manager::ShardManager> {
    let connections = Arc::new(mem_store::connection::ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(turso_connect_factory()),
    ));
    Arc::new(mem_store::shard_manager::ShardManager::new(
        config.storage_path.clone(),
        config,
        connections,
        probe,
        None,
    ))
}

// ---------------------------------------------------------------------------
// Fake embedder
// ---------------------------------------------------------------------------

/// Deterministic [`Embedder`] fake.
///
/// - `default_vector` is returned when the queue is empty;
/// - `vectors` (FIFO) lets a test hand a distinct vector to each insert;
/// - `fail` makes every `embed` return an error without writing anything;
/// - `calls` captures the exact final (already-prefixed) text handed to the
///   provider — the D-4 template pin.
pub struct FakeEmbedder {
    default_vector: Vec<f32>,
    vectors: Mutex<VecDeque<Vec<f32>>>,
    fail: bool,
    calls: Mutex<Vec<String>>,
}

impl FakeEmbedder {
    pub fn new(default_vector: Vec<f32>) -> Self {
        Self {
            default_vector,
            vectors: Mutex::new(VecDeque::new()),
            fail: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Fake returning `vector` for every call.
    pub fn with_vector(vector: Vec<f32>) -> Self {
        Self::new(vector)
    }

    /// Fake returning `vectors` in FIFO order, then the last one.
    pub fn with_vectors(vectors: Vec<Vec<f32>>) -> Self {
        let default_vector = vectors.last().cloned().unwrap_or_default();
        Self {
            default_vector,
            vectors: Mutex::new(vectors.into_iter().collect()),
            fail: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Fake whose `embed` always fails.
    pub fn failing() -> Self {
        Self {
            default_vector: Vec::new(),
            vectors: Mutex::new(VecDeque::new()),
            fail: true,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The exact texts passed to `embed`, in call order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("embedder calls lock").clone()
    }

    /// The most recent captured text, if any.
    pub fn last_call(&self) -> Option<String> {
        self.calls
            .lock()
            .expect("embedder calls lock")
            .last()
            .cloned()
    }
}

impl Embedder for FakeEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        self.calls
            .lock()
            .expect("embedder calls lock")
            .push(text.to_string());
        let failure = self.fail;
        let mut queued = self.vectors.lock().expect("embedder vectors lock");
        let vector = queued
            .pop_front()
            .unwrap_or_else(|| self.default_vector.clone());
        drop(queued);
        Box::pin(async move {
            if failure {
                return Err(EmbedError::Inference {
                    message: "fake embedder failure".to_string(),
                });
            }
            Ok(vector)
        })
    }
}

/// The pinned `EmbedTask` used by the write path (readability helper).
pub const DOCUMENT_TASK: EmbedTask = EmbedTask::Document;

// ---------------------------------------------------------------------------
// W4.3 shared helpers (Step 2 sole owner; later steps only *use* them)
// ---------------------------------------------------------------------------

/// A deterministic clock: an `Arc<AtomicI64>`-backed millisecond stamp the
/// test advances by hand. The W4.3 time seam never reads the wall clock.
#[derive(Clone, Debug)]
pub struct FixedClock {
    millis: Arc<AtomicI64>,
}

impl FixedClock {
    /// A clock pinned at `now` (unix millis).
    pub fn new(now: i64) -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(now)),
        }
    }

    /// Moves the clock to `now`.
    pub fn set(&self, now: i64) {
        self.millis.store(now, Ordering::SeqCst);
    }

    /// The current stamp.
    pub fn now(&self) -> i64 {
        self.millis.load(Ordering::SeqCst)
    }

    /// The [`mem_episodic::store::Clock`] seam the store consumes.
    pub fn clock(&self) -> mem_episodic::store::Clock {
        let millis = Arc::clone(&self.millis);
        Arc::new(move || millis.load(Ordering::SeqCst))
    }
}

// ---------------------------------------------------------------------------
// W4.3 session-row fixtures
// ---------------------------------------------------------------------------

/// Builds one in-memory session row carrying `decisions`.
///
/// The row is NOT persisted — it is the pure input of the promotion
/// selection and of the transcript builder.
pub fn session_row(
    id: &str,
    project_id: &str,
    agent_id: &str,
    summary: &str,
    decisions: &[&str],
    created_at: i64,
) -> mem_episodic::record::AgentSession {
    mem_episodic::record::AgentSession {
        id: id.to_string(),
        project_id: project_id.to_string(),
        session_id: format!("session-{id}"),
        agent_id: agent_id.to_string(),
        parent_session_id: None,
        summary: summary.to_string(),
        metadata: mem_episodic::payload::SessionMetadata {
            timestamp: created_at,
            agent_name: agent_id.to_string(),
            decisions: decisions
                .iter()
                .map(|description| mem_episodic::record::Decision {
                    description: (*description).to_string(),
                })
                .collect(),
            ..Default::default()
        },
        embedding: Vec::new(),
        created_at,
        updated_at: created_at,
        deleted_at: None,
        capture_id: None,
        orphaned: false,
    }
}

// ---------------------------------------------------------------------------
// W4.3 fake process probe
// ---------------------------------------------------------------------------

/// A [`mem_store::platform::ProcessProbe`] fake: answers "is this PID alive"
/// from an injected set.
pub struct FakeProcessProbe {
    alive: Mutex<std::collections::HashSet<u32>>,
}

impl FakeProcessProbe {
    /// A probe that reports exactly `pids` as alive.
    pub fn with_alive(pids: &[u32]) -> Self {
        Self {
            alive: Mutex::new(pids.iter().copied().collect()),
        }
    }
}

impl mem_store::platform::ProcessProbe for FakeProcessProbe {
    fn is_alive(&self, pid: u32) -> bool {
        self.alive.lock().expect("probe lock").contains(&pid)
    }
}

// ---------------------------------------------------------------------------
// W4.3 consolidation fakes
// ---------------------------------------------------------------------------

/// The hook the fake summarizer runs BEFORE it answers.
///
/// It runs between the claim and the episodic batch — the window the
/// soft-delete-between-claim-and-batch case needs.
pub type SummarizerHook = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// A [`mem_episodic::summarizer::MilestoneSummarizer`] fake.
///
/// It records every transcript it receives and answers with a canned text
/// (or an error), optionally running a hook first.
pub struct FakeSummarizer {
    response: Mutex<Result<String, String>>,
    transcripts: Mutex<Vec<String>>,
    hook: Mutex<Option<SummarizerHook>>,
}

impl FakeSummarizer {
    /// A summarizer answering with `text`.
    pub fn returning(text: impl Into<String>) -> Self {
        Self {
            response: Mutex::new(Ok(text.into())),
            transcripts: Mutex::new(Vec::new()),
            hook: Mutex::new(None),
        }
    }

    /// A summarizer that fails with `message`.
    pub fn failing(message: &str) -> Self {
        Self {
            response: Mutex::new(Err(message.to_string())),
            transcripts: Mutex::new(Vec::new()),
            hook: Mutex::new(None),
        }
    }

    /// A summarizer that runs `hook` before answering.
    pub fn with_hook(mut self, hook: SummarizerHook) -> Self {
        self.hook = Mutex::new(Some(hook));
        self
    }

    /// The transcripts the summarizer received, in call order.
    pub fn transcripts(&self) -> Vec<String> {
        self.transcripts.lock().expect("transcript lock").clone()
    }

    /// How many times the summarizer was called.
    pub fn call_count(&self) -> usize {
        self.transcripts.lock().expect("transcript lock").len()
    }
}

impl mem_episodic::summarizer::MilestoneSummarizer for FakeSummarizer {
    fn summarize<'a>(
        &'a self,
        transcript: &'a str,
    ) -> BoxFuture<'a, Result<String, mem_episodic::EpisodicError>> {
        self.transcripts
            .lock()
            .expect("transcript lock")
            .push(transcript.to_string());
        let response = self.response.lock().expect("response lock").clone();
        let hook = self.hook.lock().expect("hook lock").clone();
        Box::pin(async move {
            if let Some(hook) = hook {
                hook().await;
            }
            match response {
                Ok(text) => Ok(text),
                Err(message) => Err(mem_episodic::EpisodicError::Summary { message }),
            }
        })
    }
}

/// A [`mem_episodic::promotion::PermanentMemorySink`] fake.
///
/// It records every promotion (scope hash + record) and can fail every call.
pub struct RecordingSink {
    calls: Mutex<Vec<(String, mem_episodic::promotion::PromotionRecord)>>,
    events: Option<Arc<Mutex<Vec<String>>>>,
    fail: bool,
}

impl RecordingSink {
    /// A sink that records every promotion and succeeds.
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            events: None,
            fail: false,
        }
    }

    /// A sink that records every promotion and fails.
    pub fn failing() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            events: None,
            fail: true,
        }
    }

    /// A sink that appends `"promote"` to the shared event log.
    pub fn with_events(events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            events: Some(events),
            fail: false,
        }
    }

    /// The recorded promotions, in call order.
    pub fn calls(&self) -> Vec<(String, mem_episodic::promotion::PromotionRecord)> {
        self.calls.lock().expect("sink calls lock").clone()
    }

    /// How many promotions were attempted.
    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("sink calls lock").len()
    }
}

impl mem_episodic::promotion::PermanentMemorySink for RecordingSink {
    fn promote<'a>(
        &'a self,
        scope_hash: &'a str,
        record: &'a mem_episodic::promotion::PromotionRecord,
    ) -> BoxFuture<'a, Result<mem_episodic::promotion::PromoteOutcome, mem_episodic::EpisodicError>>
    {
        self.calls
            .lock()
            .expect("sink calls lock")
            .push((scope_hash.to_string(), record.clone()));
        if let Some(events) = &self.events {
            events
                .lock()
                .expect("event log lock")
                .push("promote".to_string());
        }
        let fail = self.fail;
        Box::pin(async move {
            if fail {
                return Err(mem_episodic::EpisodicError::Store(
                    mem_store::error::StoreError::internal("fake sink failure"),
                ));
            }
            Ok(mem_episodic::promotion::PromoteOutcome::Inserted)
        })
    }
}

/// An [`mem_episodic::consolidation::EpisodicBatchWriter`] fake.
///
/// It records every batch, can fail every call and can append `"batch"` to
/// a shared event log (so the promotion/batch ORDER is observable).
pub struct RecordingBatchWriter {
    calls: Mutex<Vec<Vec<mem_store::db::Statement>>>,
    events: Option<Arc<Mutex<Vec<String>>>>,
    fail: bool,
}

impl RecordingBatchWriter {
    /// A writer that records every batch and succeeds.
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            events: None,
            fail: false,
        }
    }

    /// A writer that records every batch and fails.
    pub fn failing() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            events: None,
            fail: true,
        }
    }

    /// A writer that appends `"batch"` to the shared event log.
    pub fn with_events(events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            events: Some(events),
            fail: false,
        }
    }

    /// The recorded batches, in call order.
    pub fn calls(&self) -> Vec<Vec<mem_store::db::Statement>> {
        self.calls.lock().expect("batch calls lock").clone()
    }

    /// How many batches were attempted.
    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("batch calls lock").len()
    }
}

impl mem_episodic::consolidation::EpisodicBatchWriter for RecordingBatchWriter {
    fn write_batch<'a>(
        &'a self,
        statements: Vec<mem_store::db::Statement>,
    ) -> BoxFuture<'a, Result<Vec<mem_store::db::ExecResult>, mem_episodic::EpisodicError>> {
        self.calls
            .lock()
            .expect("batch calls lock")
            .push(statements.clone());
        if let Some(events) = &self.events {
            events
                .lock()
                .expect("event log lock")
                .push("batch".to_string());
        }
        let fail = self.fail;
        Box::pin(async move {
            if fail {
                return Err(mem_episodic::EpisodicError::Store(
                    mem_store::error::StoreError::internal("fake batch failure"),
                ));
            }
            Ok(statements
                .iter()
                .map(|_| mem_store::db::ExecResult {
                    changes: 1,
                    last_insert_rowid: 0,
                })
                .collect())
        })
    }
}

// ---------------------------------------------------------------------------
// W4.3 consolidation fixture
// ---------------------------------------------------------------------------

/// The W4.3 "now" the fixture shares (2026-10-20T00:00:00Z).
pub const CONSOLIDATION_NOW: i64 = 1_789_516_800_000;

/// One day in milliseconds.
pub const DAY_MILLIS: i64 = 86_400_000;

/// The W4.3 consolidation fixture: a real store over a temp dir plus the
/// recording fakes.
pub struct ConsolidationFixture {
    /// The temp storage root.
    pub dir: TestDir,
    /// The episodic store (clock-injected).
    pub store: Arc<mem_episodic::store::EpisodicStore>,
    /// The fake embedder (8 dimensions, like the store).
    pub embedder: Arc<FakeEmbedder>,
    /// The fake summarizer.
    pub summarizer: Arc<FakeSummarizer>,
    /// The recording promotion sink.
    pub sink: Arc<RecordingSink>,
    /// The recording batch writer.
    pub recorder: Arc<RecordingBatchWriter>,
    /// The injected clock.
    pub clock: FixedClock,
}

impl ConsolidationFixture {
    /// A fresh fixture.
    pub fn new() -> Self {
        let dir = TestDir::new("consolidation");
        let clock = FixedClock::new(CONSOLIDATION_NOW);
        let store = Arc::new(
            mem_episodic::store::EpisodicStore::new(dir.to_path_buf(), dir.store_config())
                .with_clock(clock.clock()),
        );
        Self {
            dir,
            store,
            embedder: Arc::new(FakeEmbedder::with_vector(vec![0.1_f32; 8])),
            summarizer: Arc::new(FakeSummarizer::returning("a merged milestone summary")),
            sink: Arc::new(RecordingSink::new()),
            recorder: Arc::new(RecordingBatchWriter::new()),
            clock,
        }
    }

    /// Inserts one session row `offset_millis` away from the fixture's now.
    pub async fn insert_at(
        &self,
        offset_millis: i64,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
        summary: &str,
        decisions: &[&str],
    ) -> String {
        self.clock.set(CONSOLIDATION_NOW + offset_millis);
        let new = mem_episodic::record::NewAgentSession {
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            summary: summary.to_string(),
            decisions: decisions
                .iter()
                .map(|description| mem_episodic::record::Decision {
                    description: (*description).to_string(),
                })
                .collect(),
            ..Default::default()
        };
        let id = self
            .store
            .insert_session(new, self.embedder.as_ref())
            .await
            .expect("insert session");
        // The clock returns to "now": the row keeps its own `created_at`, and
        // every later run happens at the fixture's now.
        self.clock.set(CONSOLIDATION_NOW);
        id
    }

    /// The promotion scope the tests use.
    pub fn scope() -> mem_episodic::consolidation::PromotionScope {
        mem_episodic::consolidation::PromotionScope {
            container_tag: "opencode_project_0123456789abcdef".to_string(),
            scope_hash: "0123456789abcdef".to_string(),
        }
    }

    /// The consolidator over the recording batch writer.
    pub fn consolidator(
        &self,
        age_days: u32,
        batch_writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter>,
    ) -> mem_episodic::consolidation::Consolidator {
        self.consolidator_with_limit(age_days, batch_writer, 200)
    }

    /// The consolidator with an explicit claim limit.
    pub fn consolidator_with_limit(
        &self,
        age_days: u32,
        batch_writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter>,
        batch_limit: usize,
    ) -> mem_episodic::consolidation::Consolidator {
        let deps = mem_episodic::consolidation::ConsolidationDeps {
            summarizer: self.summarizer.clone(),
            embedder: self.embedder.clone(),
            sink: self.sink.clone(),
            batch_writer,
            prefix_options: mem_embed::prefix::PrefixOptions::default(),
            batch_limit,
            probe: Arc::new(mem_store::platform::SystemProcessProbe),
            cross_tier: Arc::new(mem_episodic::cross_tier::NoopCrossTierTask),
        };
        mem_episodic::consolidation::Consolidator::new(
            self.store.clone(),
            mem_episodic::consolidation::ConsolidationPolicy::new(age_days).expect("policy"),
            deps,
        )
    }

    /// The consolidator over the REAL store batch writer.
    pub fn real_consolidator(&self, age_days: u32) -> mem_episodic::consolidation::Consolidator {
        let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> = Arc::new(
            mem_episodic::consolidation::StoreBatchWriter::new(self.store.clone()),
        );
        self.consolidator(age_days, writer)
    }

    /// The consolidator with every fake replaced.
    pub fn consolidator_with(
        &self,
        age_days: u32,
        summarizer: Arc<dyn mem_episodic::summarizer::MilestoneSummarizer>,
        embedder: Arc<dyn mem_embed::embedder::Embedder>,
        sink: Arc<dyn mem_episodic::promotion::PermanentMemorySink>,
        batch_writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter>,
    ) -> mem_episodic::consolidation::Consolidator {
        let deps = mem_episodic::consolidation::ConsolidationDeps {
            summarizer,
            embedder,
            sink,
            batch_writer,
            prefix_options: mem_embed::prefix::PrefixOptions::default(),
            batch_limit: 200,
            probe: Arc::new(mem_store::platform::SystemProcessProbe),
            cross_tier: Arc::new(mem_episodic::cross_tier::NoopCrossTierTask),
        };
        mem_episodic::consolidation::Consolidator::new(
            self.store.clone(),
            mem_episodic::consolidation::ConsolidationPolicy::new(age_days).expect("policy"),
            deps,
        )
    }
}

// ---------------------------------------------------------------------------
// W4.3 fake vector-count bump
// ---------------------------------------------------------------------------

/// A [`mem_episodic::promotion::VectorCountBump`] fake: records the bumped
/// shard ids and can fail every bump.
pub struct FakeVectorCountBump {
    bumped: Mutex<Vec<i64>>,
    fail: bool,
}

impl FakeVectorCountBump {
    /// A bump that records every call and succeeds.
    pub fn new() -> Self {
        Self {
            bumped: Mutex::new(Vec::new()),
            fail: false,
        }
    }

    /// A bump that records every call and fails.
    pub fn failing() -> Self {
        Self {
            bumped: Mutex::new(Vec::new()),
            fail: true,
        }
    }

    /// The bumped shard ids, in call order.
    pub fn bumped(&self) -> Vec<i64> {
        self.bumped.lock().expect("bump lock").clone()
    }
}

impl mem_episodic::promotion::VectorCountBump for FakeVectorCountBump {
    fn bump<'a>(
        &'a self,
        shard_id: i64,
    ) -> BoxFuture<'a, Result<(), mem_store::error::StoreError>> {
        let fail = self.fail;
        self.bumped.lock().expect("bump lock").push(shard_id);
        Box::pin(async move {
            if fail {
                return Err(mem_store::error::StoreError::internal(
                    "fake vector count bump failure",
                ));
            }
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// W4.3 fake provider (the `AiProvider` seam the milestone summarizer uses)
// ---------------------------------------------------------------------------
/// A deterministic [`AiProvider`] fake.
///
/// - `text_chunks` are yielded as `{"text": ...}` chunks, followed by the
///   `{"done": true}` terminator;
/// - `fail` makes the stream yield one provider error instead;
/// - `requests` records every request the provider received.
pub struct FakeProvider {
    text_chunks: Vec<String>,
    fail: Option<String>,
    requests: Mutex<Vec<mem_ai::provider::request::CompletionRequest>>,
}

impl FakeProvider {
    /// A provider that streams `text_chunks` and then terminates.
    pub fn with_text_chunks(text_chunks: Vec<String>) -> Self {
        Self {
            text_chunks,
            fail: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// A provider whose stream yields one error carrying `message`.
    pub fn failing(message: &str) -> Self {
        Self {
            text_chunks: Vec::new(),
            fail: Some(message.to_string()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// The requests the provider received, in call order.
    pub fn requests(&self) -> Vec<mem_ai::provider::request::CompletionRequest> {
        self.requests
            .lock()
            .expect("provider requests lock")
            .clone()
    }
}

impl AiProvider for FakeProvider {
    fn name(&self) -> &str {
        "fake-provider"
    }

    fn complete(&self, request: mem_ai::provider::request::CompletionRequest) -> CompletionStream {
        self.requests
            .lock()
            .expect("provider requests lock")
            .push(request);
        let mut items: Vec<Result<mem_store::serde_json::Value, mem_ai::provider::ProviderError>> =
            Vec::new();
        if let Some(message) = &self.fail {
            items.push(Err(mem_ai::provider::ProviderError::new(
                mem_ai::provider::ProviderErrorKind::Internal,
                message.clone(),
            )));
        } else {
            for chunk in &self.text_chunks {
                items.push(Ok(mem_store::serde_json::json!({ "text": chunk })));
            }
            items.push(Ok(mem_store::serde_json::json!({ "done": true })));
        }
        Box::pin(futures::stream::iter(items))
    }

    // The `AiProvider` trait is declared with `#[async_trait]`; the impl is
    // written in the desugared form so this crate needs no `async-trait`
    // dependency (the macro's expansion is the boxed-future signature).
    fn embed<'life0, 'async_trait>(
        &'life0 self,
        _request: mem_ai::provider::request::EmbedRequest,
    ) -> BoxFuture<'async_trait, Result<Vec<Vec<f32>>, mem_ai::provider::ProviderError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Err(mem_ai::provider::ProviderError::new(
                mem_ai::provider::ProviderErrorKind::Unsupported,
                "the fake provider does not embed",
            ))
        })
    }

    fn list_models<'life0, 'async_trait>(
        &'life0 self,
    ) -> BoxFuture<
        'async_trait,
        Result<Vec<mem_ai::provider::request::ModelInfo>, mem_ai::provider::ProviderError>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn count_tokens<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _model: &'life1 str,
        _text: &'life2 str,
    ) -> BoxFuture<'async_trait, Result<u32, mem_ai::provider::ProviderError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(0) })
    }
}

// ---------------------------------------------------------------------------
// W4.2 shared helpers (Step 2 sole owner; later steps only *use* them)
// ---------------------------------------------------------------------------

/// Inserts one session through the public write path and reads it back,
/// returning `(row_id, created_at)`.
pub async fn insert_row(
    store: &mem_episodic::store::EpisodicStore,
    project_id: &str,
    session_id: &str,
    agent_id: &str,
    summary: &str,
    files_touched: Vec<String>,
    vector: Vec<f32>,
) -> (String, i64) {
    let embedder = FakeEmbedder::with_vector(vector);
    let new = mem_episodic::record::NewAgentSession {
        project_id: project_id.to_string(),
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        summary: summary.to_string(),
        files_touched,
        ..Default::default()
    };
    let id = store
        .insert_session(new, &embedder)
        .await
        .expect("insert row");
    let stored = store
        .get_session(&id)
        .await
        .expect("get inserted row")
        .expect("inserted row is visible");
    (id, stored.created_at)
}

/// D-12 invariant: no timeline entry claims a parent that is invisible —
/// `parent_session_id.is_none() || orphaned == true || get_session(parent).is_some()`.
pub async fn assert_no_dangling_parent_links(
    store: &mem_episodic::store::EpisodicStore,
    summaries: &[mem_episodic::timeline::SessionSummary],
) {
    for summary in summaries {
        if let Some(parent) = &summary.parent_session_id {
            let visible = store
                .get_session(parent)
                .await
                .expect("get parent session")
                .is_some();
            assert!(
                summary.orphaned || visible,
                "timeline entry {} claims invisible parent {parent} without the orphan flag",
                summary.id
            );
        }
    }
}

/// Re-exported `serde_json` (mem-episodic declares no serde edge of its own).
#[allow(unused_imports)]
pub use mem_store::serde_json;

/// Re-exported `serde` derive traits for test-local types.
#[allow(unused_imports)]
pub use mem_store::serde::{Deserialize, Serialize};
