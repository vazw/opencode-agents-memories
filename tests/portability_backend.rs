//! Issue 70 wave 1 — the production portability backend in the root lib
//! (T36-T37).
//!
//! The backend is built by `ProductionPortabilityFactory` over the same deps
//! the engine's seam assembles, so the resolved prefix/embedding identity and
//! the merge-only import mode are exercised end to end.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_bridge::tools::portability::{
    AbortKind, ExportRequest, ImportRequest, PortabilityBackend, PortabilityDeps,
    PortabilityFactory, PortabilityFault,
};
use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::store::EpisodicStore;
use mem_profile::profile_store::{ProfileConfig, ProfileStore};
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::project_tags::ProjectTagSource;
use mem_store::scope::Scope;
use mem_store::serde_json::Value;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{MemoryInsert, get_memory_by_id, insert_vector};
use mem_web::api::store_adapter::ProductionFacts;
use opencode_agents_memory::portability_backend::ProductionPortabilityFactory;

const PREFIX: &str = "probe-prefix";
const MODEL: &str = "probe-embed-model";
const DIMENSIONS: u32 = 8;
const MEMORY_ID: &str = "portable-memory-1";

/// A dropped temp root.
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(prefix: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "opencode-portability-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the temp root");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The deterministic 8-dim embedder (import re-embeds every row).
struct FakeEmbedder;

impl Embedder for FakeEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async { Ok(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]) })
    }
}

fn store_config(root: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: DIMENSIONS,
        embedding_model: MODEL.to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: PREFIX.to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// One storage root with the four composed stores the backend consumes.
struct Rig {
    root: TempRoot,
    shards: Arc<ShardManager>,
    episodic: Arc<EpisodicStore>,
    profile: Arc<ProfileStore>,
}

impl Rig {
    async fn new(prefix: &str) -> Self {
        let root = TempRoot::new(prefix);
        let config = store_config(root.path());
        let connections = Arc::new(ConnectionManager::new(
            root.path().to_path_buf(),
            config.clone(),
            Platform::current(),
            Arc::new(mem_store::db::turso_connect_factory()),
        ));
        let shards = Arc::new(ShardManager::new(
            root.path().to_path_buf(),
            config.clone(),
            connections,
            Arc::new(SystemProcessProbe),
            None,
        ));
        let episodic = Arc::new(EpisodicStore::new(
            root.path().to_path_buf(),
            config.clone(),
        ));
        episodic.ready().await.expect("episodic store ready");
        let profile = Arc::new(ProfileStore::new(
            root.path().to_path_buf(),
            config,
            ProfileConfig::default(),
        ));
        Self {
            root,
            shards,
            episodic,
            profile,
        }
    }

    fn deps(&self) -> PortabilityDeps {
        PortabilityDeps {
            shards: self.shards.clone(),
            profile: self.profile.clone(),
            episodic: self.episodic.clone(),
            embedder: Arc::new(FakeEmbedder),
            container_tag_prefix: PREFIX.to_string(),
            embedding_model: MODEL.to_string(),
            embedding_dimensions: DIMENSIONS,
        }
    }

    fn backend(&self) -> Arc<dyn PortabilityBackend> {
        ProductionPortabilityFactory
            .build(&self.deps())
            .expect("the production factory builds a backend")
    }
}

/// The container tag the composition root's tag source derives for a path.
fn tag_for(path: &Path) -> String {
    mem_store::project_tags::CoreProjectTagSource {
        container_tag_prefix: PREFIX.to_string(),
        user_email_override: None,
        user_name_override: None,
    }
    .project_tag_info(path)
    .tag
}

fn hash_of(tag: &str) -> String {
    tag.rsplit('_').next().unwrap_or_default().to_string()
}

/// Seeds one row into the rig's project shard for `path`.
async fn seed_memory(rig: &Rig, path: &Path) {
    let tag = tag_for(path);
    let hash = hash_of(&tag);
    let shard = rig
        .shards
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("write shard");
    let db = rig
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    let insert = MemoryInsert {
        id: MEMORY_ID.to_string(),
        content: "portable content".to_string(),
        vector: vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        tags_vector: None,
        container_tag: tag,
        tags: None,
        memory_type: None,
        created_at: 1,
        updated_at: 1,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: Some(path.to_string_lossy().to_string()),
        project_name: None,
        git_repo_url: None,
        source_prompt_created_at: None,
    };
    insert_vector(db.as_ref(), &insert)
        .await
        .expect("seed the portable row");
}

/// Reads the row's content back from the rig's project shard for `path`.
async fn read_content(rig: &Rig, path: &Path) -> Option<String> {
    let tag = tag_for(path);
    let hash = hash_of(&tag);
    let shards = rig
        .shards
        .get_all_shards(Scope::Project, Some(&hash))
        .await
        .expect("target shards");
    for shard in shards {
        let db = rig
            .shards
            .open_shard(&shard.db_path)
            .await
            .expect("open shard");
        if let Some(row) = get_memory_by_id(db.as_ref(), MEMORY_ID)
            .await
            .expect("read row")
        {
            return row.text("content");
        }
    }
    None
}

/// Exports root A's one memory to `output`, returning the bundle's source tag.
async fn export_fixture(rig: &Rig, path: &Path, output: &Path) -> String {
    seed_memory(rig, path).await;
    let backend = rig.backend();
    let outcome = backend
        .export(&ExportRequest {
            project_path: path.to_string_lossy().to_string(),
            output_path: output.to_path_buf(),
        })
        .await
        .expect("the export succeeds");
    assert_eq!(outcome.count, 1);
    assert_eq!(outcome.container_tag, tag_for(path));
    outcome.container_tag
}

// ---------------------------------------------------------------------------
// T36 — the resolved prefix/identity reach the bundle, and the import lands
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_production_backend_round_trips_an_export_and_an_import() {
    let source = Rig::new("source").await;
    let source_path = source.root.path().join("project-a");
    std::fs::create_dir_all(&source_path).expect("source project dir");
    let export_path = source.root.path().join("bundle.json");
    let source_tag = export_fixture(&source, &source_path, &export_path).await;

    // The bundle's bytes carry the RESOLVED prefix and embedding identity.
    let text = std::fs::read_to_string(&export_path).expect("bundle file");
    let document: Value = mem_store::serde_json::from_str(&text).expect("bundle JSON");
    assert_eq!(document["source"]["containerTag"], source_tag);
    assert_eq!(document["source"]["scopeHash"], hash_of(&source_tag));
    assert_eq!(
        document["embedding"]["model"], MODEL,
        "with_embedding_info was applied"
    );
    assert_eq!(document["embedding"]["dimensions"], DIMENSIONS);

    // Import into a different root/path: one row lands under the target tag.
    let target = Rig::new("target").await;
    let target_path = target.root.path().join("project-b");
    std::fs::create_dir_all(&target_path).expect("target project dir");
    let backend = target.backend();
    let outcome = backend
        .import(&ImportRequest {
            project_path: target_path.to_string_lossy().to_string(),
            input_path: export_path.clone(),
            dry_run: false,
        })
        .await
        .expect("the import succeeds");
    assert_eq!(outcome.imported, 1);
    assert_eq!(outcome.container_tag, tag_for(&target_path));
    assert_eq!(
        read_content(&target, &target_path).await.as_deref(),
        Some("portable content")
    );

    // Proof the factory really built over ProductionFacts: an export from the
    // TARGET root now carries the imported row.
    let second_export = target.root.path().join("target-bundle.json");
    let outcome = backend
        .export(&ExportRequest {
            project_path: target_path.to_string_lossy().to_string(),
            output_path: second_export.clone(),
        })
        .await
        .expect("target export");
    assert_eq!(outcome.count, 1);
    let _ = ProductionFacts::new(target.shards.clone());
}

// ---------------------------------------------------------------------------
// T37 — the merge-only import aborts on a duplicate id with zero writes
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_production_backend_import_aborts_on_a_duplicate_id_with_zero_writes() {
    let source = Rig::new("dup-source").await;
    let source_path = source.root.path().join("project-a");
    std::fs::create_dir_all(&source_path).expect("source project dir");
    let export_path = source.root.path().join("bundle.json");
    export_fixture(&source, &source_path, &export_path).await;

    let target = Rig::new("dup-target").await;
    let target_path = target.root.path().join("project-b");
    std::fs::create_dir_all(&target_path).expect("target project dir");
    let backend = target.backend();
    let request = ImportRequest {
        project_path: target_path.to_string_lossy().to_string(),
        input_path: export_path,
        dry_run: false,
    };
    let first = backend
        .import(&request)
        .await
        .expect("the first import succeeds");
    assert_eq!(first.imported, 1);

    let second = backend.import(&request).await;
    match second {
        Err(PortabilityFault::ImportAborted {
            kind,
            count,
            rejected,
        }) => {
            assert_eq!(kind, AbortKind::Memory);
            assert_eq!(count, 1);
            assert_eq!(rejected.len(), 1);
            assert_eq!(rejected[0].id, MEMORY_ID);
        }
        _ => panic!("the duplicate must abort in merge mode"),
    }
    assert_eq!(
        read_content(&target, &target_path).await.as_deref(),
        Some("portable content"),
        "zero writes: the target still holds exactly the first import"
    );
}
