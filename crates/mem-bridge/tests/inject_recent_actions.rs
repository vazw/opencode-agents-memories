//! Issue 149 — the `## Recent actions` section of the inject block.
//!
//! Actions render for the requesting session only, first in tier order, are
//! ordinary candidates (never reserved bytes, never counted in `memory_count`),
//! survive `exclude_session`, honour the age cutoff, and vanish once ten newer
//! project rows evict the child from the read window.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use common::{FakeEmbedder, FixedScopeResolver, TestDir};
use mem_bridge::inject::{InjectParams, InjectService};
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::{Clock, EpisodicStore};
use mem_store::serde_json::json;

const DAY_MS: i64 = 86_400_000;

fn params(include: mem_store::serde_json::Value) -> InjectParams {
    InjectParams {
        project_id: Some("/srv/proj".to_string()),
        token_budget: Some(2_000),
        include: Some(include),
        user_id: None,
        session_id: None,
        ..Default::default()
    }
}

async fn fixture(name: &str, clock: Option<Clock>) -> (TestDir, Arc<EpisodicStore>, InjectService) {
    let dir = TestDir::new(&format!("w149-inject-{name}"));
    let mut store = EpisodicStore::new(dir.path().to_path_buf(), dir.store_config());
    if let Some(clock) = clock {
        store = store.with_clock(clock);
    }
    let store = Arc::new(store);
    store.ready().await.expect("store ready");
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let service = InjectService::new(
        store.clone(),
        shards,
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    (dir, store, service)
}

/// Seeds one episodic row. `actions` empty means the row has no action lines.
async fn seed(
    store: &EpisodicStore,
    session_id: &str,
    summary: &str,
    actions: &[&str],
    deterministic: bool,
) -> String {
    let session = NewAgentSession {
        project_id: "/srv/proj".to_string(),
        session_id: session_id.to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: "Builder".to_string(),
        summary: summary.to_string(),
        recent_actions: actions.iter().map(|a| a.to_string()).collect(),
        capture_deterministic: deterministic,
        ..Default::default()
    };
    store
        .insert_session(session, &FakeEmbedder::with_vector(common::unit_vector(0)))
        .await
        .expect("seed row")
}

/// Seeds one promoted fact row (tier-1 fact, no session).
async fn seed_fact(shards: &Arc<mem_store::shard_manager::ShardManager>, content: &str) {
    let record = mem_episodic::promotion::PromotionRecord {
        id: format!("fact-{}", content.len()),
        content: content.to_string(),
        vector: common::unit_vector(2),
        container_tag: "opencode_project_0123456789abcdef".to_string(),
        memory_type: "decision".to_string(),
        created_at: 1_000,
        source_row_ids: vec![],
    };
    let sink = mem_episodic::promotion::ShardPermanentMemorySink::new(shards.clone(), 8);
    mem_episodic::promotion::PermanentMemorySink::promote(&sink, "0123456789abcdef", &record)
        .await
        .expect("promote fact");
}

fn shards_for(dir: &TestDir) -> Arc<mem_store::shard_manager::ShardManager> {
    Arc::new(common::shard_manager(dir.store_config()))
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_section_renders_first_for_the_current_session() {
    let (dir, store, service) = fixture("first", None).await;
    seed(
        &store,
        "sess-cur",
        "child summary",
        &["bash: a", "bash: b"],
        false,
    )
    .await;
    seed_fact(&shards_for(&dir), "promoted fact line").await;

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .expect("block");

    assert!(block.contains("bash: a"), "block: {block}");
    assert!(block.contains("bash: b"), "block: {block}");
    let actions = block.find("## Recent actions").expect("actions heading");
    let facts = block.find("## Project facts").expect("facts heading");
    assert!(actions < facts, "actions must precede facts: {block}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_section_is_absent_for_another_session() {
    let (_dir, store, service) = fixture("other", None).await;
    seed(
        &store,
        "sess-other",
        "other summary",
        &["bash: secret-other"],
        false,
    )
    .await;

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(!block.contains("## Recent actions"), "block: {block}");
    assert!(!block.contains("bash: secret-other"), "block: {block}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_section_is_absent_for_a_row_without_actions() {
    let (_dir, store, service) = fixture("no-actions", None).await;
    seed(&store, "sess-cur", "child summary", &[], false).await;

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(
        !block.contains("## Recent actions"),
        "empty heading emitted: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_survive_a_tight_budget_and_truncated_reports_the_dropped_fact() {
    let (dir, store, service) = fixture("tight", None).await;
    seed(
        &store,
        "sess-cur",
        "child summary",
        &["bash: keep-me"],
        false,
    )
    .await;
    seed_fact(&shards_for(&dir), &"fact padding ".repeat(40)).await;

    // Sized so the actions (heading + one line + bullets) fit and the long
    // fact does not. 2 bytes per token; fixed overhead is charged in greedy.
    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    p.token_budget = Some(120);
    let result = service.assemble(&p).await.expect("assemble");
    let block = result.block.unwrap_or_default();

    assert!(block.contains("bash: keep-me"), "block: {block}");
    assert!(
        !block.contains("fact padding fact padding fact padding fact padding"),
        "the fact must not fit: {block}"
    );
    assert!(result.truncated, "the dropped fact must report truncated");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_do_not_raise_memory_count_or_consume_max_memories() {
    let (dir, store, service) = fixture("count", None).await;
    seed(
        &store,
        "sess-cur",
        "child summary",
        &["bash: a1", "bash: a2"],
        false,
    )
    .await;
    seed(&store, "sess-x", "row x", &[], false).await;
    seed(&store, "sess-y", "row y", &[], false).await;
    seed_fact(&shards_for(&dir), "count fact").await;

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    p.max_memories = Some(1);
    let result = service.assemble(&p).await.expect("assemble");
    let block = result.block.expect("block");

    assert!(
        block.contains("## Recent actions"),
        "actions survive the cap: {block}"
    );
    assert!(
        block.contains("bash: a1") && block.contains("bash: a2"),
        "block: {block}"
    );
    // Only one counted memory is emitted; the action lines are not counted.
    assert_eq!(result.memory_count, 1, "memory_count excludes actions");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_render_even_when_the_current_session_is_excluded_from_history() {
    let (_dir, store, service) = fixture("excluded", None).await;
    seed(
        &store,
        "sess-cur",
        "child summary",
        &["bash: still-here"],
        false,
    )
    .await;

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    p.exclude_session = Some(true);
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(block.contains("## Recent actions"), "block: {block}");
    assert!(block.contains("bash: still-here"), "block: {block}");
    assert!(
        !block.contains("child summary"),
        "the excluded row's summary stays out of history: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_are_dropped_past_the_age_cutoff() {
    // The inject cutoff reads the wall clock, so the row is seeded relative
    // to it: ten days old, against a one-day window.
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let old: Clock = Arc::new(move || wall - 10 * DAY_MS);
    let dir = TestDir::new("w149-inject-age");
    let store =
        Arc::new(EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(old));
    store.ready().await.expect("store ready");
    seed(
        &store,
        "sess-cur",
        "old child",
        &["bash: stale-action"],
        false,
    )
    .await;
    let service = InjectService::new(
        store.clone(),
        shards_for(&dir),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    p.max_age_days = Some(1);
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(!block.contains("## Recent actions"), "block: {block}");
    assert!(!block.contains("bash: stale-action"), "block: {block}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_other_sessions_get_a_neutral_line_not_the_action_text() {
    let (_dir, store, service) = fixture("neutral", None).await;
    seed(
        &store,
        "sess-a",
        "cargo test --private-flag summary",
        &["bash: command: \"cargo test --private-flag\""],
        true,
    )
    .await;

    let mut p = params(json!({}));
    p.session_id = Some("sess-b".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(
        block.contains("recent actions recorded without a provider summary"),
        "neutral line missing: {block}"
    );
    assert!(
        !block.contains("cargo test --private-flag"),
        "the deterministic summary leaked into the block: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_vanish_once_ten_newer_sessions_outrank_the_child() {
    // Child at clock 1_000; nine newer rows keep it in the 10-row window,
    // a tenth newer row evicts it. Pins the `2 × recent_count` ceiling.
    let child_clock: Clock = Arc::new(|| 1_000);
    let (dir, store, service) = fixture("evict", None).await;
    let child_store =
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(child_clock);
    seed(
        &child_store,
        "sess-cur",
        "child",
        &["bash: window-child"],
        false,
    )
    .await;

    let mut newer = 0;
    for n in 0..10 {
        let clock_value = 2_000 + n as i64;
        let c: Clock = Arc::new(move || clock_value);
        let s = EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(c);
        seed(&s, &format!("newer-{n}"), "newer row", &[], false).await;
        newer += 1;

        let mut p = params(json!({}));
        p.session_id = Some("sess-cur".to_string());
        let block = service
            .assemble(&p)
            .await
            .expect("assemble")
            .block
            .unwrap_or_default();
        let present = block.contains("## Recent actions");
        if newer < 10 {
            assert!(present, "child inside the window after {newer} newer rows");
        } else {
            assert!(!present, "child evicted after {newer} newer rows: {block}");
        }
    }
    drop(store);
}
