//! Issue 70 wave 1 — the nine memory-tool wire methods (T01-T18).
//!
//! Every oracle drives the production `Engine::dispatch`, never a handler
//! directly: the session gate, the membership test and the degrade path are
//! part of the contract this file pins.

mod common;

use mem_bridge::engine::{Engine, EngineConfig};
use mem_bridge::jsonrpc::{Response, parse_line};
use serde_json::{Value, json};

/// The nine runtime methods issue 70 adds, in advertised order.
const NINE: [&str; 9] = [
    "add",
    "search",
    "forget",
    "list",
    "profile",
    "list-shards",
    "migrate",
    "export",
    "import",
];

fn engine() -> Engine {
    Engine::new(EngineConfig::default())
}

async fn dispatch_value(engine: &Engine, value: Value) -> Response {
    let request = parse_line(&value.to_string()).expect("request line parses");
    engine.dispatch(&request).await
}

async fn dispatch(engine: &Engine, method: &str, params: Value) -> Response {
    dispatch_value(
        engine,
        json!({"id": "t", "jsonrpc": "2.0", "method": method, "params": params}),
    )
    .await
}

fn error_code(response: &Response) -> i32 {
    response
        .error
        .as_ref()
        .map(|error| error.code)
        .expect("response carries an error member")
}

fn data(response: &Response) -> &Value {
    response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref())
        .expect("error carries advisory data")
}

fn result(response: &Response) -> &Value {
    response.result.as_ref().expect("response carries a result")
}

// ---------------------------------------------------------------------------
// T01 — the hello contract: fifteen methods, fourteen capabilities
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hello_lists_the_fifteen_methods_and_fourteen_capabilities() {
    let response = dispatch(&engine(), "hello", json!({"client": "probe"})).await;
    let result = result(&response);

    let methods: Vec<&str> = result["methods"]
        .as_array()
        .expect("methods is an array")
        .iter()
        .map(|value| value.as_str().expect("method is a string"))
        .collect();
    assert_eq!(
        methods,
        [
            "capture",
            "inject",
            "recall",
            "skills",
            "health",
            "shutdown",
            "add",
            "search",
            "forget",
            "list",
            "profile",
            "list-shards",
            "migrate",
            "export",
            "import"
        ],
        "the advertised order is the contract"
    );

    let capabilities = result["capabilities"]
        .as_object()
        .expect("capabilities is an object");
    assert_eq!(capabilities.len(), 14, "fourteen capability keys");
    for key in [
        "compaction",
        "fire_and_forget_capture",
        "skills",
        "standalone",
        "web",
        "add",
        "search",
        "forget",
        "list",
        "profile",
        "list-shards",
        "migrate",
        "export",
        "import",
    ] {
        assert_eq!(
            capabilities.get(key),
            Some(&Value::Bool(true)),
            "capability {key} must be the boolean true"
        );
    }
}

// ---------------------------------------------------------------------------
// T02 — the nine are session-gated exactly like the four W5.3 runtime methods
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_nine_tool_methods_require_a_session_id() {
    for method in NINE {
        let response = dispatch(&engine(), method, json!({})).await;
        assert_eq!(
            error_code(&response),
            -32602,
            "{method} without a session id is invalid params"
        );
        assert_eq!(
            data(&response).get("violation").and_then(Value::as_str),
            Some("missing"),
            "{method} answers the session-id violation shape"
        );
    }
}

// ---------------------------------------------------------------------------
// T03 — without tool services the nine degrade (-32001), never -32603
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_nine_tool_methods_degrade_without_tool_services() {
    for method in NINE {
        let response = dispatch(
            &engine(),
            method,
            json!({"session_id": "probe-session", "project_id": "/probe"}),
        )
        .await;
        assert_eq!(
            error_code(&response),
            -32001,
            "{method} must degrade without a runtime, not fail internally"
        );
        assert_eq!(
            response.error.as_ref().map(|error| error.message.as_str()),
            Some("MEM_DEGRADED")
        );
    }
}

// ---------------------------------------------------------------------------
// Issue 70 helpers: the real wire fixture
// ---------------------------------------------------------------------------

use std::sync::Arc;

use common::{FakeEmbedder, MapScopeResolver, TestDir, build_wire_fixture, read_row, unit_vector};
use mem_core::scope::Scope;

async fn wire_fixture(
    prefix: &str,
    entries: Vec<(&str, &str)>,
    email: Option<&str>,
) -> common::WireFixture {
    let tags: Vec<(&str, String)> = entries
        .into_iter()
        .map(|(id, hash)| (id, MapScopeResolver::project_tag(hash)))
        .collect();
    let resolver = Arc::new(MapScopeResolver::new(tags, email));
    build_wire_fixture(
        TestDir::new(prefix),
        resolver,
        Arc::new(FakeEmbedder::with_vector(unit_vector(0))),
        None,
    )
    .await
}

/// The registered vector count of one project hash (0 when no shard exists).
async fn scope_count(fixture: &common::WireFixture, scope: Scope, hash: &str) -> u64 {
    let shards = fixture
        .shards
        .get_all_shards(scope, Some(hash))
        .await
        .expect("list shards");
    shards.iter().map(|shard| shard.vector_count).sum()
}

// ---------------------------------------------------------------------------
// T04 — one add persists the row, returns its id, and adopts the count
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_add_persists_the_row_and_returns_its_id() {
    let hash = "1111111111111111";
    let fixture = wire_fixture(
        "wire-add",
        vec![("/probe/project", hash)],
        Some("probe@example.com"),
    )
    .await;

    let response = dispatch(
        &fixture.engine,
        "add",
        json!({
            "session_id": "session-add",
            "project_id": "/probe/project",
            "content": "the first wire memory",
        }),
    )
    .await;
    let id = result(&response)["id"]
        .as_str()
        .expect("add returns its id")
        .to_string();
    assert!(
        id.starts_with("mem_") && id.len() == 4 + 13 + 1 + 9,
        "the upstream id shape mem_<millis>_<9hex>, got {id}"
    );

    assert_eq!(
        scope_count(&fixture, Scope::Project, hash).await,
        1,
        "the counted insert and the adopted registry count"
    );
    let shards = fixture
        .shards
        .get_all_shards(Scope::Project, Some(hash))
        .await
        .expect("shards");
    let row = read_row(&fixture.shards, &shards[0], &id)
        .await
        .expect("the row is in the shard");
    assert_eq!(
        row.text("content").as_deref(),
        Some("the first wire memory")
    );
    assert_eq!(
        row.text("container_tag").as_deref(),
        Some(MapScopeResolver::project_tag(hash).as_str())
    );
    assert_eq!(
        row.text("session_id").as_deref(),
        Some("session-add"),
        "add is the one method that persists the session id"
    );
}

// ---------------------------------------------------------------------------
// T05 — strip then block: the security order of `add`
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_add_strips_private_regions_and_blocks_fully_private_content() {
    let hash = "2222222222222222";
    let fixture = wire_fixture(
        "wire-add-privacy",
        vec![("/probe/project", hash)],
        Some("probe@example.com"),
    )
    .await;

    let blocked = dispatch(
        &fixture.engine,
        "add",
        json!({
            "session_id": "session-add",
            "project_id": "/probe/project",
            "content": "<private>hunter2</private>",
        }),
    )
    .await;
    assert_eq!(error_code(&blocked), -32602);
    assert_eq!(
        data(&blocked).get("reason").and_then(Value::as_str),
        Some("private")
    );
    assert_eq!(
        scope_count(&fixture, Scope::Project, hash).await,
        0,
        "fully private content writes nothing and bumps no count"
    );

    let stripped = dispatch(
        &fixture.engine,
        "add",
        json!({
            "session_id": "session-add",
            "project_id": "/probe/project",
            "content": "<private>hunter2</private>public",
        }),
    )
    .await;
    let id = result(&stripped)["id"].as_str().expect("id").to_string();
    let shards = fixture
        .shards
        .get_all_shards(Scope::Project, Some(hash))
        .await
        .expect("shards");
    let row = read_row(&fixture.shards, &shards[0], &id)
        .await
        .expect("row");
    assert_eq!(row.text("content").as_deref(), Some("[REDACTED]public"));
}

// ---------------------------------------------------------------------------
// T06 — the embedder sees the sanitized content and the tags line
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_add_embeds_the_redacted_content_and_the_tags() {
    let hash = "3333333333333333";
    let fixture = wire_fixture(
        "wire-add-embed",
        vec![("/probe/project", hash)],
        Some("probe@example.com"),
    )
    .await;

    let response = dispatch(
        &fixture.engine,
        "add",
        json!({
            "session_id": "session-add",
            "project_id": "/probe/project",
            "content": "<private>x</private>kept",
            "tags": "a, b",
        }),
    )
    .await;
    assert!(
        response.result.is_some(),
        "add succeeds: {:?}",
        response.error
    );
    assert_eq!(
        fixture.embedder.calls(),
        vec!["[REDACTED]kept".to_string(), "Topics: a, b".to_string()],
        "the embedded inputs are the sanitized content and the shared tags formatter"
    );
}

// ---------------------------------------------------------------------------
// T07 — a blank project scope is refused, never coerced to a wildcard
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_add_requires_content_and_a_non_blank_project() {
    let hash = "4444444444444444";
    let fixture = wire_fixture(
        "wire-add-project",
        vec![("/probe/project", hash)],
        Some("probe@example.com"),
    )
    .await;

    let response = dispatch(
        &fixture.engine,
        "add",
        json!({
            "session_id": "session-add",
            "project_id": "   ",
            "content": "orphan",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    assert_eq!(
        data(&response).get("reason").and_then(Value::as_str),
        Some("project_id")
    );
    assert_eq!(scope_count(&fixture, Scope::Project, hash).await, 0);
    let user_shards = fixture
        .shards
        .get_all_shards(Scope::User, None)
        .await
        .expect("user shards");
    assert!(
        user_shards.is_empty(),
        "a blank project id never writes under the user scope"
    );
}

// ---------------------------------------------------------------------------
// T08 — the project filter: one scope, one tag
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_returns_full_rows_scoped_to_the_project() {
    let project_hash = "aaaa000000000001";
    let foreign_hash = "bbbb000000000002";
    let fixture = wire_fixture(
        "wire-list",
        vec![("/probe/a", project_hash)],
        Some("probe@example.com"),
    )
    .await;
    let project_tag = MapScopeResolver::project_tag(project_hash);
    let foreign_tag = MapScopeResolver::project_tag(foreign_hash);
    let shard = common::create_shard(&fixture.shards, Scope::Project, project_hash, 0).await;
    let mut own = common::new_insert("row-own", "own content", &project_tag);
    own.created_at = 10;
    own.updated_at = 10;
    common::insert_row(&fixture.shards, &shard, &own).await;
    let mut foreign = common::new_insert("row-foreign", "foreign content", &foreign_tag);
    foreign.created_at = 20;
    common::insert_row(&fixture.shards, &shard, &foreign).await;

    let response = dispatch(
        &fixture.engine,
        "list",
        json!({
            "session_id": "session-list",
            "project_id": "/probe/a",
            "scope": "project",
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["count"], 1, "the foreign tag's row is filtered out");
    let items = result["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["content"], "own content");

    let mut keys: Vec<&str> = items[0]
        .as_object()
        .expect("item object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "container_tag",
            "content",
            "created_at",
            "display_name",
            "git_repo_url",
            "id",
            "is_pinned",
            "memory_type",
            "metadata",
            "project_name",
            "project_path",
            "session_id",
            "tags",
            "updated_at",
            "user_email",
            "user_name",
        ],
        "the list row is exactly the 16 wire keys"
    );
    assert_eq!(items[0]["tags"], json!([]));
    assert_eq!(items[0]["metadata"], Value::Null);
    assert_eq!(items[0]["is_pinned"], json!(false));
}

// ---------------------------------------------------------------------------
// T09 — all-projects spans the user scope too
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_all_projects_includes_the_user_scope() {
    let project_hash = "aaaa000000000011";
    let user_hash = "cccc000000000012";
    let fixture = wire_fixture(
        "wire-list-all",
        vec![("/probe/a", project_hash)],
        Some("probe@example.com"),
    )
    .await;
    let project_shard =
        common::create_shard(&fixture.shards, Scope::Project, project_hash, 0).await;
    let project_row = common::new_insert(
        "row-project",
        "project row",
        &MapScopeResolver::project_tag(project_hash),
    );
    common::insert_row(&fixture.shards, &project_shard, &project_row).await;
    let user_shard = common::create_shard(&fixture.shards, Scope::User, user_hash, 0).await;
    let user_row = common::new_insert(
        "row-user",
        "user row",
        &format!("opencode_user_{user_hash}"),
    );
    common::insert_row(&fixture.shards, &user_shard, &user_row).await;

    let response = dispatch(
        &fixture.engine,
        "list",
        json!({
            "session_id": "session-list",
            "project_id": "/probe/a",
            "scope": "all-projects",
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["count"], 2, "the user scope is part of all-projects");
    let mut contents: Vec<&str> = result["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["content"].as_str().expect("content"))
        .collect();
    contents.sort_unstable();
    assert_eq!(contents, vec!["project row", "user row"]);
}

// ---------------------------------------------------------------------------
// T10 — one total order and one truncate across shards
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_limit_bounds_the_page() {
    let hash = "dddd000000000021";
    let fixture = wire_fixture(
        "wire-list-limit",
        vec![("/probe/a", hash)],
        Some("probe@example.com"),
    )
    .await;
    let tag = MapScopeResolver::project_tag(hash);
    let shard_zero = common::create_shard(&fixture.shards, Scope::Project, hash, 0).await;
    for (id, content, created) in [("row-1", "one", 1_i64), ("row-3", "three", 3_i64)] {
        let mut row = common::new_insert(id, content, &tag);
        row.created_at = created;
        common::insert_row(&fixture.shards, &shard_zero, &row).await;
    }
    // Rotation retires shard 0 and creates shard 1 in one transaction.
    let shard_one = fixture
        .shards
        .rotate_shard(Scope::Project, hash, shard_zero.id, 1)
        .await
        .expect("rotate shard");
    let mut row = common::new_insert("row-2", "two", &tag);
    row.created_at = 2;
    common::insert_row(&fixture.shards, &shard_one, &row).await;

    let response = dispatch(
        &fixture.engine,
        "list",
        json!({
            "session_id": "session-list",
            "project_id": "/probe/a",
            "scope": "project",
            "limit": 2,
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(
        result["count"], 2,
        "the merged page is truncated to the limit"
    );
    let items = result["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["created_at"], 3, "created_at DESC across shards");
    assert_eq!(items[1]["created_at"], 2);
}

// ---------------------------------------------------------------------------
// T11 — search hydrates the full 16-key row and adds similarity + rank
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_search_returns_full_ranked_rows() {
    let hash = "eeee000000000031";
    let fixture = wire_fixture(
        "wire-search",
        vec![("/probe/a", hash)],
        Some("probe@example.com"),
    )
    .await;
    let tag = MapScopeResolver::project_tag(hash);
    let shard = common::create_shard(&fixture.shards, Scope::Project, hash, 0).await;

    // Two rows whose CONTENT and TAGS vectors point the same way: the hybrid
    // score is 1.0 for the aligned row and 0.0 for the orthogonal one.
    let mut aligned = common::new_insert("search-aligned", "alpha needle", &tag);
    aligned.vector = unit_vector(0);
    aligned.tags_vector = Some(unit_vector(0));
    aligned.memory_type = Some("note".to_string());
    aligned.session_id = Some("sess-aligned".to_string());
    aligned.updated_at = 1234;
    common::insert_row(&fixture.shards, &shard, &aligned).await;
    let mut orthogonal = common::new_insert("search-orthogonal", "beta", &tag);
    orthogonal.vector = unit_vector(1);
    orthogonal.tags_vector = Some(unit_vector(1));
    common::insert_row(&fixture.shards, &shard, &orthogonal).await;

    let response = dispatch(
        &fixture.engine,
        "search",
        json!({
            "session_id": "session-search",
            "project_id": "/probe/a",
            "scope": "project",
            "query": "needle",
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["count"], 2);
    let items = result["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);

    // The aligned row is rank 1 with similarity ~1; the orthogonal one rank 2.
    assert_eq!(items[0]["id"], "search-aligned");
    assert_eq!(items[1]["id"], "search-orthogonal");
    assert!(
        items[0]["similarity"].as_f64().expect("similarity") > 0.99,
        "the aligned row scores 1.0, got {}",
        items[0]["similarity"]
    );
    assert!(
        items[1]["similarity"].as_f64().expect("similarity") < 0.01,
        "the orthogonal row scores 0.0, got {}",
        items[1]["similarity"]
    );
    assert_eq!(items[0]["rank"], 1);
    assert_eq!(items[1]["rank"], 2);

    // The hydration carries the columns `SearchResult` cannot.
    assert_eq!(items[0]["memory_type"], "note");
    assert_eq!(items[0]["session_id"], "sess-aligned");
    assert_eq!(items[0]["updated_at"], 1234);
    assert_eq!(items[0]["content"], "alpha needle");

    let mut keys: Vec<&str> = items[0]
        .as_object()
        .expect("item object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "container_tag",
            "content",
            "created_at",
            "display_name",
            "git_repo_url",
            "id",
            "is_pinned",
            "memory_type",
            "metadata",
            "project_name",
            "project_path",
            "rank",
            "session_id",
            "similarity",
            "tags",
            "updated_at",
            "user_email",
            "user_name",
        ],
        "a search item is the 16 row keys plus similarity + rank"
    );
}

// ---------------------------------------------------------------------------
// T12 — the project branch filters on the project's own tag
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_search_scope_project_excludes_another_projects_rows() {
    let project_hash = "eeee000000000041";
    let other_hash = "eeee000000000042";
    let fixture = wire_fixture(
        "wire-search-scope",
        vec![("/probe/a", project_hash), ("/probe/b", other_hash)],
        Some("probe@example.com"),
    )
    .await;
    let project_tag = MapScopeResolver::project_tag(project_hash);
    let other_tag = MapScopeResolver::project_tag(other_hash);
    // Identical content and vectors; only the container tag differs. A shard
    // can legitimately carry both (migration copies), so the filter is what
    // keeps another project's row out.
    let shard = common::create_shard(&fixture.shards, Scope::Project, project_hash, 0).await;
    for (id, tag) in [
        ("row-project-a", &project_tag),
        ("row-project-b", &other_tag),
    ] {
        let mut row = common::new_insert(id, "identical content", tag);
        row.vector = unit_vector(0);
        row.tags_vector = Some(unit_vector(0));
        common::insert_row(&fixture.shards, &shard, &row).await;
    }

    let response = dispatch(
        &fixture.engine,
        "search",
        json!({
            "session_id": "session-search",
            "project_id": "/probe/a",
            "scope": "project",
            "query": "needle",
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["count"], 1, "only the project's own tag survives");
    assert_eq!(result["items"][0]["id"], "row-project-a");
}

// ---------------------------------------------------------------------------
// T13 — the search parameter order: session → project → scope → query → limit
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_search_requires_scope_query_and_a_bounded_limit() {
    let hash = "eeee000000000051";
    let fixture = wire_fixture(
        "wire-search-bounds",
        vec![("/probe/a", hash)],
        Some("probe@example.com"),
    )
    .await;

    let cases = [
        (
            json!({"session_id": "s", "project_id": "/probe/a", "query": "x"}),
            "scope",
        ),
        (
            json!({
                "session_id": "s",
                "project_id": "/probe/a",
                "scope": "project"
            }),
            "query",
        ),
        (
            json!({
                "session_id": "s",
                "project_id": "/probe/a",
                "scope": "project",
                "query": "   "
            }),
            "query",
        ),
        (
            json!({
                "session_id": "s",
                "project_id": "/probe/a",
                "scope": "project",
                "query": "x",
                "limit": 0
            }),
            "limit",
        ),
        (
            json!({
                "session_id": "s",
                "project_id": "/probe/a",
                "scope": "project",
                "query": "x",
                "limit": 1001
            }),
            "limit",
        ),
    ];
    for (params, reason) in cases {
        let response = dispatch(&fixture.engine, "search", params).await;
        assert_eq!(error_code(&response), -32602);
        assert_eq!(
            data(&response).get("reason").and_then(Value::as_str),
            Some(reason),
            "expected reason {reason}, got {:?}",
            data(&response)
        );
    }
}

// ---------------------------------------------------------------------------
// T14 — the 43.1 scope-wide seam deletes every copy, retired shards included
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_forget_deletes_every_copy_in_the_project_scope() {
    let hash = "ffff000000000061";
    let fixture = wire_fixture(
        "wire-forget",
        vec![("/probe/a", hash)],
        Some("probe@example.com"),
    )
    .await;
    let tag = MapScopeResolver::project_tag(hash);
    let shard_zero = common::create_shard(&fixture.shards, Scope::Project, hash, 0).await;
    let row = common::new_insert("forget-me", "copy zero", &tag);
    common::insert_row(&fixture.shards, &shard_zero, &row).await;
    let shard_one = fixture
        .shards
        .rotate_shard(Scope::Project, hash, shard_zero.id, 1)
        .await
        .expect("rotate shard");
    let row = common::new_insert("forget-me", "copy one", &tag);
    common::insert_row(&fixture.shards, &shard_one, &row).await;

    let response = dispatch(
        &fixture.engine,
        "forget",
        json!({
            "session_id": "session-forget",
            "project_id": "/probe/a",
            "memoryId": "forget-me",
            "scope": "project",
        }),
    )
    .await;
    assert_eq!(result(&response)["deleted"], 2, "both copies are deleted");
    assert!(
        common::read_row(&fixture.shards, &shard_zero, "forget-me")
            .await
            .is_none(),
        "the retired copy is gone"
    );
    assert!(
        common::read_row(&fixture.shards, &shard_one, "forget-me")
            .await
            .is_none(),
        "the active copy is gone"
    );
}

// ---------------------------------------------------------------------------
// T15 — all-projects expands to the user scope too
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_forget_all_projects_reaches_the_user_scope_copy() {
    let project_hash = "ffff000000000071";
    let user_hash = "ffff000000000072";
    let fixture = wire_fixture(
        "wire-forget-all",
        vec![("/probe/a", project_hash)],
        Some("probe@example.com"),
    )
    .await;
    let user_shard = common::create_shard(&fixture.shards, Scope::User, user_hash, 0).await;
    let user_row = common::new_insert(
        "forget-user-copy",
        "user copy",
        &format!("opencode_user_{user_hash}"),
    );
    common::insert_row(&fixture.shards, &user_shard, &user_row).await;

    // `project` never reaches the user scope: the copy survives.
    let project_only = dispatch(
        &fixture.engine,
        "forget",
        json!({
            "session_id": "session-forget",
            "project_id": "/probe/a",
            "memoryId": "forget-user-copy",
            "scope": "project",
        }),
    )
    .await;
    assert_eq!(result(&project_only)["deleted"], 0);
    assert!(
        common::read_row(&fixture.shards, &user_shard, "forget-user-copy")
            .await
            .is_some(),
        "the project scope never crosses into the user scope"
    );

    // `all-projects` includes the user scope.
    let all = dispatch(
        &fixture.engine,
        "forget",
        json!({
            "session_id": "session-forget",
            "project_id": "/probe/a",
            "memoryId": "forget-user-copy",
            "scope": "all-projects",
        }),
    )
    .await;
    assert_eq!(result(&all)["deleted"], 1);
    assert!(
        common::read_row(&fixture.shards, &user_shard, "forget-user-copy")
            .await
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// T16 — the scope hash is a tenant boundary, never a wildcard
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_forget_never_crosses_the_scope_hash() {
    let our_hash = "ffff000000000081";
    let foreign_hash = "ffff000000000082";
    let fixture = wire_fixture(
        "wire-forget-tenant",
        vec![("/probe/a", our_hash), ("/probe/b", foreign_hash)],
        Some("probe@example.com"),
    )
    .await;
    let ours = common::create_shard(&fixture.shards, Scope::Project, our_hash, 0).await;
    let foreign = common::create_shard(&fixture.shards, Scope::Project, foreign_hash, 0).await;
    let row = common::new_insert(
        "same-id",
        "our row",
        &MapScopeResolver::project_tag(our_hash),
    );
    common::insert_row(&fixture.shards, &ours, &row).await;
    let row = common::new_insert(
        "same-id",
        "foreign row",
        &MapScopeResolver::project_tag(foreign_hash),
    );
    common::insert_row(&fixture.shards, &foreign, &row).await;

    let response = dispatch(
        &fixture.engine,
        "forget",
        json!({
            "session_id": "session-forget",
            "project_id": "/probe/a",
            "memoryId": "same-id",
        }),
    )
    .await;
    assert_eq!(
        result(&response)["deleted"],
        1,
        "exactly the resolved scope's copy is deleted"
    );
    assert!(
        common::read_row(&fixture.shards, &foreign, "same-id")
            .await
            .is_some(),
        "the other tenant's row survives"
    );
}

// ---------------------------------------------------------------------------
// T17 — an absent id is a zero count, not an error
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_forget_reports_zero_for_an_absent_id() {
    let hash = "ffff000000000091";
    let fixture = wire_fixture(
        "wire-forget-absent",
        vec![("/probe/a", hash)],
        Some("probe@example.com"),
    )
    .await;
    let response = dispatch(
        &fixture.engine,
        "forget",
        json!({
            "session_id": "session-forget",
            "project_id": "/probe/a",
            "memoryId": "never-written",
        }),
    )
    .await;
    assert_eq!(error_code_or_ok(&response), 0);
    assert_eq!(result(&response)["deleted"], 0);
}

// ---------------------------------------------------------------------------
// T18 — an unknown scope literal is invalid params, never a default
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_an_unknown_scope_value_is_invalid_params() {
    let hash = "ffff0000000000a1";
    let fixture = wire_fixture(
        "wire-scope-literal",
        vec![("/probe/a", hash)],
        Some("probe@example.com"),
    )
    .await;
    let response = dispatch(
        &fixture.engine,
        "forget",
        json!({
            "session_id": "session-forget",
            "project_id": "/probe/a",
            "memoryId": "any-id",
            "scope": "both",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    assert_eq!(
        data(&response).get("reason").and_then(Value::as_str),
        Some("scope")
    );
}

/// 0 for a result-bearing response (the helper keeps T17's assertion explicit).
fn error_code_or_ok(response: &Response) -> i32 {
    response.error.as_ref().map(|error| error.code).unwrap_or(0)
}
