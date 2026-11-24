//! Issue 149 — the `## Recent actions` section of the inject block.
//!
//! Actions render for the requesting session only, first in tier order, are
//! ordinary candidates (never reserved bytes, never counted in `memory_count`),
//! survive `exclude_session`, and come from the requesting session's own row —
//! the newest row of that session that carries actions, whichever agent wrote
//! it. Neither the `2 x recent_count` project window nor the age cutoff governs
//! that row: a session displaced by newer project rows, or resumed days later,
//! still renders its own lines.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use common::{FakeEmbedder, FixedScopeResolver, TestDir};
use mem_bridge::inject::{DEFAULT_RECENT_COUNT, InjectParams, InjectService};
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
    let service = service_for(&dir, store.clone());
    (dir, store, service)
}

/// An inject service over an already-prepared store in `dir`.
fn service_for(dir: &TestDir, store: Arc<EpisodicStore>) -> InjectService {
    InjectService::new(
        store,
        Arc::new(common::shard_manager(dir.store_config())),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Arc::new(mem_store::platform::SystemProcessProbe),
    )
}

/// Seeds one episodic row. `actions` empty means the row has no action lines.
async fn seed(
    store: &EpisodicStore,
    session_id: &str,
    summary: &str,
    actions: &[&str],
    deterministic: bool,
) -> String {
    seed_as(
        store,
        session_id,
        "agent-1",
        summary,
        actions,
        deterministic,
    )
    .await
}

/// Seeds one episodic row under a chosen `agent_id`: the upsert key is
/// `(project_id, session_id, agent_id)`, so a session holds one row per agent.
async fn seed_as(
    store: &EpisodicStore,
    session_id: &str,
    agent_id: &str,
    summary: &str,
    actions: &[&str],
    deterministic: bool,
) -> String {
    seed_in(
        store,
        "/srv/proj",
        session_id,
        agent_id,
        summary,
        actions,
        deterministic,
    )
    .await
}

/// `seed_as` in an explicit project: the IDOR oracle needs a row whose
/// `session_id` exists only outside the requesting project.
async fn seed_in(
    store: &EpisodicStore,
    project_id: &str,
    session_id: &str,
    agent_id: &str,
    summary: &str,
    actions: &[&str],
    deterministic: bool,
) -> String {
    let session = NewAgentSession {
        project_id: project_id.to_string(),
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
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
async fn test_actions_come_from_the_newest_row_that_has_them() {
    // The upsert key is `(project_id, session_id, agent_id)`, so one session
    // holds several rows. The newest row *that carries actions* feeds the
    // section: an empty newer row cannot mask an older one.
    let dir = TestDir::new("w149-inject-agent-rows");
    let ticks = Arc::new(AtomicI64::new(1_000));
    let reader = ticks.clone();
    let clock: Clock = Arc::new(move || reader.load(Ordering::SeqCst));
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock),
    );
    store.ready().await.expect("store ready");
    seed_as(
        &store,
        "sess-cur",
        "agent-old",
        "old agent",
        &["bash: older-agent"],
        false,
    )
    .await;
    ticks.store(2_000, Ordering::SeqCst);
    seed_as(&store, "sess-cur", "agent-new", "new agent", &[], false).await;

    let service = service_for(&dir, store.clone());
    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(block.contains("## Recent actions"), "block: {block}");
    assert!(block.contains("bash: older-agent"), "block: {block}");
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
    // Discriminating sizing, measured with the `greedy` cost model (not
    // guessed). Each candidate costs `line + BULLET_BYTES (3) + heading`.
    // Fixed overhead `fixed_overhead_bytes()` = 257 (measured).
    //   action:  line "bash: keep-me" = 13 B, heading "## Recent actions" 18 B
    //            -> cost 13 + 3 + 18 = 34, fits alone: 257 + 34 = 291 <= 380.
    //   fact:    line = 70 `f` chars (fact renders raw, measured), heading
    //            "## Project facts" 17 B -> cost 70 + 3 + 17 = 90, fits alone:
    //            257 + 90 = 347 <= 380. Tier-1 cap = 380 / 4 = 95 >= 90, so the
    //            cap does not drop it.
    //   together: 257 + 34 + 90 = 381 > 380, so exactly one of them is kept.
    // Whichever candidate greedy meets first wins. Actions are extended first,
    // so the actions win and the fact is truncated. A reorder (facts first)
    // keeps the fact and drops the action: the assertions below go red.
    let fact_text = "f".repeat(70);
    seed_fact(&shards_for(&dir), &fact_text).await;

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    p.token_budget = Some(95); // 95 tokens * 4 bytes = 380 bytes
    let result = service.assemble(&p).await.expect("assemble");
    let block = result.block.unwrap_or_default();

    assert!(block.contains("## Recent actions"), "block: {block}");
    assert!(block.contains("bash: keep-me"), "block: {block}");
    assert!(
        !block.contains(&fact_text),
        "the fact must lose to the actions under this budget: {block}"
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
async fn test_actions_render_for_an_old_session_resumed_days_later() {
    // The own row is the agent's position, not history: the age cutoff bounds
    // history only. Resumed ten days later against a one-day window, the row's
    // actions still render while its summary stays out of `## Recent sessions`.
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
        &["bash: resumed-action"],
        false,
    )
    .await;
    let service = service_for(&dir, store.clone());

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    p.max_age_days = Some(1);
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(block.contains("## Recent actions"), "block: {block}");
    assert!(block.contains("bash: resumed-action"), "block: {block}");
    assert!(
        !block.contains("old child"),
        "the old row's summary stays out of history: {block}"
    );
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
async fn test_actions_render_for_own_row_beyond_the_project_window() {
    // The measured defect: subagents wrote more rows than the read window
    // serves, so the requesting session's own row sat far outside it. The own
    // row keeps its actions regardless.
    let dir = TestDir::new("w149-inject-own-row");
    let ticks = Arc::new(AtomicI64::new(1_000));
    let reader = ticks.clone();
    let clock: Clock = Arc::new(move || reader.load(Ordering::SeqCst));
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock),
    );
    store.ready().await.expect("store ready");
    seed(
        &store,
        "sess-cur",
        "own summary",
        &["bash: owned-action"],
        false,
    )
    .await;

    for n in 0..(DEFAULT_RECENT_COUNT * 2 + 2) {
        ticks.store(2_000 + i64::from(n), Ordering::SeqCst);
        seed(&store, &format!("newer-{n}"), "newer row", &[], false).await;
    }

    let service = service_for(&dir, store.clone());
    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(block.contains("## Recent actions"), "block: {block}");
    assert!(block.contains("bash: owned-action"), "block: {block}");
    assert!(
        !block.contains("own summary"),
        "the own row's summary stays out of history: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_come_from_the_newest_carrying_row_not_the_oldest() {
    // Two rows of the same session both carry actions. The newest one wins:
    // `max_by(created_at)` keeps it, `min_by` would render the older row and a
    // `flat_map` over every row would render both.
    let dir = TestDir::new("w149-inject-two-carrying");
    let ticks = Arc::new(AtomicI64::new(1_000));
    let reader = ticks.clone();
    let clock: Clock = Arc::new(move || reader.load(Ordering::SeqCst));
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock),
    );
    store.ready().await.expect("store ready");
    seed_as(
        &store,
        "sess-cur",
        "agent-old",
        "old agent",
        &["bash: older-agent"],
        false,
    )
    .await;
    ticks.store(2_000, Ordering::SeqCst);
    seed_as(
        &store,
        "sess-cur",
        "agent-new",
        "new agent",
        &["bash: newer-agent"],
        false,
    )
    .await;

    let service = service_for(&dir, store.clone());
    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(block.contains("bash: newer-agent"), "block: {block}");
    assert!(
        !block.contains("bash: older-agent"),
        "only the newest carrying row feeds the section: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_break_a_created_at_tie_by_row_id() {
    // Two carrying rows at the same tick: "created_at, then id" must decide,
    // and the minted ids make the winner knowable ("row-0001" > "row-0000").
    // Dropping the `id` tie-break leaves `max_by` on the last equal element,
    // which the SQL order makes the older id.
    let dir = TestDir::new("w149-inject-tie");
    let counter = Arc::new(AtomicU64::new(0));
    let ids = counter.clone();
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config())
            .with_id_factory(Arc::new(move || {
                let index = ids.fetch_add(1, Ordering::SeqCst);
                format!("row-{index:04}")
            }))
            .with_clock(Arc::new(|| 1_000)),
    );
    store.ready().await.expect("store ready");
    seed_as(
        &store,
        "sess-cur",
        "agent-alpha",
        "alpha",
        &["bash: alpha-action"],
        false,
    )
    .await;
    seed_as(
        &store,
        "sess-cur",
        "agent-beta",
        "beta",
        &["bash: beta-action"],
        false,
    )
    .await;

    let service = service_for(&dir, store.clone());
    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(block.contains("bash: beta-action"), "block: {block}");
    assert!(
        !block.contains("bash: alpha-action"),
        "the larger row id must win a created_at tie: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_follow_created_at_not_updated_at() {
    // A recapture moves `updated_at` without moving `created_at`. The rule is
    // the newest row, so the row created later (agent-new) wins even though
    // the recaptured older row was updated last.
    let dir = TestDir::new("w149-inject-updated");
    let ticks = Arc::new(AtomicI64::new(1_000));
    let reader = ticks.clone();
    let clock: Clock = Arc::new(move || reader.load(Ordering::SeqCst));
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock),
    );
    store.ready().await.expect("store ready");
    seed_as(
        &store,
        "sess-cur",
        "agent-old",
        "old agent",
        &["bash: older-agent"],
        false,
    )
    .await;
    ticks.store(2_000, Ordering::SeqCst);
    seed_as(
        &store,
        "sess-cur",
        "agent-new",
        "new agent",
        &["bash: newer-agent"],
        false,
    )
    .await;
    ticks.store(3_000, Ordering::SeqCst);
    let recapture = NewAgentSession {
        project_id: "/srv/proj".to_string(),
        session_id: "sess-cur".to_string(),
        agent_id: "agent-old".to_string(),
        agent_name: "Builder".to_string(),
        summary: "old agent, recaptured".to_string(),
        recent_actions: vec!["bash: older-agent".to_string()],
        capture_id: Some("cap-2".to_string()),
        ..Default::default()
    };
    store
        .upsert_session(
            recapture,
            &FakeEmbedder::with_vector(common::unit_vector(0)),
        )
        .await
        .expect("recapture");

    let service = service_for(&dir, store.clone());
    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(block.contains("bash: newer-agent"), "block: {block}");
    assert!(
        !block.contains("bash: older-agent"),
        "the created_at order decides, not updated_at: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_do_not_leak_from_another_project() {
    // A session id reused in another project is a different session: the
    // session-keyed read must stay project-scoped.
    let dir = TestDir::new("w149-inject-cross-project");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    seed_in(
        &store,
        "/srv/other",
        "sess-cur",
        "agent-1",
        "other summary",
        &["bash: other-project-action"],
        false,
    )
    .await;

    let service = service_for(&dir, store.clone());
    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(!block.contains("## Recent actions"), "block: {block}");
    assert!(
        !block.contains("bash: other-project-action"),
        "block: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_section_is_absent_for_a_soft_deleted_own_row() {
    // A soft-deleted own row is not live: its actions must not render. Dropping
    // `deleted_at IS NULL` from the session-keyed read renders them.
    let (_dir, store, service) = fixture("soft-deleted", None).await;
    let id = seed(
        &store,
        "sess-cur",
        "deleted summary",
        &["bash: deleted-action"],
        false,
    )
    .await;
    assert!(
        store
            .soft_delete_session("/srv/proj", &id)
            .await
            .expect("soft delete"),
        "soft delete must hit the seeded row"
    );

    let mut p = params(json!({}));
    p.session_id = Some("sess-cur".to_string());
    let block = service
        .assemble(&p)
        .await
        .expect("assemble")
        .block
        .unwrap_or_default();

    assert!(!block.contains("## Recent actions"), "block: {block}");
    assert!(!block.contains("bash: deleted-action"), "block: {block}");
}
