//! `GET /api/memories?tags=` — the A1 free-text memory-tag filter.
//!
//! The UI's own request line is a shared fixture file: the wasm suite asserts
//! the UI builds it, and this suite feeds the same bytes to the router.

mod common;

use std::sync::Arc;

use common::fakes::{FakeBackend, USER_ID, fake_services, memory_row};
use common::{api_request, app_state_with_api};
use mem_web::api::memories::{normalize_tag, tags_match};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

const API_TOKEN: &str = "test-token";
const FIXTURE: &str = include_str!("fixtures/a1_tags_request.txt");

fn router() -> (Arc<FakeBackend>, axum::Router) {
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

async fn get(router: axum::Router, path: &str) -> (u16, Value) {
    let response = api_request(router, "GET", path, Some(API_TOKEN), None).await;
    let body: Value = serde_json::from_str(&response.body_string()).unwrap_or(Value::Null);
    (response.status, body)
}

fn total(body: &Value) -> u64 {
    body["data"]["total"].as_u64().unwrap_or(u64::MAX)
}

fn seed_tagged(backend: &FakeBackend, id: &str, tags: &str) {
    let mut row = memory_row(id, common::fakes::PROJECT_TAG);
    row.tags = Some(tags.to_string());
    backend.seed_memory(&backend.project_shard(), row);
}

#[tokio::test]
async fn test_a1_fixture_answers_200_with_the_filtered_row() {
    let (_backend, router) = router();
    let (status, body) = get(router, FIXTURE.trim()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(total(&body), 1, "the predicate must be applied: {body}");
    assert_eq!(
        body["data"]["items"][0]["tags"],
        serde_json::json!(["probe"])
    );
}

#[tokio::test]
async fn test_tags_filter_never_rejects() {
    let (backend, router_a) = router();
    seed_tagged(&backend, "rust-row", "rust");
    let base = "/api/memories?page=1&pageSize=20&includePrompts=false";
    let (status, body) = get(router_a.clone(), &format!("{base}&tags=")).await;
    assert_eq!((status, total(&body)), (200, 3));
    let many: String = (0..33).map(|i| format!("&tags=v{i}")).collect();
    let (status, _) = get(router_a.clone(), &format!("{base}{many}")).await;
    assert_eq!(status, 200);
    let long = "x".repeat(129);
    let (status, _) = get(router_a.clone(), &format!("{base}&tags={long}")).await;
    assert_eq!(status, 200);
    let (status, body) = get(router_a.clone(), &format!("{base}&tags=Rust%20")).await;
    assert_eq!((status, total(&body)), (200, 1));
    let (status, _) = get(router_a, &format!("{base}&tags=ops-drill")).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_tags_filter_caps_at_32_and_128() {
    let (_backend, router) = router();
    let base = "/api/memories?page=1&pageSize=20&includePrompts=false";
    // The only matching value is the 33rd, so the cap drops it.
    let mut many: String = (0..32).map(|i| format!("&tags=v{i}")).collect();
    many.push_str("&tags=probe");
    let (_, body) = get(router.clone(), &format!("{base}{many}")).await;
    assert_eq!(total(&body), 0);
    // A 129-char value is dropped, leaving the listing unfiltered.
    let long = "x".repeat(129);
    let (_, body) = get(router, &format!("{base}&tags={long}")).await;
    assert_eq!(total(&body), 2);
}

#[tokio::test]
async fn test_tags_filter_omits_prompts() {
    let (_backend, router) = router();
    let (_, body) = get(
        router,
        "/api/memories?page=1&pageSize=20&tags=probe&includePrompts=true",
    )
    .await;
    assert_eq!(total(&body), 1);
}

#[tokio::test]
async fn test_tag_container_param_still_rejects_malformed() {
    let (_backend, router) = router();
    let (status, _) = get(router, "/api/memories?tag=not-a-scope-tag").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn test_tags_composes_with_the_container_tag() {
    let (backend, router) = router();
    seed_tagged(&backend, "rust-row", "rust");
    let path = format!(
        "/api/memories?tag={}&tags=rust&includePrompts=false",
        common::fakes::PROJECT_TAG
    );
    let (status, body) = get(router, &path).await;
    assert_eq!((status, total(&body)), (200, 1), "{body}");
}

#[test]
fn test_tags_match_whole_tag_case_and_trim() {
    let w = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(!tags_match(Some("rust,architecture"), &w(&["rus"])));
    assert!(tags_match(Some("rust,architecture"), &w(&["Rust "])));
    assert!(tags_match(Some(" Rust , x"), &w(&["rust"])));
    assert!(!tags_match(None, &w(&["rust"])));
    assert!(!tags_match(Some(""), &w(&["rust"])));
    assert!(tags_match(Some("rust"), &[]));
    assert!(tags_match(Some("rust"), &w(&["  "])));
    assert_eq!(normalize_tag("  MiXed "), "mixed");
}

#[tokio::test]
async fn test_tags_match_a_space_bearing_tag() {
    let (backend, router) = router();
    seed_tagged(&backend, "space-row", "Architecture Docs, ops ");
    let base = "/api/memories?page=1&pageSize=20&includePrompts=false";
    let (_, body) = get(router.clone(), &format!("{base}&tags=ops")).await;
    assert_eq!(total(&body), 1);
    let (_, body) = get(router, &format!("{base}&tags=Architecture%20Docs")).await;
    assert_eq!(total(&body), 1);
}
