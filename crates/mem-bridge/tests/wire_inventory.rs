//! Issue 70 wave 1 — the `list-shards` and `migrate` wire methods (T26-T29).

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, MapScopeResolver, TestDir, build_wire_fixture, unit_vector};
use mem_bridge::jsonrpc::{Response, parse_line};
use mem_store::project_tags::ProjectTagSource;
use serde_json::{Value, json};

const HASH: &str = "b1b1b1b1b1b1b1b1";

async fn fixture(prefix: &str) -> common::WireFixture {
    let resolver = Arc::new(MapScopeResolver::single(
        "/probe/a",
        MapScopeResolver::project_tag(HASH),
        Some("probe@example.com"),
    ));
    build_wire_fixture(
        TestDir::new(prefix),
        resolver,
        Arc::new(FakeEmbedder::with_vector(unit_vector(0))),
        None,
    )
    .await
}

async fn dispatch(engine: &mem_bridge::engine::Engine, method: &str, params: Value) -> Response {
    let request = parse_line(
        &json!({"id": "i", "jsonrpc": "2.0", "method": method, "params": params}).to_string(),
    )
    .expect("inventory request parses");
    engine.dispatch(&request).await
}

fn error_code(response: &Response) -> i32 {
    response
        .error
        .as_ref()
        .map(|error| error.code)
        .expect("error envelope")
}

fn data(response: &Response) -> &Value {
    response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref())
        .expect("advisory data")
}

fn result(response: &Response) -> &Value {
    response.result.as_ref().expect("result envelope")
}

/// The production tag source over the test config's prefix.
fn tag_source() -> mem_store::project_tags::CoreProjectTagSource {
    mem_store::project_tags::CoreProjectTagSource {
        container_tag_prefix: "opencode".to_string(),
        user_email_override: None,
        user_name_override: None,
    }
}

fn tag_hash(tag: &str) -> String {
    tag.rsplit('_').next().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// T26 — `project_path` is the path the inventory is asked about
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_shards_returns_the_inventory_for_the_project_path() {
    let fixture = fixture("wire-inventory").await;
    let project_path = fixture.storage_root().join("probe-project");
    let project_path = project_path.to_string_lossy().to_string();
    let tags = tag_source();
    let info = tags.project_tag_info(std::path::Path::new(&project_path));
    let hash = tag_hash(&info.tag);
    // A registered shard under the path's own hash: the group the inventory
    // must report.
    let shard =
        common::create_shard(&fixture.shards, mem_core::scope::Scope::Project, &hash, 0).await;
    let mut row = common::new_insert("inventory-row", "inventory content", &info.tag);
    row.project_path = Some(project_path.clone());
    common::insert_row(&fixture.shards, &shard, &row).await;

    let response = dispatch(
        &fixture.engine,
        "list-shards",
        json!({"session_id": "session-inventory", "project_path": project_path}),
    )
    .await;
    let result = result(&response);
    assert_eq!(
        result["currentProject"]["scopeHash"], hash,
        "the hash is the project PATH's tag, never the storage root's"
    );
    assert_eq!(result["currentProject"]["tag"], info.tag);
    assert_eq!(result["summary"]["total"], 1);
    let root_info = tags.project_tag_info(&fixture.storage_root());
    assert_ne!(
        info.tag, root_info.tag,
        "the discriminating input: the path and the root hash differently"
    );
}

// ---------------------------------------------------------------------------
// T27 — at least one of from_path/from_hash, and an empty one is refused
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_migrate_requires_from_path_or_from_hash() {
    let fixture = fixture("wire-migrate-from").await;
    let project_path = fixture
        .storage_root()
        .join("probe-project")
        .to_string_lossy()
        .to_string();

    let cases = [
        json!({"session_id": "s", "project_path": project_path}),
        json!({"session_id": "s", "project_path": project_path, "from_path": ""}),
        json!({"session_id": "s", "project_path": project_path, "from_hash": "   "}),
    ];
    for params in cases {
        let response = dispatch(&fixture.engine, "migrate", params).await;
        assert_eq!(error_code(&response), -32602);
        assert_eq!(
            data(&response).get("reason").and_then(Value::as_str),
            Some("from_required"),
            "an absent or empty selector is refused at the wire, got {:?}",
            data(&response)
        );
    }
}

// ---------------------------------------------------------------------------
// T28 — dry_run reports but never moves a file
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_migrate_dry_run_reports_the_actions_without_moving_files() {
    let fixture = fixture("wire-migrate-dry").await;
    let old_path = "/definitely/not/a/real/project/path".to_string();
    let old_hash = "b2b2b2b2b2b2b2b2";
    let old_tag = MapScopeResolver::project_tag(old_hash);
    let shard = common::create_shard(
        &fixture.shards,
        mem_core::scope::Scope::Project,
        old_hash,
        0,
    )
    .await;
    let mut row = common::new_insert("orphan-row", "orphan content", &old_tag);
    row.project_path = Some(old_path.clone());
    common::insert_row(&fixture.shards, &shard, &row).await;
    let before = std::fs::metadata(&shard.db_path)
        .expect("the source file exists")
        .len();

    let target_path = fixture
        .storage_root()
        .join("probe-project")
        .to_string_lossy()
        .to_string();
    let response = dispatch(
        &fixture.engine,
        "migrate",
        json!({
            "session_id": "session-migrate",
            "project_path": target_path,
            "from_path": old_path,
            "dry_run": true,
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["dry_run"], true);
    assert!(
        result["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .any(|action| action["type"] == "Rename"),
        "the dry run reports the rename it would do: {result}"
    );
    let after = std::fs::metadata(&shard.db_path)
        .expect("dry run leaves the file in place")
        .len();
    assert_eq!(before, after, "dry run never touches the shard file");
    assert!(
        common::read_row(&fixture.shards, &shard, "orphan-row")
            .await
            .is_some(),
        "the row is still queryable under the old hash"
    );
}

// ---------------------------------------------------------------------------
// T29 — the real move rewrites the prompt path through the new adapter
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_migrate_moves_the_orphaned_shard_and_rewrites_the_prompt_path() {
    let fixture = fixture("wire-migrate-move").await;
    // The claimed mutation needs the root's exclusivity proof.
    let _proof = common::exclusive_root(&fixture.storage_root());
    let old_path = "/definitely/not/a/real/project/path".to_string();
    let old_hash = "b3b3b3b3b3b3b3b3";
    let old_tag = MapScopeResolver::project_tag(old_hash);
    let shard = common::create_shard(
        &fixture.shards,
        mem_core::scope::Scope::Project,
        old_hash,
        0,
    )
    .await;
    let mut row = common::new_insert("moving-row", "moving content", &old_tag);
    row.project_path = Some(old_path.clone());
    common::insert_row(&fixture.shards, &shard, &row).await;
    fixture
        .prompts
        .save_prompt(
            "session-prompt",
            "message-prompt",
            &old_path,
            "prompt content",
        )
        .await
        .expect("seed the prompt row");

    let target_path = fixture
        .storage_root()
        .join("probe-project")
        .to_string_lossy()
        .to_string();
    let response = dispatch(
        &fixture.engine,
        "migrate",
        json!({
            "session_id": "session-migrate",
            "project_path": target_path,
            "from_path": old_path,
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["dry_run"], false);
    let new_hash = result["new_hash"].as_str().expect("new hash").to_string();
    assert_ne!(new_hash, old_hash);

    // The shard file moved: the target hash now holds the row.
    let target_shards = fixture
        .shards
        .get_all_shards(mem_core::scope::Scope::Project, Some(&new_hash))
        .await
        .expect("target shards");
    assert_eq!(
        target_shards.len(),
        1,
        "the orphan shard moved to the target"
    );
    assert!(
        common::read_row(&fixture.shards, &target_shards[0], "moving-row")
            .await
            .is_some(),
        "the row is queryable under the new hash"
    );

    // The prompt-path adapter rewrote the secondary metadata (the
    // `NoopPromptPathUpdater` mutant leaves this path unchanged).
    let db = fixture.prompts.ready().await.expect("prompt db");
    let rows = db
        .query("SELECT project_path FROM user_prompts", Vec::new())
        .await
        .expect("read prompt rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].text("project_path").as_deref(),
        Some(target_path.as_str()),
        "the prompt row follows the project path"
    );
}
