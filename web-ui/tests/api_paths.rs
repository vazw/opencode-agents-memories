//! The pinned REST path table: the upstream contract plus the port's three
//! additions (the two W6.3 session rows and the W6.6 milestone row). This is a
//! compatibility table, not a suggestion: the set is
//! asserted exactly, so an omission, an invention or a method swap fails.
#![cfg(target_arch = "wasm32")]

use std::collections::BTreeSet;

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::paths::{ENDPOINTS, EndpointId, endpoint, endpoints_for, fill};
use web_ui::routes::RouteId;

#[wasm_bindgen_test]
fn test_pinned_endpoints_are_the_exact_rest_set() {
    // Transcribed from `example/opencode-mem/src/services/web-server.ts` (the
    // routing table) plus the paths the upstream client calls, plus the port's
    // three additions (two W6.3 session rows, one W6.6 milestone row).
    let expected: BTreeSet<(&str, &str)> = [
        ("GET", "/api/health"),
        ("GET", "/api/tags"),
        ("GET", "/api/memories"),
        ("POST", "/api/memories"),
        ("PUT", "/api/memories/{id}"),
        ("DELETE", "/api/memories/{id}"),
        ("POST", "/api/memories/bulk-delete"),
        ("POST", "/api/memories/{id}/pin"),
        ("POST", "/api/memories/{id}/unpin"),
        ("GET", "/api/search"),
        ("GET", "/api/stats"),
        ("POST", "/api/cleanup"),
        ("POST", "/api/deduplicate"),
        ("GET", "/api/migration/detect"),
        ("GET", "/api/migration/tags/detect"),
        ("POST", "/api/migration/tags/run-batch"),
        ("GET", "/api/migration/tags/progress"),
        ("POST", "/api/migration/run"),
        ("DELETE", "/api/prompts/{id}"),
        ("POST", "/api/prompts/bulk-delete"),
        ("GET", "/api/user-profile"),
        ("GET", "/api/user-profile/changelog"),
        ("GET", "/api/user-profile/snapshot"),
        ("POST", "/api/user-profile/refresh"),
        ("POST", "/api/user-profile/ai-cleanup"),
        ("POST", "/api/user-profile/ai-cleanup/apply"),
        ("PATCH", "/api/user-profile/item"),
        ("GET", "/api/skills"),
        ("POST", "/api/skills"),
        ("GET", "/api/skills/{id}"),
        ("PUT", "/api/skills/{id}"),
        ("DELETE", "/api/skills/{id}"),
        ("GET", "/api/projects/{project_id}/sessions"),
        ("DELETE", "/api/projects/{project_id}/sessions/{session_id}"),
        ("GET", "/api/projects/{project_id}/milestones"),
        // Issues 115/116: the port-only rosters.
        ("GET", "/api/user-profiles"),
        ("GET", "/api/projects"),
    ]
    .into_iter()
    .collect();

    let actual: BTreeSet<(&str, &str)> = ENDPOINTS
        .iter()
        .map(|endpoint| (endpoint.method, endpoint.template))
        .collect();

    assert_eq!(ENDPOINTS.len(), expected.len(), "no duplicate or extra row");
    assert_eq!(actual, expected, "the pinned endpoint set drifted");
}

#[wasm_bindgen_test]
fn test_pinned_endpoints_all_use_the_api_prefix() {
    for endpoint in ENDPOINTS {
        assert!(
            endpoint.template.starts_with("/api/"),
            "{:?} is outside the API surface",
            endpoint.id
        );
    }
}

#[wasm_bindgen_test]
fn test_fill_percent_encodes_the_path_parameter() {
    assert_eq!(
        fill("/api/memories/{id}", "a b/c"),
        "/api/memories/a%20b%2Fc"
    );
}

#[wasm_bindgen_test]
fn test_endpoint_accessor_round_trips_every_table_row() {
    for spec in ENDPOINTS {
        assert_eq!(endpoint(spec.id), *spec, "{:?} did not round-trip", spec.id);
    }
}

#[wasm_bindgen_test]
fn test_profile_roster_spec_matches_the_pinned_template() {
    // Issue 115: the profile page's picker reads the identity roster from the
    // port-only route. Mutant: the spec still points at `/api/user-profile`.
    assert_eq!(
        endpoint(EndpointId::ListUserProfiles).template,
        "/api/user-profiles"
    );
    assert_eq!(endpoint(EndpointId::ListUserProfiles).method, "GET");
}

#[wasm_bindgen_test]
fn test_agents_route_consumes_the_project_and_milestone_endpoints() {
    // Issue 116: the panel no longer asks for a pasted id — it reads the
    // project roster first, then that project's milestones. Mutant: only the
    // milestone template (the pre-picker screen).
    assert_eq!(
        ENDPOINTS.len(),
        37,
        "the pinned REST table grew by the project-roster row"
    );
    let expected: &[&str] = &["/api/projects", "/api/projects/{project_id}/milestones"];
    assert_eq!(
        endpoints_for(RouteId::Agents),
        expected,
        "Agents consumes the project roster and the milestone projection"
    );
}
