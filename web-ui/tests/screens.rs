//! The W6.3 session-history screen: the request it builds and the answer shapes
//! it decodes.
//!
//! Everything here is pure (no `fetch`): the list and delete requests are
//! compared against the pinned [`ENDPOINTS`] templates filled with
//! [`encode_component`], and the delete's data-less success envelope is pinned
//! by [`delete_outcome`]. The live `fetch` itself is the adapter exercised by
//! the manual demo.
//!
//! Wave 3 (issue 118): the roster and the timeline are classified by the pure
//! [`SessionState`] state machine (`state_block::session_state`) before they
//! render, so every arm the panel shows — loading, empty, error, ready — has a
//! table-tested classifier behind it with the error > loading > empty > ready
//! precedence pinned by mutants.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::api::envelope::ApiError;
use web_ui::paths::{ENDPOINTS, EndpointId, encode_component};
use web_ui::screens::{
    SessionRow, delete_outcome, delete_session_spec, format_created_at, initial_project,
    list_sessions_spec, session_path, session_request,
};
use web_ui::state_block::{SessionState, session_state};

#[wasm_bindgen_test]
fn test_home_summary_uses_payload_values_including_zero() {
    use web_ui::core_screens::{ByScopeView, ShardStatsView, StatsView};
    let stats = StatsView {
        total: 7,
        by_scope: ByScopeView {
            user: 0,
            project: 3,
        },
        shards: vec![
            ShardStatsView {
                is_active: true,
                ..Default::default()
            },
            ShardStatsView {
                is_active: false,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        web_ui::screens::home_summary(&stats),
        [
            ("Total memories", 7),
            ("User memories", 0),
            ("Project memories", 3),
            ("Active shards", 1),
        ]
    );
}

/// One timeline row for the classifier tables.
fn row(id: &str) -> SessionRow {
    SessionRow {
        id: id.to_string(),
        agent_name: "Coder".to_string(),
        summary: "did the thing".to_string(),
        created_at: 1_788_220_800_000,
        orphaned: false,
    }
}

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

// ---------------------------------------------------------------------------
// Wave 3 — the timeline classifier (`session_state`)
//
// Precedence: error > loading > empty > ready. Each test names the mutant it
// kills; the distinguishing input is the one on which the rule and the mutant
// disagree.
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn test_session_state_error_dominates() {
    // Mutant: precedence swapped so Loading beats Error — the spinner would
    // hide a real failure forever while a load is in flight.
    let state = session_state(&[], Some("boom"), true);
    assert_eq!(
        state,
        SessionState::Error("boom".to_string()),
        "an error beats an in-flight load"
    );
}

#[wasm_bindgen_test]
fn test_session_state_ready_beats_empty() {
    // Mutant: the Ready arm dropped — populated rows classified as Empty and
    // the timeline renders "no sessions" over real data.
    let rows = vec![row("a"), row("b"), row("c")];
    let state = session_state(&rows, None, false);
    assert_eq!(state, SessionState::Ready(rows), "populated rows are Ready");
}

#[wasm_bindgen_test]
fn test_session_state_empty() {
    // Mutant: Empty conflated with Ready — an empty timeline renders an empty
    // list with no explanation.
    let state = session_state(&[], None, false);
    assert_eq!(state, SessionState::Empty, "no rows and no error is Empty");
}

#[wasm_bindgen_test]
fn test_session_state_loading() {
    // Mutant: Loading returns Ready when rows carry stale data — the previous
    // project's timeline would flash as current while the new one loads.
    let stale = vec![row("stale")];
    let state = session_state(&stale, None, true);
    assert_eq!(state, SessionState::Loading, "an in-flight load wins");
}

// ---------------------------------------------------------------------------
// Wave 3 — the roster-to-SessionState map (one-to-one over ProjectsView)
// ---------------------------------------------------------------------------

use web_ui::agents::{ProjectRow, ProjectsOutcome, ProjectsView};
use web_ui::state_block::roster_session_view;

fn roster_rows() -> Vec<ProjectRow> {
    vec![roster_row("/a/one"), roster_row("/b/two")]
}

#[wasm_bindgen_test]
fn test_roster_map_loading() {
    // Mutant: None mapped to Error or NoProjects — the picker would show an
    // error (or a falsely empty roster) during the mount fetch.
    assert_eq!(roster_session_view(None), ProjectsView::Loading);
}

#[wasm_bindgen_test]
fn test_roster_map_ready() {
    // Mutant: Ready conflated with NoProjects — the picker disappears with a
    // populated roster.
    let loaded = ProjectsOutcome::Loaded(roster_rows());
    assert_eq!(
        roster_session_view(Some(&loaded)),
        ProjectsView::Ready(roster_rows()),
        "a populated roster renders the picker"
    );
}

#[wasm_bindgen_test]
fn test_roster_map_no_projects() {
    // Mutant: an empty Loaded roster classified Failed — a fresh install
    // would show an error instead of the documented empty state.
    let loaded = ProjectsOutcome::Loaded(Vec::new());
    assert_eq!(
        roster_session_view(Some(&loaded)),
        ProjectsView::NoProjects,
        "a successful empty roster is NoProjects, never Failed"
    );
}

#[wasm_bindgen_test]
fn test_roster_map_unavailable_is_not_failed() {
    // Mutant: the 501 degrade mapped to Failed(msg) — the documented
    // informational state would render as a hard error.
    let outcome = ProjectsOutcome::Unavailable;
    assert_eq!(
        roster_session_view(Some(&outcome)),
        ProjectsView::Unavailable(web_ui::agents::PROJECTS_UNAVAILABLE_MESSAGE),
        "the 501 degrade keeps its own arm"
    );
}

#[wasm_bindgen_test]
fn test_roster_map_failed() {
    let outcome = ProjectsOutcome::Failed("boom".to_string());
    assert_eq!(
        roster_session_view(Some(&outcome)),
        ProjectsView::Failed("boom".to_string())
    );
}
