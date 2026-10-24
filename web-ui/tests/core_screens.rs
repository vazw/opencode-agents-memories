//! W6.7 — the four core screens' pure oracles (AC1 / AC3).
//!
//! Everything asserted here is pure (no `fetch`, no DOM): the request specs are
//! compared against the pinned [`ENDPOINTS`] templates and the shared URL
//! builder, the DTOs are decoded from the exact `mem-web` projections, and the
//! render shapes pin the empty / loaded / missing branches. The live path is the
//! browser smoke, recorded in the wave handoff.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::api::envelope::ApiError;
use web_ui::api_request::build_url;
use web_ui::core_screens::{
    DailyStatsView, HealthStatus, MemoriesView, MemoryPage, ProfileScreen, ProfileView,
    SettingsScreen, ShardStatsView, StatsView, TagsPage, TagsView, active_shard_count, daily_peak,
    delete_memory_spec, error_buckets, get_user_profile_spec, health_outcome, health_spec,
    list_memories_spec, list_tags_spec, memories_view, pin_memory_spec, profile_view, search_spec,
    settings_view, sorted_shards, stats_spec, tags_view, unpin_memory_spec,
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
            ("tag".to_string(), "alpha".to_string()),
            ("tag".to_string(), "beta gamma".to_string()),
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
    let spec = search_spec("rust async", 1, 10, &["alpha"]);
    assert_eq!(spec.method, method(EndpointId::Search));
    assert_eq!(spec.path, template(EndpointId::Search));
    assert_eq!(
        spec.query,
        vec![
            ("q".to_string(), "rust async".to_string()),
            ("page".to_string(), "1".to_string()),
            ("pageSize".to_string(), "10".to_string()),
            ("tag".to_string(), "alpha".to_string()),
        ]
    );
    assert_eq!(
        build_url(&spec).expect("the spec builds a root-relative URL"),
        "/api/search?q=rust%20async&page=1&pageSize=10&tag=alpha",
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
    };
    assert_eq!(tags_view(&empty), TagsView::Empty);
}

#[wasm_bindgen_test]
fn test_tags_view_loaded_groups() {
    let page: TagsPage = serde_json::from_str(TAGS_JSON).expect("the tags projection decodes");
    let TagsView::Loaded { project, user } = tags_view(&page) else {
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
