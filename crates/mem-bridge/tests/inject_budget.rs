//! W5.3 Step 9 / AC6 — injection assembly oracles (`InjectService::assemble`
//! over the real episodic store and a real `ShardManager`).
//!
//! The fixture uses the FIXED scope resolver (`opencode_project_
//! 0123456789abcdef`), a fixed 8-dim embedder and REAL turso storage.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use common::{FakeEmbedder, FixedScopeResolver, TestDir};
use mem_bridge::inject::{InjectError, InjectParams, InjectService};
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;

const TAG: &str = "opencode_project_0123456789abcdef";
const HASH: &str = "0123456789abcdef";

fn params(project_id: &str, include: mem_store::serde_json::Value) -> InjectParams {
    InjectParams {
        project_id: Some(project_id.to_string()),
        token_budget: Some(2_000),
        include: Some(include),
        user_id: None,
        session_id: None,
        ..Default::default()
    }
}

fn fake_embedder() -> FakeEmbedder {
    FakeEmbedder::with_vector(common::unit_vector(0))
}

async fn fixture(
    name: &str,
) -> (
    TestDir,
    Arc<EpisodicStore>,
    Arc<mem_store::shard_manager::ShardManager>,
    InjectService,
) {
    let dir = TestDir::new(&format!("w53-inject-{name}"));
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
        Arc::new(fake_embedder()),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    (dir, store, shards, service)
}

/// Seeds one episodic row through the PUBLIC store.
async fn seed_episode(
    store: &EpisodicStore,
    session_id: &str,
    summary: &str,
    embedder: &FakeEmbedder,
    open_threads: &[&str],
) -> String {
    let mut session = mem_episodic::record::NewAgentSession {
        project_id: "/srv/proj".to_string(),
        session_id: session_id.to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: "Builder".to_string(),
        summary: summary.to_string(),
        ..Default::default()
    };
    session.open_threads = open_threads.iter().map(|t| t.to_string()).collect();
    store
        .insert_session(session, embedder)
        .await
        .expect("seed episode")
}

/// Seeds one tier-1 capture-summary row through the PUBLIC writer.
async fn seed_capture_summary(
    shards: &Arc<mem_store::shard_manager::ShardManager>,
    capture_id: &str,
    session_id: &str,
    summary: &str,
) {
    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(1))),
        prompt_stamps: Arc::new(mem_bridge::capture::AbsentPromptStampSource),
    };
    let scope =
        mem_bridge::project_scope::apply_resolved_scope("/srv/proj", TAG, HASH).expect("scope");
    let outcome = mem_bridge::capture::write_capture_summary(
        &deps,
        &scope,
        &mem_bridge::capture::CaptureSummaryInput {
            capture_id: capture_id.to_string(),
            session_id: session_id.to_string(),
            summary: summary.to_string(),
            memory_type: "bug".to_string(),
            tags: vec![],
            prompt_id: Some("p-1".to_string()),
            source_prompt_created_at: None,
        },
    )
    .await
    .expect("tier-1 seed");
    assert_eq!(outcome, mem_bridge::capture::Tier1WriteOutcome::Written);
}

// ---------------------------------------------------------------------------
// AC6 oracles
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_respects_token_budget() {
    let (_dir, store, _shards, service) = fixture("budget").await;
    for index in 0..4 {
        let _ = seed_episode(
            store.as_ref(),
            &format!("sess-{index}"),
            &"x".repeat(1_000),
            &fake_embedder(),
            &[],
        )
        .await;
    }

    // token_budget 1_000 -> 4_000 bytes. Each rendered line is
    // "agent-1: " + 1_000 = 1_009 bytes; the first accepted item of a section
    // also pays for its heading. Three fit, the fourth does not.
    let small = service
        .assemble(&InjectParams {
            token_budget: Some(1_000),
            ..params("/srv/proj", json!({ "recent_count": 5 }))
        })
        .await
        .expect("assemble");
    let small_block = small
        .block
        .as_ref()
        .expect("a partial block is still rendered");
    assert!(
        small_block.len() <= 4_000,
        "the EMITTED block respects the requested budget, no slop: {}",
        small_block.len()
    );
    assert!(small.truncated, "one of the four candidates was skipped");
    assert_eq!(small.memory_count, 3, "three fit, the fourth does not");
    assert_eq!(
        small.tokens,
        (small_block.len() / 4) as u64,
        "tokens is the byte estimate of the EMITTED block"
    );
    assert!(
        small.tokens <= 1_000,
        "reported tokens never exceed the ask"
    );

    // token_budget 2_000 -> 8_000 bytes: all four fit.
    let plain = service
        .assemble(&params("/srv/proj", json!({ "recent_count": 5 })))
        .await
        .expect("assemble");
    let plain_block = plain.block.as_ref().expect("block");
    assert!(plain_block.len() <= 8_000, "no slop: {}", plain_block.len());
    assert!(!plain.truncated, "all four fit in 8_000");
    assert_eq!(plain.memory_count, 4);
    assert_eq!(plain.tokens, (plain_block.len() / 4) as u64);

    // SENSITIVITY SWEEP: a single budget is knife-edge (a rounding window can
    // hide a leak), so walk every budget and assert the contract each time.
    let (_sweep_dir, sweep_store, _sweep_shards, sweep_service) = fixture("budget-sweep").await;
    for index in 0..8 {
        let _ = seed_episode(
            sweep_store.as_ref(),
            &format!("sess-s{index}"),
            &"y".repeat(200),
            &fake_embedder(),
            &[],
        )
        .await;
    }
    let mut saw_block = false;
    let mut saw_truncated = false;
    for tokens in 1u64..=300 {
        let result = sweep_service
            .assemble(&InjectParams {
                token_budget: Some(tokens),
                ..params("/srv/proj", json!({ "recent_count": 8 }))
            })
            .await
            .expect("assemble");
        if let Some(block) = &result.block {
            saw_block = true;
            assert!(
                block.len() <= 4 * tokens as usize,
                "tokens={tokens} block={}",
                block.len()
            );
            assert_eq!(result.tokens, (block.len() / 4) as u64);
            assert!(
                result.tokens <= tokens,
                "tokens={tokens} reported={}",
                result.tokens
            );
        }
        saw_truncated |= result.truncated;
    }
    assert!(
        saw_block && saw_truncated,
        "the sweep must exercise both outcomes"
    );

    let zero_params = InjectParams {
        token_budget: Some(0),
        ..params("/srv/proj", json!({ "recent_count": 5 }))
    };
    let zero = service.assemble(&zero_params).await.expect("zero budget");
    assert_eq!(zero.block, None, "zero budget -> block None");

    let huge_params = InjectParams {
        token_budget: Some(u64::MAX),
        ..params("/srv/proj", json!({ "recent_count": 5 }))
    };
    let huge = service.assemble(&huge_params).await;
    assert!(
        matches!(huge, Err(InjectError::Overflow)),
        "checked multiplication rejects overflow, never wraps: {huge:?}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_blocker_first_ordering() {
    let (_dir, store, _shards, service) = fixture("blocker").await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-new",
        "clean newer session",
        &fake_embedder(),
        &[],
    )
    .await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-old",
        "older blocked session",
        &fake_embedder(),
        &["waiting on the gate"],
    )
    .await;

    let result = service
        .assemble(&params("/srv/proj", json!({ "recent_count": 2 })))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    let blocker_index = block.find("waiting on the gate").expect("blocker included");
    let clean_index = block.find("clean newer session").expect("clean included");
    assert!(
        blocker_index < clean_index,
        "the older blocker outranks the newer clean row"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_memory_count_accounting() {
    let (_dir, store, shards, service) = fixture("count").await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-1",
        "live session one",
        &fake_embedder(),
        &[],
    )
    .await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-2",
        "live session two",
        &fake_embedder(),
        &[],
    )
    .await;
    seed_capture_summary(
        &shards,
        "019a0000-0000-7f00-8000-000000000003",
        "sess-3",
        "summary row",
    )
    .await;

    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    assert_eq!(
        result.memory_count, 3,
        "two recent + one capture-summary row"
    );
    let block = result.block.expect("block");
    assert_eq!(
        block.matches("summary row").count(),
        1,
        "each tier-1 row counts individually"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_multi_byte_never_splits_char() {
    let (_dir, store, _shards, service) = fixture("multibyte").await;
    // ~420 bytes: one line that fits comfortably.
    let small_multibyte = "日本語のセッション概要です。".repeat(10);
    // 6 000 bytes: cannot fit in the 4 000-byte budget and must be SKIPPED.
    let large_multibyte = "あ".repeat(2_000);
    let _ = seed_episode(
        store.as_ref(),
        "sess-1",
        &small_multibyte,
        &fake_embedder(),
        &[],
    )
    .await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-2",
        &large_multibyte,
        &fake_embedder(),
        &[],
    )
    .await;

    let result = service
        .assemble(&InjectParams {
            token_budget: Some(1_000),
            ..params("/srv/proj", json!({ "recent_count": 2 }))
        })
        .await
        .expect("assemble");
    let block = result
        .block
        .as_ref()
        .expect("the multibyte block IS rendered (never Option-blind)");
    assert!(block.len() <= 4_000, "the UTF-8 cap holds: {}", block.len());
    assert!(
        !block.contains('\u{FFFD}'),
        "no replacement char: a char was split"
    );
    assert!(
        block.contains(&small_multibyte),
        "the fitting multibyte line is included WHOLE"
    );
    assert!(
        !block.contains(&large_multibyte),
        "the oversized line is skipped, never cut"
    );
    assert!(
        !block.contains(&"あ".repeat(50)),
        "no partial run of the oversized line survives a byte cut"
    );
    assert!(result.truncated, "the oversized candidate was skipped");
    assert_eq!(result.memory_count, 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_reads_same_project_tag_as_capture() {
    let (_dir, _store, shards, service) = fixture("tag").await;
    seed_capture_summary(
        &shards,
        "019a0000-0000-7f00-8000-000000000010",
        "sess-x",
        "wrote-by-capture",
    )
    .await;

    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert!(block.contains("wrote-by-capture"));
    assert!(block.contains("## Captured session summaries"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_missing_project_returns_empty_block_without_shard_query() {
    let (dir, _store, _shards, service) = fixture("absent").await;
    let missing = InjectParams {
        project_id: None,
        token_budget: None,
        include: None,
        user_id: None,
        session_id: None,
        ..Default::default()
    };
    let blank_project = InjectParams {
        project_id: Some("   ".to_string()),
        ..missing.clone()
    };
    for params_set in [missing, blank_project] {
        let result = service.assemble(&params_set).await.expect("absent policy");
        assert_eq!(result.block, None);
        assert_eq!(result.memory_count, 0);
    }
    let files = common::list_data_files(dir.path()).await;
    assert!(
        !files.iter().any(|path| path.ends_with("_shard_0.db")),
        "no shard materialized for the absent policy: {files:?}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_skips_tier1_leg_when_consolidation_lock_is_busy_but_returns_episodic_block() {
    let (_dir, store, shards, _idle) = fixture("busy").await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-1",
        "episode during consolidation",
        &fake_embedder(),
        &[],
    )
    .await;
    seed_capture_summary(
        &shards,
        "019a0000-0000-7f00-8000-000000000011",
        "sess-2",
        "summary suppressed",
    )
    .await;

    let busy = InjectService::new(
        store.clone(),
        shards.clone(),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(true)),
        Arc::new(fake_embedder()),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    let result = busy
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble under a busy lock");
    let block = result.block.expect("the episodic block still renders");
    assert!(block.contains("episode during consolidation"));
    assert!(
        !block.contains("summary suppressed"),
        "the tier-1 leg was skipped"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_tier1_leg_real_shard() {
    let (_dir, _store, shards, service) = fixture("real").await;
    seed_capture_summary(
        &shards,
        "019a0000-0000-7f00-8000-000000000012",
        "sess-r",
        "real shard row",
    )
    .await;

    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    assert!(result.block.expect("block").contains("real shard row"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_tier1_section_caps_at_one_quarter_budget() {
    let (_dir, _store, shards, service) = fixture("cap25").await;
    for index in 0..8 {
        seed_capture_summary(
            &shards,
            &format!("019a0000-0000-7f00-8000-{index:012}"),
            &format!("sess-c{index}"),
            &"T".repeat(600),
        )
        .await;
    }
    let result = service
        .assemble(&InjectParams {
            token_budget: Some(2_000),
            ..params("/srv/proj", json!({}))
        })
        .await
        .expect("assemble");
    let block = result.block.as_ref().expect("summary rows render");

    // Measure the RENDERED tier-1 subsection, not the whole block.
    let start = block
        .find("## Captured session summaries")
        .expect("summaries heading");
    let rest = &block[start..];
    let end = rest[1..]
        .find("\n## ")
        .map(|offset| start + 1 + offset)
        .unwrap_or_else(|| {
            block
                .rfind("</project_agent_session_history>")
                .expect("close tag")
        });
    let section = &block[start..end];
    assert!(section.starts_with("## Captured session summaries"));
    assert!(
        section.len() <= 2_000,
        "the tier-1 subsection is capped at budget/4 (2 000 B): {}",
        section.len()
    );
    assert!(
        section.matches('\n').count() >= 2,
        "more than the heading: the section is non-empty"
    );
    assert!(result.truncated, "some tier-1 candidate was skipped");
    assert_eq!(result.memory_count, 3, "three fit under the quarter cap");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_read_tag_equals_promotion_container_tag() {
    // A SECOND tag with the SAME scope hash: the shard file is hash-keyed, so
    // both rows land in ONE shard and only the container-tag filter can
    // separate them. Without that, this oracle would be vacuous.
    const OTHER_TAG: &str = "otherscope_project_0123456789abcdef";

    let (_dir, _store, shards, service) = fixture("read-tag").await;
    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(1))),
        prompt_stamps: Arc::new(mem_bridge::capture::AbsentPromptStampSource),
    };
    let matching =
        mem_bridge::project_scope::apply_resolved_scope("/srv/proj", TAG, HASH).expect("scope");
    let foreign = mem_bridge::project_scope::apply_resolved_scope("/srv/proj", OTHER_TAG, HASH)
        .expect("the arbitrary prefix is legal; only the last two segments are read");

    let rows_to_write = [
        (
            &matching,
            "019a0000-0000-7f00-8000-000000000117",
            "sess-m",
            "MATCHING-TAG-ROW",
        ),
        (
            &foreign,
            "019a0000-0000-7f00-8000-000000000118",
            "sess-f",
            "FOREIGN-TAG-ROW",
        ),
    ];
    for (scope, capture_id, session_id, text) in rows_to_write {
        let outcome = mem_bridge::capture::write_capture_summary(
            &deps,
            scope,
            &mem_bridge::capture::CaptureSummaryInput {
                capture_id: capture_id.to_string(),
                session_id: session_id.to_string(),
                summary: text.to_string(),
                memory_type: "memory".to_string(),
                tags: vec![],
                prompt_id: None,
                source_prompt_created_at: None,
            },
        )
        .await
        .expect("tier-1 write");
        assert_eq!(outcome, mem_bridge::capture::Tier1WriteOutcome::Written);
    }

    // Prove the two rows really share ONE shard, otherwise the filter oracle
    // proves nothing.
    let shard = shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("shard");
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    let stored = db
        .query("SELECT container_tag FROM memories ORDER BY id", vec![])
        .await
        .expect("query");
    assert_eq!(stored.len(), 2, "both rows live in this shard");
    assert!(
        stored
            .iter()
            .any(|row| row.text("container_tag").as_deref() == Some(OTHER_TAG)),
        "the foreign-tag row really is present (so a missing filter would render it)"
    );

    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    let block = result.block.expect("the matching row renders");
    assert!(block.contains("MATCHING-TAG-ROW"));
    assert!(
        !block.contains("FOREIGN-TAG-ROW"),
        "the read is filtered by the resolved container tag: {block}"
    );
    assert!(block.contains("## Captured session summaries"));

    // Second half (R5): the promotion record's tag IS the injector's tag.
    let promotion_record = mem_episodic::promotion::PromotionRecord {
        id: "fact-00000000000000000000000000000000".to_string(),
        content: "the promoted fact".to_string(),
        vector: common::unit_vector(2),
        container_tag: matching.container_tag.clone(),
        memory_type: "decision".to_string(),
        created_at: 1_000,
        source_row_ids: vec![],
    };
    assert_eq!(
        promotion_record.container_tag.as_str(),
        matching.container_tag.as_str()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_separates_capture_summaries_from_promoted_facts() {
    let (_dir, _store, shards, service) = fixture("classes").await;
    seed_capture_summary(
        &shards,
        "019a0000-0000-7f00-8000-000000000016",
        "sess-c",
        "the capture summary row",
    )
    .await;

    // A promotion-shaped row over the SAME tag (source episodic-consolidation, session-less).
    let record = mem_episodic::promotion::PromotionRecord {
        id: "fact-00000000000000000000000000000009".to_string(),
        content: "the promoted fact row".to_string(),
        vector: common::unit_vector(2),
        container_tag: TAG.to_string(),
        memory_type: "decision".to_string(),
        created_at: 1_000,
        source_row_ids: vec![],
    };
    let sink = mem_episodic::promotion::ShardPermanentMemorySink::new(shards.clone(), 8);
    let promoted = mem_episodic::promotion::PermanentMemorySink::promote(&sink, HASH, &record)
        .await
        .expect("promote");
    assert_eq!(promoted, mem_episodic::promotion::PromoteOutcome::Inserted);

    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    let facts_heading = block.find("## Project facts").expect("facts heading");
    let fact_index = block
        .find("the promoted fact row")
        .expect("fact row included");
    let summaries_heading = block
        .find("## Captured session summaries")
        .expect("summaries heading");
    let summary_index = block
        .find("the capture summary row")
        .expect("summary row included");
    assert!(
        fact_index > facts_heading,
        "the promoted fact renders under facts"
    );
    assert!(
        summary_index > summaries_heading,
        "the capture summary renders under summaries"
    );
    assert!(
        fact_index < summaries_heading,
        "the two classes never merge"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_returns_none_when_empty() {
    let (_dir, _store, _shards, service) = fixture("empty").await;
    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    assert_eq!(result.block, None);
    assert_eq!(result.memory_count, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_semantic_deduplicates_recent() {
    let (_dir, store, _shards, service) = fixture("dedup").await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-1",
        "the matched session row",
        &fake_embedder(),
        &[],
    )
    .await;

    let result = service
        .assemble(&params(
            "/srv/proj",
            json!({ "recent_count": 5, "query": "matched session" }),
        ))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert_eq!(
        block.matches("the matched session row").count(),
        1,
        "one logical row, rendered once"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_strips_private_content() {
    let (_dir, store, _shards, service) = fixture("privacy").await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-1",
        "public part <private>secret token</private> visible",
        &fake_embedder(),
        &[],
    )
    .await;
    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    let block = result.block.expect("block");
    assert!(block.contains("public part") && block.contains("visible"));
    assert!(
        !block.contains("secret token"),
        "privacy strip before rendering"
    );
}

// ---------------------------------------------------------------------------
// r1 rework oracles
// ---------------------------------------------------------------------------

/// A malformed metadata document must be SKIPPED with a bounded warning —
/// never rendered as a "promoted fact" (raw capture text shown to the model
/// as durable project truth).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_skips_malformed_metadata_row() {
    let (_dir, _store, shards, service) = fixture("corrupt-metadata").await;
    let shard = shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("write shard");
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    mem_store::vector::insert_vector(
        db.as_ref(),
        &mem_store::vector::MemoryInsert {
            id: "corrupt-row-0000000000000000000001".to_string(),
            content: "CORRUPT-ROW".to_string(),
            vector: common::unit_vector(0),
            tags_vector: None,
            container_tag: TAG.to_string(),
            tags: None,
            memory_type: Some("memory".to_string()),
            created_at: 1_000,
            updated_at: 1_000,
            metadata: Some("{not json".to_string()),
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
    .expect("insert the corrupt row");

    let result = service
        .assemble(&params("/srv/proj", json!({})))
        .await
        .expect("assemble");
    let block = result.block.unwrap_or_default();
    assert!(
        !block.contains("CORRUPT-ROW"),
        "malformed metadata is skipped, never classified as a fact: {block}"
    );
    assert!(
        !block.contains("## Project facts"),
        "no facts section for a skipped row: {block}"
    );
}

/// The duplicate capture-summary suppression is keyed on the sessions whose
/// episode row was ACTUALLY INCLUDED: a budget-dropped episode row must not
/// suppress its capture summary (otherwise neither renders).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_inject_summary_renders_when_its_episode_row_is_budget_dropped() {
    let (_dir, store, shards, service) = fixture("suppression").await;
    let _ = seed_episode(
        store.as_ref(),
        "sess-x",
        &"z".repeat(5_000),
        &fake_embedder(),
        &[],
    )
    .await;
    seed_capture_summary(
        &shards,
        "019a0000-0000-7f00-8000-000000000119",
        "sess-x",
        "SHORT SUMMARY TEXT",
    )
    .await;

    let result = service
        .assemble(&InjectParams {
            token_budget: Some(1_000),
            ..params("/srv/proj", json!({}))
        })
        .await
        .expect("assemble");
    let block = result.block.expect("the summary still renders");
    assert!(
        block.contains("## Captured session summaries"),
        "the summary heading survives an unmatched episode row: {block}"
    );
    assert!(
        block.contains("SHORT SUMMARY TEXT"),
        "the budget-dropped episode row does not suppress the summary: {block}"
    );
    assert!(
        !block.contains(&"z".repeat(5_000)),
        "the oversized episode row really was dropped"
    );
    assert_eq!(result.memory_count, 1);
}

/// The `<memory_context>` frame is charged INSIDE the greedy pass: with a
/// 480-byte budget (`used_at_zero = 257`) two 99-byte lines fit and the
/// emitted block is `192 + 63 + 19 + 2×100 = 474` bytes, never 574 (frame
/// emitted but uncharged) and never 382 (frame absent).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_wrapper_frame_bytes_are_charged_inside_the_budget() {
    let (_dir, store, _shards, service) = fixture("frame-charge").await;
    for index in 0..4 {
        let _ = seed_episode(
            store.as_ref(),
            &format!("sess-frame-{index}"),
            &"y".repeat(90),
            &fake_embedder(),
            &[],
        )
        .await;
    }

    let result = service
        .assemble(&InjectParams {
            token_budget: Some(120),
            ..params("/srv/proj", json!({ "recent_count": 4 }))
        })
        .await
        .expect("assemble");
    let block = result
        .block
        .as_ref()
        .expect("two rows plus the frame render");
    assert_eq!(block.len(), 474, "192-byte frame + history bytes: {block}");
    assert!(
        block.len() <= 480,
        "the frame is charged inside 4 x token_budget: {}",
        block.len()
    );
    assert_eq!(
        result.memory_count, 2,
        "the frame's 192 bytes evict the third row"
    );
    assert!(result.truncated, "the frame charge truncates the row set");
}
