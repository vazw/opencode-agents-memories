//! Issue 165 — `GET /api/search` paging contract:
//!
//! * `total` is the exact row count in scope, stable across `pageSize`;
//! * `items` is `skip(offset).take(pageSize)` of the full ranking, so walking
//!   `page` 1..=`total_pages` yields every entry exactly once;
//! * `shard_prefix` bounds each shard's `LIMIT` by that shard's own row count,
//!   so a deep page never binds `LIMIT -1`;
//! * a failed per-shard count undercounts `total` and does **not** shorten the
//!   page;
//! * the served top 20 equals a full-scan reference for three queries.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use common::fakes::fake_services;
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
use mem_store::vector_utils::{blob_to_float32_array, distance_to_similarity};
use mem_web::api::memories::{MAX_SHARD_PREFIX, shard_prefix};
use mem_web::api::ports::{ApiServices, EmbedPort, MemoryRow, PortError};
use mem_web::api::store_adapter::ProductionDeps;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

const API_TOKEN: &str = "test-token";
const DIMENSIONS: usize = 768;
const PROJECT_HASH_A: &str = "a1b2c3d4e5f67890";
const PROJECT_HASH_B: &str = "4bb4d8f9597749d4";
const TAG_A: &str = "opencode_project_a1b2c3d4e5f67890";
const TAG_B: &str = "opencode_project_4bb4d8f9597749d4";
const BASE_CREATED_AT: i64 = 1_700_000_000_000;
/// Shard A's row count (250) and shard B's (30): 280 total.
const SHARD_A_ROWS: usize = 250;
const SHARD_B_ROWS: usize = 30;
const TOTAL_ROWS: usize = SHARD_A_ROWS + SHARD_B_ROWS;
/// The served/reference similarity window. `vector_distance_cos` is an f32
/// computation, so an f64 reference cannot agree to the blueprint's 1e-12;
/// measured spread is ~2.2e-8, and 1e-6 is two orders below the fixture's
/// smallest non-zero separation.
const TOLERANCE: f64 = 1e-6;

/// The two frozen queries, verbatim.
const QUERY_ONE: &str = "97 sites across 8 crates";
const QUERY_TWO: &str = "turso database lock retry on shard write";
/// The implementer-chosen query, with its tokens recorded: `["alpha", "beta"]`.
///
/// Observed difference (fixture arithmetic, asserted below): `mem_deep` is
/// **rank 75 by content** (74 rows sit at a higher cosine) and outside the
/// keyword channel (`deep row` contains neither token, and its tags
/// `alph, bet` match only through containment — `alph` inside `alpha` — which
/// the boost scores but `LIKE '%token%'` does not), so the old
/// union with `k = max(2*20, 32) = 40` could not return it at all; its blend is
/// `0.4*0.5 + 1.0*0.3 = 0.50`, the highest in the store, so the exact arm
/// serves it first.
const QUERY_THREE: &str = "alpha beta";

/// `[1, 0, …, 0]`: the query direction, so a row's cosine is its first lane.
struct FixedEmbedder;

#[async_trait]
impl EmbedPort for FixedEmbedder {
    async fn warmup(&self) -> Result<(), PortError> {
        Ok(())
    }

    async fn embed(&self, _text: &str, _task: EmbedTask) -> Result<Vec<f32>, PortError> {
        Ok(unit_query())
    }
}

fn unit_query() -> Vec<f32> {
    let mut vector = vec![0.0f32; DIMENSIONS];
    vector[0] = 1.0;
    vector
}

/// A unit vector at cosine `c` from [`unit_query`].
fn with_cos(c: f64) -> Vec<f32> {
    let mut vector = vec![0.0f32; DIMENSIONS];
    vector[0] = c as f32;
    vector[1] = (1.0 - c * c).max(0.0).sqrt() as f32;
    vector
}

struct FixtureRow {
    id: String,
    content: String,
    created_at: i64,
    vector: Vec<f32>,
    tags: Option<String>,
}

fn row(
    id: String,
    content: String,
    created_at: i64,
    vector: Vec<f32>,
    tags: Option<&str>,
) -> FixtureRow {
    FixtureRow {
        id,
        content,
        created_at,
        vector,
        tags: tags.map(str::to_string),
    }
}

/// The real-store fixture: two project shards, one container tag each, no
/// prompts, and a fixed query direction.
struct RealStore {
    config: StoreConfig,
    manager: ShardManager,
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
            embed: Arc::new(FixedEmbedder),
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

async fn seed(manager: &ShardManager, shard: &ShardInfo, rows: &[FixtureRow], tag: &str) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    for fixture in rows {
        vector::insert_vector(
            db.as_ref(),
            &MemoryInsert {
                id: fixture.id.clone(),
                content: fixture.content.clone(),
                vector: fixture.vector.clone(),
                tags_vector: None,
                container_tag: tag.to_string(),
                tags: fixture.tags.clone(),
                memory_type: None,
                created_at: fixture.created_at,
                updated_at: fixture.created_at,
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

/// Shard A: 10 `alpha` fillers, 60 `beta` fillers, the two frozen-query rows
/// apiece, the deep row, the exact tie pair, and noise to 250 rows. Shard B:
/// 30 noise rows.
async fn real_store() -> RealStore {
    let config = StoreConfig {
        storage_path: temp_dir_unique("issue165-exact-paging"),
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

    let mut rows_a: Vec<FixtureRow> = Vec::new();
    for index in 0..10 {
        rows_a.push(row(
            format!("mem_alpha_{index:02}"),
            format!("alpha filler {index:02}"),
            BASE_CREATED_AT + index as i64,
            with_cos(0.70 - index as f64 * 0.004),
            None,
        ));
    }
    for index in 0..60 {
        rows_a.push(row(
            format!("mem_beta_{index:02}"),
            format!("beta filler {index:02}"),
            BASE_CREATED_AT + 100 + index as i64,
            with_cos(0.66 - index as f64 * 0.004),
            None,
        ));
    }
    for index in 0..2 {
        rows_a.push(row(
            format!("mem_crates_{index}"),
            format!("across 8 crates survey {index}"),
            BASE_CREATED_AT + 200 + index as i64,
            with_cos(0.75 - index as f64 * 0.004),
            None,
        ));
    }
    for index in 0..2 {
        rows_a.push(row(
            format!("mem_turso_{index}"),
            format!("turso database lock retry on shard write {index}"),
            BASE_CREATED_AT + 300 + index as i64,
            with_cos(0.76 - index as f64 * 0.004),
            None,
        ));
    }
    // The deep row: content rank 75 for `alpha beta`, blend rank 1.
    rows_a.push(row(
        "mem_deep".to_string(),
        "deep row".to_string(),
        BASE_CREATED_AT + 400,
        with_cos(0.4),
        Some("alph, bet"),
    ));
    // The exact tie pair: identical content, vector, tags and stamp.
    for id in ["mem_tie_a", "mem_tie_b"] {
        rows_a.push(row(
            id.to_string(),
            "tie row".to_string(),
            BASE_CREATED_AT + 500,
            with_cos(0.36),
            Some("alph, bet"),
        ));
    }
    // Noise to 250: cosine falls below the tie pair and clamps to 0 at the end.
    for index in 0..(SHARD_A_ROWS - rows_a.len()) {
        rows_a.push(row(
            format!("mem_noise_a_{index:03}"),
            "noise a filler".to_string(),
            BASE_CREATED_AT + 1_000 + index as i64,
            with_cos(0.2 - index as f64 * 0.002),
            None,
        ));
    }
    assert_eq!(rows_a.len(), SHARD_A_ROWS);
    seed(&manager, &shard_a, &rows_a, TAG_A).await;

    let mut rows_b: Vec<FixtureRow> = Vec::new();
    for index in 0..SHARD_B_ROWS {
        rows_b.push(row(
            format!("mem_noise_b_{index:02}"),
            "noise b filler".to_string(),
            BASE_CREATED_AT + 3_000 + index as i64,
            with_cos(0.19 - index as f64 * 0.006),
            None,
        ));
    }
    seed(&manager, &shard_b, &rows_b, TAG_B).await;

    RealStore { config, manager }
}

fn query_path(query: &str, page_size: usize, page: usize) -> String {
    format!(
        "/api/search?q={}&pageSize={page_size}&page={page}",
        query.replace(' ', "%20")
    )
}

async fn search_page(router: axum::Router, query: &str, page_size: usize, page: usize) -> Value {
    let path = query_path(query, page_size, page);
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

fn served(data: &Value) -> Vec<(String, f64)> {
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

// ---------------------------------------------------------------------------
// `shard_prefix`
// ---------------------------------------------------------------------------

#[test]
fn test_shard_prefix_bounds_the_limit() {
    // The first case is the `LIMIT -1` guard: `usize::MAX - 1 + 20` saturates,
    // and an unclamped bind reaches SQLite as `-1` = no limit.
    assert_eq!(shard_prefix(usize::MAX - 1, 20, 3), 3);
    assert_eq!(shard_prefix(39, 20, 3), 3);
    assert_eq!(shard_prefix(0, 20, 280), 20);
    assert_eq!(shard_prefix(0, 200, 0), 0);
    assert_eq!(
        shard_prefix(usize::MAX - 1, 20, MAX_SHARD_PREFIX),
        MAX_SHARD_PREFIX
    );
    // The count-failure deep-offset pin: the cap still covers offset+page.
    assert_eq!(shard_prefix(180, 20, MAX_SHARD_PREFIX), 200);
}

// ---------------------------------------------------------------------------
// Exact paging over the real store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_paging_walk_visits_every_memory_exactly_once() {
    let store = real_store().await;
    let page_size = 20;

    let first = search_page(store.router(), QUERY_THREE, page_size, 1).await;
    assert_eq!(
        first["total"].as_u64().expect("total") as usize,
        TOTAL_ROWS,
        "total is the exact row count in scope"
    );
    let total_pages = first["totalPages"].as_u64().expect("totalPages") as usize;
    assert_eq!(total_pages, TOTAL_ROWS.div_ceil(page_size));

    let mut walked: Vec<(usize, String)> = Vec::new();
    for page in 1..=total_pages {
        let data = search_page(store.router(), QUERY_THREE, page_size, page).await;
        assert_eq!(data["total"].as_u64().expect("total") as usize, TOTAL_ROWS);
        walked.extend(served(&data).into_iter().map(|(id, _)| (page, id)));
    }
    assert_eq!(walked.len(), TOTAL_ROWS, "the walk visits every row once");
    let mut pages_by_id: std::collections::BTreeMap<&str, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (page, id) in &walked {
        pages_by_id.entry(id.as_str()).or_default().push(*page);
    }
    let repeated: Vec<(&str, &Vec<usize>)> = pages_by_id
        .iter()
        .filter(|(_, pages)| pages.len() > 1)
        .map(|(id, pages)| (*id, pages))
        .collect();
    assert!(
        repeated.is_empty(),
        "no id repeats across pages; repeated: {repeated:?}"
    );
    let unique = pages_by_id.len();
    assert_eq!(unique, TOTAL_ROWS, "the walk covers every id");

    // The walk must equal one wide walk's ranking exactly, not merely contain
    // the same ids — a per-shard `page_size` limit would duplicate and short.
    let mut wide: Vec<String> = Vec::new();
    for page in 1..=TOTAL_ROWS.div_ceil(200) {
        let data = search_page(store.router(), QUERY_THREE, 200, page).await;
        wide.extend(served(&data).into_iter().map(|(id, _)| id));
    }
    let walked_ids: Vec<String> = walked.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(walked_ids, wide, "paged walk == one wide walk, in order");
}

#[tokio::test]
async fn test_search_page_past_the_end_returns_an_empty_page() {
    let store = real_store().await;
    let data = search_page(store.router(), QUERY_THREE, 20, 1_000_000).await;
    assert_eq!(
        data["items"].as_array().expect("items").len(),
        0,
        "a page past the end serves nothing"
    );
    assert_eq!(
        data["total"].as_u64().expect("total") as usize,
        TOTAL_ROWS,
        "the total stays exact"
    );
}

// ---------------------------------------------------------------------------
// The count-failure arm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_count_failure_does_not_truncate_the_page() {
    // (a) The arithmetic pin on the helper (secondary, never the oracle: it
    // supplies the constant itself). The healthy 500-row shard needs 200 at
    // `offset = 180`; the failure path's third argument *is* the fallback, so
    // its pair is `MAX_SHARD_PREFIX -> 200` vs `page_size -> 20`.
    assert_eq!(shard_prefix(180, 20, 500), 200);
    assert_eq!(shard_prefix(180, 20, MAX_SHARD_PREFIX), 200);
    assert_eq!(shard_prefix(180, 20, 20), 20);
    assert_eq!(
        shard_prefix(usize::MAX - 1, 20, MAX_SHARD_PREFIX),
        MAX_SHARD_PREFIX
    );

    // (b) The handler: a failing count answers 200 with the same items and an
    // undercounted total. The fake seeds exactly two memories (one per shard),
    // so the delta is two; prompts are counted identically in both runs.
    let (backend, services) = fake_services();
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        common::fakes::USER_ID,
        "127.0.0.1",
    );
    let router = build_router(Arc::new(state));
    let path = query_path("probe", 20, 1);

    let healthy_response = api_request(router.clone(), "GET", &path, Some(API_TOKEN), None).await;
    assert_eq!(healthy_response.status, 200);
    let healthy: Value =
        serde_json::from_str(&healthy_response.body_string()).expect("healthy JSON");
    let healthy_items = healthy["data"]["items"].clone();
    let healthy_total = healthy["data"]["total"].as_u64().expect("total");

    backend.set_count_in_shard_failure(true);
    let failing_response = api_request(router, "GET", &path, Some(API_TOKEN), None).await;
    assert_eq!(failing_response.status, 200, "a broken count is not a 503");
    let failing: Value = serde_json::from_str(&failing_response.body_string()).expect("JSON");
    assert_eq!(
        failing["data"]["items"], healthy_items,
        "the page is not shortened by a failed count"
    );
    let failing_total = failing["data"]["total"].as_u64().expect("total");
    assert_eq!(
        healthy_total - failing_total,
        2,
        "each failed shard count undercounts by that shard's rows"
    );

    // (c) The arm the constant exists for: at a deep offset the two fallbacks
    // diverge. 500 rows, page 10 at pageSize 20 -> offset 180; a failed count
    // with `MAX_SHARD_PREFIX` still asks the shard for 200 rows, so the page is
    // full; with the round-2 `page_size` fallback it asks for 20 and serves
    // nothing. A fresh backend, so arm (b)'s two-row seed cannot mask it.
    // At page 1 (offset 0) `min(20, 50_000) == min(20, 20)`, so this offset is
    // the only input that can distinguish the two branches.
    let (deep_backend, deep_services) = fake_services();
    let deep_shard = deep_backend.project_shard();
    for index in 0..500i64 {
        deep_backend.seed_memory(
            &deep_shard,
            MemoryRow {
                id: format!("probe_{index:03}"),
                content: format!("probe content {index:03}"),
                container_tag: common::fakes::PROJECT_TAG.to_string(),
                tags: None,
                memory_type: Some("note".to_string()),
                created_at: 1_700_000_000_000 + index,
                updated_at: Some(1_700_000_000_000 + index),
                metadata: None,
                session_id: None,
                display_name: None,
                user_name: None,
                user_email: None,
                project_path: None,
                project_name: None,
                git_repo_url: None,
                is_pinned: false,
                has_tags_vector: false,
            },
        );
    }
    let deep_state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        deep_services,
        common::fakes::USER_ID,
        "127.0.0.1",
    );
    let deep_router = build_router(Arc::new(deep_state));

    // Both call sites: the global branch (`memories.rs:997`) and the tagged
    // branch (`:1063`).
    let deep_paths = [
        (
            "global",
            "/api/search?q=probe&pageSize=20&page=10".to_string(),
        ),
        (
            "tagged",
            format!(
                "/api/search?q=probe&tag={}&pageSize=20&page=10",
                common::fakes::PROJECT_TAG
            ),
        ),
    ];
    for (branch, path) in &deep_paths {
        let healthy = api_request(deep_router.clone(), "GET", path, Some(API_TOKEN), None).await;
        assert_eq!(healthy.status, 200, "{branch}: healthy deep page");
        let healthy: Value = serde_json::from_str(&healthy.body_string()).expect("JSON");
        let healthy_items = healthy["data"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        deep_backend.set_count_in_shard_failure(true);
        let failing = api_request(deep_router.clone(), "GET", path, Some(API_TOKEN), None).await;
        assert_eq!(failing.status, 200, "{branch}: a broken count is not a 503");
        let failing: Value = serde_json::from_str(&failing.body_string()).expect("JSON");
        let failing_items = failing["data"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        deep_backend.set_count_in_shard_failure(false);

        // Non-vacuity guard: a fixture that serves nothing in both worlds would
        // pass the equality below without exercising the fallback at all.
        assert_eq!(
            healthy_items.len(),
            20,
            "{branch}: the healthy deep page must be full (offset 180 of 500 rows)"
        );
        assert_eq!(
            failing_items,
            healthy_items,
            "{branch}: a failed count must not shorten the deep page ({} vs {} items)",
            failing_items.len(),
            healthy_items.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Step 9 — the served top 20 against a full-scan reference
// ---------------------------------------------------------------------------

struct RefRow {
    id: String,
    content: String,
    tags: Option<String>,
    created_at: i64,
    vector: Vec<f32>,
}

async fn read_shard_rows(manager: &ShardManager, shard: &ShardInfo) -> Vec<RefRow> {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    let rows = db
        .query(
            "SELECT id, content, tags, created_at, vector FROM memories",
            Vec::new(),
        )
        .await
        .expect("read the shard rows");
    rows.iter()
        .map(|row| RefRow {
            id: row.text("id").expect("id"),
            content: row.text("content").unwrap_or_default(),
            tags: row.text("tags"),
            created_at: row.i64("created_at").expect("created_at"),
            vector: row
                .get("vector")
                .and_then(blob_to_float32_array)
                .expect("decode the vector"),
        })
        .collect()
}

fn cosine(query: &[f32], vector: &[f32]) -> f64 {
    let dot: f64 = query
        .iter()
        .zip(vector)
        .map(|(q, v)| f64::from(*q) * f64::from(*v))
        .sum();
    let query_norm: f64 = query.iter().map(|q| f64::from(*q) * f64::from(*q)).sum();
    let vector_norm: f64 = vector.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    distance_to_similarity(1.0 - dot / (query_norm.sqrt() * vector_norm.sqrt()))
}

fn tokens(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '.' | '_' | '/' | '-'))
        .map(str::trim)
        .filter(|token| token.chars().count() > 1)
        .map(str::to_string)
        .collect()
}

/// The blend, written independently of `search_in_shard`.
fn reference_sim(row: &RefRow, query: &[f32], words: &[String]) -> f64 {
    let content_sim = cosine(query, &row.vector);
    let tags_sim: f64 = 0.0;
    let elements: Vec<String> = row
        .tags
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    let matches = words
        .iter()
        .filter(|word| {
            elements
                .iter()
                .any(|tag| tag.contains(word.as_str()) || word.contains(tag.as_str()))
        })
        .count();
    let boost = matches as f64 / words.len() as f64;
    let haystack = row.tags.as_deref().unwrap_or("").to_lowercase();
    let hits = words
        .iter()
        .filter(|word| {
            row.content.to_lowercase().contains(word.as_str()) || haystack.contains(word.as_str())
        })
        .count();
    let keyword = hits as f64 * (1.0 / words.len() as f64);
    content_sim * 0.5 + tags_sim.max(boost) * 0.3 + keyword * 0.2
}

#[tokio::test]
async fn test_search_top20_equals_the_full_scan_reference_for_three_queries() {
    let store = real_store().await;
    let shards = store
        .manager
        .get_all_shards(Scope::Project, Some(PROJECT_HASH_A))
        .await
        .expect("list shard A");
    let shards_b = store
        .manager
        .get_all_shards(Scope::Project, Some(PROJECT_HASH_B))
        .await
        .expect("list shard B");
    let mut rows: Vec<RefRow> = Vec::new();
    for shard in shards.iter().chain(shards_b.iter()) {
        rows.extend(read_shard_rows(&store.manager, shard).await);
    }
    assert_eq!(rows.len(), TOTAL_ROWS, "the reference reads every row");

    let query_vector = unit_query();
    for query in [QUERY_ONE, QUERY_TWO, QUERY_THREE] {
        let words = tokens(query);
        assert!(!words.is_empty());
        let mut ranked: Vec<(String, f64, i64)> = rows
            .iter()
            .map(|row| {
                (
                    row.id.clone(),
                    reference_sim(row, &query_vector, &words),
                    row.created_at,
                )
            })
            .collect();
        // The served order: similarity desc, `createdAt` desc, `id` asc (the
        // shard SQL's own tie-break, preserved by the stable sort).
        ranked.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .expect("finite")
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| left.0.cmp(&right.0))
        });

        // The tolerance can never reorder the served window: no adjacent
        // non-tied pair inside the top 20 sits within it (10× margin; the
        // spacing is itself an f32 computation, so an exact 1e-3 boundary is
        // not stable). Deep in the ranking, two unrelated rows may legitimately
        // land a hair apart — that is a fixture coincidence, not an ordering
        // hazard, because those rows are never both in one page.
        let head: Vec<(String, f64)> = ranked
            .iter()
            .take(12)
            .map(|(id, sim, _)| (id.clone(), *sim))
            .collect();
        for pair in ranked.iter().take(21).collect::<Vec<_>>().windows(2) {
            let delta = (pair[0].1 - pair[1].1).abs();
            assert!(
                delta == 0.0 || delta > TOLERANCE * 10.0,
                "query {query}: {} ({}) vs {} ({}) differ by {delta}, inside the tolerance; head: {head:?}",
                pair[0].0,
                pair[0].1,
                pair[1].0,
                pair[1].1
            );
        }

        let data = search_page(store.router(), query, 20, 1).await;
        let served = served(&data);
        assert_eq!(served.len(), 20, "query {query}: page 1 is full");
        assert_eq!(
            served.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            ranked
                .iter()
                .take(20)
                .map(|(id, _, _)| id.clone())
                .collect::<Vec<_>>(),
            "query {query}: ids in order"
        );
        for ((id, sim), (expected_id, expected_sim, _)) in served.iter().zip(&ranked) {
            assert_eq!(id, expected_id);
            assert!(
                (sim - expected_sim).abs() < TOLERANCE,
                "query {query}, {id}: served {sim} vs reference {expected_sim}"
            );
        }
    }

    // The chosen query's premise, asserted: the deep row is outside the old
    // union's content top-40 and outside its keyword channel, yet rank 1.
    let words = tokens(QUERY_THREE);
    let mut by_content: Vec<(String, f64)> = rows
        .iter()
        .map(|row| (row.id.clone(), cosine(&query_vector, &row.vector)))
        .collect();
    by_content.sort_by(|left, right| right.1.partial_cmp(&left.1).expect("finite"));
    let deep_content_rank = by_content
        .iter()
        .position(|(id, _)| id == "mem_deep")
        .expect("mem_deep in the shard")
        + 1;
    assert!(
        deep_content_rank > 40,
        "mem_deep must be outside the union's content top-40, rank was {deep_content_rank}"
    );
    let deep = rows.iter().find(|row| row.id == "mem_deep").expect("row");
    assert!(
        words
            .iter()
            .all(|word| !deep.content.contains(word.as_str())),
        "mem_deep must be outside the union's keyword channel"
    );
    let deep_sim = reference_sim(deep, &query_vector, &words);
    assert!(
        by_content
            .iter()
            .filter(|(id, _)| id != "mem_deep")
            .all(|(id, _)| {
                let row = rows.iter().find(|row| &row.id == id).expect("row");
                reference_sim(row, &query_vector, &words) < deep_sim
            }),
        "mem_deep must be rank 1 by the exact blend"
    );
    let data = search_page(store.router(), QUERY_THREE, 20, 1).await;
    assert_eq!(
        served(&data).first().map(|(id, _)| id.as_str()),
        Some("mem_deep")
    );
}
