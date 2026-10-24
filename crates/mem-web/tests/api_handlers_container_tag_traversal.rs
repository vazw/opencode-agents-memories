//! Container-tag traversal protection on the scoped write path — upstream
//! `api-handlers-container-tag-traversal.test.ts`.
//!
//! The guard resolves a container tag to exactly one scope + one 16-hex hash;
//! a malformed tag is rejected **before** any embedding or store call
//! (D-17/D-10). The later slices append the six-row scoped-endpoint matrix and
//! the D-A prefix case.

mod common;

use std::sync::Arc;

use common::fakes::{FakeBackend, PROJECT_HASH, fake_services};
use common::{api_request, app_state_with_api};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

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
        common::throwaway_log_dir(),
        services,
        common::fakes::USER_ID,
        "127.0.0.1",
    );
    (backend, build_router(Arc::new(state)))
}

/// `POST /api/memories` with the given container tag.
async fn add_memory(router: axum::Router, container_tag: &str) -> common::RawResponse {
    let body = serde_json::json!({
        "content": "hello",
        "containerTag": container_tag,
    })
    .to_string();
    api_request(
        router,
        "POST",
        "/api/memories",
        Some(API_TOKEN),
        Some(&body),
    )
    .await
}

#[tokio::test]
async fn test_add_memory_rejects_a_container_tag_whose_hash_segment_contains_path_traversal_sequences()
 {
    let (backend, router) = router();
    let response = add_memory(router, "project_x_../../../../Temp/pwn").await;

    assert_eq!(
        response.status,
        400,
        "a malformed tag is a client error (D-10): {}",
        response.body_string()
    );
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Invalid container tag"}"#
    );
    assert!(
        !response.body_string().contains("Temp"),
        "the offending tag is never echoed"
    );
    assert!(
        backend.calls().is_empty(),
        "a rejected tag never reaches the store: {:?}",
        backend.calls()
    );
}

#[tokio::test]
async fn test_add_memory_rejects_a_container_tag_whose_hash_segment_contains_a_path_separator() {
    let (backend, router) = router();
    let response = add_memory(router, "project_x_foo/bar").await;

    assert_eq!(response.status, 400);
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Invalid container tag"}"#
    );
    assert!(backend.calls().is_empty());
}

#[tokio::test]
async fn test_add_memory_accepts_a_legitimate_sha256_style_container_tag() {
    let (backend, router) = router();
    let response = add_memory(router, "mem_project_abcdef1234567890").await;

    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = response.body_string();
    assert!(
        body.starts_with(r#"{"success":true,"data":{"id":"mem_"#),
        "the created id is returned: {body}"
    );
    assert!(
        backend
            .calls()
            .contains(&"insert_memory:project:abcdef1234567890".to_string()),
        "the insert ran under the resolved scope: {:?}",
        backend.calls()
    );
}

#[tokio::test]
async fn test_a_rejected_container_tag_never_reaches_the_store_port() {
    let (backend, router) = router();

    // Every rejected shape, one after the other: the call log must stay empty.
    // An empty tag is upstream's missing-field business error (HTTP 200); the
    // malformed shapes are client errors (D-10).
    for (tag, expected_status) in [
        ("project_x_../../../../Temp/pwn", 400),
        ("project_x_foo/bar", 400),
        ("mem_project_abcdefg", 400),
        ("", 200),
    ] {
        let response = add_memory(router.clone(), tag).await;
        assert_eq!(
            response.status,
            expected_status,
            "the tag {tag:?} is answered with its documented status: {}",
            response.body_string()
        );
        assert!(
            response.body_string().contains(r#""success":false"#),
            "the tag {tag:?} is refused: {}",
            response.body_string()
        );
    }

    assert!(
        backend.calls().is_empty(),
        "no rejected request reached any port: {:?}",
        backend.calls()
    );
}

/// The seeded project hash is the guard's happy-path hash (used by the walk).
#[test]
fn test_the_fixture_hash_is_a_valid_scope_hash() {
    assert!(mem_web::api::guard::is_valid_container_tag(&format!(
        "mem_project_{PROJECT_HASH}"
    )));
}

/// The six scoped shapes of the guard matrix (H2 + D-A): every one of them is
/// rejected **for the reason stated** — the scope-segment rule for rows 1–2,
/// the hash-segment rule for rows 3–6 — with the fixed `400` and never an
/// unfiltered fallback.
#[tokio::test]
async fn test_every_scoped_endpoint_rejects_a_traversal_container_tag() {
    let (backend, router) = router();

    // (method, path, body, why the guard rejects it)
    let cases: [(&str, &str, Option<String>, &str); 6] = [
        (
            "GET",
            "/api/memories?tag=project_x_../../../../Temp/pwn",
            None,
            "the scope segment is `x`, not `user`/`project`",
        ),
        (
            "GET",
            "/api/search?q=probe&tag=project_x_foo/bar",
            None,
            "the scope segment is `x`",
        ),
        (
            "GET",
            "/api/skills?tag=mem_project_../../../../Temp/pwn",
            None,
            "the hash segment is not 16 lowercase hex",
        ),
        (
            "POST",
            "/api/memories",
            Some(
                serde_json::json!({
                    "content": "hello",
                    "containerTag": "mem_project_../../../../Temp/pwn",
                })
                .to_string(),
            ),
            "the hash segment is not 16 lowercase hex",
        ),
        (
            "POST",
            "/api/skills",
            Some(
                serde_json::json!({
                    "id": "matrix-probe",
                    "name": "Matrix Probe",
                    "content": "## Matrix",
                    "containerTag": "mem_project_foo/bar",
                })
                .to_string(),
            ),
            "the hash segment is not 16 lowercase hex",
        ),
        (
            "PUT",
            "/api/skills/ship-it",
            Some(serde_json::json!({"containerTag": "mem_project_foo/bar"}).to_string()),
            "the hash segment is not 16 lowercase hex",
        ),
    ];

    for (method, path, body, reason) in cases {
        let response = api_request(
            router.clone(),
            method,
            path,
            Some(API_TOKEN),
            body.as_deref(),
        )
        .await;
        assert_eq!(
            response.status,
            400,
            "{method} {path} is refused ({reason}): {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            r#"{"success":false,"error":"Invalid container tag"}"#,
            "{method} {path} ({reason})"
        );
        assert!(
            !response.body_string().contains("Temp") && !response.body_string().contains("etc"),
            "the offending tag is never echoed: {}",
            response.body_string()
        );
    }

    assert!(
        backend.calls().is_empty(),
        "no rejected scoped request reached the store port: {:?}",
        backend.calls()
    );
}

/// D-A — the guard's documented limit: the **prefix** is not validated, so a
/// traversal-shaped prefix parses. It is harmless: the request resolves to
/// exactly one scope ref (user + the 16-hex hash), the tag only travels as a
/// bound value, and the prefix never becomes a path or an authority.
#[tokio::test]
async fn test_a_traversal_shaped_prefix_is_accepted_but_never_becomes_a_path() {
    let (backend, router) = router();
    let response = api_request(
        router,
        "POST",
        "/api/memories",
        Some(API_TOKEN),
        Some(
            &serde_json::json!({
                "content": "prefix probe",
                "containerTag": "../../etc_user_c1c2c3d4e5f67890",
            })
            .to_string(),
        ),
    )
    .await;

    assert_eq!(response.status, 200, "{}", response.body_string());
    let body: serde_json::Value =
        serde_json::from_str(&response.body_string()).expect("the body is JSON");
    assert_eq!(body.get("success"), Some(&serde_json::json!(true)));
    let id = body
        .get("data")
        .and_then(|data| data.get("id"))
        .and_then(serde_json::Value::as_str)
        .expect("the created id is returned");
    assert!(
        id.starts_with("mem_") && !id.contains("..") && !id.contains("etc"),
        "the prefix never becomes part of the id: {id}"
    );

    assert_eq!(
        backend.calls(),
        vec!["insert_memory:user:c1c2c3d4e5f67890".to_string()],
        "the request resolved to exactly one scope ref and one store call"
    );
    let stored = backend.memory(id).expect("the row is stored");
    assert_eq!(stored.container_tag, "../../etc_user_c1c2c3d4e5f67890");
    assert_eq!(
        backend.user_shard().scope_hash,
        "c1c2c3d4e5f67890",
        "the shard path comes from the validated hash, never from the prefix"
    );
}
