//! W4.3 lane — promoted facts land in the permanent memory tier (AC-3).
//!
//! The seams under test are the pure `select_promotable_facts` selection,
//! the `ShardPermanentMemorySink` (over a real `ShardManager`) and
//! `Consolidator::run`.

mod common;

use std::sync::Arc;

use common::{FakeVectorCountBump, TestDir, session_row, shard_manager};
use mem_episodic::promotion::{
    PermanentMemorySink, PromoteOutcome, PromotionRecord, ShardPermanentMemorySink,
    select_promotable_facts,
};
use mem_store::db::Value;
use mem_store::scope::Scope;
use mem_store::shard_manager::ShardManager;

/// The project scope hash the promoted facts land under.
const SCOPE_HASH: &str = "0123456789abcdef";
/// The container tag the caller supplies for that scope.
const CONTAINER_TAG: &str = "opencode_project_0123456789abcdef";
/// Another project's container tag.
const OTHER_CONTAINER_TAG: &str = "opencode_project_fedcba9876543210";

/// A promotion record with `dims` dimensions.
fn record(id: &str, content: &str, dims: usize) -> PromotionRecord {
    PromotionRecord {
        id: id.to_string(),
        content: content.to_string(),
        vector: (0..dims).map(|index| index as f32 * 0.1).collect(),
        container_tag: CONTAINER_TAG.to_string(),
        memory_type: "decision".to_string(),
        created_at: 1_000,
        source_row_ids: vec!["row-1".to_string()],
    }
}

/// The registry rows of one project scope.
async fn registry_rows(
    shards: &ShardManager,
    scope_hash: &str,
) -> Vec<mem_store::shard_manager::ShardInfo> {
    shards
        .get_all_shards(Scope::Project, Some(scope_hash))
        .await
        .expect("registry rows")
}

/// The `memories` rows of one shard, filtered by container tag.
async fn memory_rows(
    shards: &ShardManager,
    scope_hash: &str,
    container_tag: &str,
) -> Vec<(String, Option<String>, String)> {
    let rows = registry_rows(shards, scope_hash).await;
    let Some(info) = rows.first() else {
        return Vec::new();
    };
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    let rows = db
        .query(
            "SELECT id, container_tag, type, content FROM memories WHERE container_tag = ?",
            vec![Value::Text(container_tag.to_string())],
        )
        .await
        .expect("query memories");
    rows.iter()
        .map(|row| {
            (
                row.text("id").unwrap_or_default(),
                row.text("type"),
                row.text("content").unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn test_singleton_decision_in_one_session_is_promoted() {
    // A decision stated in exactly ONE session yields exactly ONE fact with
    // exactly ONE source row id (the regression test for the singleton drop).
    let rows = vec![session_row(
        "row-1",
        "project-a",
        "agent-a",
        "summary",
        &["use the postgres adapter"],
        10,
    )];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].content, "use the postgres adapter");
    assert_eq!(facts[0].source_row_ids, vec!["row-1".to_string()]);
    assert_eq!(facts[0].key, "use the postgres adapter");
    assert!(
        facts[0].id.starts_with("episodic-decision-"),
        "unexpected id: {}",
        facts[0].id
    );
    assert_eq!(facts[0].id.len(), "episodic-decision-".len() + 32);
}

#[test]
fn test_identical_decision_in_two_sessions_yields_one_fact_with_both_row_ids() {
    let rows = vec![
        session_row("row-1", "project-a", "agent-a", "s1", &["use redis"], 10),
        session_row("row-2", "project-a", "agent-a", "s2", &["use redis"], 11),
    ];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 1);
    assert_eq!(
        facts[0].source_row_ids,
        vec!["row-1".to_string(), "row-2".to_string()]
    );
}

#[test]
fn test_select_promotable_facts_keeps_first_appearance_order() {
    let rows = vec![
        session_row(
            "row-1",
            "project-a",
            "agent-a",
            "s1",
            &["first decision", "second decision"],
            10,
        ),
        session_row(
            "row-2",
            "project-a",
            "agent-a",
            "s2",
            &["first decision", "third decision"],
            11,
        ),
    ];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 3);
    assert_eq!(facts[0].content, "first decision");
    assert_eq!(facts[1].content, "second decision");
    assert_eq!(facts[2].content, "third decision");
}

#[test]
fn test_select_promotable_facts_preserves_first_appearance_order_across_singletons_and_groups() {
    // A singleton key and a two-member key interleave by FIRST APPEARANCE,
    // not "groups first, singletons later".
    let rows = vec![
        session_row("row-1", "project-a", "agent-a", "s1", &["alpha"], 10),
        session_row("row-2", "project-a", "agent-a", "s2", &["beta"], 11),
        session_row("row-3", "project-a", "agent-a", "s3", &["beta"], 12),
    ];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].content, "alpha");
    assert_eq!(facts[1].content, "beta");
    assert_eq!(facts[0].source_row_ids, vec!["row-1".to_string()]);
    assert_eq!(
        facts[1].source_row_ids,
        vec!["row-2".to_string(), "row-3".to_string()]
    );
}

#[test]
fn test_promotion_dedups_identical_decisions_across_sessions() {
    let rows = vec![
        session_row("row-1", "project-a", "agent-a", "s1", &["use redis"], 10),
        session_row("row-2", "project-a", "agent-a", "s2", &["use redis"], 11),
    ];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 1);
}

#[test]
fn test_promotion_skips_blank_decisions() {
    let rows = vec![session_row(
        "row-1",
        "project-a",
        "agent-a",
        "s1",
        &["", "   ", "real decision"],
        10,
    )];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].content, "real decision");
}

#[test]
fn test_promotion_strips_private_tags_before_storage() {
    let rows = vec![session_row(
        "row-1",
        "project-a",
        "agent-a",
        "s1",
        &["rotate the <private>api key</private> weekly"],
        10,
    )];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 1);
    assert!(
        !facts[0].content.contains("api key"),
        "{}",
        facts[0].content
    );
    assert!(
        facts[0].content.contains("[REDACTED]"),
        "{}",
        facts[0].content
    );
}

#[test]
fn test_promotion_normalizes_key_by_case_and_whitespace() {
    let rows = vec![
        session_row("row-1", "project-a", "agent-a", "s1", &["Use   Redis"], 10),
        session_row("row-2", "project-a", "agent-a", "s2", &["use redis"], 11),
    ];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].key, "use redis");
    // The content is the FIRST member's content.
    assert_eq!(facts[0].content, "Use   Redis");
}

#[test]
fn test_duplicate_decision_inside_one_session_lists_the_row_once() {
    let rows = vec![session_row(
        "row-1",
        "project-a",
        "agent-a",
        "s1",
        &["use redis", "use redis"],
        10,
    )];
    let facts = select_promotable_facts("project-a", &rows);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].source_row_ids, vec!["row-1".to_string()]);
}

#[test]
fn test_promoted_fact_id_is_deterministic_and_project_scoped() {
    let rows = vec![session_row(
        "row-1",
        "project-a",
        "agent-a",
        "s1",
        &["use redis"],
        10,
    )];
    let first = select_promotable_facts("project-a", &rows);
    let again = select_promotable_facts("project-a", &rows);
    assert_eq!(first[0].id, again[0].id);

    let other = select_promotable_facts("project-b", &rows);
    assert_ne!(first[0].id, other[0].id);
}

// ---------------------------------------------------------------------------
// The promotion sink (Step 6)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotes_decisions_into_project_memory_tier_with_project_container_tag() {
    let dir = TestDir::new("promotion-write");
    let shards = shard_manager(dir.store_config());
    let sink = ShardPermanentMemorySink::new(shards.clone(), 8);

    let outcome = sink
        .promote(SCOPE_HASH, &record("fact-1", "use redis", 8))
        .await
        .expect("promote");
    assert_eq!(outcome, PromoteOutcome::Inserted);

    let rows = memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "fact-1");
    assert_eq!(rows[0].1.as_deref(), Some("decision"));
    assert_eq!(rows[0].2, "use redis");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_rejects_invalid_scope_hash() {
    let dir = TestDir::new("promotion-bad-scope");
    let shards = shard_manager(dir.store_config());
    let sink = ShardPermanentMemorySink::new(shards.clone(), 8);

    let error = sink
        .promote("not-a-scope-hash", &record("fact-1", "use redis", 8))
        .await
        .expect_err("invalid scope hash");
    assert!(
        matches!(
            &error,
            mem_episodic::EpisodicError::Validation { field, .. } if field == "scope_hash"
        ),
        "unexpected error: {error}"
    );
    // Nothing was written.
    assert!(
        memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG)
            .await
            .is_empty()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_rejects_dimension_mismatch() {
    let dir = TestDir::new("promotion-dims");
    let shards = shard_manager(dir.store_config());
    let sink = ShardPermanentMemorySink::new(shards.clone(), 8);

    let error = sink
        .promote(SCOPE_HASH, &record("fact-1", "use redis", 4))
        .await
        .expect_err("dimension mismatch");
    assert!(
        matches!(
            &error,
            mem_episodic::EpisodicError::Validation { field, .. } if field == "embedding"
        ),
        "unexpected error: {error}"
    );
    assert!(
        memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG)
            .await
            .is_empty()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promoted_facts_do_not_surface_under_other_project_container_tag() {
    let dir = TestDir::new("promotion-container");
    let shards = shard_manager(dir.store_config());
    let sink = ShardPermanentMemorySink::new(shards.clone(), 8);

    sink.promote(SCOPE_HASH, &record("fact-1", "use redis", 8))
        .await
        .expect("promote");

    assert_eq!(
        memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await.len(),
        1
    );
    assert!(
        memory_rows(&shards, SCOPE_HASH, OTHER_CONTAINER_TAG)
            .await
            .is_empty(),
        "the fact must not surface under another project's container tag"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_is_idempotent_on_rerun() {
    let dir = TestDir::new("promotion-idempotent");
    let shards = shard_manager(dir.store_config());
    let sink = ShardPermanentMemorySink::new(shards.clone(), 8);
    let record = record("fact-1", "use redis", 8);

    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("first promote"),
        PromoteOutcome::Inserted
    );
    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("second promote"),
        PromoteOutcome::AlreadyPresent
    );
    assert_eq!(
        memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await.len(),
        1
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_does_not_increment_vector_count_when_already_present() {
    let dir = TestDir::new("promotion-no-double-bump");
    let shards = shard_manager(dir.store_config());
    let bump = Arc::new(FakeVectorCountBump::new());
    let sink = ShardPermanentMemorySink::with_vector_bump(shards.clone(), 8, bump.clone());
    let record = record("fact-1", "use redis", 8);

    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("first promote"),
        PromoteOutcome::Inserted
    );
    assert_eq!(bump.bumped().len(), 1, "the insert path bumps exactly once");

    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("second promote"),
        PromoteOutcome::AlreadyPresent
    );
    assert_eq!(
        bump.bumped().len(),
        1,
        "the AlreadyPresent path never bumps (no double count)"
    );
    assert_eq!(
        memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await.len(),
        1
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_reports_error_when_vector_count_increment_fails() {
    let dir = TestDir::new("promotion-bump-failure");
    let shards = shard_manager(dir.store_config());
    let bump = Arc::new(FakeVectorCountBump::failing());
    let sink = ShardPermanentMemorySink::with_vector_bump(shards.clone(), 8, bump.clone());

    let error = sink
        .promote(SCOPE_HASH, &record("fact-1", "use redis", 8))
        .await
        .expect_err("the failed bump is reported");
    assert!(
        matches!(error, mem_episodic::EpisodicError::Store(_)),
        "{error}"
    );

    // The memory row IS written ...
    let rows = memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await;
    assert_eq!(rows.len(), 1, "the row was written before the failed bump");
    // ... but the shard's vector count was NOT incremented (drift of one).
    let registry = registry_rows(&shards, SCOPE_HASH).await;
    let count = registry
        .first()
        .map(|info| info.vector_count)
        .expect("a shard registry row");
    assert_eq!(count, 0, "the failed bump left the count untouched");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_shard_vector_count_drift_is_reconciled_on_next_write() {
    let dir = TestDir::new("promotion-reconcile");
    let shards = shard_manager(dir.store_config());
    let bump = Arc::new(FakeVectorCountBump::failing());
    let sink = ShardPermanentMemorySink::with_vector_bump(shards.clone(), 8, bump.clone());

    sink.promote(SCOPE_HASH, &record("fact-1", "use redis", 8))
        .await
        .expect_err("the failed bump is reported");

    // The next write-shard resolution re-syncs the count from the shard's
    // `memories` table, so the drift is corrected to the true row count.
    let info = shards
        .get_write_shard(Scope::Project, SCOPE_HASH)
        .await
        .expect("write shard");
    assert_eq!(info.vector_count, 1, "the drift was reconciled");

    let registry = registry_rows(&shards, SCOPE_HASH).await;
    assert_eq!(
        registry.first().map(|info| info.vector_count),
        Some(1),
        "the registry row carries the reconciled count"
    );
}
