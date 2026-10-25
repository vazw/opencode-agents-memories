//! The shared real-store fixture for the deduplication integration tests
//! (`tests/dedup_store.rs` and `tests/dedup_endpoint.rs`).
//!
//! Generalized from the `turso_api_scope.rs` fixture (that file keeps its own
//! copy and is not refactored here). Both consumers are separate integration
//! crates, so a `common` module is the only way they can share it.
//!
//! After hardening 43.2 the shard becomes the source of truth for its own
//! count; [`vector_count`] is the single place this fixture retargets when
//! that lands.

use std::sync::Arc;

use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};

use super::temp_dir_unique;

/// The embedding width the fixture's store is configured with.
pub const DIMENSIONS: usize = 768;
/// The embedding model name the fixture's store is configured with.
pub const MODEL: &str = "test-model";

/// A temp storage root plus the manager that owns it.
pub struct RealStore {
    pub config: StoreConfig,
    pub manager: ShardManager,
}

/// A fresh 768-dim store under its own temp root.
pub fn new_store(label: &str) -> RealStore {
    let storage = temp_dir_unique(label);
    let config = StoreConfig {
        storage_path: storage,
        embedding_dimensions: DIMENSIONS as u32,
        embedding_model: MODEL.to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    };
    let connections = Arc::new(ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        connections,
        Arc::new(SystemProcessProbe),
    );
    RealStore { config, manager }
}

/// Creates one shard in the registry (and initializes its file).
pub async fn create_shard(store: &RealStore, scope: Scope, hash: &str, index: u32) -> ShardInfo {
    store
        .manager
        .create_shard(scope, hash, index)
        .await
        .expect("the fixture shard is created")
}

/// Writes one memory row into `shard` and bumps the registry's vector count.
pub async fn insert(
    store: &RealStore,
    shard: &ShardInfo,
    id: &str,
    content: &str,
    tag: &str,
    created_at: i64,
    embedding: Vec<f32>,
) {
    let db = store
        .manager
        .open_shard(&shard.db_path)
        .await
        .expect("the fixture shard opens");
    vector::insert_vector(
        db.as_ref(),
        &MemoryInsert {
            id: id.to_string(),
            content: content.to_string(),
            vector: embedding,
            tags_vector: None,
            container_tag: tag.to_string(),
            tags: None,
            memory_type: None,
            created_at,
            updated_at: created_at,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
        },
    )
    .await
    .expect("the fixture row inserts");
    store
        .manager
        .increment_vector_count(shard.id)
        .await
        .expect("the fixture bumps the registry count");
}

/// Re-reads the registry row (the registered vector count).
pub async fn vector_count(store: &RealStore, shard: &ShardInfo) -> ShardInfo {
    store
        .manager
        .get_shard_by_id(shard.id)
        .await
        .expect("the registry row reads")
        .expect("the registry row exists")
}

/// The ids currently present in one shard's `memories` table.
pub async fn row_ids(store: &RealStore, shard: &ShardInfo) -> Vec<String> {
    let db = store
        .manager
        .open_shard(&shard.db_path)
        .await
        .expect("the fixture shard opens");
    let rows = db
        .query("SELECT id FROM memories", Vec::new())
        .await
        .expect("the fixture rows read");
    rows.iter().filter_map(|row| row.text("id")).collect()
}

/// A 768-dim vector along `x`.
pub fn unit_x() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[0] = 1.0;
    vector
}

/// A 768-dim vector along `y` (orthogonal to [`unit_x`]).
pub fn unit_y() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[1] = 1.0;
    vector
}

/// A unit vector in the `x`/`y` plane at `cos = c` from [`unit_x`].
pub fn with_cos(c: f64) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[0] = c as f32;
    vector[1] = (1.0 - c * c).sqrt() as f32;
    vector
}

/// A unit vector in the `y`/`z` plane at `cos = c` from [`unit_y`].
pub fn with_cos_y(c: f64) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[1] = c as f32;
    vector[2] = (1.0 - c * c).sqrt() as f32;
    vector
}

/// A vector whose cosine with itself is **exactly** `1.0`: four `3.0` lanes
/// give `|v|^2 = 36 = 6^2`, so `dot / (|v| * |v|)` is `36 / 36` with no
/// rounding (the `sqrt(mag)^2` recipe is not always exact).
pub fn identical_vector() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    for lane in vector.iter_mut().take(4) {
        *lane = 3.0;
    }
    vector
}
