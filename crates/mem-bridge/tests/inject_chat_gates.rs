mod common;

use common::{FakeEmbedder, TestDir};
use mem_bridge::inject::InjectParams;
use mem_episodic::record::NewAgentSession;
use serde_json::json;
use std::time::Duration;

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_assemble_caps_recent_rows_at_max_memories() {
    let dir = TestDir::new("inject-chat-gates-cap");
    let fixture = common::build_fixture_runtime(dir.path()).await;
    let embedder = std::sync::Arc::new(FakeEmbedder::with_vector(common::unit_vector(0)));

    // Seed R1..R5 with sess-other, then R0 with sess-self, sleep between to enforce created_at order:
    // newest-first: R0, R5, R4, R3, R2, R1
    let rows = [
        ("sess-other-1", "R1_CONTENT"),
        ("sess-other-2", "R2_CONTENT"),
        ("sess-other-3", "R3_CONTENT"),
        ("sess-other-4", "R4_CONTENT"),
        ("sess-other-5", "R5_CONTENT"),
        ("sess-self", "R0_CONTENT"),
    ];

    for (session_id, summary) in rows {
        fixture
            .store
            .insert_session(
                NewAgentSession {
                    project_id: "/srv/proj".into(),
                    session_id: session_id.into(),
                    agent_id: "agent-1".into(),
                    summary: summary.into(),
                    ..Default::default()
                },
                embedder.as_ref(),
            )
            .await
            .expect("insert_session");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    let params: InjectParams = serde_json::from_value(json!({
        "project_id": "/srv/proj",
        "session_id": "sess-self",
        "exclude_session": true,
        "max_memories": 2,
        "token_budget": 5000,
        "include": { "recent_count": 10 }
    }))
    .expect("params");

    let result = fixture.inject.assemble(&params).await.expect("assemble");
    let block = result.block.expect("block exists");

    // Exactly R5, R4 present and R0, R3, R2, R1 absent
    assert!(block.contains("R5_CONTENT"), "R5 must be present: {block}");
    assert!(block.contains("R4_CONTENT"), "R4 must be present: {block}");
    assert!(!block.contains("R0_CONTENT"), "R0 must be absent: {block}");
    assert!(!block.contains("R3_CONTENT"), "R3 must be absent: {block}");
    assert!(!block.contains("R2_CONTENT"), "R2 must be absent: {block}");
    assert!(!block.contains("R1_CONTENT"), "R1 must be absent: {block}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_assemble_drops_rows_older_than_max_age_days() {
    let dir = TestDir::new("inject-chat-gates-age");
    let current_time = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let time_reader = current_time.clone();
    let clock: std::sync::Arc<dyn Fn() -> i64 + Send + Sync> =
        std::sync::Arc::new(move || time_reader.load(std::sync::atomic::Ordering::SeqCst));

    let config = dir.store_config();
    let store = std::sync::Arc::new(
        mem_episodic::store::EpisodicStore::new(dir.path().to_path_buf(), config.clone())
            .with_clock(clock.clone()),
    );
    store.ready().await.expect("store ready");

    let shards = std::sync::Arc::new(common::shard_manager(config));
    let embedder = std::sync::Arc::new(FakeEmbedder::with_vector(common::unit_vector(0)));
    let service = mem_bridge::inject::InjectService::new(
        store.clone(),
        shards,
        std::sync::Arc::new(common::FixedScopeResolver::default()),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        embedder.clone(),
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
    )
    .with_clock(clock.clone());

    // T0 = 1_700_000_000_000ms, max_age_days = 30 (cutoff = T0 - 30 * 86_400_000ms = 1_697_408_000_000ms)
    const T0: i64 = 1_700_000_000_000;
    const CUTOFF: i64 = T0 - 30 * 86_400_000;

    // Seed B_exact at exact cutoff, B_inside at cutoff + 1ms, B_outside at cutoff - 1ms
    let cases = [
        ("sess-exact", "B_EXACT", CUTOFF),
        ("sess-inside", "B_INSIDE", CUTOFF + 1),
        ("sess-outside", "B_OUTSIDE", CUTOFF - 1),
    ];

    for (session_id, summary, stamp) in cases {
        current_time.store(stamp, std::sync::atomic::Ordering::SeqCst);
        store
            .insert_session(
                NewAgentSession {
                    project_id: "/srv/proj".into(),
                    session_id: session_id.into(),
                    agent_id: "agent-1".into(),
                    summary: summary.into(),
                    ..Default::default()
                },
                embedder.as_ref(),
            )
            .await
            .expect("insert_session");
    }

    current_time.store(T0, std::sync::atomic::Ordering::SeqCst);

    let params: InjectParams = serde_json::from_value(json!({
        "project_id": "/srv/proj",
        "session_id": "sess-req",
        "max_age_days": 30,
        "token_budget": 5000,
        "include": { "recent_count": 10 }
    }))
    .expect("params");

    let result = service.assemble(&params).await.expect("assemble");
    let block = result.block.expect("block exists");

    // B_inside and B_exact retained, B_outside dropped
    assert!(
        block.contains("B_INSIDE"),
        "B_inside must be retained: {block}"
    );
    assert!(
        block.contains("B_EXACT"),
        "B_exact must be retained: {block}"
    );
    assert!(
        !block.contains("B_OUTSIDE"),
        "B_outside must be dropped: {block}"
    );
}

// ---------------------------------------------------------------------------
// Production composition fixture & AC-5a/b/c policy tests
// ---------------------------------------------------------------------------

use mem_bridge::engine::{Engine, EngineConfig};
use mem_bridge::jsonrpc::{Response, parse_line};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

fn test_config(storage_path: std::path::PathBuf, home: &Path) -> mem_core::config::Config {
    let env: BTreeMap<String, String> = [("HOME".into(), home.to_string_lossy().to_string())]
        .into_iter()
        .collect();
    let mut config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(home), &env)
            .expect("load config");
    config.storage_path = storage_path;
    config.embedding_dimensions = 8;
    config
}

async fn production_engine(
    root: &TestDir,
    home: &TestDir,
    mutate: impl FnOnce(&mut mem_core::config::Config),
) -> (Engine, Arc<mem_episodic::store::EpisodicStore>, String) {
    let mut config = test_config(root.path().to_path_buf(), home.path());
    // Neutralize chat policy defaults so individual AC-5 tests isolate single variables:
    config.chat_message.max_memories = 100;
    config.chat_message.exclude_current_session = false;
    config.chat_message.max_age_days = None;
    mutate(&mut config);
    let embedder = Arc::new(FakeEmbedder::with_vector(common::unit_vector(0)));
    let composed =
        mem_bridge::composition::build_composed_runtime(root.path(), &config, embedder, None)
            .await
            .expect("composed runtime");
    let store = composed.store.clone();
    let engine = Engine::from_composed(EngineConfig::default(), &composed, &config, None);
    let project_dir = root.path().join("probe-proj");
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    (engine, store, project_dir.to_string_lossy().to_string())
}

async fn dispatch_req(engine: &Engine, method: &str, params: serde_json::Value) -> Response {
    let line = json!({
        "id": "t",
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    })
    .to_string();
    let req = parse_line(&line).expect("parse line");
    engine.dispatch(&req).await
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_policy_max_memories_enforced_independently() {
    let root = TestDir::new("ac5a-root");
    let home = TestDir::new("ac5a-home");
    let (engine, store, project_id) = production_engine(&root, &home, |cfg| {
        cfg.chat_message.max_memories = 2;
    })
    .await;
    let embedder = FakeEmbedder::with_vector(common::unit_vector(0));

    for i in 1..=4 {
        store
            .insert_session(
                NewAgentSession {
                    project_id: project_id.clone(),
                    session_id: format!("ses-mem-{i}"),
                    agent_id: "agent-1".into(),
                    summary: format!("SUMMARY_{i}"),
                    ..Default::default()
                },
                &embedder,
            )
            .await
            .expect("insert");
    }

    let response = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "ses-req",
            "project_id": project_id,
            "chat_injection": true,
            "token_budget": 5000,
            "include": { "recent_count": 10 }
        }),
    )
    .await;

    let res = response.result.expect("result");
    assert_eq!(res["memory_count"], 2, "max_memories=2 cap enforced: {res}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_policy_max_age_days_enforced_independently() {
    let root = TestDir::new("ac5b-root");
    let home = TestDir::new("ac5b-home");
    let (engine, store, project_id) = production_engine(&root, &home, |cfg| {
        cfg.chat_message.max_age_days = Some(10);
    })
    .await;
    let embedder = FakeEmbedder::with_vector(common::unit_vector(0));

    for (sid, summary) in [("ses-recent", "ROW_5_DAYS"), ("ses-old", "ROW_20_DAYS")] {
        store
            .insert_session(
                NewAgentSession {
                    project_id: project_id.clone(),
                    session_id: sid.into(),
                    agent_id: "agent-1".into(),
                    summary: summary.into(),
                    ..Default::default()
                },
                &embedder,
            )
            .await
            .expect("insert");
    }

    // Age the ses-old row in episodic.db to 20 days ago, and ses-recent to 5 days ago
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_millis() as i64;
    let db = common::open_raw(root.path(), "episodic.db").await;
    db.execute(
        "UPDATE agent_sessions SET created_at = ?1 WHERE session_id = 'ses-recent'",
        vec![mem_store::db::Value::Integer(now - 5 * 86_400_000)],
    )
    .await
    .expect("update recent");
    db.execute(
        "UPDATE agent_sessions SET created_at = ?1 WHERE session_id = 'ses-old'",
        vec![mem_store::db::Value::Integer(now - 20 * 86_400_000)],
    )
    .await
    .expect("update old");

    let response = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "ses-req",
            "project_id": project_id,
            "chat_injection": true,
            "token_budget": 5000,
            "include": { "recent_count": 10 }
        }),
    )
    .await;

    let res = response.result.expect("result");
    let block = res["block"].as_str().expect("block");
    assert!(block.contains("ROW_5_DAYS"), "5-day row kept: {block}");
    assert!(
        !block.contains("ROW_20_DAYS"),
        "20-day row dropped: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_policy_exclude_current_session_enforced_independently() {
    let root = TestDir::new("ac5c-root");
    let home = TestDir::new("ac5c-home");
    let (engine, store, project_id) = production_engine(&root, &home, |cfg| {
        cfg.chat_message.exclude_current_session = true;
    })
    .await;
    let embedder = FakeEmbedder::with_vector(common::unit_vector(0));

    for (sid, summary) in [("ses-s1", "S1_CONTENT"), ("ses-s2", "S2_CONTENT")] {
        store
            .insert_session(
                NewAgentSession {
                    project_id: project_id.clone(),
                    session_id: sid.into(),
                    agent_id: "agent-1".into(),
                    summary: summary.into(),
                    ..Default::default()
                },
                &embedder,
            )
            .await
            .expect("insert");
    }

    // Chat request with session_id: ses-s1, chat_injection: true -> S1 excluded, S2 kept
    let chat_resp = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "ses-s1",
            "project_id": project_id,
            "chat_injection": true,
            "token_budget": 5000,
            "include": { "recent_count": 10 }
        }),
    )
    .await;
    let chat_block = chat_resp.result.expect("res")["block"]
        .as_str()
        .expect("block")
        .to_string();
    assert!(
        !chat_block.contains("S1_CONTENT"),
        "S1 must be excluded from chat: {chat_block}"
    );
    assert!(
        chat_block.contains("S2_CONTENT"),
        "S2 must be kept in chat: {chat_block}"
    );

    // Direct request without chat marker keeps S1
    let direct_resp = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "ses-s1",
            "project_id": project_id,
            "token_budget": 5000,
            "include": { "recent_count": 10 }
        }),
    )
    .await;
    let direct_block = direct_resp.result.expect("res")["block"]
        .as_str()
        .expect("block")
        .to_string();
    assert!(
        direct_block.contains("S1_CONTENT"),
        "S1 must be kept in direct request: {direct_block}"
    );
    assert!(
        direct_block.contains("S2_CONTENT"),
        "S2 must be kept in direct request: {direct_block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_policy_explicit_exclusion_override() {
    let root = TestDir::new("chat-override-root");
    let home = TestDir::new("chat-override-home");
    let (engine, store, project_id) = production_engine(&root, &home, |cfg| {
        cfg.chat_message.exclude_current_session = true;
    })
    .await;
    store
        .insert_session(
            NewAgentSession {
                project_id: project_id.clone(),
                session_id: "self".into(),
                agent_id: "agent".into(),
                summary: "OVERRIDE_SELF".into(),
                ..Default::default()
            },
            &FakeEmbedder::with_vector(common::unit_vector(0)),
        )
        .await
        .expect("insert");
    let response = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "self", "project_id": project_id, "chat_injection": true,
            "exclude_session": false, "token_budget": 5000
        }),
    )
    .await;
    assert!(
        response.result.expect("result")["block"]
            .as_str()
            .expect("block")
            .contains("OVERRIDE_SELF")
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_malformed_inject_parameter_is_rejected() {
    let root = TestDir::new("invalid-inject-root");
    let home = TestDir::new("invalid-inject-home");
    let (engine, _, project_id) = production_engine(&root, &home, |_| {}).await;
    let response = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "self", "project_id": project_id, "user_id": 42,
            "exclude_session": true
        }),
    )
    .await;
    assert_eq!(response.error.expect("invalid params").code, -32602);
}

// ---------------------------------------------------------------------------
// Tier-1 eligibility and cap-vs-budget ordering (issue 63). Direct service
// fixture: tier-1 rows need a controlled `created_at` and `sessionID` metadata.
// ---------------------------------------------------------------------------

const TAG: &str = "opencode_project_0123456789abcdef";
const HASH: &str = "0123456789abcdef";

struct Tier1Fixture {
    _dir: TestDir,
    store: Arc<mem_episodic::store::EpisodicStore>,
    shards: Arc<mem_store::shard_manager::ShardManager>,
    service: mem_bridge::inject::InjectService,
}

async fn tier1_fixture(name: &str, now: i64) -> Tier1Fixture {
    let dir = TestDir::new(name);
    let store = Arc::new(mem_episodic::store::EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let service = mem_bridge::inject::InjectService::new(
        store.clone(),
        shards.clone(),
        Arc::new(common::FixedScopeResolver::default()),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Arc::new(mem_store::platform::SystemProcessProbe),
    )
    .with_clock(Arc::new(move || now));
    Tier1Fixture {
        _dir: dir,
        store,
        shards,
        service,
    }
}

async fn seed_tier1(
    shards: &Arc<mem_store::shard_manager::ShardManager>,
    id: &str,
    content: &str,
    metadata: Option<&str>,
    created_at: i64,
) {
    let shard = shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("write shard");
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    mem_store::vector::insert_vector(
        db.as_ref(),
        &mem_store::vector::MemoryInsert {
            id: id.to_string(),
            content: content.to_string(),
            vector: common::unit_vector(0),
            tags_vector: None,
            container_tag: TAG.to_string(),
            tags: None,
            memory_type: Some("memory".to_string()),
            created_at,
            updated_at: created_at,
            metadata: metadata.map(str::to_string),
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
            source_prompt_created_at: None,
        },
    )
    .await
    .expect("insert memory");
}

async fn seed_episode_row(fx: &Tier1Fixture, session_id: &str, summary: &str) {
    fx.store
        .insert_session(
            NewAgentSession {
                project_id: "/srv/proj".into(),
                session_id: session_id.into(),
                agent_id: "agent-1".into(),
                summary: summary.into(),
                ..Default::default()
            },
            &FakeEmbedder::with_vector(common::unit_vector(0)),
        )
        .await
        .expect("insert_session");
}

fn chat_params(value: serde_json::Value) -> InjectParams {
    serde_json::from_value(value).expect("params")
}

const SUMMARY_SELF: &str = r#"{"source":"auto-capture","sessionID":"sess-self"}"#;
const SUMMARY_OTHER: &str = r#"{"source":"auto-capture","sessionID":"sess-other"}"#;

/// Rule: exclusion removes the requester's own capture summary too, else the
/// session is re-injected into itself through tier 1. Mutant: exclusion only
/// filters episodic rows.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_exclusion_drops_the_requesting_sessions_tier1_summary() {
    let fx = tier1_fixture("t1-exclude", 1_000_000).await;
    seed_tier1(
        &fx.shards,
        "m-self",
        "T1_SELF_SUMMARY",
        Some(SUMMARY_SELF),
        1_000_000,
    )
    .await;
    seed_tier1(
        &fx.shards,
        "m-other",
        "T1_OTHER_SUMMARY",
        Some(SUMMARY_OTHER),
        1_000_000,
    )
    .await;

    let result = fx
        .service
        .assemble(&chat_params(json!({
            "project_id": "/srv/proj", "session_id": "sess-self",
            "exclude_session": true, "token_budget": 5000
        })))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert!(
        !block.contains("T1_SELF_SUMMARY"),
        "own summary must go: {block}"
    );
    assert!(
        block.contains("T1_OTHER_SUMMARY"),
        "other summary kept: {block}"
    );
}

/// Rule: max_age_days applies to tier-1 rows too. Mutant: age only filters
/// episodic rows. Fact (no session link) so exclusion cannot explain the drop.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_max_age_days_drops_old_tier1_rows() {
    const NOW: i64 = 1_700_000_000_000;
    let fx = tier1_fixture("t1-age", NOW).await;
    seed_tier1(
        &fx.shards,
        "m-old",
        "T1_OLD_FACT",
        None,
        NOW - 20 * 86_400_000,
    )
    .await;
    seed_tier1(
        &fx.shards,
        "m-new",
        "T1_NEW_FACT",
        None,
        NOW - 5 * 86_400_000,
    )
    .await;

    let result = fx
        .service
        .assemble(&chat_params(json!({
            "project_id": "/srv/proj", "max_age_days": 10, "token_budget": 5000
        })))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert!(!block.contains("T1_OLD_FACT"), "old fact must go: {block}");
    assert!(block.contains("T1_NEW_FACT"), "new fact kept: {block}");
}

/// Rule: `max_memories` bounds the EMITTED count, so a candidate the budget
/// rejects must not consume a slot. Mutant: truncate before the budget pass —
/// the oversized fact takes slot 1, is then budget-dropped, and only one row
/// is emitted although two episodic rows would fit.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_max_memories_counts_emitted_rows_not_budget_rejected_candidates() {
    let fx = tier1_fixture("cap-budget", 1_000_000).await;
    // 600 bytes > tier-1 sub-budget (1600 / 4 = 400): always budget-rejected.
    seed_tier1(&fx.shards, "m-big", &"F".repeat(600), None, 1_000_000).await;
    seed_episode_row(&fx, "sess-a", "CAP_ROW_A").await;
    tokio::time::sleep(Duration::from_millis(15)).await;
    seed_episode_row(&fx, "sess-b", "CAP_ROW_B").await;

    let result = fx
        .service
        .assemble(&chat_params(json!({
            "project_id": "/srv/proj", "max_memories": 2, "token_budget": 400,
            "include": { "recent_count": 10 }
        })))
        .await
        .expect("assemble");
    assert_eq!(result.memory_count, 2, "block: {:?}", result.block);
}

/// Rule: a summary suppressed as a duplicate of an accepted episodic row must
/// not consume a cap slot. Mutant: truncate before suppression — summary X and
/// R_X take both slots, suppression then leaves one row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_max_memories_slot_not_lost_to_duplicate_suppression() {
    let fx = tier1_fixture("cap-dup", 1_000_000).await;
    seed_tier1(
        &fx.shards,
        "m-x",
        "DUP_SUMMARY_X",
        Some(r#"{"source":"auto-capture","sessionID":"sess-x"}"#),
        1_000_000,
    )
    .await;
    seed_episode_row(&fx, "sess-x", "DUP_ROW_X").await;
    tokio::time::sleep(Duration::from_millis(15)).await;
    seed_episode_row(&fx, "sess-y", "DUP_ROW_Y").await;

    let result = fx
        .service
        .assemble(&chat_params(json!({
            "project_id": "/srv/proj", "max_memories": 2, "token_budget": 5000,
            "include": { "recent_count": 10 }
        })))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert!(
        !block.contains("DUP_SUMMARY_X"),
        "duplicate suppressed: {block}"
    );
    assert_eq!(result.memory_count, 2, "block: {block}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_policy_enabled_false_refuses_chat_block_while_direct_and_profile_intact() {
    let root = TestDir::new("ac1-root");
    let home = TestDir::new("ac1-home");
    let (engine, store, project_id) = production_engine(&root, &home, |cfg| {
        cfg.chat_message.enabled = false;
    })
    .await;
    let embedder = FakeEmbedder::with_vector(common::unit_vector(0));

    store
        .insert_session(
            NewAgentSession {
                project_id: project_id.clone(),
                session_id: "ses-1".into(),
                agent_id: "agent-1".into(),
                summary: "SESSION_1_SUMMARY".into(),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert");

    // Chat inject: chat_injection: true -> must refuse block (block is null, memory_count == 0)
    let chat_resp = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "ses-req",
            "project_id": project_id,
            "chat_injection": true,
            "token_budget": 5000,
            "include": { "recent_count": 10 }
        }),
    )
    .await;
    let chat_res = chat_resp.result.expect("result");
    assert!(
        chat_res["block"].is_null(),
        "chat block must be null when chat_message.enabled=false: {chat_res}"
    );
    assert_eq!(chat_res["memory_count"], 0);

    // Direct inject: chat_injection omitted -> block intact
    let direct_resp = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "ses-req",
            "project_id": project_id,
            "token_budget": 5000,
            "include": { "recent_count": 10 }
        }),
    )
    .await;
    let direct_res = direct_resp.result.expect("result");
    let direct_block = direct_res["block"]
        .as_str()
        .expect("direct block must exist");
    assert!(
        direct_block.contains("SESSION_1_SUMMARY"),
        "direct block must contain summary: {direct_block}"
    );
    assert!(direct_res["memory_count"].as_u64().unwrap_or(0) > 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_policy_enabled_false_profile_tier_preserved() {
    let fx = tier1_fixture("profile-tier-preserved", 1_000_000).await;
    seed_episode_row(&fx, "sess-1", "EPISODIC_ROW_SUMMARY").await;

    let profile = Arc::new(mem_profile::profile_store::ProfileStore::new(
        fx._dir.path().to_path_buf(),
        fx._dir.store_config(),
        mem_profile::profile_store::ProfileConfig::default(),
    ));
    profile
        .create_profile(
            "testuser@example.com",
            "Test User",
            "testuser",
            "testuser@example.com",
            &mem_profile::profile_store::ProfileData {
                preferences: vec![mem_profile::profile_store::ProfileItem {
                    description: "Prefers cargo test".to_string(),
                    category: Some("tooling".to_string()),
                    confidence: Some(0.9),
                    frequency: Some(3),
                    ..Default::default()
                }],
                ..Default::default()
            },
            1,
        )
        .await
        .expect("create profile");

    let mut config = test_config(fx._dir.path().to_path_buf(), fx._dir.path());
    config.chat_message.enabled = false;
    config.inject_profile = true;

    let service = fx
        .service
        .with_profile(
            profile,
            mem_profile::profile_context::ProfileInjectPolicy {
                enabled: true,
                caps: mem_profile::profile_context::ProfileRenderCaps {
                    preferences: 5,
                    patterns: 5,
                    workflows: 3,
                },
            },
        )
        .with_chat_policy(mem_bridge::inject::ChatInjectionPolicy::from_core(&config));

    let params: InjectParams = serde_json::from_value(json!({
        "project_id": "/srv/proj",
        "user_id": "testuser@example.com",
        "chat_injection": true,
        "token_budget": 5000,
        "include": { "recent_count": 10 }
    }))
    .expect("params");

    let result = service.assemble(&params).await.expect("assemble");
    assert_eq!(
        result.profile,
        mem_bridge::inject::ProfileInjection::Rendered
    );
    assert_eq!(result.memory_count, 0);
    let block = result.block.expect("block exists");
    assert!(
        block.contains("<user_profile>"),
        "profile must be present: {block}"
    );
    assert!(
        !block.contains("EPISODIC_ROW_SUMMARY"),
        "episodic summary must be absent: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_policy_enabled_true_allows_chat_block_on_eligible_rows() {
    let root = TestDir::new("ac1-true-root");
    let home = TestDir::new("ac1-true-home");
    let (engine, store, project_id) = production_engine(&root, &home, |cfg| {
        cfg.chat_message.enabled = true;
    })
    .await;
    let embedder = FakeEmbedder::with_vector(common::unit_vector(0));

    store
        .insert_session(
            NewAgentSession {
                project_id: project_id.clone(),
                session_id: "ses-1".into(),
                agent_id: "agent-1".into(),
                summary: "SESSION_1_SUMMARY".into(),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert");

    let chat_resp = dispatch_req(
        &engine,
        "inject",
        json!({
            "session_id": "ses-req",
            "project_id": project_id,
            "chat_injection": true,
            "token_budget": 5000,
            "include": { "recent_count": 10 }
        }),
    )
    .await;
    let chat_res = chat_resp.result.expect("result");
    let block = chat_res["block"].as_str().expect("chat block must exist");
    assert!(
        block.contains("SESSION_1_SUMMARY"),
        "chat block must contain summary: {block}"
    );
    assert!(chat_res["memory_count"].as_u64().unwrap_or(0) > 0);
}
