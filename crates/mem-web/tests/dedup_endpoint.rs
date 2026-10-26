//! `POST /api/deduplicate` end to end: real router, real handler, real auth
//! gate, `ProductionDeduplication`, `ShardDedupStore`, real shard files with
//! real vectors.
//!
//! The composition root wires the port (issue 54); the 501 case here
//! deliberately drives the untouched `ApiServices::unavailable()` wiring while
//! every other case injects `production_deduplication_port`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::real_store::{
    RealStore, create_shard, identical_vector, insert, new_store, row_ids, unit_x, unit_y,
    with_cos, with_cos_y,
};
use common::{api_request, app_state_with_api};
use mem_core::scope::Scope;
use mem_web::api::deduplication::{MAX_NEAR_SCAN_PAIRS, production_deduplication_port};
use mem_web::api::ports::ApiServices;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

const HASH: &str = "a1b2c3d4e5f67890";
const TAG: &str = "opencode_project_a1b2c3d4e5f67890";
const TOKEN: &str = "test-token";

/// A router whose maintenance port is the real deduplication pass.
fn router(store: &RealStore, enabled: bool) -> axum::Router {
    let services = ApiServices::unavailable().with_maintenance(production_deduplication_port(
        store.manager.clone(),
        enabled,
        0.9,
    ));
    router_with(services)
}

fn router_with(services: ApiServices) -> axum::Router {
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        "unknown",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

async fn post_deduplicate(router: axum::Router) -> common::RawResponse {
    api_request(router, "POST", "/api/deduplicate", Some(TOKEN), None).await
}

fn body_json(response: &common::RawResponse) -> Value {
    serde_json::from_str(&response.body_string())
        .unwrap_or_else(|error| panic!("the body is JSON: {error}: {}", response.body_string()))
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, got {value}"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn test_deduplicate_endpoint_answers_200_with_the_report() {
    let store = new_store("dedup-endpoint-report");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    insert(
        &store,
        &shard,
        "mem_new",
        "same content",
        TAG,
        3000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_old",
        "same content",
        TAG,
        2000,
        unit_x(),
    )
    .await;

    let response = post_deduplicate(router(&store, true)).await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let body = body_json(&response);
    assert_eq!(body["success"], Value::Bool(true), "{body}");
    assert_eq!(body["data"]["exactDuplicatesDeleted"], 1, "{body}");
    assert_eq!(
        row_ids(&store, &shard).await,
        vec!["mem_new".to_string()],
        "the older exact duplicate is gone"
    );
}

#[tokio::test]
async fn test_deduplicate_endpoint_serializes_the_typed_group_shape() {
    let store = new_store("dedup-endpoint-shape");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    insert(&store, &shard, "mem_a", "content a", TAG, 2000, unit_x()).await;
    insert(
        &store,
        &shard,
        "mem_b",
        "content b",
        TAG,
        1000,
        with_cos(0.95),
    )
    .await;

    let response = post_deduplicate(router(&store, true)).await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = body_json(&response);
    let groups = body["data"]["nearDuplicateGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("nearDuplicateGroups is an array: {body}"));
    assert_eq!(groups.len(), 1, "{body}");

    assert_eq!(
        sorted_keys(&groups[0]["representative"]),
        vec!["containerTag", "content", "createdAt", "id"],
        "the representative carries exactly the upstream keys"
    );
    assert_eq!(
        sorted_keys(&groups[0]["duplicates"][0]),
        vec!["content", "id", "similarity"],
        "a duplicate carries exactly the upstream keys"
    );
    let similarity = groups[0]["duplicates"][0]["similarity"]
        .as_f64()
        .unwrap_or_else(|| panic!("similarity is a JSON number: {body}"));
    assert!(
        (0.94..0.96).contains(&similarity),
        "the similarity is in the band: {similarity}"
    );

    let raw = response.body_string();
    for forbidden in [
        "\"vector\"",
        "\"shard\"",
        "\"shardId\"",
        "\"dbPath\"",
        "\"db_path\"",
        "\"isPinned\"",
        "\"is_pinned\"",
        "unused-dist",
    ] {
        assert!(
            !raw.contains(forbidden),
            "the report leaks {forbidden}: {raw}"
        );
    }
}

#[tokio::test]
async fn test_deduplicate_endpoint_answers_200_business_error_when_disabled() {
    let store = new_store("dedup-endpoint-disabled");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    insert(
        &store,
        &shard,
        "mem_new",
        "same content",
        TAG,
        3000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_old",
        "same content",
        TAG,
        2000,
        unit_x(),
    )
    .await;

    let response = post_deduplicate(router(&store, false)).await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Deduplication is disabled in config"}"#
    );
    assert_eq!(
        row_ids(&store, &shard).await.len(),
        2,
        "a disabled pass deletes nothing"
    );
}

#[tokio::test]
async fn test_deduplicate_endpoint_answers_501_when_the_port_is_unavailable() {
    let response = post_deduplicate(router_with(ApiServices::unavailable())).await;
    assert_eq!(response.status, 501, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"deduplicate is not available in this build"}"#
    );
}

#[tokio::test]
async fn test_deduplicate_endpoint_is_deterministic_across_runs() {
    async fn seeded(label: &str, reversed: bool) -> (RealStore, Vec<String>) {
        let store = new_store(label);
        let shard = create_shard(&store, Scope::Project, HASH, 0).await;
        let mut rows = vec![
            ("mem_a", "content a", unit_x()),
            ("mem_b", "content b", unit_y()),
            ("mem_c", "content c", with_cos_y(0.95)),
            ("mem_d", "content d", with_cos(0.95)),
        ];
        if reversed {
            rows.reverse();
        }
        for (id, content, embedding) in rows {
            insert(&store, &shard, id, content, TAG, 1000, embedding).await;
        }
        let ids = row_ids(&store, &shard).await;
        (store, ids)
    }

    let (first, mut first_rows) = seeded("dedup-endpoint-determinism-a", false).await;
    let (second, mut second_rows) = seeded("dedup-endpoint-determinism-b", true).await;
    first_rows.sort();
    second_rows.sort();
    assert_eq!(
        first_rows, second_rows,
        "both stores hold the same four rows before the run"
    );

    let first_body = post_deduplicate(router(&first, true)).await;
    let second_body = post_deduplicate(router(&second, true)).await;
    assert_eq!(first_body.status, 200, "{}", first_body.body_string());
    assert_eq!(second_body.status, 200, "{}", second_body.body_string());
    assert_eq!(
        first_body.body_string(),
        second_body.body_string(),
        "equal created_at + reversed insertion order must serialize identically"
    );
}

#[tokio::test]
async fn test_deduplicate_reports_the_scan_accounting_beside_the_unchanged_fields() {
    let store = new_store("dedup-endpoint-scan-accounting");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    // One exact duplicate pair (newest wins) plus one distinct row at cosine
    // 0.95 — so the pass deletes once and the two survivors are exactly one
    // comparison, reported as one near-duplicate group.
    insert(
        &store,
        &shard,
        "mem_new",
        "same content",
        TAG,
        3000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_old",
        "same content",
        TAG,
        2000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_near",
        "near content",
        TAG,
        1000,
        with_cos(0.95),
    )
    .await;

    let response = post_deduplicate(router(&store, true)).await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = body_json(&response);

    // The new key is additive and always present.
    assert_eq!(
        sorted_keys(&body["data"]),
        vec![
            "exactDuplicatesDeleted".to_string(),
            "nearDuplicateGroups".to_string(),
            "nearDuplicateScan".to_string(),
        ],
        "the data object carries exactly the legacy keys plus the scan accounting: {body}"
    );
    assert_eq!(
        sorted_keys(&body["data"]["nearDuplicateScan"]),
        vec![
            "pairBudget".to_string(),
            "pairsScanned".to_string(),
            "truncated".to_string(),
        ],
        "the accounting key's shape: {body}"
    );

    // ... and every legacy key keeps its name, value and shape.
    assert_eq!(body["success"], Value::Bool(true), "{body}");
    assert_eq!(body["data"]["exactDuplicatesDeleted"], 1, "{body}");
    let groups = body["data"]["nearDuplicateGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("nearDuplicateGroups is an array: {body}"));
    assert_eq!(groups.len(), 1, "{body}");
    assert_eq!(
        sorted_keys(&groups[0]["representative"]),
        vec!["containerTag", "content", "createdAt", "id"],
        "the representative keeps the upstream keys: {body}"
    );
    assert_eq!(
        sorted_keys(&groups[0]["duplicates"][0]),
        vec![
            "content".to_string(),
            "id".to_string(),
            "similarity".to_string()
        ],
        "a duplicate keeps the upstream keys: {body}"
    );
    assert_eq!(groups[0]["representative"]["id"], "mem_new", "{body}");
    assert_eq!(groups[0]["duplicates"][0]["id"], "mem_near", "{body}");

    // The accounting tells the truth about this fixture.
    assert_eq!(
        body["data"]["nearDuplicateScan"]["pairsScanned"],
        serde_json::json!(1),
        "two survivors are exactly one ordered comparison: {body}"
    );
    assert_eq!(
        body["data"]["nearDuplicateScan"]["pairBudget"],
        serde_json::json!(MAX_NEAR_SCAN_PAIRS),
        "the report names the pass's global budget: {body}"
    );
    assert_eq!(
        body["data"]["nearDuplicateScan"]["truncated"],
        Value::Bool(false),
        "a single shard well under the budget never truncates: {body}"
    );
    let mut remaining = row_ids(&store, &shard).await;
    remaining.sort();
    assert_eq!(
        remaining,
        vec!["mem_near".to_string(), "mem_new".to_string()],
        "only the exact duplicate is deleted"
    );
}

#[tokio::test]
async fn test_deduplicate_endpoint_excludes_a_similarity_of_exactly_one() {
    let store = new_store("dedup-endpoint-similarity-one");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    insert(
        &store,
        &shard,
        "mem_a",
        "content a",
        TAG,
        2000,
        identical_vector(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_b",
        "content b",
        TAG,
        1000,
        identical_vector(),
    )
    .await;

    let response = post_deduplicate(router(&store, true)).await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = body_json(&response);
    assert_eq!(
        body["data"]["nearDuplicateGroups"],
        serde_json::json!([]),
        "similarity == 1.0 never forms a near group: {body}"
    );
    assert_eq!(
        row_ids(&store, &shard).await.len(),
        2,
        "different contents are both kept"
    );
}
