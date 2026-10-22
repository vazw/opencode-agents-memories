//! The profile endpoints strip per-item embeddings — upstream
//! `api-handlers-profile-strip.test.ts`.
//!
//! The profile payload that leaves the server goes through
//! `mem_profile::profile_utils::to_public_profile_data`: no `centroid`, no
//! `anchor`, anywhere in the serialized body. The display order is upstream's
//! `sortProfileItems` (preferences by `confidence`, patterns and workflows by
//! `frequency`, frozen items last).

mod common;

use std::sync::Arc;

use common::fakes::{FakeBackend, USER_ID, fake_services};
use common::{RawResponse, api_request, app_state_with_api};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::{Value, json};

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// A router over the canonical fake fixture.
fn router() -> (Arc<FakeBackend>, axum::Router) {
    let (backend, services) = fake_services();
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        services,
        USER_ID,
        "127.0.0.1",
    );
    (backend, build_router(Arc::new(state)))
}

/// `GET` with the token.
async fn get(router: axum::Router, path: &str) -> RawResponse {
    api_request(router, "GET", path, Some(API_TOKEN), None).await
}

/// The `data` object of a successful envelope.
fn data(response: &RawResponse) -> Value {
    let body: Value = serde_json::from_str(&response.body_string())
        .unwrap_or_else(|error| panic!("the body is JSON: {error}: {}", response.body_string()));
    assert_eq!(
        body.get("success"),
        Some(&json!(true)),
        "the request succeeded: {}",
        response.body_string()
    );
    body.get("data")
        .cloned()
        .unwrap_or_else(|| panic!("the envelope carries data: {}", response.body_string()))
}

/// Asserts that no section item carries an embedding key, and that the
/// serialized body never contains either key name.
fn assert_no_embeddings(response: &RawResponse, profile_data: &Value) {
    for section in ["preferences", "patterns", "workflows"] {
        for item in profile_data
            .get(section)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            assert!(
                item.get("centroid").is_none() && item.get("anchor").is_none(),
                "an embedding leaked in {section}: {item}"
            );
        }
    }
    let body = response.body_string();
    assert!(
        !body.contains(r#""centroid""#) && !body.contains(r#""anchor""#),
        "the serialized body carries an embedding key: {body}"
    );
}

#[tokio::test]
async fn test_get_user_profile_omits_centroid_and_anchor() {
    let (_, router) = router();
    let response = get(router, "/api/user-profile").await;

    assert_eq!(response.status, 200, "{}", response.body_string());
    let data = data(&response);
    let profile_data = data
        .get("profileData")
        .unwrap_or_else(|| panic!("the profile carries profileData: {data}"));
    assert_no_embeddings(&response, profile_data);
    assert_eq!(
        profile_data
            .get("preferences")
            .and_then(|items| items.get(0)),
        Some(&json!({
            "id": "pref_0",
            "category": "tools",
            "description": "prefer Rust",
            "confidence": 0.9
        })),
        "the surviving fields keep their values and order"
    );
    assert_eq!(data.get("userId"), Some(&json!(USER_ID)));
    assert_eq!(data.get("exists"), Some(&json!(true)));
}

#[tokio::test]
async fn test_get_profile_snapshot_omits_centroid_and_anchor() {
    let (_, router) = router();
    let response = get(router, "/api/user-profile/snapshot?chlogId=probe").await;

    assert_eq!(response.status, 200, "{}", response.body_string());
    let data = data(&response);
    assert_eq!(data.get("version"), Some(&json!(2)));
    let profile_data = data
        .get("profileData")
        .unwrap_or_else(|| panic!("the snapshot carries profileData: {data}"));
    assert_no_embeddings(&response, profile_data);
}

#[tokio::test]
async fn test_user_profile_response_sorts_preferences_patterns_and_workflows() {
    // Frozen items last, then the section's metric descending.
    let profile_data = json!({
        "preferences": [
            {"id": "pref_0", "description": "low", "confidence": 0.2},
            {"id": "pref_1", "description": "frozen", "confidence": 0.95, "driftBelowCount": 2},
            {"id": "pref_2", "description": "high", "confidence": 0.9}
        ],
        "patterns": [
            {"id": "pat_0", "description": "rare", "frequency": 2},
            {"id": "pat_1", "description": "often", "frequency": 7}
        ],
        "workflows": [
            {"id": "wf_0", "description": "odd", "frequency": 5},
            {"id": "wf_1", "description": "first", "frequency": 1}
        ]
    });

    let (backend, router) = router();
    backend.set_profile_data(profile_data);
    let response = get(router, "/api/user-profile").await;

    let data = data(&response);
    let profile_data = data
        .get("profileData")
        .unwrap_or_else(|| panic!("the profile carries profileData: {data}"));
    let descriptions = |section: &str| -> Vec<String> {
        profile_data
            .get(section)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    assert_eq!(descriptions("preferences"), ["pref_2", "pref_0", "pref_1"]);
    assert_eq!(descriptions("patterns"), ["pat_1", "pat_0"]);
    assert_eq!(descriptions("workflows"), ["wf_0", "wf_1"]);
}
