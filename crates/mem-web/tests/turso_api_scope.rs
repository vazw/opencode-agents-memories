//! Scope-aware API behaviour over the memory store — upstream
//! `turso-api-scope.test.ts`.
//!
//! The upstream file runs against a **real database**, so this file does too:
//! a temp storage root, a real [`ShardManager`] over a 768-dim [`StoreConfig`],
//! a project shard and a user shard with real rows, and the **production**
//! adapters from [`mem_web::api::store_adapter`]. Only the embedder is a fake
//! (a fixed 768-dim vector) — exactly what upstream's test does when it
//! monkey-patches `embeddingService.embedWithTimeout`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use common::{api_request, app_state_with_api, temp_dir_unique};
use mem_core::scope::Scope;
use mem_embed::embedder::EmbedTask;
use mem_profile::profile_store::{ProfileConfig, ProfileStore};
use mem_profile::prompt::UserPromptManager;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};
use mem_web::api::ports::{ApiServices, EmbedPort, PortError};
use mem_web::api::store_adapter::ProductionDeps;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The configured API token.
const API_TOKEN: &str = "test-token";
/// The user-scope hash upstream's fixture uses.
const USER_HASH: &str = "c1c2c3d4e5f67890";
/// A distinct project-scope hash (the shard registry needs one per scope).
const PROJECT_HASH: &str = "a1b2c3d4e5f67890";
/// The user-scope container tag (carries `_user_`).
const USER_TAG: &str = "opencode_user_c1c2c3d4e5f67890";
/// The project-scope container tag (carries `_project_`).
const PROJECT_TAG: &str = "opencode_project_a1b2c3d4e5f67890";
/// A tag carrying neither scope marker (X3: not in `total`, still in `byType`).
const ORPHAN_TAG: &str = "mem_projectx_c1c2c3d4e5f67890";
/// The project-scope seed id.
const MEMORY_ID: &str = "mem_note_project";
/// The user-scope seed id.
const USER_MEMORY_ID: &str = "mem_note_user";
/// The marker-less seed id.
const ORPHAN_ID: &str = "mem_markerless";
/// The embedding width the real store is configured with.
const DIMENSIONS: usize = 768;

/// A fixed-vector embedder (upstream's monkey-patched `embedWithTimeout`).
struct FixedEmbedder {
    dimensions: usize,
}

#[async_trait]
impl EmbedPort for FixedEmbedder {
    async fn warmup(&self) -> Result<(), PortError> {
        Ok(())
    }

    async fn embed(&self, _text: &str, _task: EmbedTask) -> Result<Vec<f32>, PortError> {
        Ok(vec![1.0; self.dimensions])
    }
}

/// The real-store fixture: config, manager and the two shards.
struct RealStore {
    config: StoreConfig,
    manager: ShardManager,
    user_shard: ShardInfo,
}

impl RealStore {
    /// A router over the production adapters (the fake embedder aside).
    fn router(&self) -> axum::Router {
        let storage = self.config.storage_path.clone();
        let services = ApiServices::production(ProductionDeps {
            shards: self.manager.clone(),
            prompts: Arc::new(UserPromptManager::new(
                storage.clone(),
                self.config.clone(),
                3,
            )),
            profile: Arc::new(ProfileStore::new(
                storage,
                self.config.clone(),
                ProfileConfig::default(),
            )),
            embed: Arc::new(FixedEmbedder {
                dimensions: DIMENSIONS,
            }),
        });
        let state = app_state_with_api(
            WebAuth::new(None, None, "user"),
            Some(API_TOKEN),
            None,
            PathBuf::from("/tmp/unused-dist"),
            services,
            "unknown",
            "127.0.0.1",
        );
        build_router(Arc::new(state))
    }

    /// Re-reads the user-scope row straight from the shard (the pin assertion).
    async fn user_memory_row(&self) -> mem_store::db::Row {
        let db = self
            .manager
            .open_shard(&self.user_shard.db_path)
            .await
            .expect("open the user shard");
        vector::get_memory_by_id(db.as_ref(), USER_MEMORY_ID)
            .await
            .expect("read the user-scope row")
            .expect("the user-scope row exists")
    }
}

/// Writes one memory row into `shard` and bumps the registry's vector count.
async fn insert(
    manager: &ShardManager,
    shard: &ShardInfo,
    id: &str,
    content: &str,
    container_tag: &str,
    memory_type: Option<&str>,
) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    vector::insert_vector(
        db.as_ref(),
        &MemoryInsert {
            id: id.to_string(),
            content: content.to_string(),
            vector: vec![1.0; DIMENSIONS],
            tags_vector: None,
            container_tag: container_tag.to_string(),
            tags: None,
            memory_type: memory_type.map(str::to_string),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
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
    .expect("insert the seed row");
    manager
        .increment_vector_count(shard.id)
        .await
        .expect("bump the vector count");
}

/// Builds the real store: a 768-dim temp store with a project shard holding
/// the project row + the marker-less row, and a user shard holding the
/// user-scope row (upstream's fixture shape).
async fn real_store() -> RealStore {
    let storage = temp_dir_unique("turso-api-scope");
    let config = StoreConfig {
        storage_path: storage,
        embedding_dimensions: DIMENSIONS as u32,
        embedding_model: "test-model".to_string(),
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
    let project_shard = manager
        .create_shard(Scope::Project, PROJECT_HASH, 0)
        .await
        .expect("create the project shard");
    let user_shard = manager
        .create_shard(Scope::User, USER_HASH, 0)
        .await
        .expect("create the user shard");

    insert(
        &manager,
        &project_shard,
        MEMORY_ID,
        "probe project note",
        PROJECT_TAG,
        Some("note"),
    )
    .await;
    insert(
        &manager,
        &project_shard,
        ORPHAN_ID,
        "probe orphan row",
        ORPHAN_TAG,
        Some("orphan"),
    )
    .await;
    insert(
        &manager,
        &user_shard,
        USER_MEMORY_ID,
        "probe user note",
        USER_TAG,
        Some("user"),
    )
    .await;

    RealStore {
        config,
        manager,
        user_shard,
    }
}

/// `GET <path>` with the API token, parsed as JSON.
async fn get_json(router: axum::Router, path: &str) -> Value {
    let response = api_request(router, "GET", path, Some(API_TOKEN), None).await;
    assert_eq!(
        response.status,
        200,
        "{path} answered {}: {}",
        response.status,
        response.body_string()
    );
    serde_json::from_str(&response.body_string())
        .unwrap_or_else(|_| panic!("{path} answers JSON: {}", response.body_string()))
}

#[tokio::test]
async fn test_stats_counts_project_scope_memories_separately() {
    let store = real_store().await;
    let router = store.router();

    let body = get_json(router, "/api/stats").await;
    assert_eq!(body["success"], Value::Bool(true), "{body}");
    let data = &body["data"];

    // One project row and one user row; the marker-less row counts in neither.
    assert_eq!(data["total"], 2, "{data}");
    assert_eq!(data["byScope"]["project"], 1, "{data}");
    assert_eq!(data["byScope"]["user"], 1, "{data}");
    // `byType` counts every typed row, marker or not.
    assert_eq!(data["byType"]["note"], 1, "{data}");
    assert_eq!(data["byType"]["user"], 1, "{data}");
    assert_eq!(data["byType"]["orphan"], 1, "{data}");
}

#[tokio::test]
async fn test_user_scope_memories_are_included_in_stats_pin_and_global_search() {
    let store = real_store().await;
    let router = store.router();

    // (a) stats: the user-scope shard is walked.
    let stats = get_json(router.clone(), "/api/stats").await;
    assert_eq!(stats["data"]["byScope"]["user"], 1, "{stats}");
    assert_eq!(stats["data"]["byScope"]["project"], 1, "{stats}");

    // (b) pin: the user-scope row is located across shards and pinned.
    let pinned = api_request(
        router.clone(),
        "POST",
        &format!("/api/memories/{USER_MEMORY_ID}/pin"),
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(pinned.body_string(), r#"{"success":true}"#);
    let row = store.user_memory_row().await;
    assert_eq!(
        row.i64("is_pinned"),
        Some(1),
        "the user-scope memory is pinned in the real store"
    );

    // (c) global search: no tag filter walks every distinct container tag, so
    // both scopes contribute hits.
    let body = get_json(router, "/api/search?q=probe").await;
    assert_eq!(body["success"], Value::Bool(true), "{body}");
    let ids: Vec<&str> = body["data"]["items"]
        .as_array()
        .expect("the items array")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default())
        .collect();
    assert!(
        ids.contains(&USER_MEMORY_ID),
        "the user-scope memory is a search hit: {ids:?}"
    );
    assert!(
        ids.contains(&MEMORY_ID),
        "the project-scope memory is a search hit: {ids:?}"
    );
    assert!(body["data"]["total"].as_u64().unwrap_or(0) >= 2, "{body}");
}
