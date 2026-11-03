//! W6.7 — the four core screens' pure oracles (AC1 / AC3).
//!
//! Everything asserted here is pure (no `fetch`, no DOM): the request specs are
//! compared against the pinned [`ENDPOINTS`] templates and the shared URL
//! builder, the DTOs are decoded from the exact `mem-web` projections, and the
//! render shapes pin the empty / loaded / missing branches. This pure wasm
//! suite — run under the Node runner, no DOM and no browser — is the proof for
//! these screens; there is no browser smoke.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::api::envelope::ApiError;
use web_ui::api_request::build_url;
use web_ui::core_screens::cleanup_spec;
use web_ui::core_screens::{
    CLEANUP_ARMED_TESTID, CLEANUP_ERROR_CLASS, CLEANUP_ERROR_TESTID, CLEANUP_REFUSED_CLASS,
    CLEANUP_REFUSED_TESTID, CLEANUP_REPORT_CLASS, CLEANUP_REPORT_TESTID, CLEANUP_UNAVAILABLE_CLASS,
    CLEANUP_UNAVAILABLE_MESSAGE, CLEANUP_UNAVAILABLE_TESTID, CleanupOutcomeView, CleanupReportRow,
    cleanup_outcome, cleanup_outcome_view, cleanup_report_rows,
};
use web_ui::core_screens::{
    CleanupAction, CleanupEvent, CleanupOutcome, CleanupReport, CleanupState, CleanupStatus,
    CleanupStep, cleanup_action_label, cleanup_disabled, cleanup_next, cleanup_status,
};
use web_ui::core_screens::{
    DailyStatsView, HealthStatus, IdentityControl, MemoriesView, MemoryPage, ProfileRosterItem,
    ProfileScreen, ProfileView, SettingsScreen, ShardStatsView, StatsView, TagsPage, TagsView,
    active_shard_count, daily_peak, delete_memory_spec, error_buckets, get_user_profile_spec,
    get_user_profile_spec_for, health_outcome, health_spec, initial_tags, list_memories_spec,
    list_tags_spec, list_user_profiles_spec, memories_request, memories_view, memory_preview,
    pin_memory_spec, profile_identity_control, profile_view, range_text, search_spec,
    settings_view, sorted_shards, stats_spec, tags_json, tags_view, unpin_memory_spec,
    visible_tags,
};
use web_ui::paths::{ENDPOINTS, EndpointId, encode_component};

/// The pinned template for `id`.
fn template(id: EndpointId) -> &'static str {
    ENDPOINTS
        .iter()
        .find(|spec| spec.id == id)
        .expect("the endpoint table has a row for every EndpointId")
        .template
}

/// The pinned method for `id`.
fn method(id: EndpointId) -> &'static str {
    ENDPOINTS
        .iter()
        .find(|spec| spec.id == id)
        .expect("the endpoint table has a row for every EndpointId")
        .method
}

// ---------------------------------------------------------------------------
// Memories (Step 4)
// ---------------------------------------------------------------------------

/// One page exactly as `GET /api/memories` answers it: a memory row and a
/// captured-prompt row (the untagged `TimelineItemDto` union).
const MEMORY_PAGE_JSON: &str = r#"{
    "items": [
        {
            "type": "memory",
            "id": "mem_one",
            "content": "the first memory",
            "memoryType": "note",
            "tags": ["alpha", "beta"],
            "createdAt": "2026-10-01T12:00:00.000Z",
            "isPinned": true
        },
        {
            "type": "prompt",
            "id": "prompt_one",
            "sessionId": "session-one",
            "content": "a captured prompt",
            "createdAt": "2026-10-02T12:00:00.000Z",
            "projectPath": null,
            "linkedMemoryId": null
        }
    ],
    "total": 2,
    "page": 1,
    "pageSize": 20,
    "totalPages": 1
}"#;

fn memory_page() -> MemoryPage {
    serde_json::from_str(MEMORY_PAGE_JSON).expect("the listing projection decodes")
}

#[wasm_bindgen_test]
fn test_list_memories_spec_matches_the_pinned_template() {
    let spec = list_memories_spec(2, 25, &["alpha", "beta gamma"], false);
    assert_eq!(spec.method, method(EndpointId::ListMemories));
    assert_eq!(spec.path, template(EndpointId::ListMemories));
    assert_eq!(
        spec.query,
        vec![
            ("page".to_string(), "2".to_string()),
            ("pageSize".to_string(), "25".to_string()),
            ("tags".to_string(), "alpha".to_string()),
            ("tags".to_string(), "beta gamma".to_string()),
            ("includePrompts".to_string(), "false".to_string()),
        ],
        "page/pageSize, one tag per entry, and includePrompts only when false"
    );
    assert!(spec.body.is_none(), "a GET listing carries no request body");

    // `includePrompts=true` is the server's default: the key is omitted
    // entirely rather than sent as a redundant `true`.
    let defaulted = list_memories_spec(1, 10, &[], true);
    assert!(
        defaulted
            .query
            .iter()
            .all(|(key, _)| key != "includePrompts"),
        "no includePrompts key when prompts are included: {:?}",
        defaulted.query
    );
}

#[wasm_bindgen_test]
fn test_search_spec_builds_the_query() {
    let spec = search_spec("rust async", 1, 10);
    assert_eq!(spec.method, method(EndpointId::Search));
    assert_eq!(spec.path, template(EndpointId::Search));
    assert_eq!(
        spec.query,
        vec![
            ("q".to_string(), "rust async".to_string()),
            ("page".to_string(), "1".to_string()),
            ("pageSize".to_string(), "10".to_string()),
        ]
    );
    assert_eq!(
        build_url(&spec).expect("the spec builds a root-relative URL"),
        "/api/search?q=rust%20async&page=1&pageSize=10",
        "the shared seam percent-encodes every value"
    );
}

#[wasm_bindgen_test]
fn test_pin_spec_fills_the_id() {
    for (spec, id, verb) in [
        (
            pin_memory_spec("mem 1/x"),
            EndpointId::PinMemory,
            Some("pin"),
        ),
        (
            unpin_memory_spec("mem 1/x"),
            EndpointId::UnpinMemory,
            Some("unpin"),
        ),
        (
            delete_memory_spec("mem 1/x"),
            EndpointId::DeleteMemory,
            None,
        ),
    ] {
        assert_eq!(
            spec.path,
            template(id).replace("{id}", &encode_component("mem 1/x"))
        );
        let expected = match verb {
            Some(verb) => format!("/api/memories/mem%201%2Fx/{verb}"),
            None => "/api/memories/mem%201%2Fx".to_string(),
        };
        assert_eq!(spec.path, expected);
        assert_eq!(spec.method, method(id));
    }

    // The delete template is the bare `{id}` one (no suffix); the other two
    // append their verb to the same encoded segment.
    assert_eq!(
        delete_memory_spec("mem_one").path,
        template(EndpointId::DeleteMemory).replace("{id}", "mem_one")
    );
    assert_eq!(
        delete_memory_spec("..%2F..").path,
        "/api/memories/..%252F.."
    );
}

#[wasm_bindgen_test]
fn test_memory_page_deserializes_the_endpoint_projection() {
    let page = memory_page();
    assert_eq!(page.total, 2);
    assert_eq!(page.page, 1);
    assert_eq!(page.page_size, 20);
    assert_eq!(page.total_pages, 1);
    assert_eq!(page.items.len(), 2);

    let memory = &page.items[0];
    assert_eq!(memory.id, "mem_one");
    assert_eq!(memory.content, "the first memory");
    assert_eq!(memory.kind.as_deref(), Some("memory"));
    assert_eq!(memory.tags, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(memory.created_at, "2026-10-01T12:00:00.000Z");
    assert!(memory.is_pinned);

    // A prompt row has no tags and no pinned flag: the same DTO decodes it.
    let prompt = &page.items[1];
    assert_eq!(prompt.id, "prompt_one");
    assert_eq!(prompt.kind.as_deref(), Some("prompt"));
    assert!(prompt.tags.is_empty());
    assert!(!prompt.is_pinned);
}

#[wasm_bindgen_test]
fn test_memories_view_empty_state() {
    let empty = MemoryPage {
        items: Vec::new(),
        total: 0,
        page: 1,
        page_size: 20,
        total_pages: 0,
    };
    assert_eq!(memories_view(&empty), MemoriesView::Empty { total: 0 });

    // An out-of-range page renders the empty branch too, but keeps the real
    // total so the screen can say how many rows exist.
    let beyond = MemoryPage {
        items: Vec::new(),
        total: 7,
        page: 3,
        page_size: 20,
        total_pages: 1,
    };
    assert_eq!(memories_view(&beyond), MemoriesView::Empty { total: 7 });
}

#[wasm_bindgen_test]
fn test_memories_view_loaded_rows() {
    let view = memories_view(&memory_page());
    let MemoriesView::Loaded {
        rows,
        total,
        page,
        page_size,
        total_pages,
    } = view
    else {
        panic!("a non-empty page renders the loaded branch: {view:?}");
    };
    assert_eq!((total, page, page_size, total_pages), (2, 1, 20, 1));
    assert_eq!(rows.len(), 2);

    assert_eq!(rows[0].id, "mem_one");
    assert_eq!(rows[0].content, "the first memory");
    assert_eq!(rows[0].tags, vec!["alpha".to_string(), "beta".to_string()]);
    // The endpoint sends an ISO string; it is rendered unchanged.
    assert_eq!(rows[0].created_at, "2026-10-01T12:00:00.000Z");
    assert!(rows[0].is_pinned);
    assert!(!rows[0].is_prompt);

    assert_eq!(rows[1].id, "prompt_one");
    assert!(rows[1].is_prompt);
    assert!(!rows[1].is_pinned);
}

// ---------------------------------------------------------------------------
// Tags (Step 5)
// ---------------------------------------------------------------------------

const TAGS_JSON: &str = r#"{
    "project": [
        {
            "tag": "opencode_project_a1b2c3d4e5f67890",
            "displayName": "OpenAPI demo",
            "projectPath": "/repo/openapi-demo",
            "projectName": "openapi-demo"
        }
    ],
    "user": [
        {
            "tag": "opencode_user_c1c2c3d4e5f67890",
            "displayName": "Vaz",
            "userName": "vaz",
            "userEmail": "vaz@example.test"
        }
    ]
}"#;

#[wasm_bindgen_test]
fn test_list_tags_spec_matches_the_pinned_template() {
    let spec = list_tags_spec();
    assert_eq!(spec.method, method(EndpointId::ListTags));
    assert_eq!(spec.path, template(EndpointId::ListTags));
    assert_eq!(spec.path, "/api/tags");
    assert!(spec.query.is_empty());
    assert!(spec.body.is_none());
}

#[wasm_bindgen_test]
fn test_tags_page_deserializes_both_scopes() {
    let page: TagsPage = serde_json::from_str(TAGS_JSON).expect("the tags projection decodes");
    assert_eq!(page.project.len(), 1);
    assert_eq!(page.user.len(), 1);

    let project = &page.project[0];
    assert_eq!(project.tag, "opencode_project_a1b2c3d4e5f67890");
    assert_eq!(project.display_name.as_deref(), Some("OpenAPI demo"));
    assert_eq!(project.project_path.as_deref(), Some("/repo/openapi-demo"));
    assert_eq!(project.project_name.as_deref(), Some("openapi-demo"));

    let user = &page.user[0];
    assert_eq!(user.tag, "opencode_user_c1c2c3d4e5f67890");
    assert_eq!(user.user_email.as_deref(), Some("vaz@example.test"));
}

#[wasm_bindgen_test]
fn test_tags_view_empty_state() {
    let empty = TagsPage {
        project: Vec::new(),
        user: Vec::new(),
        memory_tags: Vec::new(),
        memory_tag_total: 0,
    };
    assert_eq!(tags_view(&empty), TagsView::Empty);
}

#[wasm_bindgen_test]
fn test_tags_view_loaded_groups() {
    let page: TagsPage = serde_json::from_str(TAGS_JSON).expect("the tags projection decodes");
    let TagsView::Loaded { project, user, .. } = tags_view(&page) else {
        panic!("a non-empty page renders the loaded branch");
    };
    assert_eq!(project.len(), 1);
    assert_eq!(user.len(), 1);
    assert_eq!(project[0].tag, "opencode_project_a1b2c3d4e5f67890");
    assert_eq!(project[0].display_name.as_deref(), Some("OpenAPI demo"));
    assert_eq!(user[0].tag, "opencode_user_c1c2c3d4e5f67890");
    assert_eq!(user[0].label(), "Vaz");
}

// ---------------------------------------------------------------------------
// Profile (Step 6)
// ---------------------------------------------------------------------------

/// The populated answer (`ProfileDto`): every field present.
const PROFILE_PRESENT_JSON: &str = r#"{
    "exists": true,
    "id": "profile_one",
    "userId": "user_one",
    "displayName": "Vaz",
    "userName": "vaz",
    "userEmail": "vaz@example.test",
    "version": 4,
    "createdAt": "2026-09-01T00:00:00.000Z",
    "lastAnalyzedAt": "2026-10-01T00:00:00.000Z",
    "totalPromptsAnalyzed": 12,
    "profileData": {
        "preferences": [
            { "description": "Prefers short answers", "category": "style", "confidence": 0.9 }
        ],
        "patterns": [
            { "description": "Runs the tests before committing", "frequency": 6 }
        ],
        "workflows": [
            { "description": "Ship small commits", "steps": ["write", "test", "commit"] }
        ]
    }
}"#;

/// The missing answer (`ProfileMissingDto`): the same struct must decode it.
const PROFILE_MISSING_JSON: &str = r#"{
    "exists": false,
    "userId": "user_one",
    "message": "No profile found. Keep chatting to build your profile."
}"#;

#[wasm_bindgen_test]
fn test_get_user_profile_spec_matches_the_pinned_template() {
    let spec = get_user_profile_spec();
    assert_eq!(spec.method, method(EndpointId::UserProfile));
    assert_eq!(spec.path, template(EndpointId::UserProfile));
    assert_eq!(spec.path, "/api/user-profile");
    assert!(spec.query.is_empty());
    assert!(spec.body.is_none());
}

/// One roster row as the `GET /api/user-profiles` projection sends it.
fn roster_row(user_id: &str, prompts: i64, is_default: bool) -> ProfileRosterItem {
    ProfileRosterItem {
        user_id: user_id.to_string(),
        display_name: user_id.to_string(),
        version: 1,
        total_prompts_analyzed: prompts,
        last_analyzed_at: "2026-10-01T00:00:00.000Z".to_string(),
        is_default,
    }
}

#[wasm_bindgen_test]
fn test_profile_roster_decodes_the_camel_case_projection() {
    let raw = r#"[{
        "userId": "vaz@siamstr.com",
        "displayName": "Vaz",
        "version": 7,
        "totalPromptsAnalyzed": 1430,
        "lastAnalyzedAt": "2026-10-01T00:00:00.000Z",
        "isDefault": true
    }]"#;
    let roster: Vec<ProfileRosterItem> =
        serde_json::from_str(raw).expect("the roster projection decodes");
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].user_id, "vaz@siamstr.com");
    assert_eq!(roster[0].total_prompts_analyzed, 1430);
    assert!(roster[0].is_default);
}

#[wasm_bindgen_test]
fn test_profile_roster_spec_reads_the_pinned_template() {
    let spec = list_user_profiles_spec();
    assert_eq!(spec.method, "GET");
    assert_eq!(spec.path, template(EndpointId::ListUserProfiles));
    assert_eq!(spec.path, "/api/user-profiles");
    assert!(spec.query.is_empty());
}

#[wasm_bindgen_test]
fn test_profile_spec_with_user_id_encodes_the_query() {
    // Mutant: the identity interpolated into the PATH (a second identity in a
    // path segment), or the query left unencoded (`?userId=vaz@siamstr.com`).
    let spec = get_user_profile_spec_for("vaz@siamstr.com");
    assert_eq!(spec.method, "GET");
    assert_eq!(spec.path, "/api/user-profile");
    assert_eq!(
        spec.query,
        vec![("userId".to_string(), "vaz@siamstr.com".to_string())]
    );
    assert_eq!(
        build_url(&spec).expect("the spec builds a URL"),
        "/api/user-profile?userId=vaz%40siamstr.com"
    );
}

#[wasm_bindgen_test]
fn test_profile_identity_control_hides_the_picker_for_one_matching_identity() {
    // Mutant: `Picker` whenever the roster is non-empty.
    let roster = vec![roster_row("vaz@siamstr.com", 1430, true)];
    assert_eq!(
        profile_identity_control(&roster, "vaz@siamstr.com"),
        IdentityControl::Hidden
    );
}

#[wasm_bindgen_test]
fn test_profile_identity_control_shows_the_picker_when_the_default_is_absent() {
    // Mutant: `Picker` only when `roster.len() > 1` — here the roster holds one
    // row, but it is NOT the composed identity, so a picker is required and
    // nothing may be auto-selected.
    let roster = vec![roster_row("other@example.com", 3, false)];
    let IdentityControl::Picker { options, selected } =
        profile_identity_control(&roster, "vaz@siamstr.com")
    else {
        panic!("a roster without the composed identity needs a picker");
    };
    assert_eq!(options.len(), 1);
    assert_eq!(selected, None, "never auto-select the only other identity");
}

#[wasm_bindgen_test]
fn test_profile_identity_control_selects_the_default_among_several() {
    let roster = vec![
        roster_row("a@example.com", 9, false),
        roster_row("vaz@siamstr.com", 3, true),
    ];
    let IdentityControl::Picker { selected, .. } =
        profile_identity_control(&roster, "vaz@siamstr.com")
    else {
        panic!("two identities need a picker");
    };
    assert_eq!(selected.as_deref(), Some("vaz@siamstr.com"));
}

#[wasm_bindgen_test]
fn test_profile_view_deserializes_present_and_missing() {
    let present: ProfileView =
        serde_json::from_str(PROFILE_PRESENT_JSON).expect("the present answer decodes");
    assert!(present.exists);
    assert_eq!(present.display_name.as_deref(), Some("Vaz"));
    assert_eq!(present.version, Some(4));
    assert_eq!(present.total_prompts_analyzed, Some(12));
    let data = present.profile_data.as_ref().expect("profileData present");
    assert_eq!(data.preferences.len(), 1);
    assert_eq!(data.patterns.len(), 1);
    assert_eq!(data.workflows.len(), 1);

    let missing: ProfileView =
        serde_json::from_str(PROFILE_MISSING_JSON).expect("the missing answer decodes");
    assert!(!missing.exists);
    assert_eq!(missing.user_id.as_deref(), Some("user_one"));
    assert_eq!(
        missing.message.as_deref(),
        Some("No profile found. Keep chatting to build your profile.")
    );
    assert!(missing.profile_data.is_none());
}

#[wasm_bindgen_test]
fn test_profile_view_missing_branch() {
    let present: ProfileView =
        serde_json::from_str(PROFILE_PRESENT_JSON).expect("the present answer decodes");
    match profile_view(&present) {
        ProfileScreen::Loaded { display_name, .. } => assert_eq!(display_name, "Vaz"),
        other => panic!("exists:true renders the loaded branch, got {other:?}"),
    }

    let missing: ProfileView =
        serde_json::from_str(PROFILE_MISSING_JSON).expect("the missing answer decodes");
    let ProfileScreen::Missing { user_id, message } = profile_view(&missing) else {
        panic!("exists:false renders the missing branch");
    };
    assert_eq!(user_id, "user_one");
    assert_eq!(
        message,
        "No profile found. Keep chatting to build your profile."
    );
}

#[wasm_bindgen_test]
fn test_profile_view_loaded_sections() {
    let present: ProfileView =
        serde_json::from_str(PROFILE_PRESENT_JSON).expect("the present answer decodes");
    let ProfileScreen::Loaded {
        display_name,
        version,
        last_analyzed_at,
        sections,
    } = profile_view(&present)
    else {
        panic!("exists:true renders the loaded branch");
    };
    assert_eq!(display_name, "Vaz");
    assert_eq!(version, 4);
    assert_eq!(last_analyzed_at, "2026-10-01T00:00:00.000Z");
    assert_eq!(sections.preferences.len(), 1);
    assert_eq!(sections.preferences[0].description, "Prefers short answers");
    assert_eq!(sections.patterns.len(), 1);
    assert_eq!(
        sections.patterns[0].description,
        "Runs the tests before committing"
    );
    assert_eq!(sections.workflows.len(), 1);
    assert_eq!(sections.workflows[0].steps, vec!["write", "test", "commit"]);
}

// ---------------------------------------------------------------------------
// Settings (Step 7)
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn test_health_spec_matches_the_pinned_template() {
    let spec = health_spec();
    assert_eq!(spec.method, method(EndpointId::Health));
    assert_eq!(spec.path, template(EndpointId::Health));
    assert_eq!(spec.path, "/api/health");
    assert!(spec.query.is_empty());
    assert!(spec.body.is_none());
}

#[wasm_bindgen_test]
fn test_health_outcome_parses_the_bare_envelope() {
    // `GET /api/health` answers `{"success":true,"status":"ok","authEnabled":…}`
    // with NO `data` wrapper, so the shared `request::<T>` decoder cannot read
    // it — the screen's own bare decoder must.
    let health: HealthStatus = health_outcome(
        200,
        br#"{"success":true,"status":"ok","authEnabled":false}"#,
    )
    .expect("the bare envelope decodes");
    assert_eq!(health.status, "ok");
    assert!(!health.auth_enabled);
    assert!(health.success);

    let enabled = health_outcome(200, br#"{"success":true,"status":"ok","authEnabled":true}"#)
        .expect("the bare envelope decodes");
    assert!(enabled.auth_enabled);

    assert_eq!(health_outcome(200, b"not json"), Err(ApiError::Malformed));
}

#[wasm_bindgen_test]
fn test_stats_view_deserializes_the_counts() {
    let stats: StatsView = serde_json::from_str(
        r#"{"total":3,"byScope":{"user":1,"project":2},"byType":{"note":2,"decision":1}}"#,
    )
    .expect("the stats projection decodes");
    assert_eq!(stats.total, 3);
    assert_eq!(stats.by_scope.user, 1);
    assert_eq!(stats.by_scope.project, 2);
    assert_eq!(stats.by_type.get("note"), Some(&2));
    assert_eq!(stats.by_type.get("decision"), Some(&1));
}

#[wasm_bindgen_test]
fn test_settings_view_loaded_counts() {
    let health = HealthStatus {
        success: true,
        status: "ok".to_string(),
        auth_enabled: false,
    };
    let stats: StatsView = serde_json::from_str(
        r#"{"total":3,"byScope":{"user":1,"project":2},"byType":{"note":2,"decision":1}}"#,
    )
    .expect("the stats projection decodes");

    let SettingsScreen::Loaded {
        status,
        auth_enabled,
        total,
        by_user,
        by_project,
        by_type,
    } = settings_view(&health, &stats)
    else {
        panic!("a decoded pair renders the loaded branch");
    };
    assert_eq!(status, "ok");
    assert!(!auth_enabled);
    assert_eq!(total, 3);
    assert_eq!(by_user, 1);
    assert_eq!(by_project, 2);
    assert_eq!(by_type.get("note"), Some(&2));
}

// The stats spec is pinned here too: it is the second request the Settings
// panel builds, and its template is the same one the pinned table declares.
#[wasm_bindgen_test]
fn test_stats_spec_matches_the_pinned_template() {
    let spec = stats_spec();
    assert_eq!(spec.method, method(EndpointId::Stats));
    assert_eq!(spec.path, template(EndpointId::Stats));
    assert_eq!(spec.path, "/api/stats");
    assert!(spec.query.is_empty());
}

// ---------------------------------------------------------------------------
// Stats screen (this wave) — full-payload decode, legacy defaults, and the
// pure render helpers mirroring upstream `StatsView.svelte`
// ---------------------------------------------------------------------------

/// The new `GET /api/stats` payload, every section present.
const STATS_FULL_JSON: &str = r#"{
    "total": 3,
    "byScope": { "user": 2, "project": 1 },
    "byType": { "context": 3 },
    "generatedAt": "2026-10-24T12:00:00.000Z",
    "shards": [
        {
            "shardId": 2,
            "scope": "user",
            "scopeHash": "aaaaaaaaaaaaaaaa",
            "shardIndex": 0,
            "fileName": "user_aaaaaaaaaaaaaaaa_shard_0.db",
            "fileExists": true,
            "isActive": true,
            "registeredVectorCount": 2,
            "memoryCount": 2,
            "oldestCreatedAt": "2026-10-12T00:00:00.000Z",
            "newestCreatedAt": "2026-10-16T00:00:00.000Z",
            "duplicateTagCount": 1
        },
        {
            "shardId": null,
            "scope": "project",
            "scopeHash": "bbbbbbbbbbbbbbbb",
            "shardIndex": 1,
            "fileName": "project_missing.db",
            "fileExists": false,
            "isActive": false,
            "registeredVectorCount": 4,
            "memoryCount": 0,
            "oldestCreatedAt": null,
            "newestCreatedAt": null,
            "duplicateTagCount": 0,
            "error": "Shard DB file missing: project_missing.db"
        }
    ],
    "profile": {
        "exists": true,
        "version": 3,
        "counts": { "preferences": 2, "patterns": 1, "workflows": 0 },
        "lastAnalyzedAt": "2023-11-14T22:13:20.000Z",
        "totalPromptsAnalyzed": 12
    },
    "daily": [
        {
            "date": "2026-10-23",
            "captures": 1,
            "captureSkipped": 0,
            "profileInjections": 2,
            "compactionInjections": 0,
            "errors": { "listMemories": 1 }
        },
        {
            "date": "2026-10-24",
            "captures": 0,
            "captureSkipped": 0,
            "profileInjections": 0,
            "compactionInjections": 0,
            "errors": {}
        }
    ],
    "dailyError": null
}"#;

#[wasm_bindgen_test]
fn test_stats_view_decodes_the_full_new_payload() {
    let stats: StatsView = serde_json::from_str(STATS_FULL_JSON).expect("the full payload decodes");
    assert_eq!(stats.total, 3);
    assert_eq!(stats.by_scope.user, 2);
    assert_eq!(stats.by_scope.project, 1);
    assert_eq!(stats.by_type.get("context"), Some(&3));
    assert_eq!(stats.generated_at, "2026-10-24T12:00:00.000Z");

    let shards = &stats.shards;
    assert_eq!(shards.len(), 2);
    let user = &shards[0];
    assert_eq!(user.shard_id, Some(2));
    assert_eq!(user.scope, "user");
    assert_eq!(user.scope_hash, "aaaaaaaaaaaaaaaa");
    assert_eq!(user.shard_index, 0);
    assert_eq!(user.file_name, "user_aaaaaaaaaaaaaaaa_shard_0.db");
    assert!(user.file_exists);
    assert!(user.is_active);
    assert_eq!(user.registered_vector_count, 2);
    assert_eq!(user.memory_count, 2);
    assert_eq!(
        user.oldest_created_at.as_deref(),
        Some("2026-10-12T00:00:00.000Z")
    );
    assert_eq!(
        user.newest_created_at.as_deref(),
        Some("2026-10-16T00:00:00.000Z")
    );
    assert_eq!(user.duplicate_tag_count, 1);
    assert!(user.error.is_none());

    let project = &shards[1];
    assert_eq!(project.shard_id, None, "shardId null decodes to None");
    assert!(!project.file_exists);
    assert!(!project.is_active);
    assert_eq!(
        project.error.as_deref(),
        Some("Shard DB file missing: project_missing.db")
    );

    let profile = stats.profile.as_ref().expect("the profile section decodes");
    assert!(profile.exists);
    assert_eq!(profile.version, Some(3));
    let counts = profile.counts.as_ref().expect("counts present");
    assert_eq!(
        (counts.preferences, counts.patterns, counts.workflows),
        (2, 1, 0)
    );
    assert_eq!(
        profile.last_analyzed_at.as_deref(),
        Some("2023-11-14T22:13:20.000Z")
    );
    assert_eq!(profile.total_prompts_analyzed, Some(12));

    assert_eq!(stats.daily.len(), 2);
    let day = &stats.daily[0];
    assert_eq!(day.date, "2026-10-23");
    assert_eq!(day.captures, 1);
    assert_eq!(day.capture_skipped, 0);
    assert_eq!(day.profile_injections, 2);
    assert_eq!(day.compaction_injections, 0);
    assert_eq!(day.errors.get("listMemories"), Some(&1));
    assert!(
        stats.daily_error.is_none(),
        "dailyError null decodes to None"
    );
}

#[wasm_bindgen_test]
fn test_stats_view_defaults_a_legacy_payload() {
    // Older responses can omit the diagnostic sections; every new field
    // defaults instead of failing the decode (upstream marks them optional).
    let stats: StatsView = serde_json::from_str(
        r#"{"total":3,"byScope":{"user":1,"project":2},"byType":{"note":2},"generatedAt":"2026-01-01T00:00:00.000Z"}"#,
    )
    .expect("the legacy payload decodes");
    assert_eq!(stats.total, 3);
    assert_eq!(stats.by_scope.user, 1);
    assert_eq!(stats.by_scope.project, 2);
    assert_eq!(stats.by_type.get("note"), Some(&2));
    assert!(stats.shards.is_empty());
    assert!(stats.profile.is_none());
    assert!(stats.daily.is_empty());
    assert!(stats.daily_error.is_none());
}

fn shard(scope: &str, memory_count: u64, shard_index: u32, is_active: bool) -> ShardStatsView {
    ShardStatsView {
        scope: scope.to_string(),
        memory_count,
        shard_index,
        is_active,
        ..ShardStatsView::default()
    }
}

#[wasm_bindgen_test]
fn test_sorted_shards_orders_by_scope_then_memory_count_desc_then_index() {
    // Upstream `StatsView.svelte` L43–50.
    let shards = vec![
        shard("user", 0, 3, true),
        shard("project", 5, 1, true),
        shard("project", 9, 0, true),
        shard("user", 4, 0, false),
        shard("project", 5, 0, false),
        shard("project", 0, 3, false),
    ];
    let sorted = sorted_shards(&shards);
    let keys: Vec<(&str, u64, u32)> = sorted
        .iter()
        .map(|s| (s.scope.as_str(), s.memory_count, s.shard_index))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("project", 9, 0),
            ("project", 5, 0),
            ("project", 5, 1),
            ("project", 0, 3),
            ("user", 4, 0),
            ("user", 0, 3),
        ],
        "scope asc, memoryCount desc, shardIndex asc"
    );
}

fn day(
    captures: u64,
    capture_skipped: u64,
    profile_injections: u64,
    compaction_injections: u64,
) -> DailyStatsView {
    DailyStatsView {
        date: "2026-10-24".to_string(),
        captures,
        capture_skipped,
        profile_injections,
        compaction_injections,
        errors: Default::default(),
    }
}

#[wasm_bindgen_test]
fn test_daily_peak_is_the_max_counter_or_one() {
    // Upstream L51–53: `Math.max(1, ...counters)`.
    assert_eq!(daily_peak(&[]), 1, "an empty series still divides");
    assert_eq!(
        daily_peak(&[day(0, 0, 0, 0)]),
        1,
        "an all-zero series peaks at 1"
    );
    assert_eq!(daily_peak(&[day(1, 2, 3, 4)]), 4);
    assert_eq!(
        daily_peak(&[day(1, 2, 3, 4), day(0, 0, 9, 0)]),
        9,
        "the peak spans every counter of every day"
    );
}

#[wasm_bindgen_test]
fn test_error_buckets_filters_zero_and_sorts_keys() {
    // Upstream L55–59: count > 0, sorted by bucket name.
    let mut day_row = day(0, 0, 0, 0);
    day_row.errors.insert("beta".to_string(), 2);
    day_row.errors.insert("alpha".to_string(), 0);
    day_row.errors.insert("gamma".to_string(), 1);
    assert_eq!(
        error_buckets(&day_row),
        vec![("beta".to_string(), 2), ("gamma".to_string(), 1),],
        "zero counts drop, keys sort ascending"
    );
    assert!(error_buckets(&day(0, 0, 0, 0)).is_empty());
}

#[wasm_bindgen_test]
fn test_active_shard_count_counts_is_active_flags() {
    // Upstream L105: the active-shards card counts `isActive` only.
    assert_eq!(active_shard_count(&[]), 0);
    assert_eq!(
        active_shard_count(&[
            shard("user", 1, 0, true),
            shard("project", 1, 0, false),
            shard("project", 1, 1, true),
        ]),
        2
    );
}

// ---------------------------------------------------------------------------
// Maintenance (issue 42 / W8.4)
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn test_cleanup_spec_is_a_bodyless_post_to_the_pinned_template() {
    // `POST /api/cleanup` is already pinned in `ENDPOINTS`; the screen-local
    // spec must not re-type the path (no path-table change in this wave).
    let spec = cleanup_spec();
    assert_eq!(spec.method, method(EndpointId::Cleanup));
    assert_eq!(spec.method, "POST");
    assert_eq!(spec.path, template(EndpointId::Cleanup));
    assert_eq!(spec.path, "/api/cleanup");
    assert!(spec.query.is_empty());
    assert!(spec.body.is_none());
}

/// A report with distinct, memorable counts.
fn cleanup_report() -> CleanupReport {
    CleanupReport {
        deleted_count: 11,
        user_count: 3,
        project_count: 8,
        prompts_deleted: 5,
        protected_memories_skipped: 2,
    }
}

#[wasm_bindgen_test]
fn test_cleanup_next_never_runs_without_a_confirmed_arm() {
    // The mount / first-click proof (D-1): a freshly mounted section is
    // `CleanupState::default()`, and no single event — nor any sequence without
    // an explicit Confirm from an armed state — may set `run`.
    let start = CleanupState::default();
    for event in [
        CleanupEvent::Action(CleanupAction::Arm),
        CleanupEvent::Action(CleanupAction::Cancel),
        CleanupEvent::Action(CleanupAction::Confirm),
    ] {
        let step = cleanup_next(&start, event);
        assert!(!step.run, "no unarmed action may start the cleanup");
    }

    let armed_once = cleanup_next(&start, CleanupEvent::Action(CleanupAction::Arm));
    assert!(!armed_once.run);
    assert!(armed_once.state.armed);

    let armed_twice = cleanup_next(&armed_once.state, CleanupEvent::Action(CleanupAction::Arm));
    assert!(!armed_twice.run, "arming twice still does not run");
    assert!(armed_twice.state.armed);

    let cancelled = cleanup_next(
        &armed_once.state,
        CleanupEvent::Action(CleanupAction::Cancel),
    );
    assert!(!cancelled.run);
    assert!(!cancelled.state.armed);

    let finished = cleanup_next(
        &armed_once.state,
        CleanupEvent::Finished(CleanupOutcome::Unavailable),
    );
    assert!(!finished.run, "finishing a run never starts another one");

    let confirmed_after_finish = cleanup_next(
        &finished.state,
        CleanupEvent::Action(CleanupAction::Confirm),
    );
    assert!(
        !confirmed_after_finish.run,
        "a finished run must be re-armed before the next cleanup"
    );
}

#[wasm_bindgen_test]
fn test_cleanup_next_runs_once_on_the_confirmed_action() {
    let armed = cleanup_next(
        &CleanupState::default(),
        CleanupEvent::Action(CleanupAction::Arm),
    );
    assert!(!armed.run);
    assert_eq!(
        armed.state,
        CleanupState {
            armed: true,
            running: false,
            last: None,
        }
    );

    let confirmed = cleanup_next(&armed.state, CleanupEvent::Action(CleanupAction::Confirm));
    assert!(confirmed.run, "the confirmed action is the one that runs");
    assert_eq!(
        confirmed.state,
        CleanupState {
            armed: false,
            running: true,
            last: None,
        },
        "confirming disarms and enters the in-flight state"
    );
}

#[wasm_bindgen_test]
fn test_cleanup_next_refuses_a_second_run_while_in_flight() {
    let in_flight = CleanupState {
        armed: false,
        running: true,
        last: None,
    };
    for event in [
        CleanupEvent::Action(CleanupAction::Arm),
        CleanupEvent::Action(CleanupAction::Cancel),
        CleanupEvent::Action(CleanupAction::Confirm),
    ] {
        let step: CleanupStep = cleanup_next(&in_flight, event);
        assert!(
            !step.run,
            "a queued or double click cannot start a second server run"
        );
        assert_eq!(step.state, in_flight, "the in-flight state is unchanged");
    }
}

#[wasm_bindgen_test]
fn test_cleanup_next_re_enables_after_every_outcome() {
    let in_flight = CleanupState {
        armed: false,
        running: true,
        last: None,
    };
    let outcomes = [
        CleanupOutcome::Ran(cleanup_report()),
        CleanupOutcome::Refused("cleanup is already running".to_string()),
        CleanupOutcome::Unavailable,
        CleanupOutcome::Failed("malformed request or response body".to_string()),
    ];
    for outcome in outcomes {
        let step = cleanup_next(&in_flight, CleanupEvent::Finished(outcome.clone()));
        assert!(!step.run);
        assert!(!step.state.running, "every outcome re-enables the action");
        assert!(!step.state.armed, "every outcome drops the arm");
        assert_eq!(step.state.last, Some(outcome));
        assert!(!cleanup_disabled(&step.state));
    }
}

#[wasm_bindgen_test]
fn test_cleanup_action_label_and_status_show_the_armed_step() {
    let idle = CleanupState::default();
    let armed = CleanupState {
        armed: true,
        running: false,
        last: None,
    };
    let running = CleanupState {
        armed: false,
        running: true,
        last: None,
    };

    assert_eq!(cleanup_status(&idle), CleanupStatus::Idle);
    assert_eq!(cleanup_status(&armed), CleanupStatus::Armed);
    assert_eq!(cleanup_status(&running), CleanupStatus::Running);

    assert_eq!(cleanup_action_label(cleanup_status(&idle)), "Run cleanup…");
    assert_eq!(
        cleanup_action_label(cleanup_status(&armed)),
        "Confirm cleanup"
    );
    assert_eq!(
        cleanup_action_label(cleanup_status(&running)),
        "Cleaning up…"
    );

    // The armed step must be readable, not just a state flag.
    let labels = [
        cleanup_action_label(CleanupStatus::Idle),
        cleanup_action_label(CleanupStatus::Armed),
        cleanup_action_label(CleanupStatus::Running),
    ];
    assert_ne!(labels[0], labels[1]);
    assert_ne!(labels[1], labels[2]);
    assert_ne!(labels[0], labels[2]);
}

#[wasm_bindgen_test]
fn test_cleanup_disabled_tracks_the_in_flight_state() {
    assert!(!cleanup_disabled(&CleanupState::default()));
    assert!(
        !cleanup_disabled(&CleanupState {
            armed: true,
            running: false,
            last: None,
        }),
        "arming alone must not disable the confirmation button"
    );
    assert!(cleanup_disabled(&CleanupState {
        armed: false,
        running: true,
        last: None,
    }));

    for outcome in [
        CleanupOutcome::Ran(cleanup_report()),
        CleanupOutcome::Failed("transport failure".to_string()),
    ] {
        let finished = cleanup_next(
            &CleanupState {
                armed: false,
                running: true,
                last: None,
            },
            CleanupEvent::Finished(outcome),
        );
        assert!(
            !cleanup_disabled(&finished.state),
            "the action re-enables after success and after failure"
        );
    }
}

#[wasm_bindgen_test]
fn test_cleanup_outcome_reads_all_five_report_fields() {
    // Distinct values, whole-struct equality: a field mapped to the wrong slot
    // (or a swapped pair) fails here.
    let body = br#"{"success":true,"data":{"deletedCount":11,"userCount":3,"projectCount":8,"promptsDeleted":5,"protectedMemoriesSkipped":2}}"#;
    assert_eq!(
        cleanup_outcome(200, body),
        CleanupOutcome::Ran(cleanup_report())
    );
}

#[wasm_bindgen_test]
fn test_cleanup_outcome_501_is_unavailable_not_a_failure() {
    // (a) the real degraded-server answer: 501 + the documented stub body.
    let stub = br#"{"success":false,"error":"cleanup is not available in this build"}"#;
    assert_eq!(cleanup_outcome(501, stub), CleanupOutcome::Unavailable);

    // (b) status is classified BEFORE the body (D-2): a 501 that is not JSON
    // (proxy page, truncated response) is still the unavailable state, never a
    // decode failure.
    assert_eq!(
        cleanup_outcome(501, b"not json"),
        CleanupOutcome::Unavailable
    );

    // (c) a `200 success:false` business refusal is not the unavailable state.
    let refused = cleanup_outcome(
        200,
        br#"{"success":false,"error":"cleanup is already running"}"#,
    );
    assert_eq!(
        refused,
        CleanupOutcome::Refused("cleanup is already running".to_string())
    );
    assert_ne!(refused, CleanupOutcome::Unavailable);
}

#[wasm_bindgen_test]
fn test_cleanup_outcome_business_refusal_keeps_the_server_message() {
    assert_eq!(
        cleanup_outcome(
            200,
            br#"{"success":false,"error":"retention window is not configured"}"#
        ),
        CleanupOutcome::Refused("retention window is not configured".to_string())
    );

    // The `message` field is the fallback when `error` is absent.
    assert_eq!(
        cleanup_outcome(200, br#"{"success":false,"message":"cleanup is disabled"}"#),
        CleanupOutcome::Refused("cleanup is disabled".to_string())
    );

    // The server message is bounded to exactly 200 characters.
    let long = "x".repeat(300);
    let body = format!(r#"{{"success":false,"error":"{long}"}}"#);
    assert_eq!(
        cleanup_outcome(200, body.as_bytes()),
        CleanupOutcome::Refused("x".repeat(200))
    );
}

#[wasm_bindgen_test]
fn test_cleanup_outcome_malformed_and_partial_reports_are_failures() {
    // A missing required field is a malformed body, never a zero (D-3).
    let partial = br#"{"success":true,"data":{"deletedCount":11,"userCount":3,"projectCount":8,"promptsDeleted":5}}"#;
    assert_eq!(
        cleanup_outcome(200, partial),
        CleanupOutcome::Failed("malformed request or response body".to_string())
    );

    // A `null` field is equally a malformed body — no `#[serde(default)]`.
    let nulled = br#"{"success":true,"data":{"deletedCount":11,"userCount":3,"projectCount":8,"promptsDeleted":5,"protectedMemoriesSkipped":null}}"#;
    assert_eq!(
        cleanup_outcome(200, nulled),
        CleanupOutcome::Failed("malformed request or response body".to_string())
    );

    // A non-JSON 200 is a failure, and no body byte is surfaced.
    assert_eq!(
        cleanup_outcome(200, b"<html>"),
        CleanupOutcome::Failed("malformed request or response body".to_string())
    );

    // Transport/auth failures use the existing health/stats wording.
    assert_eq!(
        cleanup_outcome(500, br#"{"success":false,"error":"boom"}"#),
        CleanupOutcome::Failed("server error 500: boom".to_string())
    );
    assert_eq!(
        cleanup_outcome(401, br#"{"success":false,"error":"unauthorized"}"#),
        CleanupOutcome::Failed("server error 401: unauthorized".to_string())
    );
}

#[wasm_bindgen_test]
fn test_cleanup_report_rows_carry_human_labels() {
    let rows: [CleanupReportRow; 5] = cleanup_report_rows(&cleanup_report());
    assert_eq!(rows.len(), 5);
    let triples: Vec<(&str, &str, u64)> = rows
        .iter()
        .map(|row| (row.testid, row.label, row.count))
        .collect();
    assert_eq!(
        triples,
        vec![
            ("maintenance-deleted", "Memories deleted", 11),
            ("maintenance-user", "User memories deleted", 3),
            ("maintenance-project", "Project memories deleted", 8),
            ("maintenance-prompts", "Prompts deleted", 5),
            ("maintenance-protected", "Memories spared", 2),
        ],
        "issue-39 field order, with human labels and no UI arithmetic"
    );
}

#[wasm_bindgen_test]
fn test_cleanup_protected_row_explains_the_skip() {
    let rows = cleanup_report_rows(&cleanup_report());
    let protected = &rows[4];
    assert_eq!(protected.testid, "maintenance-protected");
    assert_eq!(protected.label, "Memories spared");
    assert_eq!(
        protected.hint,
        Some("Spared because a linked prompt expired recently."),
        "the skip count carries its explanation (AC7)"
    );
    for row in &rows[..4] {
        assert_eq!(row.hint, None, "only the spared row needs a hint");
    }
}

#[wasm_bindgen_test]
fn test_cleanup_unavailable_view_is_distinct_from_the_failure_view() {
    assert_eq!(cleanup_outcome_view(None), CleanupOutcomeView::Nothing);

    let unavailable = cleanup_outcome_view(Some(&CleanupOutcome::Unavailable));
    assert_eq!(
        unavailable,
        CleanupOutcomeView::Unavailable(CLEANUP_UNAVAILABLE_MESSAGE)
    );
    assert_eq!(
        CLEANUP_UNAVAILABLE_MESSAGE,
        "Cleanup is not available in this build."
    );

    let report = cleanup_report();
    assert_eq!(
        cleanup_outcome_view(Some(&CleanupOutcome::Ran(report.clone()))),
        CleanupOutcomeView::Report(cleanup_report_rows(&report))
    );

    let refused = CleanupOutcome::Refused("cleanup is already running".to_string());
    assert_eq!(
        cleanup_outcome_view(Some(&refused)),
        CleanupOutcomeView::Refused("cleanup is already running".to_string())
    );

    let failed = CleanupOutcome::Failed("server error 500: boom".to_string());
    assert_eq!(
        cleanup_outcome_view(Some(&failed)),
        CleanupOutcomeView::Failed("server error 500: boom".to_string())
    );

    // The rendered hooks must be non-empty and distinct WITHIN each family, so
    // the unavailable state and the failure state cannot collide in the DOM.
    // (A testid equals its class string by design: that is the same hook read
    // by the DOM and by the stylesheet, so the two families are never compared
    // with each other.)
    let testids = [
        CLEANUP_REPORT_TESTID,
        CLEANUP_REFUSED_TESTID,
        CLEANUP_UNAVAILABLE_TESTID,
        CLEANUP_ERROR_TESTID,
        CLEANUP_ARMED_TESTID,
    ];
    let classes = [
        CLEANUP_REPORT_CLASS,
        CLEANUP_REFUSED_CLASS,
        CLEANUP_UNAVAILABLE_CLASS,
        CLEANUP_ERROR_CLASS,
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
    assert_ne!(CLEANUP_UNAVAILABLE_CLASS, CLEANUP_ERROR_CLASS);
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[wasm_bindgen_test]
fn test_initial_tags_percent_decodes() {
    assert_eq!(
        initial_tags("?tags=architecture%20docs"),
        strings(&["architecture docs"])
    );
    assert_eq!(
        initial_tags("?tags=rust&tags=ops%2Ddrill"),
        strings(&["rust", "ops-drill"])
    );
    assert_eq!(initial_tags("?tags=a&tags=a"), strings(&["a"]));
    assert_eq!(
        initial_tags("?tags=&other=1&tags=%20"),
        Vec::<String>::new()
    );
    assert_eq!(initial_tags(""), Vec::<String>::new());
}

#[wasm_bindgen_test]
fn test_memories_request_picks_one_mode() {
    let spec = memories_request("", &strings(&["rust", "architecture"]), 1, 20);
    assert_eq!(
        build_url(&spec).expect("builds"),
        "/api/memories?page=1&pageSize=20&tags=rust&tags=architecture&includePrompts=false"
    );
    let spec = memories_request("deploy", &strings(&["rust"]), 1, 20);
    assert_eq!(
        build_url(&spec).expect("builds"),
        "/api/memories?page=1&pageSize=20&tags=rust&includePrompts=false"
    );
    let spec = memories_request(" deploy ", &[], 2, 20);
    assert_eq!(
        build_url(&spec).expect("builds"),
        "/api/search?q=deploy&page=2&pageSize=20"
    );
    let spec = memories_request("  ", &[], 1, 20);
    assert_eq!(
        build_url(&spec).expect("builds"),
        "/api/memories?page=1&pageSize=20&includePrompts=false"
    );
}

#[wasm_bindgen_test]
fn test_request_fixture_matches_ui() {
    let spec = memories_request("", &strings(&["probe"]), 1, 20);
    assert_eq!(
        build_url(&spec).expect("builds"),
        include_str!("../../crates/mem-web/tests/fixtures/a1_tags_request.txt").trim()
    );
}

#[wasm_bindgen_test]
fn test_range_text_is_one_based() {
    // Blueprint r4 printed (191,20,3827) -> "3801-3827"; page 191 ends at 3820.
    assert_eq!(range_text(191, 20, 3827), "3801-3820 of 3827");
    assert_eq!(range_text(192, 20, 3827), "3821-3827 of 3827");
    assert_eq!(range_text(1, 20, 3827), "1-20 of 3827");
    assert_eq!(range_text(1, 20, 0), "0 of 0");
}

#[wasm_bindgen_test]
fn test_memory_preview_is_a_lossless_split() {
    let body: String = "héllo wörld ✓ ".repeat(473); // 6622 chars, multi-byte
    assert!(body.chars().count() > 6600);
    let (preview, rest) = memory_preview(&body);
    let rest = rest.expect("a long body splits");
    assert_eq!(format!("{preview}{rest}"), body);
    assert!(
        preview.chars().count() <= 700,
        "{}",
        preview.chars().count()
    );
    assert!(
        preview.chars().count() > 600,
        "word-boundary cut stays near the limit"
    );
    let (short, none) = memory_preview("short");
    assert_eq!((short.as_str(), none), ("short", None));
    // No whitespace at all: a hard cut on a char boundary.
    let (hard, rest) = memory_preview(&"é".repeat(800));
    assert_eq!(hard.chars().count(), 700);
    assert_eq!(rest.map(|r| r.chars().count()), Some(100));
}

#[wasm_bindgen_test]
fn test_visible_tags_caps_at_four() {
    let ten: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
    let (shown, more) = visible_tags(&ten);
    assert_eq!((shown.len(), more), (4, 6));
    let (shown, more) = visible_tags(&ten[..3]);
    assert_eq!((shown.len(), more), (3, 0));
}

#[wasm_bindgen_test]
fn test_tags_json_carries_every_tag() {
    let ten: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
    let back: Vec<String> = serde_json::from_str(&tags_json(&ten)).expect("valid JSON");
    assert_eq!(back, ten);
    let odd = strings(&["quote\"tag", "back\\slash"]);
    let back: Vec<String> = serde_json::from_str(&tags_json(&odd)).expect("valid JSON");
    assert_eq!(back, odd);
    assert_eq!(tags_json(&[]), "[]");
}

#[wasm_bindgen_test]
fn test_tags_page_carries_ranked_memory_tags() {
    let page: TagsPage = serde_json::from_str(
        r#"{"project":[],"user":[],"memoryTags":[{"tag":"rust","count":3},{"tag":"Architecture Docs","count":2}],"memoryTagTotal":4}"#,
    )
    .expect("decodes");
    assert_eq!(page.memory_tag_total, 4);
    let TagsView::Loaded {
        memory_tags,
        memory_tag_total,
        ..
    } = tags_view(&page)
    else {
        panic!("memory tags alone render the loaded branch");
    };
    assert_eq!(memory_tag_total, 4);
    assert_eq!(memory_tags[1].tag, "Architecture Docs");
    assert_eq!(memory_tags[0].count, 3);
    // An older server without the keys still decodes.
    let old: TagsPage = serde_json::from_str(r#"{"project":[],"user":[]}"#).expect("decodes");
    assert_eq!(tags_view(&old), TagsView::Empty);
}

#[wasm_bindgen_test]
fn test_tags_link_round_trips_a_space_bearing_tag() {
    let tag = "Architecture Docs & more";
    let href = format!(
        "/project-memories?tags={}",
        web_ui::paths::encode_component(tag)
    );
    let search = href
        .split_once('?')
        .map(|(_, q)| format!("?{q}"))
        .unwrap_or_default();
    assert_eq!(initial_tags(&search), vec![tag.to_string()]);
}
