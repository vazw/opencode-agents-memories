//! Issue 72 — the REST delete fans out over every shard of the anchor's
//! `(scope, scope_hash)`.
//!
//! The router runs over the PRODUCTION adapter (`ProductionMemory`) and a real
//! turso store; the fixtures seed the same id in two shards of one scope, so
//! the old single-shard delete leaves a readable copy behind. The wire shapes
//! are frozen: `{"deletedPrompt": false}` for the single delete and
//! `{"deleted": <ids>}` for the bulk endpoint — never a copy count.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::real_store::{
    RealStore, create_shard, insert, new_store, row_ids, unit_x, vector_count,
};
use common::{api_request, app_state_with_api, throwaway_log_dir};
use mem_core::scope::Scope;
use mem_store::shard_facts;
use mem_store::shard_manager::ShardInfo;
use mem_web::api::ports::ApiServices;
use mem_web::api::store_adapter::ProductionMemory;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::{Value, json};

/// The configured API token.
const API_TOKEN: &str = "test-token";
/// The project hash every fixture addresses.
const HASH: &str = "a1b2c3d4e5f67890";
/// The container tag of shard 0's rows.
const TAG_SHARD_0: &str = "opencode_project_a1b2c3d4e5f67891";
/// The container tag of shard 1's rows.
const TAG_SHARD_1: &str = "opencode_project_a1b2c3d4e5f67892";

/// The production memory port over the fixture's manager (the other ports stay
/// unavailable: these tests never leave the delete path).
fn router(store: &RealStore) -> axum::Router {
    router_for(store.manager.clone())
}

/// The production memory port over an explicitly built manager (the
/// no-resurrect test hands in a fresh manager with no cached handles).
fn router_for(manager: mem_store::shard_manager::ShardManager) -> axum::Router {
    let services = ApiServices::unavailable().with_memory(Arc::new(ProductionMemory::new(manager)));
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        throwaway_log_dir(),
        services,
        "unknown",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

/// The committed `shard_stats` count of `shard`.
async fn stored_count(store: &RealStore, shard: &ShardInfo) -> u64 {
    let db = store
        .manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the fixture shard");
    shard_facts::read_stored_count(db.as_ref())
        .await
        .expect("stored count")
}

/// The parsed JSON body of a response.
fn json_body(response: &common::RawResponse) -> Value {
    serde_json::from_slice(&response.body).expect("the response body is JSON")
}

/// Project shards 0 and 1 both hold `dup-id`; shard 0 has one neighbour,
/// shard 1 two. The registry is planted with drift (99) on shard 0 so a delta
/// or a copied count is observable. Both copies are asserted before the test
/// body runs.
async fn two_shard_duplicate(label: &str) -> (RealStore, ShardInfo, ShardInfo) {
    let store = new_store(label);
    let shard_zero = create_shard(&store, Scope::Project, HASH, 0).await;
    let shard_one = create_shard(&store, Scope::Project, HASH, 1).await;

    insert(
        &store,
        &shard_zero,
        "dup-id",
        "duplicate copy in shard 0",
        TAG_SHARD_0,
        1000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard_zero,
        "neighbour-0",
        "neighbour in shard 0",
        TAG_SHARD_0,
        900,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard_one,
        "dup-id",
        "duplicate copy in shard 1",
        TAG_SHARD_1,
        800,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard_one,
        "neighbour-1a",
        "first neighbour in shard 1",
        TAG_SHARD_1,
        700,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard_one,
        "neighbour-1b",
        "second neighbour in shard 1",
        TAG_SHARD_1,
        600,
        unit_x(),
    )
    .await;

    assert!(
        row_ids(&store, &shard_zero)
            .await
            .contains(&"dup-id".to_string()),
        "the fixture really seeds the retired copy"
    );
    assert!(
        row_ids(&store, &shard_one)
            .await
            .contains(&"dup-id".to_string()),
        "the fixture really seeds the active copy"
    );

    store
        .manager
        .set_vector_count(shard_zero.id, 99)
        .await
        .expect("plant the registry drift");
    (store, shard_zero, shard_one)
}

/// AC1: `DELETE /api/memories/{id}` removes every copy in the scope, keeps the
/// neighbours, reconciles both counts, and the wire DTO stays frozen. A second
/// DELETE is the existing `"Memory not found"` business error.
#[tokio::test]
async fn test_delete_endpoint_removes_every_copy_in_the_scope_and_keeps_the_neighbour() {
    let (store, shard_zero, shard_one) = two_shard_duplicate("scope-delete-api").await;
    let router = router(&store);

    let response = api_request(
        router.clone(),
        "DELETE",
        "/api/memories/dup-id",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = json_body(&response);
    assert_eq!(body["success"], json!(true));
    assert_eq!(
        body["data"],
        json!({"deletedPrompt": false}),
        "the wire DTO carries the prompt flag only — never a copy count"
    );

    assert_eq!(
        row_ids(&store, &shard_zero).await,
        vec!["neighbour-0".to_string()],
        "shard 0's copy is gone, its neighbour stays"
    );
    let mut shard_one_ids = row_ids(&store, &shard_one).await;
    shard_one_ids.sort();
    assert_eq!(
        shard_one_ids,
        vec!["neighbour-1a".to_string(), "neighbour-1b".to_string()],
        "shard 1's copy is gone, both neighbours stay"
    );
    assert_eq!(stored_count(&store, &shard_zero).await, 1);
    assert_eq!(vector_count(&store, &shard_zero).await.vector_count, 1);
    assert_eq!(stored_count(&store, &shard_one).await, 2);
    assert_eq!(vector_count(&store, &shard_one).await.vector_count, 2);

    let response = api_request(
        router,
        "DELETE",
        "/api/memories/dup-id",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = json_body(&response);
    assert_eq!(body["success"], json!(false));
    assert_eq!(body["error"], json!("Memory not found"));
}

/// AC2: `POST /api/memories/bulk-delete` routes every id through the same
/// scope-wide delete; `deleted` counts ids, never copies.
#[tokio::test]
async fn test_bulk_delete_endpoint_removes_every_copy_and_counts_the_id_once() {
    let (store, shard_zero, shard_one) = two_shard_duplicate("scope-bulk-delete-api").await;
    let router = router(&store);

    let response = api_request(
        router,
        "POST",
        "/api/memories/bulk-delete",
        Some(API_TOKEN),
        Some(r#"{"ids":["dup-id"]}"#),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = json_body(&response);
    assert_eq!(body["success"], json!(true));
    assert_eq!(
        body["data"],
        json!({"deleted": 1}),
        "the bulk count is the id count, never the copy count"
    );

    assert_eq!(
        row_ids(&store, &shard_zero).await,
        vec!["neighbour-0".to_string()],
        "shard 0's copy is gone, its neighbour stays"
    );
    let mut shard_one_ids = row_ids(&store, &shard_one).await;
    shard_one_ids.sort();
    assert_eq!(
        shard_one_ids,
        vec!["neighbour-1a".to_string(), "neighbour-1b".to_string()],
        "shard 1's copy is gone, both neighbours stay"
    );
    assert_eq!(stored_count(&store, &shard_zero).await, 1);
    assert_eq!(vector_count(&store, &shard_zero).await.vector_count, 1);
    assert_eq!(stored_count(&store, &shard_one).await, 2);
    assert_eq!(vector_count(&store, &shard_one).await.vector_count, 2);
}

/// The user scope's hash (a different tenant, not a different scope of the
/// same tenant).
const USER_HASH: &str = "c1c2c3d4e5f67890";
/// The user shard's container tag.
const USER_TAG: &str = "opencode_user_c1c2c3d4e5f67890";

/// AC3: the endpoint deletes only the anchor's scope — a project-scope call
/// never deletes the user-scope copy of the same id.
#[tokio::test]
async fn test_delete_endpoint_does_not_cross_scope_boundaries() {
    let store = new_store("scope-delete-api-boundary");
    let project_shard = create_shard(&store, Scope::Project, HASH, 0).await;
    let user_shard = create_shard(&store, Scope::User, USER_HASH, 0).await;
    insert(
        &store,
        &project_shard,
        "dup-id",
        "project copy",
        TAG_SHARD_0,
        1000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &user_shard,
        "dup-id",
        "user copy",
        USER_TAG,
        900,
        unit_x(),
    )
    .await;
    assert!(
        row_ids(&store, &project_shard)
            .await
            .contains(&"dup-id".to_string()),
        "the project copy exists before the delete"
    );
    assert!(
        row_ids(&store, &user_shard)
            .await
            .contains(&"dup-id".to_string()),
        "the user copy exists before the delete"
    );

    let response = api_request(
        router(&store),
        "DELETE",
        "/api/memories/dup-id",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(json_body(&response)["success"], json!(true));

    assert_eq!(
        row_ids(&store, &project_shard).await,
        Vec::<String>::new(),
        "the project copy is gone"
    );
    assert_eq!(
        row_ids(&store, &user_shard).await,
        vec!["dup-id".to_string()],
        "the user-scope copy survives a project-scope delete"
    );
    assert_eq!(
        vector_count(&store, &user_shard).await.vector_count,
        1,
        "the user shard's registry count survives"
    );
    assert_eq!(
        stored_count(&store, &user_shard).await,
        1,
        "the user shard's committed count survives"
    );
}

/// AC4: the anchor scan must not resurrect a missing shard. The status is
/// already 500 (a schema-less file fails the lookup), so the discriminating
/// assertion is `!db_path.exists()`: pre-fix, `memory_by_id` opens shard 0 and
/// `open_database` re-creates the file.
#[tokio::test]
async fn test_delete_endpoint_refuses_a_missing_shard_without_resurrecting_it() {
    let store = new_store("scope-delete-api-no-resurrect");
    let shard_zero = create_shard(&store, Scope::Project, HASH, 0).await;
    let shard_one = create_shard(&store, Scope::Project, HASH, 1).await;
    // The id lives only in shard 1; shard 0 is the anchor walk's first shard.
    insert(
        &store,
        &shard_one,
        "dup-id",
        "copy in shard 1",
        TAG_SHARD_1,
        1000,
        unit_x(),
    )
    .await;
    let config = store.config.clone();
    let shard_zero_path = shard_zero.db_path.clone();
    let shard_one_path = shard_one.db_path.clone();
    drop(store);

    std::fs::remove_file(&shard_zero_path).expect("remove shard 0's database");
    assert!(!shard_zero_path.exists(), "shard 0 is really gone");

    let manager = common::real_store::fresh_manager(&config);
    let read_manager = manager.clone();
    let response = api_request(
        router_for(manager),
        "DELETE",
        "/api/memories/dup-id",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 500, "{}", response.body_string());
    let body = json_body(&response);
    assert_eq!(body["success"], json!(false));
    assert_eq!(
        body["error"],
        json!("Internal Server Error"),
        "the wire stays generic — no id, scope, hash or path"
    );
    assert!(
        !shard_zero_path.exists(),
        "the anchor scan must never re-create a missing shard file"
    );

    let db = read_manager
        .open_shard(&shard_one_path)
        .await
        .expect("open the surviving shard");
    assert!(
        mem_store::vector::get_memory_by_id(db.as_ref(), "dup-id")
            .await
            .expect("lookup")
            .is_some(),
        "shard 1's copy survives the refused delete"
    );
}
