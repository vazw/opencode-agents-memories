//! Issue 50b — the scored project-knowledge read (O1, O4) and, as later tests
//! are added in their own steps, the pinned/ordering/query/cap/suppression
//! oracles.
//!
//! Rows are seeded through the REAL shard write seam
//! (`get_write_shard_allowing_own_lock` + `insert_vector`) so the read sees
//! exactly what capture would produce, and the embedder is a named
//! `Arc<FakeEmbedder>` so `calls()` observes every embed.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use common::{FakeEmbedder, FixedScopeResolver, TestDir};
use mem_bridge::inject::{InjectParams, InjectService};
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::{Value, json};

const TAG: &str = "opencode_project_0123456789abcdef";
const HASH: &str = "0123456789abcdef";
const NOW: i64 = 1_000;

struct Fixture {
    _dir: TestDir,
    store: Arc<EpisodicStore>,
    shards: Arc<mem_store::shard_manager::ShardManager>,
    embedder: Arc<FakeEmbedder>,
    service: InjectService,
}

async fn fixture(name: &str) -> Fixture {
    let dir = TestDir::new(&format!("w50-inject-scored-{name}"));
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let embedder = Arc::new(FakeEmbedder::with_vector(common::unit_vector(0)));
    let service = InjectService::new(
        store.clone(),
        shards.clone(),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        embedder.clone(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    Fixture {
        _dir: dir,
        store,
        shards,
        embedder,
        service,
    }
}

fn params(include: Value) -> InjectParams {
    InjectParams {
        project_id: Some("/srv/proj".to_string()),
        agent_id: None,
        token_budget: Some(2_000),
        include: Some(include),
        user_id: None,
        session_id: None,
    }
}

async fn seed_memory(
    shards: &Arc<mem_store::shard_manager::ShardManager>,
    id: &str,
    content: &str,
    vector: Vec<f32>,
    tags_vector: Option<Vec<f32>>,
    metadata: Option<String>,
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
            vector,
            tags_vector,
            container_tag: TAG.to_string(),
            tags: None,
            memory_type: Some("memory".to_string()),
            created_at,
            updated_at: created_at,
            metadata,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
        },
    )
    .await
    .expect("insert memory");
}

async fn pin_memory(shards: &Arc<mem_store::shard_manager::ShardManager>, id: &str) {
    let shard = shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("write shard");
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    mem_store::vector::pin_memory(db.as_ref(), id)
        .await
        .expect("pin memory");
}

async fn seed_episode(
    store: &EpisodicStore,
    embedder: &FakeEmbedder,
    session_id: &str,
    summary: &str,
) {
    let session = mem_episodic::record::NewAgentSession {
        project_id: "/srv/proj".to_string(),
        session_id: session_id.to_string(),
        agent_id: "agent-1".to_string(),
        summary: summary.to_string(),
        ..Default::default()
    };
    store
        .insert_session(session, embedder)
        .await
        .expect("seed episode");
}

fn forty(prefix: char) -> String {
    format!("{prefix}{}", prefix.to_string().repeat(39))
}

// ---------------------------------------------------------------------------
// O1 — the score is the content cosine, never a hybrid rank
// ---------------------------------------------------------------------------

/// A content-identical row renders `100%`; a tags-only match renders `0%`; the
/// `(e0+e1)/√2` row renders `71%`. `search_in_shard`'s hybrid would print
/// `50%` / `30%` for the same rows (keyword token present, no lexical hit), and
/// upstream's fabricated `1.0` would put the tags-only row at `100%`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_scored_facts_report_content_similarity_not_a_hybrid_rank() {
    let fixture = fixture("o1").await;
    let a = forty('A');
    let b = forty('B');
    let c = forty('C');
    let d = forty('D');
    let diagonal: Vec<f32> = (0..8)
        .map(|index| {
            if index < 2 {
                std::f32::consts::FRAC_1_SQRT_2
            } else {
                0.0
            }
        })
        .collect();

    seed_memory(
        &fixture.shards,
        "mem_A",
        &a,
        common::unit_vector(0),
        None,
        None,
        NOW,
    )
    .await;
    seed_memory(
        &fixture.shards,
        "mem_B",
        &b,
        common::unit_vector(1),
        None,
        None,
        NOW,
    )
    .await;
    seed_memory(&fixture.shards, "mem_C", &c, diagonal, None, None, NOW).await;
    seed_memory(
        &fixture.shards,
        "mem_D",
        &d,
        common::unit_vector(1),
        Some(common::unit_vector(0)),
        None,
        NOW,
    )
    .await;

    let result = fixture
        .service
        .assemble(&params(json!({ "recent_count": 5, "query": "zzz" })))
        .await
        .expect("assemble");
    let block = result.block.expect("the scored facts render");

    assert!(
        block.contains(&format!(r#"<memory relevance="100%">{a}</memory>"#)),
        "the content-identical row scores 100%: {block}"
    );
    assert!(
        block.contains(&format!(r#"<memory relevance="71%">{c}</memory>"#)),
        "the 45° row scores 71%: {block}"
    );
    assert!(
        block.contains(&format!(r#"<memory relevance="0%">{d}</memory>"#)),
        "the tags-only match scores 0%, never 30% or 100%: {block}"
    );
    assert!(
        !block.contains("relevance=\"50%\"") && !block.contains("relevance=\"30%\""),
        "no hybrid bytes in the scored arm: {block}"
    );
}

// ---------------------------------------------------------------------------
// O4 — the no-query arm keeps the tag-listed rows and emits no attribute
// ---------------------------------------------------------------------------

/// With `include: {}` the read is the tag list: a pinned row renders first, no
/// `relevance` attribute is emitted anywhere, and no query is embedded. This is
/// a guard (green before and after 50b); its mutation is M7a-ii.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_no_query_arm_keeps_the_tag_listed_rows_and_no_attribute() {
    let fixture = fixture("o4").await;
    let pinned = forty('P');
    seed_memory(
        &fixture.shards,
        "mem_P",
        &pinned,
        common::unit_vector(1),
        None,
        None,
        NOW + 1,
    )
    .await;
    pin_memory(&fixture.shards, "mem_P").await;

    let result = fixture
        .service
        .assemble(&params(json!({})))
        .await
        .expect("assemble");
    let block = result.block.expect("the tag-listed row renders");
    assert!(
        block.contains(&pinned),
        "the tag-listed row renders without a query: {block}"
    );
    assert!(
        !block.contains("relevance=\""),
        "the tag-listed arm emits no relevance attribute: {block}"
    );
    assert!(
        fixture.embedder.calls().is_empty(),
        "no query means no embed call: {:?}",
        fixture.embedder.calls()
    );
}

// ---------------------------------------------------------------------------
// O2 — a pinned, unrelated row is scored 0%, never fabricated to 100%
// ---------------------------------------------------------------------------

/// The tag list's first row is pinned on purpose: the precondition opens the
/// write shard and asserts `[0]` IS `mem_P`, so the negative is not vacuous.
/// The scored read ignores the pin and reports the content cosine — `0%` for
/// the `e1` row against an `e0` query. The tag-list-plus-1.0 implementation
/// (upstream's `loadMemoryContext` shape) would print `100%`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_a_pinned_tag_first_row_is_scored_zero_never_one_hundred() {
    let fixture = fixture("o2").await;
    let p = forty('P');
    seed_memory(
        &fixture.shards,
        "mem_P",
        &p,
        common::unit_vector(1),
        None,
        None,
        NOW + 5,
    )
    .await;
    // An unpinned older row so the pin genuinely decides `[0]`, not one-row luck.
    seed_memory(
        &fixture.shards,
        "mem_other",
        "OTHER-ROW",
        common::unit_vector(1),
        None,
        None,
        NOW,
    )
    .await;
    pin_memory(&fixture.shards, "mem_P").await;

    let shard = fixture
        .shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("write shard");
    let db = fixture
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    let rows = mem_store::vector::list_memories_by_container_tag(db.as_ref(), TAG, 200)
        .await
        .expect("tag list");
    assert_eq!(
        rows[0].text("id").as_deref(),
        Some("mem_P"),
        "precondition: the pinned row is the tag list's first row"
    );

    let result = fixture
        .service
        .assemble(&params(json!({ "recent_count": 5, "query": "zzz" })))
        .await
        .expect("assemble");
    let block = result.block.expect("the scored facts render");
    assert!(
        block.contains(&format!(r#"<memory relevance="0%">{p}</memory>"#)),
        "the pinned unrelated row scores 0%: {block}"
    );
    assert!(
        !block.contains(r#"<memory relevance="100%">"#),
        "no row is fabricated to 100%: {block}"
    );
}

// ---------------------------------------------------------------------------
// O3 — similarity descending, then recency, then id
// ---------------------------------------------------------------------------

/// A (1.0) at created_at 1_000, C (0.7071) at 2_000, B (0.0) at 3_000: the
/// recency order (B, C, A) is the exact REVERSE of the similarity order, so a
/// recency-only comparator puts A after C and reds the similarity assertion.
/// Within the 1.0 class the newer row (`mem_z`, 2_500) sorts lexicographically
/// AFTER the older (`mem_a`, 500), so an id-only tie-break puts the older row
/// first and reds the recency assertion. The id ascending tie-break is only the
/// last key.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_scored_facts_order_by_similarity_then_recency() {
    let fixture = fixture("o3").await;
    let a = forty('A');
    let b = forty('B');
    let c = forty('C');
    let new = forty('N');
    let old = forty('O');
    let diagonal: Vec<f32> = (0..8)
        .map(|index| {
            if index < 2 {
                std::f32::consts::FRAC_1_SQRT_2
            } else {
                0.0
            }
        })
        .collect();

    seed_memory(
        &fixture.shards,
        "mem_A",
        &a,
        common::unit_vector(0),
        None,
        None,
        1_000,
    )
    .await;
    seed_memory(&fixture.shards, "mem_C", &c, diagonal, None, None, 2_000).await;
    seed_memory(
        &fixture.shards,
        "mem_B",
        &b,
        common::unit_vector(1),
        None,
        None,
        3_000,
    )
    .await;
    seed_memory(
        &fixture.shards,
        "mem_z",
        &new,
        common::unit_vector(0),
        None,
        None,
        2_500,
    )
    .await;
    seed_memory(
        &fixture.shards,
        "mem_a",
        &old,
        common::unit_vector(0),
        None,
        None,
        500,
    )
    .await;

    let result = fixture
        .service
        .assemble(&params(json!({ "recent_count": 5, "query": "zzz" })))
        .await
        .expect("assemble");
    let block = result.block.expect("the scored facts render");
    let a_at = block.find(&a).expect("A present");
    let c_at = block.find(&c).expect("C present");
    let b_at = block.find(&b).expect("B present");
    assert!(
        a_at < c_at && c_at < b_at,
        "similarity descending: A={a_at} C={c_at} B={b_at}\n{block}"
    );
    let new_at = block.find(&new).expect("the newer equal row present");
    let old_at = block.find(&old).expect("the older equal row present");
    assert!(
        new_at < old_at,
        "the newer of the equal-similarity pair is first: new={new_at} old={old_at}\n{block}"
    );
}

// ---------------------------------------------------------------------------
// O5 / O5b — the query bound and the blank rule
// ---------------------------------------------------------------------------

/// 3000 `é` + `TAIL`: the engine embeds exactly 2000 code points, never a byte
/// slice (which would keep 1000 `é` here) and never the tail. One embed call,
/// because the vector is hoisted and shared by both legs.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_query_is_bounded_at_two_thousand_chars() {
    let fixture = fixture("o5").await;
    let query = format!("{}{}", "é".repeat(3000), "TAIL");

    let result = fixture
        .service
        .assemble(&params(json!({ "recent_count": 5, "query": query })))
        .await
        .expect("assemble");
    assert_eq!(result.block, None, "no rows: no block, but the embed ran");
    let calls = fixture.embedder.calls();
    assert_eq!(calls.len(), 1, "one embed per assemble: {calls:?}");
    assert_eq!(
        calls[0].chars().count(),
        2000,
        "the cap counts code points, not UTF-16 units or bytes"
    );
    assert!(!calls[0].contains("TAIL"), "the tail is cut");
}

/// A whitespace-only query is ABSENT: no embed call, today's tag-listed bytes,
/// no attribute.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_a_blank_query_is_treated_as_absent() {
    let fixture = fixture("o5b").await;
    let row = forty('K');
    seed_memory(
        &fixture.shards,
        "mem_K",
        &row,
        common::unit_vector(1),
        None,
        None,
        NOW,
    )
    .await;

    let result = fixture
        .service
        .assemble(&params(json!({ "query": "   " })))
        .await
        .expect("assemble");
    let block = result.block.expect("the tag-listed row renders");
    assert!(
        fixture.embedder.calls().is_empty(),
        "a blank query never reaches the embedder: {:?}",
        fixture.embedder.calls()
    );
    assert!(
        !block.contains("relevance=\""),
        "the blank-query arm emits no attribute: {block}"
    );
    assert!(
        block.contains(&row),
        "the tag-listed row still renders: {block}"
    );
}

// ---------------------------------------------------------------------------
// O6 — the attribute's bytes are charged inside the cap
// ---------------------------------------------------------------------------

/// Two 40-byte facts, the `100%` row first in similarity order. Its scored line
/// costs `40 + 34`, the `71%` line `40 + 33`, and the section heading 17, so the
/// accumulator is `17 + 77 = 94` after one row and `94 + 76 = 170` for the
/// second — over the 140-byte tier-1 cap. Applying `scored_line` after `greedy`
/// would emit both rows (170 > 140) and un-green this.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_scored_row_tags_are_charged_inside_the_cap() {
    let fixture = fixture("o6").await;
    let high = forty('H');
    let low = forty('L');
    let diagonal: Vec<f32> = (0..8)
        .map(|index| {
            if index < 2 {
                std::f32::consts::FRAC_1_SQRT_2
            } else {
                0.0
            }
        })
        .collect();
    seed_memory(
        &fixture.shards,
        "mem_high",
        &high,
        common::unit_vector(0),
        None,
        None,
        NOW,
    )
    .await;
    seed_memory(&fixture.shards, "mem_low", &low, diagonal, None, None, NOW).await;

    let result = fixture
        .service
        .assemble(&InjectParams {
            token_budget: Some(140),
            ..params(json!({ "recent_count": 5, "query": "zzz" }))
        })
        .await
        .expect("assemble");
    let block = result.block.expect("the 100% row renders");
    assert!(
        block.contains(&format!(r#"<memory relevance="100%">{high}</memory>"#)),
        "the top row survives: {block}"
    );
    assert!(
        !block.contains(&low),
        "the 71% row is evicted by the charged attribute bytes: {block}"
    );
    assert_eq!(result.memory_count, 1);
    assert!(result.truncated, "one candidate was dropped");
    assert!(
        block.len() <= 560,
        "the emitted block respects 4 x token_budget: {}",
        block.len()
    );
}

// ---------------------------------------------------------------------------
// O7 — the 25 % cap, in similarity order
// ---------------------------------------------------------------------------

fn cosine_vector(cosine: f32) -> Vec<f32> {
    let mut vector = vec![0.0_f32; 8];
    vector[0] = cosine;
    vector[1] = (1.0 - cosine * cosine).sqrt();
    vector
}

/// Four 700-byte facts at 0.9/0.8/0.7/0.6 with `token_budget: 2000` (tier-1 cap
/// 2000). The charged costs are `753 + 736 + 736 + 736 = 2961`, so the top two
/// fit (`753 + 736 = 1489`) and the two lowest are absent. A bypass emits all
/// four, 961 bytes over the cap.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_scored_rows_respect_the_one_quarter_cap_in_similarity_order() {
    let fixture = fixture("o7").await;
    let mut contents = Vec::new();
    for (index, cosine) in [0.9_f32, 0.8, 0.7, 0.6].into_iter().enumerate() {
        let content = format!("{index}{}", index.to_string().repeat(699));
        contents.push(content.clone());
        seed_memory(
            &fixture.shards,
            &format!("mem_{index}"),
            &content,
            cosine_vector(cosine),
            None,
            None,
            NOW,
        )
        .await;
    }

    let result = fixture
        .service
        .assemble(&InjectParams {
            token_budget: Some(2_000),
            ..params(json!({ "recent_count": 5, "query": "zzz" }))
        })
        .await
        .expect("assemble");
    let block = result.block.expect("the top two rows render");
    assert_eq!(result.memory_count, 2, "two rows fit under the cap");
    assert!(result.truncated, "the cap truncated the row set");
    assert!(
        block.contains(&contents[0]),
        "the 0.9 row survives: {block}"
    );
    assert!(
        block.contains(&contents[1]),
        "the 0.8 row survives: {block}"
    );
    assert!(!block.contains(&contents[2]), "the 0.7 row is evicted");
    assert!(!block.contains(&contents[3]), "the 0.6 row is evicted");
}

// ---------------------------------------------------------------------------
// O8 — duplicate suppression survives on the scored arm
// ---------------------------------------------------------------------------

/// The accepted episode `sess-1` suppresses its own capture summary even on the
/// scored arm; the `sess-2` summary still renders WITH the attribute. Dropping
/// the suppression would render the `sess-1` summary a second time.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_scored_summaries_carry_relevance_and_keep_suppression() {
    let fixture = fixture("o8").await;
    let summary_one = "SUMMARY-SESS-1";
    let summary_two = "SUMMARY-SESS-2";
    seed_memory(
        &fixture.shards,
        "summary_one",
        summary_one,
        common::unit_vector(0),
        None,
        Some(r#"{"source":"auto-capture","sessionID":"sess-1"}"#.to_string()),
        NOW,
    )
    .await;
    seed_memory(
        &fixture.shards,
        "summary_two",
        summary_two,
        common::unit_vector(0),
        None,
        Some(r#"{"source":"auto-capture","sessionID":"sess-2"}"#.to_string()),
        NOW,
    )
    .await;
    seed_episode(
        fixture.store.as_ref(),
        fixture.embedder.as_ref(),
        "sess-1",
        "episode one",
    )
    .await;

    let result = fixture
        .service
        .assemble(&params(json!({ "recent_count": 5, "query": "zzz" })))
        .await
        .expect("assemble");
    let block = result.block.expect("the summaries render");
    assert!(
        !block.contains(summary_one),
        "the accepted episode suppresses its own summary: {block}"
    );
    assert!(
        block.contains(&format!(
            r#"<memory relevance="100%">{summary_two}</memory>"#
        )),
        "the other session's summary carries the attribute: {block}"
    );
}
