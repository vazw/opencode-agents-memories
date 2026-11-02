//! `GET /api/projects` — the project roster (issue 116, D-6).
//!
//! The Agents screen used to require a pasted project id and had no list to
//! offer. This route answers the roster the picker renders: every project with
//! live sessions or milestones, its live-session count, its milestone count and
//! its newest activity stamp — newest activity first.
//!
//! Port-only row (ADR-0003): no upstream counterpart, so this file is the
//! contract. `project_id` is DATA: it is returned verbatim (a path-shaped id
//! stays one string) and never becomes a filesystem path here.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::fakes::{FakeBackend, fake_services};
use common::{api_request, app_state_with_api};
use mem_web::api::ports::ApiServices;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The API token the roster cases configure.
const API_TOKEN: &str = "test-token";

fn projects_router(seed: impl FnOnce(&FakeBackend)) -> axum::Router {
    let (backend, services) = fake_services();
    seed(&backend);
    build_router(Arc::new(state_with(services)))
}

fn unavailable_router() -> axum::Router {
    build_router(Arc::new(state_with(ApiServices::unavailable())))
}

fn state_with(api: ApiServices) -> mem_web::server::AppState {
    app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        api,
        common::fakes::USER_ID,
        "127.0.0.1",
    )
}

/// The `data` array of a `200` envelope.
fn data_rows(response: &common::RawResponse) -> Vec<Value> {
    let body = response.body_string();
    let parsed: Value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("body is JSON ({error}): {body}"));
    assert_eq!(parsed["success"], true, "the success envelope: {body}");
    parsed["data"]
        .as_array()
        .unwrap_or_else(|| panic!("`data` is an array: {body}"))
        .clone()
}

#[tokio::test]
async fn test_projects_endpoint_returns_the_roster() {
    let response = api_request(
        projects_router(|_| {}),
        "GET",
        "/api/projects",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let rows = data_rows(&response);
    assert_eq!(rows.len(), 2, "the seeded roster: {rows:?}");
    // Every camelCase key is present and carries the seeded value: a mapping
    // that drops `milestoneCount` (or renames it) fails here.
    let first = &rows[0];
    assert_eq!(first["projectId"], "/home/vaz/Code/demo");
    assert_eq!(first["sessionCount"], 4);
    assert_eq!(first["milestoneCount"], 2);
    assert_eq!(first["lastActivityAt"], 300);
    assert_eq!(rows[1]["projectId"], "/home/vaz/x");
    assert_eq!(rows[1]["sessionCount"], 1);
    assert_eq!(rows[1]["milestoneCount"], 0);
}

#[tokio::test]
async fn test_projects_endpoint_preserves_path_shaped_ids() {
    // `project_id` is a path-shaped string. Mutant: re-encoding it into the
    // JSON, splitting it on `/`, or turning it into a URL.
    let response = api_request(
        projects_router(|_| {}),
        "GET",
        "/api/projects",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let rows = data_rows(&response);
    let path_shaped = rows
        .iter()
        .find(|row| row["projectId"] == "/home/vaz/x")
        .unwrap_or_else(|| panic!("the path-shaped id is returned verbatim: {rows:?}"));
    assert_eq!(path_shaped["projectId"].as_str(), Some("/home/vaz/x"));
}

#[tokio::test]
async fn test_projects_endpoint_refuses_an_out_of_range_limit() {
    // The bound is a refusal, never a clamp: `?limit=501` must not answer the
    // first 500 rows. `"+5"` kills a bare `parse::<usize>()`, `" 5"`/`"5 "`
    // kill a `trim()`-based parse, and `"0"` kills a `1..` range slip.
    for raw in ["501", "0", "+5", "%205", "5.0", "-1"] {
        let response = api_request(
            projects_router(|_| {}),
            "GET",
            &format!("/api/projects?limit={raw}"),
            Some(API_TOKEN),
            None,
        )
        .await;
        assert_eq!(
            response.status,
            400,
            "?limit={raw} is refused: {}",
            response.body_string()
        );
        assert!(
            response.body_string().contains("Invalid limit parameter"),
            "?limit={raw} carries the fixed message: {}",
            response.body_string()
        );
    }

    // A value inside the bound is accepted and bounds the list.
    let response = api_request(
        projects_router(|_| {}),
        "GET",
        "/api/projects?limit=1",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(data_rows(&response).len(), 1);
}

#[tokio::test]
async fn test_projects_endpoint_answers_501_when_unavailable() {
    let response = api_request(
        unavailable_router(),
        "GET",
        "/api/projects",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        501,
        "an unwired port is the absent-service class, not a 500: {}",
        response.body_string()
    );
    assert!(
        response
            .body_string()
            .contains(mem_web::api::UNAVAILABLE_MESSAGE),
        "the documented 501 body: {}",
        response.body_string()
    );
}
