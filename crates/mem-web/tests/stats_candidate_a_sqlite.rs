//! Stats Candidate A — live SQLite end-to-end parity test.
//!
//! Exercises `ProductionMemory` against a real populated shard and verifies
//! that `GET /api/stats` produces the expected accumulators (memoryCount,
//! oldestCreatedAt, newestCreatedAt, duplicateTagCount, byType, byScope) when
//! the shard walk uses the narrow projection.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::real_store::{RealStore, create_shard, fresh_manager, new_store, unit_x};
use common::{api_request, app_state_with_api, throwaway_log_dir};
use mem_core::scope::Scope;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};
use mem_web::api::ports::{ApiServices, MemoryPort, PortError, ShardRef};
use mem_web::api::store_adapter::ProductionMemory;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::{Value, json};

const API_TOKEN: &str = "test-token";
const HASH: &str = "a1b2c3d4e5f67890";

fn tag() -> String {
    format!("opencode_project_{HASH}")
}

fn router_for(manager: ShardManager) -> axum::Router {
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

async fn seeded_project_shard(label: &str) -> (RealStore, ShardInfo) {
    let store = new_store(label);
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;

    // Insert three memories with out-of-order timestamps and a duplicate tag.
    for (id, created_at, memory_type, tags) in [
        (
            "m1",
            1500i64,
            Some("note".to_string()),
            Some("shared-tag".to_string()),
        ),
        (
            "m2",
            3000i64,
            Some("note".to_string()),
            Some("shared-tag".to_string()),
        ),
        (
            "m3",
            1000i64,
            Some("fact".to_string()),
            Some("other-tag".to_string()),
        ),
    ] {
        let db = store
            .manager
            .open_shard(&shard.db_path)
            .await
            .expect("the fixture shard opens");
        vector::insert_vector(
            db.as_ref(),
            &MemoryInsert {
                id: id.to_string(),
                content: "body".to_string(),
                vector: unit_x(),
                tags_vector: None,
                container_tag: tag(),
                tags,
                memory_type,
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
    }
    let db = store
        .manager
        .open_shard(&shard.db_path)
        .await
        .expect("the fixture shard opens");
    let count = mem_store::shard_facts::sync_count(db.as_ref())
        .await
        .expect("the fixture syncs the shard count");
    store
        .manager
        .set_vector_count(shard.id, count)
        .await
        .expect("the fixture adopts the registry count");

    (store, shard)
}

#[tokio::test]
async fn test_stats_candidate_a_sqlite_e2e_parity() {
    let (store, _shard) = seeded_project_shard("stats-candidate-a-sqlite").await;
    let response = api_request(
        router_for(store.manager.clone()),
        "GET",
        "/api/stats",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body: Value = serde_json::from_slice(&response.body).expect("json");
    let data = &body["data"];

    assert_eq!(data["total"], json!(3));
    assert_eq!(data["byScope"]["project"], json!(3));
    assert_eq!(data["byScope"]["user"], json!(0));
    assert_eq!(data["byType"]["note"], json!(2));
    assert_eq!(data["byType"]["fact"], json!(1));

    let shards = data["shards"].as_array().expect("shards array");
    assert_eq!(shards.len(), 1);
    let s = &shards[0];
    assert_eq!(s["memoryCount"], json!(3));
    assert_eq!(s["oldestCreatedAt"], json!("1970-01-01T00:00:01.000Z"));
    assert_eq!(s["newestCreatedAt"], json!("1970-01-01T00:00:03.000Z"));
    assert_eq!(s["duplicateTagCount"], json!(1));
    assert_eq!(s["fileExists"], json!(true));
}

/// Issue 130 at the ADAPTER layer: `stats.rs:108`'s `db_path.exists()`
/// pre-check short-circuits before the port, so `GET /api/stats` can never
/// reach `ProductionMemory::stats_memories`' guard. Only a direct call pins it.
#[tokio::test]
async fn test_stats_memories_adapter_refuses_missing_shard_file() {
    let store = new_store("stats-candidate-a-guard");
    let config = store.config.clone();
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    let db_path = shard.db_path.clone();
    let manager = fresh_manager(&config); // no cached handle can mask the absence
    drop(store);
    std::fs::remove_file(&db_path).expect("remove the shard database");
    assert!(!db_path.exists(), "the shard file is really gone");

    let shard_ref = ShardRef {
        scope: Scope::Project,
        scope_hash: HASH.to_string(),
        shard_index: 0,
        db_path: db_path.clone(),
        shard_id: Some(shard.id),
        vector_count: 0,
        is_active: true,
    };
    let port = ProductionMemory::new(manager);
    let result = port.stats_memories(&shard_ref).await;

    assert!(
        matches!(result, Err(PortError::ShardFileMissing { .. })),
        "the adapter must refuse before `open_shard` recreates the file: {result:?}"
    );
    assert!(
        !db_path.exists(),
        "a refused read must never recreate the shard file"
    );
}
