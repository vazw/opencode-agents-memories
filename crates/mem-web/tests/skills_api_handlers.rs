//! The skills endpoints and the two OpenCode catalog paths — upstream
//! `skills-api-handlers.test.ts`.
//!
//! The REST endpoints answer inside the envelope; the catalog paths answer
//! **outside** it (a bare `{"skills":[…]}` object and raw markdown). The
//! container tag — the `?tag` filter **and** the body's `containerTag` — goes
//! through the same guard as every other scoped endpoint (D-16, D-18), and a
//! skill id that is empty or carries a path separator is refused with
//! `"Invalid skill id"` (D-20).

mod common;

use std::sync::Arc;

use common::fakes::{FakeBackend, NEW_SKILL_ID, PROJECT_TAG, SKILL_ID, USER_ID, fake_services};
use common::{RawResponse, api_request, app_state_with_api};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::{Value, json};

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// A second, valid project tag (the guard accepts it; the fake holds no skill
/// under it).
const OTHER_PROJECT_TAG: &str = "mem_project_abcdef1234567890";

/// A router over the canonical fake fixture.
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

/// One request with the token and an optional JSON body.
async fn request(
    router: axum::Router,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> RawResponse {
    api_request(router, method, path, Some(API_TOKEN), body).await
}

/// The parsed body of a response.
fn body(response: &RawResponse) -> Value {
    serde_json::from_str(&response.body_string())
        .unwrap_or_else(|error| panic!("the body is JSON: {error}: {}", response.body_string()))
}

/// The `data` object of a successful envelope.
fn data(response: &RawResponse) -> Value {
    let body = body(response);
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

/// The ids of a `GET /api/skills` list answer.
fn skill_ids(response: &RawResponse) -> Vec<String> {
    data(response)
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn test_skills_crud_through_the_rest_endpoints() {
    let (backend, router) = router();

    // GET /api/skills — the seeded skill.
    let listed = request(router.clone(), "GET", "/api/skills", None).await;
    assert_eq!(listed.status, 200, "{}", listed.body_string());
    assert_eq!(skill_ids(&listed), [SKILL_ID.to_string()]);
    assert_eq!(
        data(&listed).get("items").and_then(|items| items.get(0)),
        Some(&json!({
            "id": "ship-it",
            "name": "Ship It",
            "description": "Release flow",
            "content": "## Ship\n\n1. Build\n2. Tag",
            "autoinvoke": true,
            "enabled": true,
            "containerTag": PROJECT_TAG,
            "createdAt": "2023-11-14T22:13:20.000Z",
            "updatedAt": "2023-11-14T22:13:20.000Z"
        })),
        "the skill wire shape follows skillToApi"
    );

    // POST /api/skills — a fresh id (the seed keeps `ship-it` intact).
    let created = request(
        router.clone(),
        "POST",
        "/api/skills",
        Some(
            &json!({
                "id": NEW_SKILL_ID,
                "name": "Probe Skill",
                "description": "Created by the contract test",
                "content": "## Probe",
                "containerTag": PROJECT_TAG,
                "enabled": false
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(created.status, 200, "{}", created.body_string());
    assert_eq!(
        data(&created).get("id"),
        Some(&json!(NEW_SKILL_ID)),
        "the created skill is returned: {}",
        created.body_string()
    );
    assert!(
        backend
            .calls()
            .contains(&format!("create_skill:{NEW_SKILL_ID}"))
    );

    // GET /api/skills/{id}
    let fetched = request(
        router.clone(),
        "GET",
        &format!("/api/skills/{NEW_SKILL_ID}"),
        None,
    )
    .await;
    assert_eq!(fetched.status, 200, "{}", fetched.body_string());
    assert_eq!(data(&fetched).get("name"), Some(&json!("Probe Skill")));

    // PUT /api/skills/{id}
    let updated = request(
        router.clone(),
        "PUT",
        &format!("/api/skills/{NEW_SKILL_ID}"),
        Some(&json!({"name": "Probe Skill v2", "enabled": true}).to_string()),
    )
    .await;
    assert_eq!(updated.status, 200, "{}", updated.body_string());
    assert_eq!(data(&updated).get("name"), Some(&json!("Probe Skill v2")));
    assert_eq!(data(&updated).get("enabled"), Some(&json!(true)));

    // GET on an unknown id — 404 with the fixed message (D-19).
    let missing = request(router.clone(), "GET", "/api/skills/nope", None).await;
    assert_eq!(missing.status, 404, "{}", missing.body_string());
    assert_eq!(
        body(&missing),
        json!({"success": false, "error": "Skill not found"})
    );

    // DELETE /api/skills/{id}
    let deleted = request(
        router.clone(),
        "DELETE",
        &format!("/api/skills/{NEW_SKILL_ID}"),
        None,
    )
    .await;
    assert_eq!(deleted.status, 200, "{}", deleted.body_string());
    assert_eq!(data(&deleted), json!({"deleted": true}));

    // DELETE again — 404, the fixed message, no id echo.
    let deleted_again = request(
        router,
        "DELETE",
        &format!("/api/skills/{NEW_SKILL_ID}"),
        None,
    )
    .await;
    assert_eq!(deleted_again.status, 404);
    assert_eq!(
        body(&deleted_again),
        json!({"success": false, "error": "Skill not found"})
    );
}

#[tokio::test]
async fn test_skills_list_filters_by_container_tag_and_enabled_only() {
    let (_, router) = router();

    // The seeded skill lives under the project tag.
    let tagged = request(
        router.clone(),
        "GET",
        &format!("/api/skills?tag={PROJECT_TAG}"),
        None,
    )
    .await;
    assert_eq!(tagged.status, 200, "{}", tagged.body_string());
    assert_eq!(skill_ids(&tagged), [SKILL_ID.to_string()]);

    // A different valid tag matches nothing (never an unfiltered fallback).
    let other = request(
        router.clone(),
        "GET",
        &format!("/api/skills?tag={OTHER_PROJECT_TAG}"),
        None,
    )
    .await;
    assert_eq!(other.status, 200, "{}", other.body_string());
    assert!(skill_ids(&other).is_empty(), "{}", other.body_string());

    // A disabled skill is filtered by `enabledOnly=true`.
    let created = request(
        router.clone(),
        "POST",
        "/api/skills",
        Some(
            &json!({
                "id": "disabled-skill",
                "name": "Disabled Skill",
                "content": "## Disabled",
                "containerTag": PROJECT_TAG,
                "enabled": false
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(created.status, 200, "{}", created.body_string());

    let all = request(
        router.clone(),
        "GET",
        &format!("/api/skills?tag={PROJECT_TAG}"),
        None,
    )
    .await;
    assert_eq!(
        skill_ids(&all),
        ["disabled-skill".to_string(), SKILL_ID.to_string()],
        "both skills share the tag (the fake lists by id)"
    );

    let enabled_only = request(
        router.clone(),
        "GET",
        &format!("/api/skills?tag={PROJECT_TAG}&enabledOnly=true"),
        None,
    )
    .await;
    assert_eq!(
        skill_ids(&enabled_only),
        [SKILL_ID.to_string()],
        "a disabled skill is not listed: {}",
        enabled_only.body_string()
    );

    // D-16: a malformed filter is refused, never answered with an empty list.
    let (fresh, fresh_router) = self::router();
    let malformed = request(
        fresh_router,
        "GET",
        "/api/skills?tag=mem_project_../../../../Temp/pwn",
        None,
    )
    .await;
    assert_eq!(malformed.status, 400, "{}", malformed.body_string());
    assert_eq!(
        body(&malformed),
        json!({"success": false, "error": "Invalid container tag"})
    );
    assert!(
        fresh.calls().is_empty(),
        "a rejected filter never reaches the skill port: {:?}",
        fresh.calls()
    );
}

#[tokio::test]
async fn test_skill_id_with_a_path_separator_is_refused() {
    let (backend, router) = router();

    // `%2F` is the shape that reaches the handler: axum percent-decodes the
    // captured segment (D-20). A literal `/` never matches `{id}` at all.
    for (method, path) in [
        ("GET", "/api/skills/a%2Fb"),
        ("PUT", "/api/skills/a%2Fb"),
        ("DELETE", "/api/skills/a%2Fb"),
        ("GET", "/api/skills/a%5Cb"),
    ] {
        let response = request(router.clone(), method, path, Some("{}")).await;
        assert_eq!(
            response.status,
            400,
            "{method} {path} is a client error: {}",
            response.body_string()
        );
        assert_eq!(
            body(&response),
            json!({"success": false, "error": "Invalid skill id"}),
            "{method} {path}"
        );
    }

    assert!(
        backend.calls().is_empty(),
        "a refused id never reaches the skill port: {:?}",
        backend.calls()
    );
}

#[tokio::test]
async fn test_skill_body_container_tag_is_validated_by_the_guard() {
    let (backend, router) = router();

    // POST — a malformed body tag is a 400 and never reaches the port (D-18).
    let created = request(
        router.clone(),
        "POST",
        "/api/skills",
        Some(
            &json!({
                "id": "guard-probe",
                "name": "Guard Probe",
                "content": "## Guard",
                "containerTag": "mem_project_foo/bar"
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(created.status, 400, "{}", created.body_string());
    assert_eq!(
        body(&created),
        json!({"success": false, "error": "Invalid container tag"})
    );

    // PUT — the same guard on the update body.
    let updated = request(
        router.clone(),
        "PUT",
        &format!("/api/skills/{SKILL_ID}"),
        Some(&json!({"containerTag": "mem_project_../../../../Temp/pwn"}).to_string()),
    )
    .await;
    assert_eq!(updated.status, 400, "{}", updated.body_string());
    assert_eq!(
        body(&updated),
        json!({"success": false, "error": "Invalid container tag"})
    );

    assert!(
        backend.calls().is_empty(),
        "no rejected body tag reached the skill port: {:?}",
        backend.calls()
    );
}

#[tokio::test]
async fn test_skills_catalog_index_answers_the_bare_catalog_object() {
    let (_, router) = router();
    let response = request(router, "GET", "/skills/index.json", None).await;

    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(
        response.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(
        body(&response),
        json!({"skills": [{"name": "ship-it", "version": "1.0.0", "files": ["ship-it.md"]}]}),
        "the catalog is NOT wrapped in the envelope"
    );
}

#[tokio::test]
async fn test_skills_catalog_file_answers_raw_markdown_or_a_plain_404() {
    let (_, router) = router();

    let markdown = request(router.clone(), "GET", "/skills/ship-it/ship-it.md", None).await;
    assert_eq!(markdown.status, 200, "{}", markdown.body_string());
    assert_eq!(
        markdown.header("content-type"),
        Some("text/markdown; charset=utf-8")
    );
    assert_eq!(
        markdown.body_string(),
        "# Ship It\n\n## Ship\n\n1. Build\n2. Tag"
    );

    // The file name must be `{id}.md`.
    let wrong_file = request(router, "GET", "/skills/ship-it/other.md", None).await;
    assert_eq!(wrong_file.status, 404);
    assert_eq!(wrong_file.body_string(), "Not Found");
}
