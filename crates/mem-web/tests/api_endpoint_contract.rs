//! The router-level contract of the pinned W6.2 REST surface.
//!
//! Step 2 pins the table and the envelope; later steps append the
//! behavioural cases for each endpoint group.

mod common;

use common::fakes::{MEMORY_ID, PROJECT_TAG, USER_ID, fake_services};
use common::{api_request, app_state_with_api};
use mem_web::api::{PLACEHOLDER_MARKER, ROUTE_TABLE, RouteExpectation};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// A router with the fake backend and a configured API token.
fn router_with_fakes() -> (std::sync::Arc<common::fakes::FakeBackend>, axum::Router) {
    let (backend, services) = fake_services();
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some("test-token"),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        USER_ID,
        "127.0.0.1",
    );
    (backend, build_router(std::sync::Arc::new(state)))
}

#[tokio::test]
async fn test_the_pinned_route_table_lists_every_upstream_endpoint() {
    assert_eq!(ROUTE_TABLE.len(), 37, "the pinned table is 37 rows");

    let mut keys: Vec<(&str, &str)> = ROUTE_TABLE
        .iter()
        .map(|row| (row.method, row.path))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(
        keys.len(),
        ROUTE_TABLE.len(),
        "no duplicate method+path rows"
    );

    let api_rows = ROUTE_TABLE
        .iter()
        .filter(|row| row.path.starts_with("/api/"))
        .count();
    assert_eq!(api_rows, 35, "33 upstream API entries + 2 session stubs");

    let catalog_rows = ROUTE_TABLE
        .iter()
        .filter(|row| row.path.starts_with("/skills/"))
        .count();
    assert_eq!(catalog_rows, 2, "the two OpenCode catalog paths");

    for row in &ROUTE_TABLE {
        assert!(
            !row.path.contains(':'),
            "axum 0.8 syntax: no colon parameters ({})",
            row.path
        );
        assert!(!row.module.is_empty());
        assert!(!row.upstream_line.is_empty());
    }

    let stub_rows: Vec<_> = ROUTE_TABLE
        .iter()
        .filter(|row| row.expectation == RouteExpectation::Stub)
        .map(|row| (row.method, row.path))
        .collect();
    assert_eq!(
        stub_rows,
        vec![
            ("POST", "/api/cleanup"),
            ("POST", "/api/deduplicate"),
            ("GET", "/api/projects/{project_id}/sessions"),
            ("DELETE", "/api/projects/{project_id}/sessions/{session_id}"),
        ]
    );

    assert!(
        ROUTE_TABLE
            .iter()
            .any(|row| row.method == "PUT" && row.path == "/api/memories/bulk-delete"),
        "the synthetic PUT /api/memories/bulk-delete row is pinned"
    );
}

#[tokio::test]
async fn test_every_api_response_uses_the_success_data_error_envelope() {
    let (_, router) = router_with_fakes();

    for row in ROUTE_TABLE.iter() {
        if row.path.starts_with("/skills/") {
            // The catalog paths answer outside the envelope by design.
            continue;
        }
        let path = row
            .path
            .replace("{project_id}", "probe")
            .replace("{session_id}", "probe")
            .replace("{id}", "probe");
        let token = if row.path == "/api/health" {
            None
        } else {
            Some("test-token")
        };
        let response = api_request(router.clone(), row.method, &path, token, None).await;
        let body = response.body_string();
        assert!(
            body.starts_with(r#"{"success":"#),
            "{} {} answered a non-envelope body: {body}",
            row.method,
            row.path
        );
        let content_type = response.header("content-type").unwrap_or_default();
        assert!(
            content_type.contains("application/json"),
            "{} {} answered content-type {content_type}",
            row.method,
            row.path
        );
        assert!(
            !body.contains("Unauthorized"),
            "{} {} is authenticated and must not answer Unauthorized",
            row.method,
            row.path
        );
    }
}

/// The concrete URL a table row's parameterised path maps to (the fake seed's
/// ids), so a reachability check can drive the real route.
fn concrete_path(path: &str) -> String {
    path.replace("{project_id}", "probe")
        .replace("{session_id}", "probe")
        .replace("{file}", "ship-it.md")
        .replace("{id}", "ship-it")
}

#[tokio::test]
async fn test_route_table_registers_every_upstream_endpoint() {
    // Every row must be REACHABLE: with a token, the request must not fall to
    // the plain-text 404 fallback (an unregistered route or a mis-methoded
    // one), and the answer must be the route's own (the body is never the
    // not-found text).
    for row in ROUTE_TABLE.iter() {
        let (_, router) = router_with_fakes();
        let path = concrete_path(row.path);
        let token = if row.path == "/api/health" {
            None
        } else {
            Some("test-token")
        };
        let response = api_request(router, row.method, &path, token, Some("{}")).await;
        assert_ne!(
            response.status,
            404,
            "{} {} is not registered (plain-text 404 fallback): {}",
            row.method,
            row.path,
            response.body_string()
        );
        assert_ne!(
            response.status, 405,
            "{} {} is registered with the wrong method",
            row.method, row.path
        );
        assert_ne!(
            response.body_string().trim(),
            "Not Found",
            "{} {} answered the not-found fallback",
            row.method,
            row.path
        );
    }
}

#[tokio::test]
async fn test_memories_list_rejects_a_malformed_tag_filter() {
    let (backend, router) = router_with_fakes();
    let response = api_request(
        router,
        "GET",
        "/api/memories?tag=project_x_../../../../Temp/pwn",
        Some("test-token"),
        None,
    )
    .await;

    assert_eq!(response.status, 400, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Invalid container tag"}"#
    );
    assert!(
        backend.calls().is_empty(),
        "a malformed filter never falls back to the unfiltered listing: {:?}",
        backend.calls()
    );
}

#[tokio::test]
async fn test_delete_bulk_delete_path_answers_the_upstream_invalid_id() {
    let (_, router) = router_with_fakes();

    for path in ["/api/memories/bulk-delete", "/api/prompts/bulk-delete"] {
        let response = api_request(router.clone(), "DELETE", path, Some("test-token"), None).await;
        assert_eq!(
            response.status,
            200,
            "DELETE {path} keeps upstream's status: {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            r#"{"success":false,"error":"Invalid ID"}"#,
            "DELETE {path} keeps upstream's message"
        );
    }

    // The synthetic PUT on the same static path answers `handleUpdateMemory`'s
    // not-found business error.
    let response = api_request(
        router,
        "PUT",
        "/api/memories/bulk-delete",
        Some("test-token"),
        Some(r#"{"content":"probe"}"#),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Memory not found"}"#
    );
}

#[tokio::test]
async fn test_pin_and_unpin_flip_the_pinned_flag() {
    let (backend, router) = router_with_fakes();

    let pinned = api_request(
        router.clone(),
        "POST",
        &format!("/api/memories/{MEMORY_ID}/pin"),
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(pinned.status, 200, "{}", pinned.body_string());
    assert_eq!(
        pinned.body_string(),
        r#"{"success":true}"#,
        "a handler with no data omits the data key"
    );
    assert!(
        backend.memory(MEMORY_ID).expect("the seed row").is_pinned,
        "the row is pinned"
    );

    let unpinned = api_request(
        router,
        "POST",
        &format!("/api/memories/{MEMORY_ID}/unpin"),
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(unpinned.status, 200);
    assert_eq!(unpinned.body_string(), r#"{"success":true}"#);
    assert!(
        !backend.memory(MEMORY_ID).expect("the seed row").is_pinned,
        "the row is unpinned"
    );
}

#[tokio::test]
async fn test_pagination_parameters_are_validated() {
    let (_, router) = router_with_fakes();

    for query in ["page=0", "page=abc", "pageSize=0", "pageSize=201"] {
        let response = api_request(
            router.clone(),
            "GET",
            &format!("/api/memories?{query}"),
            Some("test-token"),
            None,
        )
        .await;
        assert_eq!(
            response.status,
            400,
            "?{query} is refused: {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            r#"{"success":false,"error":"Invalid pagination parameters"}"#
        );
    }

    let accepted = api_request(
        router,
        "GET",
        "/api/memories?page=1&pageSize=200",
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(accepted.status, 200, "{}", accepted.body_string());
}

#[tokio::test]
async fn test_validation_errors_keep_the_upstream_message_and_status() {
    let (_, router) = router_with_fakes();

    // Upstream's handler-level validation errors are HTTP 200 + success:false.
    let cases = [
        (
            "POST",
            "/api/memories",
            Some(r#"{}"#),
            "content and containerTag are required",
        ),
        (
            "POST",
            "/api/memories",
            Some(r#"{"content":"only content"}"#),
            "content and containerTag are required",
        ),
        (
            "PUT",
            "/api/memories/does-not-exist",
            Some(r#"{"content":"probe"}"#),
            "Memory not found",
        ),
        (
            "POST",
            "/api/memories/does-not-exist/pin",
            None,
            "Memory not found",
        ),
        (
            "DELETE",
            "/api/memories/does-not-exist",
            None,
            "Memory not found",
        ),
    ];

    for (method, path, body, message) in cases {
        let response = api_request(router.clone(), method, path, Some("test-token"), body).await;
        assert_eq!(
            response.status,
            200,
            "{method} {path} keeps HTTP 200: {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            format!(r#"{{"success":false,"error":"{message}"}}"#),
            "{method} {path} keeps the upstream message"
        );
    }

    // A malformed body is a fixed 400, never an echo of the body.
    let malformed = api_request(
        router,
        "POST",
        "/api/memories",
        Some("test-token"),
        Some("not json"),
    )
    .await;
    assert_eq!(malformed.status, 400);
    assert_eq!(
        malformed.body_string(),
        r#"{"success":false,"error":"Invalid JSON body"}"#
    );
}

/// D-21 — the G1 body cap: a payload larger than 1 MiB is refused with 413
/// before it is parsed, and the refusal never echoes the payload.
#[tokio::test]
async fn test_request_bodies_over_one_mib_are_refused_with_413() {
    let (_, router) = router_with_fakes();

    // Comfortably over the cap, still well under anything that would make the
    // test itself slow.
    let filler = "a".repeat(mem_web::api::MAX_API_BODY_BYTES);
    let huge_body = format!(r#"{{"content":"{filler}","containerTag":"{PROJECT_TAG}"}}"#);
    assert!(huge_body.len() > mem_web::api::MAX_API_BODY_BYTES);

    let response = api_request(
        router.clone(),
        "POST",
        "/api/memories",
        Some("test-token"),
        Some(&huge_body),
    )
    .await;
    assert_eq!(
        response.status,
        413,
        "a body over 1 MiB is refused with 413: {}",
        response.body_string()
    );
    assert!(
        !response.body_string().contains(&filler),
        "the 413 refusal never echoes the payload"
    );

    // A body under the cap still reaches the handler.
    let small_body = format!(r#"{{"content":"probe","containerTag":"{PROJECT_TAG}"}}"#);
    let accepted = api_request(
        router,
        "POST",
        "/api/memories",
        Some("test-token"),
        Some(&small_body),
    )
    .await;
    assert_eq!(
        accepted.status,
        200,
        "a body under the cap still reaches the handler: {}",
        accepted.body_string()
    );
    assert!(
        accepted.body_string().contains(r#""success":true"#),
        "the accepted body answers the success envelope: {}",
        accepted.body_string()
    );
}

#[tokio::test]
async fn test_unknown_api_paths_answer_404_not_found() {
    let (_, router) = router_with_fakes();
    let response = api_request(
        router,
        "GET",
        "/api/does-not-exist",
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(response.status, 404);
    assert_eq!(response.body_string(), "Not Found");
}

/// The seeded project tag is the guard's canonical shape (used throughout).
#[test]
fn test_the_seeded_project_tag_has_the_canonical_shape() {
    assert!(mem_web::api::guard::is_valid_container_tag(PROJECT_TAG));
}

/// The `data` object of a successful envelope.
fn envelope_data(response: &common::RawResponse) -> serde_json::Value {
    let body: serde_json::Value = serde_json::from_str(&response.body_string())
        .unwrap_or_else(|error| panic!("the body is JSON: {error}: {}", response.body_string()));
    assert_eq!(
        body.get("success"),
        Some(&serde_json::json!(true)),
        "the request succeeded: {}",
        response.body_string()
    );
    body.get("data")
        .cloned()
        .unwrap_or_else(|| panic!("the envelope carries data: {}", response.body_string()))
}

/// One untagged project row (the tag-migration seed).
fn seed_untagged_project_memory(backend: &common::fakes::FakeBackend) {
    let shard = backend.project_shard();
    backend.seed_memory(
        &shard,
        common::fakes::memory_row("mem_untagged_1", PROJECT_TAG),
    );
}

#[tokio::test]
async fn test_maintenance_endpoints_answer_their_documented_envelopes() {
    let (backend, router) = router_with_fakes();
    seed_untagged_project_memory(&backend);

    let detect = api_request(
        router.clone(),
        "GET",
        "/api/migration/detect",
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(detect.status, 200, "{}", detect.body_string());
    assert_eq!(
        envelope_data(&detect),
        serde_json::json!({
            "needsMigration": false,
            "configDimensions": 768,
            "configModel": "fake-model",
            "shardMismatches": []
        })
    );

    let tag_detect = api_request(
        router.clone(),
        "GET",
        "/api/migration/tags/detect",
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(tag_detect.status, 200, "{}", tag_detect.body_string());
    assert_eq!(
        envelope_data(&tag_detect),
        serde_json::json!({"needsMigration": true, "count": 1}),
        "the untagged project row is counted"
    );

    let progress = api_request(
        router.clone(),
        "GET",
        "/api/migration/tags/progress",
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(progress.status, 200, "{}", progress.body_string());
    assert_eq!(
        envelope_data(&progress),
        serde_json::json!({
            "processed": 0,
            "cursor": 0,
            "total": 0,
            "currentBatch": 0,
            "totalBatches": 0,
            "isComplete": true,
            "errors": []
        }),
        "the initial progress is the migrator's idle state"
    );

    let run = api_request(
        router.clone(),
        "POST",
        "/api/migration/run",
        Some("test-token"),
        Some(r#"{"strategy":"fresh-start"}"#),
    )
    .await;
    assert_eq!(run.status, 200, "{}", run.body_string());
    assert_eq!(
        run.body_string(),
        r#"{"success":true,"data":{"success":true,"strategy":"fresh-start","deletedShards":0,"reEmbeddedMemories":0,"duration":5}}"#,
        "handleRunMigration carries `success: result.success` AND the data"
    );

    // Upstream's strategy check happens before the service call.
    let invalid = api_request(
        router,
        "POST",
        "/api/migration/run",
        Some("test-token"),
        Some(r#"{"strategy":"bogus"}"#),
    )
    .await;
    assert_eq!(invalid.status, 200, "{}", invalid.body_string());
    assert_eq!(
        invalid.body_string(),
        r#"{"success":false,"error":"Invalid strategy"}"#
    );
    assert!(
        !backend
            .calls()
            .iter()
            .any(|call| call.starts_with("run_migration:bogus")),
        "an invalid strategy never reaches the service: {:?}",
        backend.calls()
    );
}

#[tokio::test]
async fn test_cleanup_and_deduplicate_answer_501_until_the_services_land() {
    let (backend, router) = router_with_fakes();

    for (path, message) in [
        ("/api/cleanup", "cleanup is not available in this build"),
        (
            "/api/deduplicate",
            "deduplicate is not available in this build",
        ),
    ] {
        let response = api_request(router.clone(), "POST", path, Some("test-token"), None).await;
        assert_eq!(
            response.status,
            501,
            "{path} is the documented stub: {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            format!(r#"{{"success":false,"error":"{message}"}}"#)
        );
        assert!(
            backend.calls().is_empty(),
            "{path} must not touch a port: {:?}",
            backend.calls()
        );
    }
}

#[tokio::test]
async fn test_sessions_endpoints_are_registered_and_report_the_w63_stub() {
    let (_, router) = router_with_fakes();

    for (method, path) in [
        ("GET", "/api/projects/probe/sessions"),
        ("DELETE", "/api/projects/probe/sessions/session_probe"),
    ] {
        let response = api_request(router.clone(), method, path, Some("test-token"), None).await;
        assert_eq!(
            response.status,
            501,
            "{method} {path} is the documented W6.3 stub: {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            r#"{"success":false,"error":"Session history endpoints land with W6.3"}"#,
            "{method} {path}"
        );
        assert_ne!(
            response.body_string().trim(),
            "Not Found",
            "{method} {path} is registered"
        );
    }

    // The ids are data, never a path: a percent-encoded traversal-shaped id
    // reaches the port and the stub answers — it never escapes the store.
    let traversal = api_request(
        router,
        "GET",
        "/api/projects/..%2F..%2Fetc/sessions",
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(
        traversal.status,
        501,
        "the traversal-shaped id is carried as data: {}",
        traversal.body_string()
    );
}

#[tokio::test]
async fn test_the_tag_migration_batch_bridges_the_sync_seam_without_blocking_a_worker() {
    // The default `#[tokio::test]` runtime is current-thread: `block_in_place`
    // would panic here, and blocking a worker on an async future would hang.
    // The bridge is `spawn_blocking` + `Handle::block_on` (D-B).
    let (backend, router) = router_with_fakes();
    seed_untagged_project_memory(&backend);

    let batch = api_request(
        router.clone(),
        "POST",
        "/api/migration/tags/run-batch",
        Some("test-token"),
        Some(r#"{"batchSize":5}"#),
    )
    .await;
    assert_eq!(batch.status, 200, "{}", batch.body_string());
    assert_eq!(
        envelope_data(&batch),
        serde_json::json!({"processed": 2, "total": 2, "hasMore": false, "errors": 0}),
        "the tagged row is skipped, the untagged one is migrated"
    );

    let calls = backend.calls();
    assert!(
        calls.contains(&"generate_tags:mem_untagged_1".to_string()),
        "the untagged row's tags were pre-computed asynchronously: {calls:?}"
    );
    assert!(
        !calls.contains(&"generate_tags:mem_probe_1".to_string()),
        "an already-tagged row with a tags vector is skipped: {calls:?}"
    );
    assert!(
        calls.contains(&"run_tag_migration_batch:5".to_string()),
        "the sync migrator ran with the requested window: {calls:?}"
    );

    let progress = api_request(
        router,
        "GET",
        "/api/migration/tags/progress",
        Some("test-token"),
        None,
    )
    .await;
    assert_eq!(
        envelope_data(&progress),
        serde_json::json!({
            "processed": 2,
            "cursor": 2,
            "total": 2,
            "currentBatch": 1,
            "totalBatches": 1,
            "isComplete": true,
            "errors": []
        }),
        "the migrator's cursor advanced to the end"
    );
}

/// The scaffolding answer every slice replaced during this wave (Step 15).
///
/// The marker must be absent from **every** route's body, whatever the
/// status: a route that still answers `"… is not implemented yet"` is an
/// unfinished slice, not a documented stub (the stubs answer their own fixed
/// message).
#[tokio::test]
async fn test_no_route_answers_the_placeholder_envelope() {
    for row in ROUTE_TABLE.iter() {
        // A fresh router per row: the walk must not depend on the order of the
        // destructive rows.
        let (_, router) = router_with_fakes();
        let path = concrete_path(row.path);
        let body = match row.method {
            "POST" | "PUT" | "PATCH" => Some("{}"),
            _ => None,
        };
        let response = api_request(router, row.method, &path, Some("test-token"), body).await;
        let answer = response.body_string();
        assert!(
            !answer.contains(PLACEHOLDER_MARKER),
            "{} {} still answers the placeholder envelope ({}): {answer}",
            row.method,
            row.path,
            response.status
        );
    }
}
