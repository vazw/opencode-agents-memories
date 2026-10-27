//! Std-only helpers shared by mem-bridge integration tests (house law
//! mirror of `mem-episodic/tests/common`): temp dirs under
//! `std::env::temp_dir()`, injected seams, no dev-dependencies.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::store::EpisodicStore;
use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory as _, ConnectOptions, turso_connect_factory};
use mem_store::platform::Platform;
use mem_store::platform::ProcessProbe;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory (`mem-bridge-w53-{prefix}-{pid}-{counter}`).
pub fn temp_dir(prefix: &str) -> PathBuf {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "mem-bridge-w53-{prefix}-{}-{counter}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Best-effort recursive removal.
pub fn cleanup_temp_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let _ = fs::remove_dir_all(dir);
}

/// Owns a temp dir and removes it on drop.
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

    pub fn store_config(&self) -> StoreConfig {
        store_config(self.path.to_path_buf())
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        cleanup_temp_dir(&self.path);
    }
}

/// Hermetic store config over a fresh temp dir (encryption disabled, 8 dims).
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

/// A wired shard manager over the given storage root.
pub fn shard_manager(config: StoreConfig) -> mem_store::shard_manager::ShardManager {
    shard_manager_with_probe(config, Arc::new(SystemProcessProbe))
}

/// A wired shard manager with an INJECTED process probe.
pub fn shard_manager_with_probe(
    config: StoreConfig,
    probe: Arc<dyn ProcessProbe>,
) -> mem_store::shard_manager::ShardManager {
    let connections = Arc::new(mem_store::connection::ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(turso_connect_factory()),
    ));
    mem_store::shard_manager::ShardManager::new(
        config.storage_path.clone(),
        config,
        connections,
        probe,
    )
}

/// Probe that reports the OWNED pids alive — exactly the OWN process pid
/// (the engine's own consolidation lock) and the injected FOREIGN pid the
/// fixture writes into the lock file. Every other pid is dead.
pub struct OwnedAndForeignProbe {
    pub foreign_pid: u32,
}

impl ProcessProbe for OwnedAndForeignProbe {
    fn is_alive(&self, pid: u32) -> bool {
        pid == std::process::id() || pid == self.foreign_pid
    }
}

pub use mem_store::platform::SystemProcessProbe;

/// Deterministic [`Embedder`] fake (same shape as mem-episodic's).
pub struct FakeEmbedder {
    default_vector: Vec<f32>,
    vectors: Mutex<std::collections::VecDeque<Vec<f32>>>,
    fail: bool,
    calls: Mutex<Vec<String>>,
}

impl FakeEmbedder {
    pub fn new(default_vector: Vec<f32>) -> Self {
        Self {
            default_vector,
            vectors: Mutex::new(std::collections::VecDeque::new()),
            fail: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn with_vector(vector: Vec<f32>) -> Self {
        Self::new(vector)
    }

    pub fn with_vectors(vectors: Vec<Vec<f32>>) -> Self {
        let default_vector = vectors.last().cloned().unwrap_or_default();
        Self {
            default_vector,
            vectors: Mutex::new(vectors.into_iter().collect()),
            fail: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn failing() -> Self {
        Self {
            default_vector: Vec::new(),
            vectors: Mutex::new(std::collections::VecDeque::new()),
            fail: true,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("embedder calls lock").clone()
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

/// First 8-dim basis vector.
pub fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; 8];
    vector[index] = 1.0;
    vector
}

// ---------------------------------------------------------------------------
// W5.3 drain fakes
// ---------------------------------------------------------------------------

/// The W4.2-real store open helper (raw db connection over episodic.db).
pub async fn open_raw(dir: &Path, name: &str) -> mem_store::db::DbHandle {
    let config = store_config(dir.to_path_buf());
    let options = ConnectOptions::for_config(&config, Platform::current());
    turso_connect_factory()
        .connect(dir.join(name), options)
        .await
        .expect("open raw database")
}

use mem_bridge::capture::{
    CaptureSummaryInput, CaptureWriterError, Tier1WriteOutcome, Tier1Writer,
};
use mem_bridge::project_scope::{ProjectScopeError, ResolvedProjectScope};

/// The fixed scope resolver every drain test uses: any project id resolves
/// to ONE valid project scope (prefix_project_<16hex>).
pub struct FixedScopeResolver {
    pub container_tag: String,
    pub scope_hash: String,
}

impl Default for FixedScopeResolver {
    fn default() -> Self {
        Self {
            container_tag: "opencode_project_0123456789abcdef".to_string(),
            scope_hash: "0123456789abcdef".to_string(),
        }
    }
}

/// Lists the files under the storage root (the no-shard-query oracle).
pub async fn list_data_files(dir: &std::path::Path) -> Vec<String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    names
}

impl mem_bridge::scope_resolver::ProjectScopeResolver for FixedScopeResolver {
    fn resolve(&self, project_id: &str) -> Result<ResolvedProjectScope, ProjectScopeError> {
        mem_bridge::project_scope::apply_resolved_scope(
            project_id,
            &self.container_tag,
            &self.scope_hash,
        )
    }
}

/// Counts WRITTEN tier-1 outcomes (failing/fixed first `fail_times`).
pub struct RecorderTier1Writer {
    fail_times: usize,
    calls: std::sync::atomic::AtomicUsize,
    writes: std::sync::atomic::AtomicUsize,
}

impl RecorderTier1Writer {
    pub fn new(fail_times: usize) -> Self {
        Self {
            fail_times,
            calls: std::sync::atomic::AtomicUsize::new(0),
            writes: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The number of successful Tier1WriteOutcome::Written results.
    pub fn writes(&self) -> usize {
        self.writes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Tier1Writer for RecorderTier1Writer {
    fn write<'a>(
        &'a self,
        _scope: &'a ResolvedProjectScope,
        _input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, CaptureWriterError>> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if call < self.fail_times {
                Err(CaptureWriterError::Failed(
                    mem_store::error::StoreError::Internal {
                        message: "injected recorder failure".to_string(),
                    },
                ))
            } else {
                self.writes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Tier1WriteOutcome::Written)
            }
        })
    }
}

/// Summarizer fakes (mem-bridge's `CaptureSummarizer` seam).
pub struct SkipSummarizer;
impl mem_bridge::capture_prompt::CaptureSummarizer for SkipSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> BoxFuture<
        'a,
        Result<mem_bridge::capture::CaptureSummaryOutput, mem_episodic::error::EpisodicError>,
    > {
        Box::pin(async {
            Ok(mem_bridge::capture::CaptureSummaryOutput::Skip(
                mem_bridge::capture::SkipReason::NoContext,
            ))
        })
    }
}

pub struct FailingSummarizer;
impl mem_bridge::capture_prompt::CaptureSummarizer for FailingSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> BoxFuture<
        'a,
        Result<mem_bridge::capture::CaptureSummaryOutput, mem_episodic::error::EpisodicError>,
    > {
        Box::pin(async {
            Err(mem_episodic::error::EpisodicError::Summary {
                message: "injected summarizer failure".to_string(),
            })
        })
    }
}

pub struct FixedSummarizer {
    pub summary: String,
    pub tags: Vec<String>,
    /// The provider `type` the fake reports (the drain must carry it to the
    /// tier-1 `type` column).
    pub memory_type: String,
    /// The episodic extension the fake reports.
    pub extension: mem_bridge::capture::CaptureExtension,
}

impl Default for FixedSummarizer {
    fn default() -> Self {
        Self {
            summary: String::new(),
            tags: Vec::new(),
            memory_type: "bug".to_string(),
            extension: mem_bridge::capture::CaptureExtension::default(),
        }
    }
}

impl mem_bridge::capture_prompt::CaptureSummarizer for FixedSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> BoxFuture<
        'a,
        Result<mem_bridge::capture::CaptureSummaryOutput, mem_episodic::error::EpisodicError>,
    > {
        let summary = self.summary.clone();
        let tags = self.tags.clone();
        let memory_type = self.memory_type.clone();
        let extension = self.extension.clone();
        Box::pin(async move {
            Ok(mem_bridge::capture::CaptureSummaryOutput::Capture {
                summary,
                memory_type,
                tags,
                extension,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Fixture runtime (the bridge_fixtures dispatch oracles)
// ---------------------------------------------------------------------------

/// The full fixture runtime over one temp storage root: REAL episodic db
/// + shards, fake embedder, fixed scope resolution, no summarizer.
pub struct RuntimeFixture {
    pub capture: Arc<mem_bridge::capture::CaptureService>,
    pub inject: Arc<mem_bridge::inject::InjectService>,
    pub recall: Arc<mem_bridge::recall::RecallService>,
    /// The composed episodic store (shared with every service above): the
    /// W5.4 `recall` oracles seed it directly.
    pub store: Arc<EpisodicStore>,
    /// The fixture's embedder, for the same direct seeding.
    pub embedder: Arc<dyn Embedder>,
}

pub async fn build_fixture_runtime(storage_root: &Path) -> RuntimeFixture {
    let config = store_config(storage_root.to_path_buf());
    let store = Arc::new(EpisodicStore::new(
        storage_root.to_path_buf(),
        config.clone(),
    ));
    store.ready().await.expect("fixture store ready");
    let shards = Arc::new(shard_manager(config));
    let embedder: Arc<dyn Embedder> = Arc::new(FakeEmbedder::with_vector(unit_vector(0)));
    let capture = mem_bridge::capture::CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(mem_bridge::capture::DefaultTier1Writer::new(
            mem_bridge::capture::CaptureWriterDeps {
                shards: shards.clone(),
                embedder: embedder.clone(),
            },
        )),
        embedder.clone(),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );
    let inject = mem_bridge::inject::InjectService::new(
        store.clone(),
        shards.clone(),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        embedder.clone(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    RuntimeFixture {
        capture: Arc::new(capture),
        inject: Arc::new(inject),
        recall: Arc::new(mem_bridge::recall::RecallService::new(
            store.clone(),
            embedder.clone(),
        )),
        store,
        embedder,
    }
}
