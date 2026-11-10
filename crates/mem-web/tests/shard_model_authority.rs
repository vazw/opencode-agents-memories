//! W3 — the model authority on the REST surface: a foreign shard refuses
//! search (409, no partial answer) and every vector-producing write (409),
//! while deletes stay permitted; the count authorities stay exact.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use common::{api_request, app_state_with_api, temp_dir_unique};
use mem_core::embedding::EmbeddingIdentity;
use mem_core::scope::Scope;
use mem_embed::embedder::EmbedTask;
use mem_profile::profile_store::{ProfileConfig, ProfileStore};
use mem_profile::prompt::UserPromptManager;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::shard_facts;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};
use mem_web::api::ports::{ApiServices, EmbedPort, PortError};
use mem_web::api::store_adapter::ProductionDeps;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

const API_TOKEN: &str = "test-token";
const DIMENSIONS: usize = 768;
const MODEL: &str = "test-model";
const PROJECT_HASH: &str = "a1b2c3d4e5f67890";
const PROJECT_TAG: &str = "opencode_project_a1b2c3d4e5f67890";
const USER_HASH: &str = "c1c2c3d4e5f67890";
const USER_TAG: &str = "opencode_user_c1c2c3d4e5f67890";
const FOREIGN_FINGERPRINT: &str = "foreign-fingerprint";

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

/// The real-store fixture: a project scope with two foreign shards and a
/// clean user shard, over the production adapters.
struct RealStore {
    config: StoreConfig,
    manager: ShardManager,
    project_shards: Vec<ShardInfo>,
    user_shard: ShardInfo,
}

impl RealStore {
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
            common::throwaway_log_dir(),
            services,
            "unknown",
            "127.0.0.1",
        );
        build_router(Arc::new(state))
    }

    async fn db(&self, shard: &ShardInfo) -> mem_store::db::DbHandle {
        self.manager
            .open_shard(&shard.db_path)
            .await
            .expect("open the fixture shard")
    }

    async fn row_count(&self, shard: &ShardInfo) -> u64 {
        let db = self.db(shard).await;
        shard_facts::read_count(db.as_ref()).await.expect("count")
    }

    async fn stats_count(&self, shard: &ShardInfo) -> u64 {
        let db = self.db(shard).await;
        shard_facts::read_stored_count(db.as_ref())
            .await
            .expect("stored count")
    }

    async fn registry_count(&self, shard: &ShardInfo) -> u64 {
        self.manager
            .get_shard_by_id(shard.id)
            .await
            .expect("registry read")
            .expect("registry row")
            .vector_count
    }
}

/// Removes the fixture's storage root (best effort, like the shared `TempDir`).
fn cleanup(store: &RealStore) {
    let _ = std::fs::remove_dir_all(&store.config.storage_path);
}

/// The identity the fixture's config describes.
fn identity_for(config: &StoreConfig) -> EmbeddingIdentity {
    EmbeddingIdentity {
        model: config.embedding_model.clone(),
        dimensions: config.embedding_dimensions,
        pooling: mem_core::config::EmbeddingPooling::Mean,
        dtype: None,
        use_task_prefixes: false,
        query_prefix: None,
        document_prefix: None,
        max_tokens: None,
    }
}

/// Writes one row directly into `shard` (bypassing the write path, like a
/// legacy install's data) and syncs BOTH count authorities.
async fn insert_row(manager: &ShardManager, shard: &ShardInfo, id: &str, content: &str, tag: &str) {
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
            container_tag: tag.to_string(),
            tags: None,
            memory_type: None,
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
            source_prompt_created_at: None,
        },
    )
    .await
    .expect("insert the fixture row");
    let count = shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync the shard count");
    manager
        .set_vector_count(shard.id, count)
        .await
        .expect("adopt the registry count");
}

/// Overwrites the shard's stored fingerprint with a foreign one.
async fn make_foreign(manager: &ShardManager, shard: &ShardInfo) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    db.execute(
        "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_fingerprint', ?1)",
        vec![mem_store::db::Value::Text(FOREIGN_FINGERPRINT.to_string())],
    )
    .await
    .expect("seed the foreign fingerprint");
    manager
        .invalidate_model_facts(&shard.db_path)
        .expect("invalidate the facts after a raw write");
}

/// Removes the shard's fingerprint (the legacy identity) and invalidates the
/// cached facts.
async fn clear_fingerprint(manager: &ShardManager, shard: &ShardInfo) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    db.execute(
        "DELETE FROM shard_metadata WHERE key = 'embedding_fingerprint'",
        Vec::new(),
    )
    .await
    .expect("clear the fingerprint");
    manager
        .invalidate_model_facts(&shard.db_path)
        .expect("invalidate the facts after a raw write");
}

/// Builds the fixture. `foreign` seeds the two project shards with a foreign
/// fingerprint and a row each.
async fn real_store(label: &str, foreign: bool) -> RealStore {
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
        allow_unguarded_open: false,
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
        Some(identity_for(&config)),
    );
    let project_shard_zero = manager
        .create_shard(Scope::Project, PROJECT_HASH, 0)
        .await
        .expect("create project shard 0");
    // W4/ADR-0022: exactly one active shard per tuple.
    manager
        .mark_shard_read_only(project_shard_zero.id)
        .await
        .expect("retire project shard 0 before creating shard 1");
    let project_shards = vec![
        project_shard_zero,
        manager
            .create_shard(Scope::Project, PROJECT_HASH, 1)
            .await
            .expect("create project shard 1"),
    ];
    let user_shard = manager
        .create_shard(Scope::User, USER_HASH, 0)
        .await
        .expect("create the user shard");

    if foreign {
        for shard in &project_shards {
            make_foreign(&manager, shard).await;
            insert_row(
                &manager,
                shard,
                &format!("legacy-{}", shard.id),
                "legacy content",
                PROJECT_TAG,
            )
            .await;
        }
    }

    RealStore {
        config,
        manager,
        project_shards,
        user_shard,
    }
}

fn project_tag_body(content: &str) -> String {
    format!(r#"{{"content":"{content}","containerTag":"{PROJECT_TAG}"}}"#)
}

fn user_tag_body(content: &str) -> String {
    format!(r#"{{"content":"{content}","containerTag":"{USER_TAG}"}}"#)
}

/// AC11 — the search refuses with 409, names EVERY mismatched shard and the
/// command, and carries no partial `data` (the warn-and-continue mutant
/// answers 200 with the good user hit).
#[tokio::test]
async fn test_search_answers_409_naming_every_mismatched_shard_and_the_command() {
    let store = real_store("model-api-search", true).await;
    insert_row(
        &store.manager,
        &store.user_shard,
        "mem_user_hit",
        "good user hit",
        USER_TAG,
    )
    .await;
    let router = store.router();

    let response = api_request(
        router.clone(),
        "GET",
        "/api/search?q=probe",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 409, "{}", response.body_string());
    let body = response.body_string();
    for shard in &store.project_shards {
        assert!(
            body.contains(&format!("shard {}", shard.id)),
            "every mismatched shard is named: {body}"
        );
    }
    assert!(body.contains("--mode=reembed-scope"), "{body}");
    assert!(body.contains(PROJECT_TAG), "{body}");
    assert!(!body.contains("\"data\""), "no partial answer: {body}");
    assert!(
        !body.contains("mem_user_hit"),
        "the good hit is absent: {body}"
    );

    cleanup(&store);
}

/// AC12 — POST into a foreign shard is 409 and the row count is unchanged.
#[tokio::test]
async fn test_a_write_to_a_mismatched_shard_answers_409() {
    let store = real_store("model-api-post", true).await;
    let active = store
        .project_shards
        .iter()
        .find(|shard| shard.shard_index == 1)
        .expect("the active project shard");
    let before = store.row_count(active).await;
    let router = store.router();

    let response = api_request(
        router.clone(),
        "POST",
        "/api/memories",
        Some(API_TOKEN),
        Some(&project_tag_body("new content")),
    )
    .await;
    assert_eq!(response.status, 409, "{}", response.body_string());
    assert!(response.body_string().contains("--mode=reembed-scope"));
    assert_eq!(
        store.row_count(active).await,
        before,
        "the refused write must not append"
    );

    cleanup(&store);
}

/// AC15 — the PUT hole: a row in a foreign shard is refused with 409; its
/// content and the row count are unchanged (the r1 hole answered 200 and
/// rewrote the row).
#[tokio::test]
async fn test_update_into_a_mismatched_shard_answers_409() {
    let store = real_store("model-api-put", true).await;
    let active = store
        .project_shards
        .iter()
        .find(|shard| shard.shard_index == 1)
        .expect("the active project shard");
    insert_row(
        &store.manager,
        active,
        "mem_put",
        "original content",
        PROJECT_TAG,
    )
    .await;
    let before = store.row_count(active).await;
    let router = store.router();

    let response = api_request(
        router.clone(),
        "PUT",
        "/api/memories/mem_put",
        Some(API_TOKEN),
        Some(r#"{"content":"rewritten content"}"#),
    )
    .await;
    assert_eq!(response.status, 409, "{}", response.body_string());
    assert!(response.body_string().contains("--mode=reembed-scope"));

    let db = store.db(active).await;
    let row = vector::get_memory_by_id(db.as_ref(), "mem_put")
        .await
        .expect("read the row")
        .expect("the row still exists");
    assert_eq!(row.text("content").as_deref(), Some("original content"));
    assert_eq!(store.row_count(active).await, before);

    cleanup(&store);
}

/// AC13 — an insert on a clean shard keeps both count authorities exact.
#[tokio::test]
async fn test_an_insert_keeps_both_count_authorities_exact() {
    let store = real_store("model-api-insert-count", false).await;
    for index in 0..3 {
        insert_row(
            &store.manager,
            &store.user_shard,
            &format!("mem_seed_{index}"),
            "seed content",
            USER_TAG,
        )
        .await;
    }
    assert_eq!(store.stats_count(&store.user_shard).await, 3);
    let router = store.router();

    let response = api_request(
        router.clone(),
        "POST",
        "/api/memories",
        Some(API_TOKEN),
        Some(&user_tag_body("inserted content")),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    assert_eq!(store.row_count(&store.user_shard).await, 4);
    assert_eq!(store.stats_count(&store.user_shard).await, 4);
    assert_eq!(store.registry_count(&store.user_shard).await, 4);

    cleanup(&store);
}

/// AC14 — a delete keeps both count authorities exact, and a delete on a
/// foreign shard still succeeds (erasure must not depend on a re-embed).
#[tokio::test]
async fn test_a_delete_keeps_both_count_authorities_exact() {
    let store = real_store("model-api-delete-count", false).await;
    for index in 0..3 {
        insert_row(
            &store.manager,
            &store.user_shard,
            &format!("mem_del_{index}"),
            "seed content",
            USER_TAG,
        )
        .await;
    }
    let router = store.router();
    let response = api_request(
        router.clone(),
        "DELETE",
        "/api/memories/mem_del_0",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(store.row_count(&store.user_shard).await, 2);
    assert_eq!(store.stats_count(&store.user_shard).await, 2);
    assert_eq!(store.registry_count(&store.user_shard).await, 2);
    cleanup(&store);

    // The foreign arm: the same delete is permitted on a foreign shard.
    let store = real_store("model-api-delete-foreign", true).await;
    let legacy_id = format!("legacy-{}", store.project_shards[1].id);
    let router = store.router();
    let response = api_request(
        router.clone(),
        "DELETE",
        &format!("/api/memories/{legacy_id}"),
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(store.row_count(&store.project_shards[1]).await, 0);
    assert_eq!(store.stats_count(&store.project_shards[1]).await, 0);
    assert_eq!(store.registry_count(&store.project_shards[1]).await, 0);
    cleanup(&store);
}

/// AC6b — the legacy fallback warns on the READ seam too: one warn for the
/// unfingerprinted user shard, none for the fingerprinted project shards.
///
/// The capture is process-wide: installed once per test binary, the interest
/// cache rebuilt after the install (and once more immediately before the
/// request), and events filtered by the emitting thread. A thread-local
/// `set_default` would race the callsite's first registration in the parallel
/// workspace schedule and can leave the callsite `Interest::never` (zero
/// events); see `tests/common/capture.rs`. `api_request` polls the router on
/// this thread, so the filter is valid.
#[tokio::test]
async fn test_the_legacy_fallback_warns_on_the_search_path() {
    let capture = common::capture::install();
    let store = real_store("warn-model-search", false).await;
    insert_row(
        &store.manager,
        &store.user_shard,
        "mem_warn_hit",
        "legacy hit",
        USER_TAG,
    )
    .await;
    clear_fingerprint(&store.manager, &store.user_shard).await;
    let router = store.router();

    capture.clear_current_thread();
    tracing::callsite::rebuild_interest_cache();
    let response = api_request(
        router.clone(),
        "GET",
        "/api/search?q=probe",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let events = capture.events();
    assert_eq!(events.len(), 1, "one warn for the legacy shard: {events:?}");
    let event = &events[0];
    assert_eq!(event.shard_id, Some(store.user_shard.id));
    assert_eq!(event.authority.as_deref(), Some("model-equality"));
    assert_eq!(event.stored_model.as_deref(), Some("test-model"));
    let storage = store.config.storage_path.display().to_string();
    assert!(!event.rendered.contains(&storage), "{}", event.rendered);
    assert!(!event.rendered.contains("legacy hit"));

    cleanup(&store);
}
