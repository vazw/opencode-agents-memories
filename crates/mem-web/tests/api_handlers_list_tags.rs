//! `GET /api/tags` — upstream `api-handlers-list-tags.test.ts`.
//!
//! The handler walks every project **and** user shard, keeps only the tags
//! carrying the canonical scope marker and de-duplicates across shards
//! (first shard wins).

mod common;

use std::sync::Arc;

use common::fakes::{PROJECT_HASH, PROJECT_TAG, USER_ID, USER_TAG, fake_services, memory_row};
use common::{api_request, app_state_with_api};
use mem_core::scope::Scope;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// A router over the canonical fake fixture.
fn router() -> (Arc<common::fakes::FakeBackend>, axum::Router) {
    let (backend, services) = fake_services();
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        USER_ID,
        "127.0.0.1",
    );
    (backend, build_router(Arc::new(state)))
}

/// `GET /api/tags` and its parsed `data` object.
async fn list_tags(router: axum::Router) -> Value {
    let response = api_request(router, "GET", "/api/tags", Some(API_TOKEN), None).await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body: Value = serde_json::from_str(&response.body_string())
        .unwrap_or_else(|_| panic!("the tags answer is JSON: {}", response.body_string()));
    assert_eq!(body["success"], Value::Bool(true));
    body["data"].clone()
}

/// The tags of one scope, sorted (upstream's Map preserves insertion order,
/// which is shard order; the assertions here are order-independent).
fn tags_of(data: &Value, scope: &str) -> Vec<String> {
    let mut tags: Vec<String> = data[scope]
        .as_array()
        .unwrap_or_else(|| panic!("the {scope} list is an array: {data}"))
        .iter()
        .map(|entry| entry["tag"].as_str().unwrap_or_default().to_string())
        .collect();
    tags.sort();
    tags
}

#[tokio::test]
async fn test_list_tags_returns_user_scope_tags_alongside_project_tags() {
    let (_, router) = router();
    let data = list_tags(router).await;

    assert_eq!(tags_of(&data, "project"), vec![PROJECT_TAG.to_string()]);
    assert_eq!(tags_of(&data, "user"), vec![USER_TAG.to_string()]);

    let project = &data["project"][0];
    assert_eq!(project["displayName"], "demo-app");
    assert_eq!(project["projectName"], "demo-app");
    assert_eq!(project["projectPath"], "/tmp/probe");

    let user = &data["user"][0];
    assert_eq!(user["userName"], "Ada");
    assert_eq!(user["userEmail"], USER_ID);
}

#[tokio::test]
async fn test_list_tags_omits_a_tag_without_a_canonical_scope_marker() {
    let (backend, router) = router();
    // A tag whose prefix hides the marker (`_projectx_` is not `_project_`).
    let shard = backend.add_shard(Scope::Project, PROJECT_HASH);
    backend.seed_memory(
        &shard,
        memory_row("mem_markerless", "mem_projectx_c1c2c3d4e5f67890"),
    );

    let data = list_tags(router).await;
    assert_eq!(
        tags_of(&data, "project"),
        vec![PROJECT_TAG.to_string()],
        "the marker-less tag is omitted: {data}"
    );
}

#[tokio::test]
async fn test_list_tags_deduplicates_a_tag_present_in_two_shards() {
    let (backend, router) = router();
    // A second project shard carrying the *same* container tag.
    let shard = backend.add_shard(Scope::Project, "aaaa1111bbbb2222");
    backend.seed_memory(&shard, memory_row("mem_dup_tag", PROJECT_TAG));

    let data = list_tags(router).await;
    assert_eq!(
        tags_of(&data, "project"),
        vec![PROJECT_TAG.to_string()],
        "the tag appears once: {data}"
    );
}
