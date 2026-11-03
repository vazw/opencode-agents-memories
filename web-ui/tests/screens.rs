//! The W6.3 session-history screen: the request it builds and the answer shapes
//! it decodes.
//!
//! Everything here is pure (no `fetch`): the list and delete requests are
//! compared against the pinned [`ENDPOINTS`] templates filled with
//! [`encode_component`], and the delete's data-less success envelope is pinned
//! by [`delete_outcome`]. The live `fetch` itself is the adapter exercised by
//! the manual demo.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::api::envelope::ApiError;
use web_ui::paths::{ENDPOINTS, EndpointId, encode_component};
use web_ui::screens::{
    SessionRow, delete_outcome, delete_session_spec, format_created_at, initial_project,
    list_sessions_spec, session_path, session_request,
};

/// The pinned template for `id`.
fn template(id: EndpointId) -> &'static str {
    ENDPOINTS
        .iter()
        .find(|spec| spec.id == id)
        .expect("the endpoint table has a row for every EndpointId")
        .template
}

#[wasm_bindgen_test]
fn test_list_sessions_spec_matches_the_pinned_template() {
    let spec = list_sessions_spec("demo-project");
    assert_eq!(spec.method, "GET");
    assert_eq!(
        spec.path,
        template(EndpointId::ListProjectSessions)
            .replace("{project_id}", &encode_component("demo-project"))
    );
    assert_eq!(spec.path, "/api/projects/demo-project/sessions");
    assert!(spec.query.is_empty());
    assert!(spec.body.is_none());
}

#[wasm_bindgen_test]
fn test_delete_session_spec_matches_the_pinned_template() {
    let spec = delete_session_spec("demo-project", "row-0001");
    assert_eq!(spec.method, "DELETE");
    assert_eq!(
        spec.path,
        template(EndpointId::DeleteProjectSession)
            .replace("{project_id}", &encode_component("demo-project"))
            .replace("{session_id}", &encode_component("row-0001"))
    );
    assert_eq!(spec.path, "/api/projects/demo-project/sessions/row-0001");
}

#[wasm_bindgen_test]
fn test_session_paths_percent_encode_both_parameters() {
    // A traversal-shaped project id is encoded (the `%` is encoded too), so it
    // can never become a `../` segment; a slash in the session id stays one
    // segment.
    let spec = delete_session_spec("..%2F..", "a/b c");
    assert_eq!(spec.path, "/api/projects/..%252F../sessions/a%2Fb%20c");

    let list = list_sessions_spec("..%2F..");
    assert_eq!(list.path, "/api/projects/..%252F../sessions");

    // The filler only touches the two session parameters.
    assert_eq!(
        session_path(EndpointId::ListProjectSessions, "p", None),
        "/api/projects/p/sessions"
    );
    assert_eq!(
        session_path(EndpointId::DeleteProjectSession, "p", Some("s")),
        "/api/projects/p/sessions/s"
    );
}

#[wasm_bindgen_test]
fn test_delete_outcome_accepts_the_dataless_success() {
    // The endpoint's documented success: `{"success":true}` with no `data`.
    assert_eq!(delete_outcome(200, br#"{"success":true}"#), Ok(()));
    assert_eq!(
        delete_outcome(200, br#"{"success":true,"data":null}"#),
        Ok(())
    );
}

#[wasm_bindgen_test]
fn test_delete_outcome_maps_a_failure_envelope() {
    assert_eq!(
        delete_outcome(404, br#"{"success":false,"error":"Session not found"}"#),
        Err(ApiError::Server {
            status: 404,
            message: "Session not found".to_string(),
        })
    );
    assert_eq!(
        delete_outcome(500, br#"{"success":false,"error":"Internal Server Error"}"#),
        Err(ApiError::Server {
            status: 500,
            message: "Internal Server Error".to_string(),
        })
    );
    assert_eq!(delete_outcome(200, b"not json"), Err(ApiError::Malformed));
}

#[wasm_bindgen_test]
fn test_session_rows_deserialize_the_endpoint_projection() {
    let row: SessionRow = serde_json::from_str(
        r#"{"id":"row-0001","projectId":"demo-project","sessionId":"demo-session",
            "agentId":"agent-child","agentName":"Implementer","summary":"done",
            "parentSessionId":null,"orphaned":true,"filesTouched":["src/lib.rs"],
            "createdAt":1788220800000,"updatedAt":1788220800000}"#,
    )
    .expect("the endpoint's projection decodes into a row");
    assert_eq!(
        row,
        SessionRow {
            id: "row-0001".to_string(),
            agent_name: "Implementer".to_string(),
            summary: "done".to_string(),
            created_at: 1_788_220_800_000,
            orphaned: true,
        }
    );
}

#[wasm_bindgen_test]
fn test_format_created_at_formats_utc() {
    assert_eq!(format_created_at(0), "1970-01-01 00:00:00 UTC");
    // 2024-02-29T00:00:00Z — a leap day, so the civil-day arithmetic is pinned.
    assert_eq!(
        format_created_at(1_709_164_800_000),
        "2024-02-29 00:00:00 UTC"
    );
    assert_eq!(
        format_created_at(1_788_220_800_999),
        "2026-09-01 00:00:00 UTC"
    );
}

#[wasm_bindgen_test]
fn test_initial_project_reads_the_query_parameter() {
    assert_eq!(
        initial_project("?project=demo-project"),
        Some("demo-project".to_string())
    );
    assert_eq!(initial_project("?a=1&project=x&b=2"), Some("x".to_string()));
}

#[wasm_bindgen_test]
fn test_initial_project_ignores_absent_or_empty_values() {
    assert_eq!(initial_project(""), None);
    assert_eq!(initial_project("?project="), None);
    assert_eq!(initial_project("?otherproject=x"), None);
}

fn roster_row(id: &str) -> web_ui::agents::ProjectRow {
    serde_json::from_str(&format!(
        r#"{{"projectId":"{id}","sessionCount":1,"milestoneCount":0}}"#
    ))
    .expect("a roster row decodes")
}

#[wasm_bindgen_test]
fn test_initial_project_percent_decodes() {
    assert_eq!(
        initial_project("?project=%2Fhome%2Fvaz%2Fproj"),
        Some("/home/vaz/proj".to_string())
    );
    assert_eq!(initial_project("?project=%20"), None);
}

#[wasm_bindgen_test]
fn test_session_request_uses_only_roster_ids() {
    let roster = vec![roster_row("/a/one"), roster_row("/b/two")];
    let spec = session_request(&roster, Some("/b/two")).expect("a roster id resolves");
    assert_eq!(spec.path, list_sessions_spec("/b/two").path);
    // An unknown id never reaches the request: it falls back to the first row.
    let spec = session_request(&roster, Some("/nope")).expect("falls back");
    assert_eq!(spec.path, list_sessions_spec("/a/one").path);
    assert!(!spec.path.contains("nope"));
    assert!(session_request(&[], Some("/a/one")).is_none());
}
