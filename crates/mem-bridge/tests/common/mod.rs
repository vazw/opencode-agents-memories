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
        None,
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
    build_fixture_runtime_with_chat_capture(storage_root, true).await
}

/// Issue 98: the same fixture runtime with the chat-capture flag set, so the
/// engine-side gate is testable without a composed config.
pub async fn build_fixture_runtime_with_chat_capture(
    storage_root: &Path,
    chat_capture_enabled: bool,
) -> RuntimeFixture {
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
                prompt_stamps: Arc::new(mem_bridge::capture::AbsentPromptStampSource),
            },
        )),
        embedder.clone(),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
    .with_chat_capture_enabled(chat_capture_enabled);
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

// ---------------------------------------------------------------------------
// Issue 70 wave 1 — the map resolver (D-14) and the wire fixture
// ---------------------------------------------------------------------------

use std::sync::atomic::AtomicBool;

use mem_bridge::engine::{Engine, EngineConfig, EngineRuntime};
use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_bridge::tools::ToolServices;
use mem_bridge::tools::inventory::InventoryService;
use mem_bridge::tools::memories::MemoriesService;
use mem_bridge::tools::portability::PortabilityBackend;
use mem_bridge::tools::profile::ProfileService;
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::profile_store::{ProfileConfig, ProfileStore};
use mem_profile::prompt::UserPromptManager;
use mem_store::connection::ConnectionManager;
use mem_store::project_tags::CoreProjectTagSource;
use mem_store::shard_inventory::ShardInventory;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::shard_path_migration::PathMigrationService;
use mem_store::vector::MemoryInsert;

/// The D-14 map resolver: `project_id -> container_tag`, with the identity
/// the fixture resolver cannot carry.
///
/// The hash is parsed from the tag with `split_container_tag` (the validated
/// pair) — never re-derived from the path. A `user`-scope tag is refused, so
/// a test cannot resolve a user tag as a project.
pub struct MapScopeResolver {
    /// `project_id -> container_tag`. The hash is never supplied separately.
    pub entries: Vec<(String, String)>,
    /// `Some("probe@example.com")` for the write/read oracles; `None` only
    /// for T23.
    pub user_email: Option<String>,
    pub user_name: Option<String>,
    /// The tag an unmapped, non-blank project id resolves to.
    pub fallback_tag: String,
}

impl MapScopeResolver {
    /// Maps the given `(project_id, container_tag)` pairs.
    pub fn new(entries: Vec<(&str, String)>, user_email: Option<&str>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(id, tag)| (id.to_string(), tag))
                .collect(),
            user_email: user_email.map(str::to_string),
            user_name: None,
            fallback_tag: "opencode_project_ffffffffffffffff".to_string(),
        }
    }

    /// One project, one tag: the common case.
    pub fn single(project_id: &str, tag: String, user_email: Option<&str>) -> Self {
        Self::new(vec![(project_id, tag)], user_email)
    }

    /// The tag for a hash under the test prefix.
    pub fn project_tag(hash: &str) -> String {
        format!("opencode_project_{hash}")
    }
}

impl ProjectScopeResolver for MapScopeResolver {
    fn resolve(&self, project_id: &str) -> Result<ResolvedProjectScope, ProjectScopeError> {
        if project_id.trim().is_empty() {
            return Err(ProjectScopeError::BlankProjectId);
        }
        let tag = self
            .entries
            .iter()
            .find(|(id, _)| id == project_id)
            .map(|(_, tag)| tag.as_str())
            .unwrap_or(&self.fallback_tag);
        // (Scope, ScopeHash): the validated pair, in one call.
        let (scope, hash) = mem_core::scope::split_container_tag(tag)
            .map_err(|error| ProjectScopeError::Core(error.to_string()))?;
        // Mismatch rejection: this seam is the PROJECT half only.
        if scope != mem_core::scope::Scope::Project {
            return Err(ProjectScopeError::Core(format!(
                "resolved tag {tag:?} is not a project-scope tag"
            )));
        }
        // `apply_resolved_scope` stays the validator; the identity is
        // overwritten only AFTER it passes.
        let mut resolved =
            mem_bridge::project_scope::apply_resolved_scope(project_id, tag, hash.as_str())?;
        resolved.user_email = self.user_email.clone();
        resolved.user_name = self.user_name.clone();
        Ok(resolved)
    }
}

/// The engine plus the handles the oracles assert against. The `TestDir`
/// drops last, after every service over it.
pub struct WireFixture {
    pub engine: Engine,
    pub shards: Arc<ShardManager>,
    pub embedder: Arc<FakeEmbedder>,
    pub resolver: Arc<MapScopeResolver>,
    pub store: Arc<EpisodicStore>,
    pub profile: Arc<ProfileStore>,
    pub prompts: Arc<UserPromptManager>,
    pub learning_lock: Arc<LearningLock>,
    _dir: TestDir,
}

impl WireFixture {
    /// The storage root (for `list-shards`/`migrate` path arguments).
    pub fn storage_root(&self) -> PathBuf {
        self._dir.path().to_path_buf()
    }
}

/// Builds the wire fixture: one real store/shard manager over a temp root,
/// the fake embedder, the map resolver and the four composed tool services.
pub async fn build_wire_fixture(
    dir: TestDir,
    resolver: Arc<MapScopeResolver>,
    embedder: Arc<FakeEmbedder>,
    portability: Option<Arc<dyn PortabilityBackend>>,
) -> WireFixture {
    build_wire_fixture_gated(dir, resolver, embedder, portability, true).await
}

/// Issue 98: the wire fixture with the chat-capture flag set, so the `add`
/// boundary (AC9) can be observed on a flag-off engine.
pub async fn build_wire_fixture_gated(
    dir: TestDir,
    resolver: Arc<MapScopeResolver>,
    embedder: Arc<FakeEmbedder>,
    portability: Option<Arc<dyn PortabilityBackend>>,
    chat_capture_enabled: bool,
) -> WireFixture {
    let storage = dir.path().to_path_buf();
    let config = dir.store_config();
    let store = Arc::new(EpisodicStore::new(storage.clone(), config.clone()));
    store.ready().await.expect("wire store ready");
    let connections = Arc::new(ConnectionManager::new(
        storage.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(turso_connect_factory()),
    ));
    let shards = Arc::new(ShardManager::new(
        storage.clone(),
        config.clone(),
        connections.clone(),
        Arc::new(SystemProcessProbe),
        None,
    ));
    let capture = mem_bridge::capture::CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(mem_bridge::capture::DefaultTier1Writer::new(
            mem_bridge::capture::CaptureWriterDeps {
                shards: shards.clone(),
                embedder: embedder.clone(),
                prompt_stamps: Arc::new(mem_bridge::capture::AbsentPromptStampSource),
            },
        )),
        embedder.clone(),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
    .with_chat_capture_enabled(chat_capture_enabled);
    let inject = mem_bridge::inject::InjectService::new(
        store.clone(),
        shards.clone(),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        embedder.clone(),
        Arc::new(SystemProcessProbe),
    );
    let recall = mem_bridge::recall::RecallService::new(store.clone(), embedder.clone());

    let profile = Arc::new(ProfileStore::new(
        storage.clone(),
        config.clone(),
        ProfileConfig::default(),
    ));
    let learning_lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        storage.clone(),
        config.clone(),
    )));
    let prompts = Arc::new(UserPromptManager::new(storage.clone(), config.clone(), 3));
    let tags = Arc::new(CoreProjectTagSource {
        container_tag_prefix: config.container_tag_prefix.clone(),
        user_email_override: None,
        user_name_override: None,
    });
    let inventory = Arc::new(ShardInventory::new(
        storage.clone(),
        config.clone(),
        connections.clone(),
        shards.clone(),
        tags.clone(),
    ));
    let migration = PathMigrationService::new(
        storage.clone(),
        config.clone(),
        connections.clone(),
        shards.clone(),
        inventory.clone(),
        tags.clone(),
        Arc::new(mem_bridge::tools::inventory::PromptStorePathUpdater::new(
            prompts.clone(),
        )),
        Arc::new(SystemProcessProbe),
    );

    let memories = MemoriesService::new(shards.clone(), embedder.clone(), resolver.clone(), 20, 30);
    let profile_service =
        ProfileService::new(profile.clone(), learning_lock.clone(), resolver.clone());
    let inventory_service = InventoryService::new(inventory, migration);
    let tools = ToolServices::from_parts(memories, profile_service, inventory_service, portability);

    let runtime = EngineRuntime {
        capture: Arc::new(capture),
        inject: Arc::new(inject),
        recall: Arc::new(recall),
        skills: None,
        tools: Some(Arc::new(tools)),
    };
    WireFixture {
        engine: Engine::with_runtime(EngineConfig::default(), Arc::new(runtime)),
        shards,
        embedder,
        resolver,
        store,
        profile,
        prompts,
        learning_lock,
        _dir: dir,
    }
}

/// Creates (or joins) one registered shard of `(scope, hash)`.
pub async fn create_shard(
    shards: &ShardManager,
    scope: mem_core::scope::Scope,
    hash: &str,
    index: u32,
) -> ShardInfo {
    shards
        .create_shard(scope, hash, index)
        .await
        .expect("create shard")
}

/// Inserts one seeded row into an already-created shard.
pub async fn insert_row(shards: &ShardManager, shard: &ShardInfo, insert: &MemoryInsert) {
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    mem_store::vector::insert_vector(db.as_ref(), insert)
        .await
        .expect("insert seeded row");
}

/// A seeded row with every optional column at its default.
pub fn new_insert(id: &str, content: &str, container_tag: &str) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: content.to_string(),
        vector: unit_vector(0),
        tags_vector: None,
        container_tag: container_tag.to_string(),
        tags: None,
        memory_type: None,
        created_at: 0,
        updated_at: 0,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
        source_prompt_created_at: None,
    }
}

/// Reads one row back from a shard file.
pub async fn read_row(
    shards: &ShardManager,
    shard: &ShardInfo,
    id: &str,
) -> Option<mem_store::db::Row> {
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    mem_store::vector::get_memory_by_id(db.as_ref(), id)
        .await
        .expect("read row")
}

/// The engine lock's exclusivity proof over a test root (the claimed
/// mutation paths — `migrate` — require it).
pub fn exclusive_root(storage_root: &Path) -> Arc<mem_store::delete_guard::ExclusiveRoot> {
    let outcome =
        mem_store::engine_lock::acquire_engine_lock(storage_root, Arc::new(SystemProcessProbe))
            .expect("acquire the engine lock for a test root");
    let lock = match outcome {
        mem_store::engine_lock::EngineLockOutcome::Acquired(lock) => lock,
        mem_store::engine_lock::EngineLockOutcome::Lost { holder_pid } => {
            panic!("test root already locked by pid {holder_pid}")
        }
    };
    Arc::new(mem_store::delete_guard::ExclusiveRoot::new(lock))
}

/// Runs one `#[ignore]`d case in a CHILD of this same test binary, its stderr
/// on a FILE (`structured_stderr` writes to the raw descriptor, which libtest's
/// capture never intercepts), and returns what the child wrote there.
///
/// Moved here verbatim from `profile_decay_lifecycle.rs` (issue 98, D-98-11):
/// one implementation, two users.
pub fn run_captured(inner: &str, dir: &TestDir) -> String {
    let log = dir.path().join("child.err");
    let stderr = std::fs::File::create(&log).expect("create the child stderr file");
    let status = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            inner,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr))
        .status()
        .expect("the child test binary runs");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        status.success(),
        "the child case `{inner}` must pass: {status}\n{text}"
    );
    text
}
