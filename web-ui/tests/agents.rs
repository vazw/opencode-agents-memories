//! The Agents view's pure seam: the milestone DTO, the roster fold, the five-arm
//! state oracle, the milestone drill-down, the error mapping and the one
//! request spec. Every test drives the public `web_ui::agents` module.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::agents::{
    AGENTS_DETAIL_CLASS, AGENTS_DETAIL_TESTID, AGENTS_EMPTY_CLASS, AGENTS_EMPTY_TESTID,
    AGENTS_ERROR_CLASS, AGENTS_ERROR_TESTID, AGENTS_FACT_CLASS, AGENTS_FACT_TESTID,
    AGENTS_LOADING_CLASS, AGENTS_LOADING_TESTID, AGENTS_MILESTONE_CLASS, AGENTS_MILESTONE_TESTID,
    AGENTS_ROSTER_CLASS, AGENTS_ROSTER_TESTID, AGENTS_ROW_CLASS, AGENTS_ROW_TESTID,
    AGENTS_UNAVAILABLE_CLASS, AGENTS_UNAVAILABLE_MESSAGE, AGENTS_UNAVAILABLE_TESTID, AgentSummary,
    AgentsOutcome, AgentsView, ContributorDto, MilestoneDto, PromotedFactDto, agent_detail,
    agents_view, format_window, list_milestones_spec, outcome_for_error, roster,
};
use web_ui::api::envelope::ApiError;

fn contributor(agent_id: &str, agent_name: &str, created_at: i64) -> ContributorDto {
    ContributorDto {
        session_row_id: format!("row-{agent_id}-{created_at}"),
        session_id: format!("session-{agent_id}-{created_at}"),
        parent_session_id: None,
        agent_id: agent_id.to_string(),
        agent_name: agent_name.to_string(),
        created_at,
        summary: String::new(),
    }
}

fn milestone(
    id: &str,
    window_start: i64,
    window_end: i64,
    contributors: Vec<ContributorDto>,
) -> MilestoneDto {
    MilestoneDto {
        id: id.to_string(),
        project_id: "project-1".to_string(),
        agent_id: contributors
            .first()
            .map(|contributor| contributor.agent_id.clone())
            .unwrap_or_default(),
        summary: format!("summary of {id}"),
        contributors,
        source_count: 1,
        window_start,
        window_end,
        created_at: window_end,
        promoted_facts: Vec::new(),
    }
}

#[wasm_bindgen_test]
fn test_agents_milestone_decodes_the_camel_case_projection() {
    let payload = r#"{
        "id": "m-1",
        "projectId": "p-1",
        "agentId": "a-1",
        "summary": "consolidated two sessions",
        "contributors": [
            {
                "sessionRowId": "r-1",
                "sessionId": "s-1",
                "parentSessionId": "s-0",
                "agentId": "a-1",
                "agentName": "Alice",
                "createdAt": 100,
                "summary": "source text"
            }
        ],
        "sourceCount": 3,
        "windowStart": 100,
        "windowEnd": 900,
        "createdAt": 950,
        "promotedFacts": [
            {
                "id": "f-1",
                "content": "remember this",
                "memoryType": "project",
                "createdAt": 800
            }
        ],
        "embedding": [0.1, 0.2]
    }"#;
    let dto: MilestoneDto = serde_json::from_str(payload)
        .unwrap_or_else(|error| panic!("the camelCase projection must decode: {error}"));

    assert_eq!(dto.id, "m-1");
    assert_eq!(dto.project_id, "p-1");
    assert_eq!(dto.agent_id, "a-1");
    assert_eq!(dto.summary, "consolidated two sessions");
    assert_eq!(dto.source_count, 3);
    assert_eq!(dto.window_start, 100);
    assert_eq!(dto.window_end, 900);
    assert_eq!(dto.created_at, 950);
    assert_eq!(dto.contributors.len(), 1);
    assert_eq!(dto.contributors[0].session_row_id, "r-1");
    assert_eq!(
        dto.contributors[0].parent_session_id.as_deref(),
        Some("s-0")
    );
    assert_eq!(dto.contributors[0].agent_name, "Alice");
    assert_eq!(dto.promoted_facts.len(), 1);
    assert_eq!(dto.promoted_facts[0].id, "f-1");
    assert_eq!(dto.promoted_facts[0].content, "remember this");
    assert_eq!(
        dto.promoted_facts[0].memory_type.as_deref(),
        Some("project")
    );
    assert_eq!(dto.promoted_facts[0].created_at, 800);

    // `id` is the one required field: without it the payload is not a milestone.
    assert!(serde_json::from_str::<MilestoneDto>(r#"{"projectId":"p"}"#).is_err());
}

#[wasm_bindgen_test]
fn test_agents_roster_counts_distinct_milestones_per_agent() {
    let milestones = vec![
        milestone(
            "m-1",
            10,
            20,
            vec![
                contributor("a-1", "Alice", 10),
                // A second session of the same agent inside the same milestone:
                // distinct milestone ids are the count, not contributions.
                contributor("a-1", "Alice", 20),
            ],
        ),
        milestone("m-2", 30, 40, vec![contributor("a-1", "Alice", 30)]),
    ];

    let rows = roster(&milestones);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].milestone_count, 2,
        "two sessions in one milestone count once"
    );
}

#[wasm_bindgen_test]
fn test_agents_roster_last_contribution_is_the_latest_source_stamp() {
    // Deliberately out of chronological order: the max stamp wins, not the
    // last row visited.
    let milestones = vec![
        milestone("m-1", 100, 200, vec![contributor("a-1", "Alice", 100)]),
        milestone("m-2", 150, 400, vec![contributor("a-1", "Alice", 300)]),
        milestone("m-3", 120, 250, vec![contributor("a-1", "Alice", 200)]),
    ];

    let rows = roster(&milestones);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].last_contribution, 300);
}

#[wasm_bindgen_test]
fn test_agents_roster_display_name_comes_from_the_latest_source() {
    let milestones = vec![
        milestone(
            "m-1",
            0,
            100,
            vec![
                contributor("a-1", "Old Name", 100),
                // No `agent_name` at the latest stamp: the fallback is the id.
                contributor("a-2", "", 500),
            ],
        ),
        milestone(
            "m-2",
            0,
            400,
            vec![
                contributor("a-1", "Newest", 300),
                // Same stamp as "Newest": the smaller non-empty name wins.
                contributor("a-1", "Alpha", 300),
            ],
        ),
    ];

    let rows = roster(&milestones);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].agent_id, "a-2");
    assert_eq!(
        rows[0].display_name, "a-2",
        "an empty name falls back to the agent id"
    );
    assert_eq!(rows[1].agent_id, "a-1");
    assert_eq!(
        rows[1].display_name, "Alpha",
        "the tie breaks to the lexicographically smallest non-empty name"
    );
}

#[wasm_bindgen_test]
fn test_agents_roster_skips_a_contributor_without_an_agent_id() {
    let milestones = vec![milestone(
        "m-1",
        0,
        10,
        vec![contributor("", "Ghost", 10), contributor("a-1", "Alice", 5)],
    )];

    let rows = roster(&milestones);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent_id, "a-1");
}

#[wasm_bindgen_test]
fn test_agents_roster_is_sorted_by_last_contribution_then_agent_id() {
    // Three agents; the tied pair shares a stamp and is inserted in DESCENDING
    // agent-id order, so the expected pair order can only come from the
    // `agent_id` tiebreak (a stable last-contribution-only sort would keep
    // b before a and fail).
    let milestones = vec![
        milestone("m-z", 0, 50, vec![contributor("z-agent", "Zed", 50)]),
        milestone("m-b", 0, 200, vec![contributor("b-agent", "Bea", 200)]),
        milestone("m-a", 0, 200, vec![contributor("a-agent", "Ann", 200)]),
    ];

    let rows = roster(&milestones);
    let order: Vec<(&str, i64)> = rows
        .iter()
        .map(|row| (row.agent_id.as_str(), row.last_contribution))
        .collect();
    assert_eq!(
        order,
        vec![("a-agent", 200), ("b-agent", 200), ("z-agent", 50)]
    );
}

// ---------------------------------------------------------------------------
// The five-arm state oracle
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn test_agents_view_populated_roster_maps_every_agent() {
    let milestones = vec![
        milestone("m-1", 0, 100, vec![contributor("a-1", "Alice", 100)]),
        milestone("m-2", 0, 200, vec![contributor("a-2", "Bob", 200)]),
    ];

    assert_eq!(
        agents_view(Some(&AgentsOutcome::Loaded(milestones))),
        AgentsView::Roster(vec![
            AgentSummary {
                agent_id: "a-2".to_string(),
                display_name: "Bob".to_string(),
                milestone_count: 1,
                last_contribution: 200,
            },
            AgentSummary {
                agent_id: "a-1".to_string(),
                display_name: "Alice".to_string(),
                milestone_count: 1,
                last_contribution: 100,
            },
        ])
    );
}

#[wasm_bindgen_test]
fn test_agents_view_loading_is_the_pre_outcome_state() {
    let view = agents_view(None);
    assert_eq!(view, AgentsView::Loading);
    assert_ne!(
        view,
        AgentsView::Empty,
        "loading is distinct from an empty scope"
    );
}

#[wasm_bindgen_test]
fn test_agents_view_empty_scope_is_empty_not_error() {
    let view = agents_view(Some(&AgentsOutcome::Loaded(vec![])));
    assert_eq!(
        view,
        AgentsView::Empty,
        "an empty scope is the empty state, not an error"
    );
    assert_ne!(view, AgentsView::Unavailable(AGENTS_UNAVAILABLE_MESSAGE));
    assert_ne!(view, AgentsView::Failed(String::new()));
}

#[wasm_bindgen_test]
fn test_agents_view_unavailable_is_the_documented_degraded_state() {
    assert_eq!(
        agents_view(Some(&AgentsOutcome::Unavailable)),
        AgentsView::Unavailable("Agent attribution is not available in this build.")
    );
    assert_eq!(
        AGENTS_UNAVAILABLE_MESSAGE,
        "Agent attribution is not available in this build."
    );
}

#[wasm_bindgen_test]
fn test_agents_view_unavailable_is_not_the_failure_state() {
    let unavailable = AgentsView::Unavailable(AGENTS_UNAVAILABLE_MESSAGE);
    assert_ne!(unavailable, AgentsView::Failed(String::new()));
    assert_ne!(
        unavailable,
        AgentsView::Failed("server error 500: boom".to_string())
    );

    // The rendered hooks must be non-empty and pairwise distinct within each
    // family, so the unavailable state and the failure state cannot collide in
    // the DOM. (A testid equals its class string by design: that is the same
    // hook read by the DOM and by the stylesheet, so the two families are never
    // compared with each other.)
    let testids = [
        AGENTS_LOADING_TESTID,
        AGENTS_EMPTY_TESTID,
        AGENTS_UNAVAILABLE_TESTID,
        AGENTS_ROSTER_TESTID,
        AGENTS_ERROR_TESTID,
        AGENTS_DETAIL_TESTID,
        AGENTS_MILESTONE_TESTID,
        AGENTS_FACT_TESTID,
        AGENTS_ROW_TESTID,
    ];
    let classes = [
        AGENTS_LOADING_CLASS,
        AGENTS_EMPTY_CLASS,
        AGENTS_UNAVAILABLE_CLASS,
        AGENTS_ROSTER_CLASS,
        AGENTS_ERROR_CLASS,
        AGENTS_DETAIL_CLASS,
        AGENTS_MILESTONE_CLASS,
        AGENTS_FACT_CLASS,
        AGENTS_ROW_CLASS,
    ];
    assert!(testids.iter().all(|hook| !hook.is_empty()));
    assert!(classes.iter().all(|hook| !hook.is_empty()));
    for (index, hook) in testids.iter().enumerate() {
        for other in &testids[index + 1..] {
            assert_ne!(hook, other, "testids must be pairwise distinct");
        }
    }
    for (index, hook) in classes.iter().enumerate() {
        for other in &classes[index + 1..] {
            assert_ne!(hook, other, "classes must be pairwise distinct");
        }
    }
    assert_ne!(AGENTS_UNAVAILABLE_CLASS, AGENTS_ERROR_CLASS);
}

#[wasm_bindgen_test]
fn test_agents_view_failure_keeps_the_message() {
    let failed = AgentsOutcome::Failed("server error 500: boom".to_string());
    let view = agents_view(Some(&failed));
    assert_eq!(
        view,
        AgentsView::Failed("server error 500: boom".to_string())
    );
    assert_ne!(view, AgentsView::Empty, "a failure is not an empty scope");
}

/// The documented D-38 degrade: only the `501` absent-service answer is the
/// `Unavailable` state; every other failure — 500, transport, malformed — is a
/// real `Failed` carrying the bounded Display.
#[wasm_bindgen_test]
fn test_outcome_for_error_maps_the_501_degrade_to_unavailable_not_failed() {
    assert_eq!(
        outcome_for_error(ApiError::server(
            501,
            "the endpoint is not available in this build"
        )),
        AgentsOutcome::Unavailable,
        "the absent-service class is the documented degrade"
    );
    for error in [
        ApiError::server(500, "Internal Server Error"),
        ApiError::Transport,
        ApiError::Malformed,
    ] {
        let message = error.to_string();
        assert_eq!(
            outcome_for_error(error),
            AgentsOutcome::Failed(message),
            "a real failure is never the unavailable state"
        );
    }
}

/// The panel's one request: the pinned project-scoped milestone template with
/// the id percent-encoded as a single segment, no query and no body.
#[wasm_bindgen_test]
fn test_list_milestones_spec_builds_the_project_milestone_path() {
    let spec = list_milestones_spec("a b/c");
    assert_eq!(spec.method, "GET");
    assert_eq!(spec.path, "/api/projects/a%20b%2Fc/milestones");
    assert!(spec.query.is_empty(), "no query");
    assert!(spec.body.is_none(), "no body");
}

// ---------------------------------------------------------------------------
// The drill-down
// ---------------------------------------------------------------------------

fn fact(id: &str, content: &str, created_at: i64) -> PromotedFactDto {
    PromotedFactDto {
        id: id.to_string(),
        content: content.to_string(),
        memory_type: Some("project".to_string()),
        created_at,
    }
}

#[wasm_bindgen_test]
fn test_agent_detail_renders_milestones_newest_first() {
    // "Newest" is window_end desc then id asc; the fixture is deliberately out
    // of order and carries a milestone the agent did not contribute to.
    let milestones = vec![
        milestone("m-c", 100, 300, vec![contributor("a-1", "Alice", 300)]),
        milestone("m-b", 100, 900, vec![contributor("a-1", "Alice", 900)]),
        milestone("m-other", 100, 950, vec![contributor("a-2", "Bob", 950)]),
        milestone("m-a", 100, 900, vec![contributor("a-1", "Alice", 900)]),
    ];

    let detail = agent_detail("a-1", &milestones);
    let ids: Vec<&str> = detail
        .milestones
        .iter()
        .map(|milestone| milestone.milestone_id.as_str())
        .collect();
    assert_eq!(ids, vec!["m-a", "m-b", "m-c"]);
}

#[wasm_bindgen_test]
fn test_agent_detail_formats_the_time_window() {
    assert_eq!(
        format_window(0, 1000),
        "1970-01-01 00:00:00 UTC → 1970-01-01 00:00:01 UTC"
    );
}

#[wasm_bindgen_test]
fn test_agent_detail_lists_promoted_facts() {
    let mut milestone = milestone("m-1", 0, 100, vec![contributor("a-1", "Alice", 100)]);
    milestone.promoted_facts = vec![
        fact("f-old", "old fact", 100),
        fact("f-new", "new fact", 200),
        fact("f-mid", "mid fact", 150),
    ];

    let detail = agent_detail("a-1", &[milestone]);
    assert_eq!(detail.milestones.len(), 1);
    let ids: Vec<&str> = detail.milestones[0]
        .facts
        .iter()
        .map(|fact| fact.id.as_str())
        .collect();
    assert_eq!(ids, vec!["f-new", "f-mid", "f-old"]);
    assert_eq!(detail.milestones[0].facts[0].content, "new fact");
    assert_eq!(
        detail.milestones[0].facts[0].memory_type.as_deref(),
        Some("project")
    );
}

#[wasm_bindgen_test]
fn test_agent_detail_dedupes_a_fact_promoted_by_two_milestones() {
    // The shared fact survives from the FIRST milestone under the documented
    // order (window_end desc, id asc), so the expectation is decidable even
    // though the fixture is inserted oldest-first.
    let mut newest = milestone("m-new", 0, 900, vec![contributor("a-1", "Alice", 900)]);
    newest.promoted_facts = vec![fact("f-1", "newest copy", 500)];

    let mut older = milestone("m-old", 0, 300, vec![contributor("a-1", "Alice", 300)]);
    older.promoted_facts = vec![
        fact("f-1", "older copy", 400),
        fact("f-2", "only once", 100),
    ];

    let detail = agent_detail("a-1", &[older, newest]);
    assert_eq!(detail.milestones[0].milestone_id, "m-new");
    assert_eq!(detail.milestones[0].facts.len(), 1);
    assert_eq!(detail.milestones[0].facts[0].content, "newest copy");

    let total: usize = detail
        .milestones
        .iter()
        .map(|milestone| milestone.facts.len())
        .sum();
    assert_eq!(
        total, 2,
        "the shared fact renders once, the distinct one survives"
    );
    assert_eq!(detail.milestones[1].facts.len(), 1);
    assert_eq!(detail.milestones[1].facts[0].id, "f-2");
}

#[wasm_bindgen_test]
fn test_agent_detail_for_an_unknown_agent_is_empty() {
    let milestones = vec![milestone(
        "m-1",
        0,
        100,
        vec![contributor("a-1", "Alice", 100)],
    )];

    let detail = agent_detail("nobody", &milestones);
    assert!(detail.milestones.is_empty());
    assert_eq!(detail.agent_id, "nobody");
    assert_eq!(detail.display_name, "nobody");
}

#[wasm_bindgen_test]
fn test_agent_detail_keeps_the_server_source_count() {
    // Built directly, NOT via `roster`: 5 source sessions but exactly one
    // contributor. A fixture where source_count == contributors.len() could not
    // tell pass-through from re-derivation.
    let mut milestone = milestone("m-1", 0, 100, vec![contributor("a-1", "Alice", 100)]);
    milestone.source_count = 5;
    assert_eq!(milestone.contributors.len(), 1);

    let detail = agent_detail("a-1", &[milestone]);
    assert_eq!(
        detail.milestones[0].source_count, 5,
        "the server's source count is rendered as sent"
    );
}
