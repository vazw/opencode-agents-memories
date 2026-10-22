//! Bulk-delete semantics — upstream `bulk-delete.test.ts`.
//!
//! The first four cases drive `mem_web::api::client::delete_selected_memories`
//! through a scripted `BulkDeleteRequest` (the prompt group first, the memory
//! group second, the cascade-linked memory ids counted as deleted); the rest
//! drive the two REST endpoints through the router.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use common::fakes::{
    FakeBackend, MEMORY_ID, PROJECT_TAG, PROMPT_ID, USER_ID, fake_services, memory_row,
};
use common::{RawResponse, api_request, app_state_with_api};
use mem_web::api::client::{
    BulkDeleteAnswer, BulkDeleteLinkHint, BulkDeleteOutcome, BulkDeleteRequest,
    delete_selected_memories,
};
use mem_web::api::ports::{PromptPort, PromptRow};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// A scripted [`BulkDeleteRequest`]: records every call and fails the endpoint
/// it was told to fail.
struct ScriptedRequest {
    calls: Mutex<Vec<(String, Value)>>,
    fail_on: Option<&'static str>,
    error: &'static str,
}

impl ScriptedRequest {
    fn succeeding() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_on: None,
            error: "",
        }
    }

    fn failing(endpoint: &'static str, error: &'static str) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_on: Some(endpoint),
            error,
        }
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls
            .lock()
            .expect("the scripted request lock is never poisoned")
            .clone()
    }
}

#[async_trait]
impl BulkDeleteRequest for ScriptedRequest {
    async fn request(&self, endpoint: &'static str, body: Value) -> BulkDeleteAnswer {
        self.calls
            .lock()
            .expect("the scripted request lock is never poisoned")
            .push((endpoint.to_string(), body));
        if self.fail_on == Some(endpoint) {
            BulkDeleteAnswer {
                success: false,
                error: Some(self.error.to_string()),
            }
        } else {
            BulkDeleteAnswer {
                success: true,
                error: None,
            }
        }
    }
}

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

/// `POST` with the token and a JSON body.
async fn post(router: axum::Router, path: &str, body: &str) -> RawResponse {
    api_request(router, "POST", path, Some(API_TOKEN), Some(body)).await
}

#[tokio::test]
async fn test_bulk_delete_groups_prompt_and_memory_ids_and_reports_all_deleted_ids() {
    let request = ScriptedRequest::succeeding();
    let ids = ["prompt_1", "memory_1", "memory_2"].map(str::to_string);

    let outcome = delete_selected_memories(&ids, &request, &[]).await;

    assert_eq!(
        outcome,
        BulkDeleteOutcome {
            success: true,
            deleted_ids: vec![
                "prompt_1".to_string(),
                "memory_1".to_string(),
                "memory_2".to_string(),
            ],
            error: None,
        }
    );

    let calls = request.calls();
    let endpoints: Vec<&str> = calls
        .iter()
        .map(|(endpoint, _)| endpoint.as_str())
        .collect();
    assert_eq!(
        endpoints,
        ["/api/prompts/bulk-delete", "/api/memories/bulk-delete"],
        "the prompt group is sent first"
    );
    assert_eq!(
        calls[0].1,
        json!({"ids": ["prompt_1"], "cascade": true}),
        "the prompt group cascades"
    );
    assert_eq!(
        calls[1].1,
        json!({"ids": ["memory_1", "memory_2"], "cascade": true}),
        "the memory group cascades"
    );
}

#[tokio::test]
async fn test_bulk_delete_stops_after_the_first_failed_group_instead_of_reporting_success() {
    let request = ScriptedRequest::failing("/api/prompts/bulk-delete", "prompt delete failed");
    let ids = ["prompt_1", "memory_1"].map(str::to_string);

    let outcome = delete_selected_memories(&ids, &request, &[]).await;

    assert_eq!(
        outcome,
        BulkDeleteOutcome {
            success: false,
            deleted_ids: Vec::new(),
            error: Some("prompt delete failed".to_string()),
        }
    );
    let endpoints: Vec<String> = request
        .calls()
        .into_iter()
        .map(|(endpoint, _)| endpoint)
        .collect();
    assert_eq!(
        endpoints,
        ["/api/prompts/bulk-delete"],
        "the memory group is never sent after the prompt group fails"
    );
}

#[tokio::test]
async fn test_bulk_delete_preserves_successful_prompt_deletions_when_the_memory_group_fails() {
    let request = ScriptedRequest::failing("/api/memories/bulk-delete", "memory delete failed");
    let ids = ["prompt_1", "memory_1"].map(str::to_string);

    let outcome = delete_selected_memories(&ids, &request, &[]).await;

    assert_eq!(
        outcome,
        BulkDeleteOutcome {
            success: false,
            deleted_ids: vec!["prompt_1".to_string()],
            error: Some("memory delete failed".to_string()),
        }
    );
}

#[tokio::test]
async fn test_bulk_delete_treats_cascade_linked_memories_as_deleted_and_skips_them_on_partial_failure()
 {
    let request = ScriptedRequest::failing("/api/memories/bulk-delete", "memory delete failed");
    let ids = ["prompt_1", "memory_1", "memory_2"].map(str::to_string);
    let items = [
        BulkDeleteLinkHint {
            id: "prompt_1".to_string(),
            linked_memory_id: Some("memory_1".to_string()),
            linked_prompt_id: None,
        },
        BulkDeleteLinkHint {
            id: "memory_1".to_string(),
            linked_memory_id: None,
            linked_prompt_id: Some("prompt_1".to_string()),
        },
        BulkDeleteLinkHint {
            id: "memory_2".to_string(),
            linked_memory_id: None,
            linked_prompt_id: None,
        },
    ];

    let outcome = delete_selected_memories(&ids, &request, &items).await;

    assert_eq!(
        outcome,
        BulkDeleteOutcome {
            success: false,
            deleted_ids: vec!["prompt_1".to_string(), "memory_1".to_string()],
            error: Some("memory delete failed".to_string()),
        }
    );
    let calls = request.calls();
    assert_eq!(
        calls[1].1,
        json!({"ids": ["memory_2"], "cascade": true}),
        "the cascade-linked memory is not sent again"
    );
}

#[tokio::test]
async fn test_memories_bulk_delete_endpoint_reports_the_number_of_deleted_rows() {
    let (backend, router) = router();
    let shard = backend.project_shard();
    let mut linked = memory_row("mem_linked_1", PROJECT_TAG);
    linked.metadata = Some(format!(r#"{{"promptId":"{PROMPT_ID}"}}"#));
    backend.seed_memory(&shard, linked);

    let response = post(
        router,
        "/api/memories/bulk-delete",
        r#"{"ids":["mem_probe_1","mem_linked_1","mem_missing"]}"#,
    )
    .await;

    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"data":{"deleted":2}}"#,
        "only the rows that existed are counted"
    );
    assert!(
        backend.memory(MEMORY_ID).is_none(),
        "the seeded row is gone"
    );
    assert!(
        backend.memory("mem_linked_1").is_none(),
        "the linked row is gone"
    );
    assert!(
        backend
            .prompt_by_id(PROMPT_ID)
            .await
            .expect("the fake prompt lookup answers")
            .is_none(),
        "the bulk endpoint defaults cascade to true"
    );
}

#[tokio::test]
async fn test_bulk_delete_endpoint_rejects_an_empty_ids_array() {
    let (_, router) = router();

    for body in [r#"{"ids":[]}"#, "{}"] {
        let response = post(router.clone(), "/api/memories/bulk-delete", body).await;
        assert_eq!(
            response.status,
            200,
            "an empty ids array keeps upstream's status: {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            r#"{"success":false,"error":"ids array is required"}"#,
            "the body {body} is refused with upstream's message"
        );
    }
}

#[tokio::test]
async fn test_delete_prompt_endpoint_reports_the_deleted_memory_flag() {
    let (backend, router) = router();
    backend.seed_prompt(PromptRow {
        id: "prompt_linked_1".to_string(),
        session_id: "session_probe".to_string(),
        content: "linked prompt".to_string(),
        created_at: 1_700_000_000_000,
        project_path: Some("/tmp/probe".to_string()),
        linked_memory_id: Some(MEMORY_ID.to_string()),
    });

    let cascaded = api_request(
        router.clone(),
        "DELETE",
        "/api/prompts/prompt_linked_1?cascade=true",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(cascaded.status, 200, "{}", cascaded.body_string());
    assert_eq!(
        cascaded.body_string(),
        r#"{"success":true,"data":{"deletedMemory":true}}"#
    );
    assert!(
        backend.memory(MEMORY_ID).is_none(),
        "the cascade deleted the linked memory"
    );

    let uncascaded = api_request(
        router.clone(),
        "DELETE",
        &format!("/api/prompts/{PROMPT_ID}"),
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(
        uncascaded.body_string(),
        r#"{"success":true,"data":{"deletedMemory":false}}"#,
        "without ?cascade=true the linked memory is kept"
    );

    let missing = api_request(
        router,
        "DELETE",
        "/api/prompts/does-not-exist",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(missing.status, 200);
    assert_eq!(
        missing.body_string(),
        r#"{"success":false,"error":"Prompt not found"}"#
    );
}

#[tokio::test]
async fn test_prompts_bulk_delete_endpoint_reports_the_number_of_deleted_rows() {
    let (_, router) = router();

    let response = post(
        router.clone(),
        "/api/prompts/bulk-delete",
        r#"{"ids":["prompt_probe_1","prompt_missing"]}"#,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"data":{"deleted":1}}"#
    );

    let empty = post(router, "/api/prompts/bulk-delete", r#"{"ids":[]}"#).await;
    assert_eq!(empty.status, 200);
    assert_eq!(
        empty.body_string(),
        r#"{"success":false,"error":"ids array is required"}"#
    );
}
