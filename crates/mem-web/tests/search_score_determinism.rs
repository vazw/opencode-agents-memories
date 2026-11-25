//! Issue 169 regression suite (Rust-only — no upstream test file maps to it):
//! `/api/search` similarity must be a function of `(memory, query)`, never of
//! the caller's `pageSize`.
//!
//! The defect: the keyword bump was collected under a per-token `LIMIT k`
//! derived from `page_size`, so a row deeper than `k` inside its token's scan
//! silently lost that token's bump and reported three different similarities
//! for the same query at `pageSize` 20 / 30 / 40.
//!
//! These cases drive the shipped router (`build_router` + `oneshot`) over a
//! real store: a temp root, two project shards (102 and 10 rows, one container
//! tag each), and the production adapters. Only the embedder is a fake — a
//! fixed 768-dim vector, so the content channel is constant and the keyword
//! channel is the only variable. No prompts are seeded, so the handler's
//! prompt term is provably zero and `total` is the exact row count in scope
//! (112), stable across `pageSize`.

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
/// The embedding width the real store is configured with.
const DIMENSIONS: usize = 768;
/// Shard A's project-scope hash.
const PROJECT_HASH_A: &str = "a1b2c3d4e5f67890";
/// Shard B's project-scope hash.
const PROJECT_HASH_B: &str = "4bb4d8f9597749d4";
/// Shard A's container tag.
const TAG_A: &str = "opencode_project_a1b2c3d4e5f67890";
/// Shard B's container tag.
const TAG_B: &str = "opencode_project_4bb4d8f9597749d4";
/// The 102nd `alpha` match and a `beta` match: loses its `alpha` bump below
/// `k = 102`, so its score moves with `page_size` before the fix.
const VICTIM: &str = "mem_victim";
/// `beta` only, and the newest row overall: leads the victim at `pageSize=10`
/// and trails it at `pageSize=200` before the fix.
const RIVAL: &str = "mem_rival";
/// `alpha`-only fillers in shard A (the victim is the next `alpha` match).
const ALPHA_FILLERS: usize = 101;
/// Shard A's row count: the fillers plus the victim.
const SHARD_A_ROWS: usize = ALPHA_FILLERS + 1;
/// Shard B's row count: the rival plus 9 token-free fillers.
const SHARD_B_ROWS: usize = 10;
/// The frozen query: two tokens, so one hit is `0.1` of the blend. Its
/// URL-encoded form is the `q` every call sends.
const QUERY_PATH: &str = "q=alpha%20beta";
/// The API's `page_size` ceiling.
const PAGE_SIZE_CEILING: usize = 200;
/// A base stamp; every row carries a distinct `created_at`.
const BASE_CREATED_AT: i64 = 1_700_000_000_000;

/// A fixed-vector embedder: every text embeds to `[1; DIMENSIONS]`.
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

/// The cosine between the fixture's query vector and its row vector — both are
/// `[1; DIMENSIONS]`, so this is `1` computed from the fixture itself rather
/// than restated.
fn fixture_cosine(dimensions: usize) -> f64 {
    let vector = vec![1.0f64; dimensions];
    let dot: f64 = vector.iter().map(|value| value * value).sum();
    let norm = dot.sqrt();
    dot / (norm * norm)
}

/// The real-store fixture: a temp root, two project shards, one container tag
/// each, and no prompts.
struct RealStore {
    config: StoreConfig,
    manager: ShardManager,
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
            common::throwaway_log_dir(),
            services,
            "unknown",
            "127.0.0.1",
        );
        build_router(Arc::new(state))
    }
}

/// Writes `rows` into `shard` — the shard is opened once, then the registry's
/// vector count is synced once.
async fn seed(
    manager: &ShardManager,
    shard: &ShardInfo,
    rows: &[(String, String, i64, Vec<f32>)],
    container_tag: &str,
) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    for (id, content, created_at, vector) in rows {
        vector::insert_vector(
            db.as_ref(),
            &MemoryInsert {
                id: id.clone(),
                content: content.clone(),
                vector: vector.clone(),
                tags_vector: None,
                container_tag: container_tag.to_string(),
                tags: None,
                memory_type: None,
                created_at: *created_at,
                updated_at: *created_at,
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
    }
    let count = mem_store::shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync the shard count");
    manager
        .set_vector_count(shard.id, count)
        .await
        .expect("adopt the registry count");
}

/// Builds the real store: shard A holds 101 `alpha` fillers then the victim
/// (the 102nd `alpha` match); shard B holds the rival plus 9 token-free rows.
/// `created_at` is distinct for every row and the rival is the newest, so the
/// ranking is total and the victim/rival order has something to flip.
///
/// The fillers' vector is half-orthogonal to the query, so their content
/// similarity (`≈ 0.05`) is far below the victim's (`≈ 1`): the victim is
/// served at every budget, and the failure under test is its *score*, not its
/// absence. The shard's own `results.truncate(limit)` breaks score ties by
/// hydration order, so a tie would silently hide the row.
async fn real_store() -> RealStore {
    let config = StoreConfig {
        storage_path: temp_dir_unique("issue169-score-determinism"),
        embedding_dimensions: DIMENSIONS as u32,
        embedding_model: "test-model".to_string(),
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
        None,
    );
    let shard_a = manager
        .create_shard(Scope::Project, PROJECT_HASH_A, 0)
        .await
        .expect("create shard A");
    let shard_b = manager
        .create_shard(Scope::Project, PROJECT_HASH_B, 0)
        .await
        .expect("create shard B");

    let query_direction = vec![1.0f32; DIMENSIONS];
    let mut filler_direction = vec![0.0f32; DIMENSIONS];
    filler_direction[0] = 1.0;
    filler_direction[1] = 1.0;

    let mut rows_a: Vec<(String, String, i64, Vec<f32>)> = (0..ALPHA_FILLERS)
        .map(|index| {
            (
                format!("mem_alpha_{index:03}"),
                format!("alpha filler {index:03}"),
                BASE_CREATED_AT + index as i64,
                filler_direction.clone(),
            )
        })
        .collect();
    rows_a.push((
        VICTIM.to_string(),
        "alpha beta victim".to_string(),
        BASE_CREATED_AT + ALPHA_FILLERS as i64,
        query_direction.clone(),
    ));
    seed(&manager, &shard_a, &rows_a, TAG_A).await;

    let mut rows_b: Vec<(String, String, i64, Vec<f32>)> = Vec::new();
    for index in 0..(SHARD_B_ROWS - 1) {
        rows_b.push((
            format!("mem_noise_{index:02}"),
            format!("filler text {index:02}"),
            BASE_CREATED_AT + 500 + index as i64,
            query_direction.clone(),
        ));
    }
    rows_b.push((
        RIVAL.to_string(),
        "beta rival".to_string(),
        BASE_CREATED_AT + 1_000,
        query_direction.clone(),
    ));
    seed(&manager, &shard_b, &rows_b, TAG_B).await;

    RealStore { config, manager }
}

/// `GET /api/search` at `page_size`, page 1, returning the `data` object.
async fn search(router: axum::Router, page_size: usize) -> Value {
    search_page(router, page_size, 1).await
}

/// `GET /api/search` at `page_size` and `page`, returning the `data` object.
async fn search_page(router: axum::Router, page_size: usize, page: usize) -> Value {
    let path = format!("/api/search?{QUERY_PATH}&pageSize={page_size}&page={page}");
    let response = api_request(router, "GET", &path, Some(API_TOKEN), None).await;
    assert_eq!(
        response.status,
        200,
        "GET {path} must answer 200 (body: {})",
        response.body_string()
    );
    let body: Value = serde_json::from_str(&response.body_string()).expect("search answers JSON");
    body["data"].clone()
}

/// The served `(id, similarity)` sequence of one search response.
fn scored(data: &Value) -> Vec<(String, f64)> {
    data["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|item| {
            (
                item["id"].as_str().expect("item id").to_string(),
                item["similarity"].as_f64().expect("item similarity"),
            )
        })
        .collect()
}

/// The served id sequence of one search response.
fn ids(data: &Value) -> Vec<String> {
    scored(data).into_iter().map(|(id, _)| id).collect()
}

/// One served item by id.
fn item<'a>(data: &'a Value, id: &str) -> Option<&'a Value> {
    data["items"]
        .as_array()?
        .iter()
        .find(|item| item["id"].as_str() == Some(id))
}

/// AC1: the victim's similarity is one value at every `pageSize`. Before the
/// fix it was `0.6` at 10/20/40 (only `beta` counted) and `0.7` at 200.
#[tokio::test]
async fn test_search_similarity_is_identical_across_page_sizes() {
    let store = real_store().await;
    let saturated = 0.5 * fixture_cosine(DIMENSIONS) + 0.2 * (2.0 / 2.0);

    let mut measured: Vec<(usize, f64)> = Vec::new();
    for page_size in [10usize, 20, 40, PAGE_SIZE_CEILING] {
        let data = search(store.router(), page_size).await;
        let victim = item(&data, VICTIM).unwrap_or_else(|| {
            panic!(
                "the victim must be served at pageSize={page_size}: {:?}",
                ids(&data)
            )
        });
        let similarity = victim["similarity"].as_f64().expect("victim similarity");
        assert!(
            (similarity - saturated).abs() < 1e-6,
            "pageSize={page_size}: victim similarity {similarity} != {saturated} (both tokens match the victim)"
        );
        measured.push((page_size, similarity));
    }

    for pair in measured.windows(2) {
        assert_eq!(
            pair[0].1, pair[1].1,
            "victim similarity moved with page_size: {:?}",
            measured
        );
    }
}

/// AC2 (order/partition form): the pages actually served are the `pageSize=200`
/// ranking filtered to the ids those pages returned. Before the fix the rival
/// preceded the victim at `pageSize=10` and followed it at 200, so the
/// sequences disagreed.
#[tokio::test]
async fn test_search_pages_are_a_partition_of_the_ranking() {
    let store = real_store().await;
    let page_size = 10;

    let mut paged_ids: Vec<String> = Vec::new();
    for page in 1..=2 {
        let data = search_page(store.router(), page_size, page).await;
        assert_eq!(
            data["total"].as_u64().expect("total") as usize,
            SHARD_A_ROWS + SHARD_B_ROWS,
            "total is the exact row count in scope with zero prompts"
        );
        paged_ids.extend(ids(&data));
    }
    assert_eq!(paged_ids.len(), 2 * page_size, "two full pages are served");
    assert!(
        paged_ids.contains(&VICTIM.to_string()) && paged_ids.contains(&RIVAL.to_string()),
        "precondition: both the victim and the rival are served at pageSize={page_size}: {paged_ids:?}"
    );
    assert_eq!(
        paged_ids.len(),
        paged_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        "no id repeats across the pages"
    );

    let wide = search(store.router(), PAGE_SIZE_CEILING).await;
    assert_eq!(
        wide["total"].as_u64().expect("total") as usize,
        SHARD_A_ROWS + SHARD_B_ROWS,
        "total at the ceiling is the same exact row count"
    );
    let wide_ids = ids(&wide);
    assert!(
        wide_ids.contains(&VICTIM.to_string()) && wide_ids.contains(&RIVAL.to_string()),
        "precondition: the wide call serves both the victim and the rival"
    );

    let expected: Vec<String> = wide_ids
        .iter()
        .filter(|id| paged_ids.contains(id))
        .cloned()
        .collect();
    assert_eq!(
        paged_ids, expected,
        "the paged sequence must be the pageSize={PAGE_SIZE_CEILING} ranking filtered to the served ids"
    );
}

/// AC4 — `total` is the exact row count in scope and does not move with
/// `pageSize`. The cross-size comparison is the property: a single size is
/// satisfied by `total = entries.len()` at the ceiling (112 of 112 rows fit),
/// so the mutant survives every one-size-at-a-time assertion.
#[tokio::test]
async fn test_search_total_is_the_exact_row_count_at_every_page_size() {
    let store = real_store().await;
    let expected = SHARD_A_ROWS + SHARD_B_ROWS; // 112, zero prompts
    let mut measured: Vec<(usize, usize)> = Vec::new();
    for page_size in [10usize, 20, 40, PAGE_SIZE_CEILING] {
        let data = search(store.router(), page_size).await;
        let total = data["total"].as_u64().expect("total") as usize;
        assert_eq!(
            total, expected,
            "pageSize={page_size}: total must be the exact row count in scope"
        );
        measured.push((page_size, total));
    }
    for pair in measured.windows(2) {
        assert_eq!(
            pair[0].1, pair[1].1,
            "total moved with pageSize: {measured:?}"
        );
    }
}

/// CONTROL: two identical calls at a fixed `pageSize` are already byte
/// identical today, so the RED cases above are attributable to `page_size`,
/// not to run-to-run noise.
#[tokio::test]
async fn test_search_repeats_byte_identically_at_a_fixed_page_size() {
    let store = real_store().await;
    let first = scored(&search(store.router(), 20).await);
    let second = scored(&search(store.router(), 20).await);
    assert_eq!(
        first, second,
        "two identical pageSize=20 calls must agree exactly"
    );
}
