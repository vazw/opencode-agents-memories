//! W5.5 — store-backed oracles for the compaction leg (`InjectService::assemble`
//! with `include.compaction`), over a real episodic store and a real
//! `ShardManager` (the `capture_tier1.rs:66-80` pattern).
//!
//! Fixture `S` (one project shard, `container_tag = opencode_project_0123456789abcdef`):
//! a project-scoped fact row with no session and the two `sess-A` tier-1 rows.
//! The restore must render ONLY the two session rows, byte-exactly, and the
//! project fact / history wrapper must never appear.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use common::{FakeEmbedder, FixedScopeResolver, TestDir};
use mem_bridge::capture::{CaptureParams, DrainReport};
use mem_bridge::compaction::CompactionPolicy;
use mem_bridge::composition::{ComposedRuntime, build_composed_runtime};
use mem_bridge::inject::{InjectParams, InjectService, ProfileInjection};
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{MemoryInsert, get_memories_by_session_id, insert_vector};

const TAG: &str = "opencode_project_0123456789abcdef";
const HASH: &str = "0123456789abcdef";
const PROJECT: &str = "/srv/proj";

const RESTORE_2: &str = "## Restored Session Memory\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n### Memory 2\nDeploys go through the staging gate first\n\nTags: project, opencode-mem\n\n";

struct SeedRow<'row> {
    id: &'row str,
    content: &'row str,
    tags: Option<&'row str>,
    session_id: Option<&'row str>,
    metadata: Option<&'row str>,
    created_at: i64,
}

fn fixture_s() -> Vec<SeedRow<'static>> {
    vec![
        SeedRow {
            id: "fact-1",
            content: "The deploy key lives in op://infra/deploy",
            tags: Some("project"),
            session_id: None,
            metadata: None,
            created_at: 3000,
        },
        SeedRow {
            id: "row-1",
            content: "Retries are capped at three",
            tags: Some("project,opencode-mem"),
            session_id: Some("sess-A"),
            metadata: Some(r#"{"source":"auto-capture","sessionId":"sess-A"}"#),
            created_at: 2000,
        },
        SeedRow {
            id: "row-2",
            content: "Deploys go through the staging gate first",
            tags: Some("project,opencode-mem"),
            session_id: Some("sess-A"),
            metadata: Some(r#"{"source":"auto-capture","sessionId":"sess-A"}"#),
            created_at: 1000,
        },
    ]
}

fn params(session_id: Option<&str>, compaction: &str) -> InjectParams {
    InjectParams {
        project_id: Some(PROJECT.to_string()),
        agent_id: None,
        token_budget: None,
        include: Some(json!({ "compaction": compaction })),
        user_id: None,
        session_id: session_id.map(str::to_string),
    }
}

fn restore_params(project_id: &str, session_id: &str) -> InjectParams {
    InjectParams {
        project_id: Some(project_id.to_string()),
        agent_id: None,
        token_budget: None,
        include: Some(json!({ "compaction": "restore" })),
        user_id: None,
        session_id: Some(session_id.to_string()),
    }
}

fn plain_params() -> InjectParams {
    InjectParams {
        project_id: Some(PROJECT.to_string()),
        agent_id: None,
        token_budget: None,
        include: None,
        user_id: None,
        session_id: None,
    }
}

async fn build_service(
    dir: &TestDir,
    policy: CompactionPolicy,
) -> (Arc<EpisodicStore>, Arc<ShardManager>, InjectService) {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let service = InjectService::new(
        store.clone(),
        shards.clone(),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        Arc::new(FakeEmbedder::new(common::unit_vector(0))),
        Arc::new(mem_store::platform::SystemProcessProbe),
    )
    .with_compaction(policy);
    (store, shards, service)
}

async fn seed(shards: &ShardManager, rows: &[SeedRow<'_>]) -> ShardInfo {
    let shard = shards
        .get_write_shard(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("the project shard is created");
    seed_into(shards, &shard, rows).await;
    shard
}

async fn seed_into(shards: &ShardManager, shard: &ShardInfo, rows: &[SeedRow<'_>]) {
    for (index, row) in rows.iter().enumerate() {
        let db = shards.open_shard(&shard.db_path).await.expect("open shard");
        insert_vector(
            db.as_ref(),
            &MemoryInsert {
                id: row.id.to_string(),
                content: row.content.to_string(),
                vector: common::unit_vector(index % 8),
                tags_vector: None,
                container_tag: TAG.to_string(),
                tags: row.tags.map(str::to_string),
                memory_type: Some("note".to_string()),
                created_at: row.created_at,
                updated_at: row.created_at,
                metadata: row.metadata.map(str::to_string),
                session_id: row.session_id.map(str::to_string),
                display_name: None,
                user_name: None,
                user_email: None,
                project_path: None,
                project_name: None,
                git_repo_url: None,
            },
        )
        .await
        .expect("tier-1 row insert");
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_include_compaction_restore_never_returns_the_chat_block() {
    let dir = TestDir::new("w55-compaction-restore");
    let (_store, shards, service) = build_service(&dir, CompactionPolicy::default()).await;
    let _shard = seed(&shards, &fixture_s()).await;

    let result = service
        .assemble(&params(Some("sess-A"), "restore"))
        .await
        .expect("the restore leg never fails the request");
    let block = result.block.expect("two session rows render a block");
    assert_eq!(block, RESTORE_2, "restore bytes are upstream-exact");
    assert_eq!(result.memory_count, 2, "two rendered memories");
    assert_eq!(
        result.tokens,
        (block.len() / 4) as u64,
        "tokens is the emitted-byte estimate, never Default's 0"
    );
    assert_eq!(
        result.profile,
        ProfileInjection::NoProjectScope,
        "the leg reports the one 'tier never ran' outcome, never Default's Disabled"
    );
    assert!(
        !block.contains("<project_agent_session_history>"),
        "the chat block is never returned by the compaction leg"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_restore_excludes_project_scoped_facts_and_the_history_wrapper() {
    let dir = TestDir::new("w55-compaction-exclude");
    let (_store, shards, service) = build_service(&dir, CompactionPolicy::default()).await;
    let _shard = seed(&shards, &fixture_s()).await;

    let result = service
        .assemble(&params(Some("sess-A"), "restore"))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert!(
        !block.contains("The deploy key lives in op://infra/deploy"),
        "fixture S's project-scoped fact must not be in the session render"
    );
    assert!(
        !block.contains("<project_agent_session_history>"),
        "the history wrapper belongs to the chat block, never the session render"
    );
    assert_eq!(block, RESTORE_2, "the session render is byte-exact");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_compaction_disabled_returns_none_and_does_not_query_the_store() {
    // Oracle: the observable is the shard FILE, so every leg gets a FRESH
    // ShardManager — `get_connection` caches handles by db_path, and a reused
    // manager would serve the unlinked inode and never re-create the file.
    let shard_pattern = |name: &str| {
        let base = name
            .strip_prefix("project_")
            .and_then(|rest| rest.strip_suffix(".db"));
        let Some(base) = base else { return false };
        let Some((hash, rest)) = base.rsplit_once("_shard_") else {
            return false;
        };
        hash.len() == 16
            && hash.chars().all(|c| c.is_ascii_hexdigit())
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit())
    };

    // 1. Seed a real shard and capture its ONE file name.
    let seed_dir = TestDir::new("w55-oracle-seed");
    let (seed_store, seed_shards, seed_service) =
        build_service(&seed_dir, CompactionPolicy::default()).await;
    let shard = seed(&seed_shards, &fixture_s()[1..]).await;
    let seeded_name = shard
        .db_path
        .file_name()
        .expect("shard path has a file name")
        .to_string_lossy()
        .to_string();
    // The shard lives in `<storage>/projects/` (scope dir), so the file
    // observable is taken from its own parent, never the storage root.
    let shard_dir = shard
        .db_path
        .parent()
        .expect("shard path has a parent")
        .to_path_buf();
    let names: Vec<String> = common::list_data_files(&shard_dir)
        .await
        .into_iter()
        .filter(|name| shard_pattern(name))
        .collect();
    assert_eq!(
        names,
        vec![seeded_name.clone()],
        "exactly one project shard file exists after seeding"
    );
    drop(seed_service);
    drop(seed_shards);
    drop(seed_store);

    // 2. Delete that exact file.
    std::fs::remove_file(shard.db_path.clone()).expect("delete the seeded shard");

    // 3. Disabled leg over a FRESH manager: no store access, so no re-creation.
    let (disabled_store, disabled_shards, disabled_service) = build_service(
        &seed_dir,
        CompactionPolicy {
            enabled: false,
            memory_limit: 10,
        },
    )
    .await;
    let result = disabled_service
        .assemble(&params(Some("sess-A"), "restore"))
        .await
        .expect("a disabled policy is a no-op, never an error");
    assert_eq!(result.block, None, "a disabled policy renders nothing");
    let files = common::list_data_files(&shard_dir).await;
    assert!(
        !files.iter().any(|name| shard_pattern(name)),
        "the disabled leg must not query the store (file re-created): {files:?}"
    );
    drop(disabled_service);
    drop(disabled_shards);
    drop(disabled_store);

    // 4. Rebuild the seed from scratch, capture the single name again, delete
    //    it again, then run the ENABLED control over ANOTHER fresh manager.
    let (control_store, control_shards, control_service) =
        build_service(&seed_dir, CompactionPolicy::default()).await;
    // The registry row survived the file deletion, so `get_write_shard` would
    // (correctly) refuse the missing file as ShardIncompatible; `create_shard`
    // re-initializes and re-fetches the SAME row on its UNIQUE-conflict path.
    let rebuilt = control_shards
        .create_shard(mem_core::scope::Scope::Project, HASH, 0)
        .await
        .expect("the shard file is rebuilt over the surviving registry row");
    seed_into(&control_shards, &rebuilt, &fixture_s()[1..]).await;
    let control_name = rebuilt
        .db_path
        .file_name()
        .expect("shard path has a file name")
        .to_string_lossy()
        .to_string();
    assert_eq!(
        control_name, seeded_name,
        "the rebuilt seed re-uses the same shard file name"
    );
    let names: Vec<String> = common::list_data_files(&shard_dir)
        .await
        .into_iter()
        .filter(|name| shard_pattern(name))
        .collect();
    assert_eq!(
        names,
        vec![control_name.clone()],
        "exactly one shard after rebuild"
    );
    std::fs::remove_file(rebuilt.db_path.clone()).expect("delete the rebuilt shard");
    drop(control_service);
    drop(control_shards);
    drop(control_store);

    // The enabled control is a HARD PRECONDITION: it must prove the file comes
    // back, otherwise this oracle does not discriminate and the step is BLOCKED.
    let (_enabled_store, _enabled_shards, enabled_service) =
        build_service(&seed_dir, CompactionPolicy::default()).await;
    let control = enabled_service
        .assemble(&params(Some("sess-A"), "restore"))
        .await
        .expect("the enabled leg swallows its read failure");
    assert_eq!(
        control.block, None,
        "the deleted rows make the enabled leg's render empty — the FILE is the observable"
    );
    let files = common::list_data_files(&shard_dir).await;
    assert!(
        files.iter().any(|name| name == &control_name),
        "the enabled control re-created the captured shard file {control_name}: {files:?}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_restore_ignores_a_metadata_nested_session_id_row() {
    let dir = TestDir::new("w55-compaction-decoy");
    let (_store, shards, service) = build_service(&dir, CompactionPolicy::default()).await;
    let decoy = SeedRow {
        id: "decoy-1",
        content: "Decoy nested session row",
        tags: Some("project,opencode-mem"),
        session_id: None,
        metadata: Some(r#"{"source":"auto-capture","parent":{"sessionID":"sess-A"}}"#),
        created_at: 2500,
    };
    let mut rows = fixture_s();
    rows.push(decoy);
    let shard = seed(&shards, &rows).await;

    // The SQL LIKE arm DOES return the decoy (raw substring); only the exact
    // `resolve_session_id` filter drops it. Prove the read is non-vacuous.
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    let raw = get_memories_by_session_id(db.as_ref(), "sess-A")
        .await
        .expect("session read");
    assert!(
        raw.iter()
            .any(|row| row.text("id").as_deref() == Some("decoy-1")),
        "precondition: the SQL read returns the metadata-nested decoy ({} rows)",
        raw.len()
    );

    let result = service
        .assemble(&params(Some("sess-A"), "restore"))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert_eq!(
        block, RESTORE_2,
        "the decoy renders no heading: its OWN session identity is NULL"
    );
    assert!(!block.contains("Decoy nested session row"));
    assert_eq!(result.memory_count, 2);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_without_include_compaction_is_unchanged() {
    let dir = TestDir::new("w55-compaction-plain");
    let (_store, shards, service) = build_service(&dir, CompactionPolicy::default()).await;
    let _shard = seed(&shards, &fixture_s()).await;

    let result = service
        .assemble(&plain_params())
        .await
        .expect("the plain inject path still assembles");
    let block = result.block.expect("the fixture renders the chat block");
    assert!(
        block.starts_with("<project_agent_session_history>"),
        "a bare inject returns the chat block, never the session render: {block}"
    );
    assert!(
        block.contains("The deploy key lives in op://infra/deploy"),
        "the plain path still carries the project-scoped facts"
    );
    assert_ne!(block, RESTORE_2, "the compaction leg was not taken");
}

// ---------------------------------------------------------------------------
// Step 8 — the configured value wins over the default (AC 6), and AC 5
// ---------------------------------------------------------------------------

/// Builds the production composition over `root` with an explicit compaction
/// configuration (or the loaded defaults when `None`).
async fn composed_runtime(root: &Path, compaction: Option<(bool, u32)>) -> ComposedRuntime {
    let home = common::temp_dir("w55-composed-home");
    let env: BTreeMap<String, String> = [("HOME", home.to_string_lossy().to_string())]
        .map(|(key, value)| (key.to_string(), value))
        .into_iter()
        .collect();
    let mut config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(&home), &env)
            .expect("the default config loads");
    config.storage_path = root.to_path_buf();
    config.embedding_dimensions = 8;
    if let Some((enabled, memory_limit)) = compaction {
        config.compaction = mem_core::config::Compaction {
            enabled,
            memory_limit,
        };
    }
    build_composed_runtime(
        root,
        &config,
        Arc::new(FakeEmbedder::new(common::unit_vector(0))),
        None,
    )
    .await
    .expect("the runtime composes")
}

fn capture_params(root: &Path, index: usize) -> CaptureParams {
    CaptureParams {
        session_id: "sess-A".to_string(),
        capture_id: format!("019a0000-0000-7f00-8000-0000000000{index:02}"),
        project_id: root.to_string_lossy().to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: None,
        parent_session_id: None,
        transcript: None,
        summary: Some(format!("seed fact {index}")),
        metadata: None,
        raw_context: None,
        agent_role: None,
        vault: false,
    }
}

/// Seeds three tier-1 rows through the composed runtime's capture path, then
/// proves the seed: the ROWS first (the counters cannot prove a row — a
/// summary-less provider-less entry is a `Skip` that still reports `done`),
/// then `done: 3` with the other three counters `0`.
async fn seed_three_tier1_rows(root: &Path) -> DrainReport {
    let runtime = composed_runtime(root, None).await;
    for index in 0..3 {
        runtime
            .capture
            .enqueue(&capture_params(root, index))
            .await
            .expect("the capture entry is enqueued");
    }
    let report = runtime.capture.drain_once().await.expect("drain once");

    let project_id = root.to_string_lossy().to_string();
    let scope = runtime
        .scope_resolver
        .resolve(&project_id)
        .expect("the project scope resolves");
    let shard = runtime
        .shards
        .get_write_shard(mem_core::scope::Scope::Project, &scope.scope_hash)
        .await
        .expect("the capture shard exists after the drain");
    let db = runtime
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    let rows = get_memories_by_session_id(db.as_ref(), "sess-A")
        .await
        .expect("session read");
    assert_eq!(
        rows.len(),
        3,
        "the seed wrote three tier-1 rows for sess-A (else BLOCKED (seed fault))"
    );
    assert_eq!(
        report,
        DrainReport {
            done: 3,
            retryable: 0,
            dead_lettered: 0,
            deferred: 0,
        },
        "three supplied-summary entries drain as done"
    );
    drop(runtime);
    report
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_disabled_config_returns_no_block_despite_rows() {
    let root = TestDir::new("w55-config-disabled");
    let _seed = seed_three_tier1_rows(root.path()).await;

    // Its OWN runtime over the same root: rows pre-exist, and `{enabled:
    // false}` must still return None — which a `default()` policy cannot fake
    // here, because the default is enabled.
    let runtime = composed_runtime(root.path(), Some((false, 2))).await;
    let project_id = root.path().to_string_lossy().to_string();
    let result = runtime
        .inject
        .assemble(&restore_params(&project_id, "sess-A"))
        .await
        .expect("assemble");
    assert_eq!(
        result.block, None,
        "compaction.enabled = false must render nothing despite three session rows"
    );
    assert_eq!(result.memory_count, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_configured_limit_renders_two_and_counts_two() {
    let root = TestDir::new("w55-config-limit");
    let _seed = seed_three_tier1_rows(root.path()).await;

    let runtime = composed_runtime(root.path(), Some((true, 2))).await;
    let project_id = root.path().to_string_lossy().to_string();
    let result = runtime
        .inject
        .assemble(&restore_params(&project_id, "sess-A"))
        .await
        .expect("assemble");
    let block = result.block.expect("two of the three rows render");
    assert_eq!(
        block.matches("### Memory ").count(),
        2,
        "compaction.memory_limit = 2 truncates the rendered rows: {block}"
    );
    assert_eq!(
        result.memory_count, 2,
        "memory_count is the RENDERED count, never the three read rows"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_written_tier1_row_is_restored_by_session_id() {
    let root = TestDir::new("w55-capture-chain");
    let runtime = composed_runtime(root.path(), None).await;
    let summary = "Captured fact: the deploy uses the staging gate";
    let mut params_row = capture_params(root.path(), 7);
    params_row.summary = Some(summary.to_string());
    runtime.capture.enqueue(&params_row).await.expect("enqueue");
    let report = runtime.capture.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "the supplied-summary entry wrote its row");

    let project_id = root.path().to_string_lossy().to_string();
    let result = runtime
        .inject
        .assemble(&restore_params(&project_id, "sess-A"))
        .await
        .expect("assemble");
    let block = result.block.expect("the capture row is restored");
    assert!(
        block.contains(summary),
        "the capture-time write is consumed by session_id: {block}"
    );
    assert_eq!(result.memory_count, 1);
}
