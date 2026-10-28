//! W4.3 lane — promoted facts land in the permanent memory tier (AC-3).
//!
//! The seams under test are the pure `select_promotable_facts` selection,
//! the `ShardPermanentMemorySink` (over a real `ShardManager`) and
//! `Consolidator::run`.

mod common;

use std::sync::Arc;

use common::{FakeVectorCountSync, TestDir, session_row, shard_manager};
use mem_episodic::promotion::{
    PermanentMemorySink, PromoteOutcome, PromotionRecord, ShardPermanentMemorySink,
    select_promotable_facts,
};
use mem_store::db::Value;
use mem_store::scope::Scope;
use mem_store::shard_manager::{ShardInfo, ShardManager};

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
async fn test_promotion_does_not_resync_the_shard_count_when_already_present() {
    let dir = TestDir::new("promotion-no-double-resync");
    let shards = shard_manager(dir.store_config());
    let sync = Arc::new(FakeVectorCountSync::new());
    let sink = ShardPermanentMemorySink::with_vector_sync(shards.clone(), 8, sync.clone());
    let record = record("fact-1", "use redis", 8);

    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("first promote"),
        PromoteOutcome::Inserted
    );
    assert_eq!(
        sync.resynced().len(),
        1,
        "the insert path resyncs exactly once"
    );

    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("second promote"),
        PromoteOutcome::AlreadyPresent
    );
    assert_eq!(
        sync.resynced().len(),
        1,
        "the AlreadyPresent path never resyncs (no second count)"
    );
    assert_eq!(
        memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await.len(),
        1
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_reports_error_when_the_count_resync_fails() {
    let dir = TestDir::new("promotion-resync-failure");
    let shards = shard_manager(dir.store_config());
    let sync = Arc::new(FakeVectorCountSync::failing());
    let sink = ShardPermanentMemorySink::with_vector_sync(shards.clone(), 8, sync.clone());

    let error = sink
        .promote(SCOPE_HASH, &record("fact-1", "use redis", 8))
        .await
        .expect_err("the failed resync is reported");
    assert!(
        matches!(error, mem_episodic::EpisodicError::Store(_)),
        "{error}"
    );

    // The memory row IS written ...
    let rows = memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await;
    assert_eq!(
        rows.len(),
        1,
        "the row was written before the failed resync"
    );
    // ... but the shard's vector count was NOT incremented (drift of one).
    let registry = registry_rows(&shards, SCOPE_HASH).await;
    let count = registry
        .first()
        .map(|info| info.vector_count)
        .expect("a shard registry row");
    assert_eq!(count, 0, "the failed resync left the count untouched");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_shard_vector_count_drift_is_reconciled_on_next_write() {
    let dir = TestDir::new("promotion-reconcile");
    let shards = shard_manager(dir.store_config());
    let sync = Arc::new(FakeVectorCountSync::failing());
    let sink = ShardPermanentMemorySink::with_vector_sync(shards.clone(), 8, sync.clone());

    sink.promote(SCOPE_HASH, &record("fact-1", "use redis", 8))
        .await
        .expect_err("the failed resync is reported");

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

// ---------------------------------------------------------------------------
// W1 (issue 43.1) — the deterministic fact id is resolved across the scope
// ---------------------------------------------------------------------------

/// `SELECT COUNT(*) FROM memories WHERE id = ?` inside ONE shard file.
async fn count_rows_in_shard(shards: &ShardManager, info: &ShardInfo, id: &str) -> i64 {
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM memories WHERE id = ?",
        vec![Value::Text(id.to_string())],
    )
    .await
    .expect("count query")
    .expect("a count row");
    row.i64("count").unwrap_or(0)
}

/// The same count summed over every registered shard of the scope whose FILE
/// still exists: a removed shard holds no rows, and opening it would
/// re-create it.
async fn count_rows_across_shards(shards: &ShardManager, scope_hash: &str, id: &str) -> i64 {
    let mut total = 0;
    for info in registry_rows(shards, scope_hash).await {
        if !info.db_path.exists() {
            continue;
        }
        total += count_rows_in_shard(shards, &info, id).await;
    }
    total
}

/// AC6 — promotion after a rotation reports `AlreadyPresent` instead of
/// re-inserting, and the replay's own resolution rotates before the scan.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_replay_after_rotation_is_already_present_and_leaves_one_row_in_the_scope() {
    let dir = TestDir::new("promotion-rotation-replay");
    let mut config = dir.store_config();
    config.embedding_dimensions = 2;
    config.max_vectors_per_shard = 1;
    let shards = shard_manager(config);
    let sink = ShardPermanentMemorySink::new(shards.clone(), 2);

    let fact_a = record("fact-a", "use redis", 2);
    let fact_b = record("fact-b", "use postgres", 2);

    assert_eq!(
        sink.promote(SCOPE_HASH, &fact_a).await.expect("promote A"),
        PromoteOutcome::Inserted
    );
    assert_eq!(
        sink.promote(SCOPE_HASH, &fact_b).await.expect("promote B"),
        PromoteOutcome::Inserted
    );
    assert_eq!(
        registry_rows(&shards, SCOPE_HASH).await.len(),
        2,
        "B's resolution retired the full shard 0 and created shard 1 (non-vacuity)"
    );

    assert_eq!(
        sink.promote(SCOPE_HASH, &fact_a)
            .await
            .expect("the replay must not fail"),
        PromoteOutcome::AlreadyPresent,
        "A lives in the retired shard 0: `AlreadyPresent`, not a second insert"
    );

    let registry = registry_rows(&shards, SCOPE_HASH).await;
    assert_eq!(
        registry.len(),
        3,
        "the replay's own resolution rotated shard 1 → shard 2 BEFORE the scan: {registry:?}"
    );
    assert_eq!(
        count_rows_in_shard(&shards, &registry[0], &fact_a.id).await,
        1,
        "shard 0 holds A"
    );
    assert_eq!(
        count_rows_in_shard(&shards, &registry[1], &fact_b.id).await,
        1,
        "shard 1 holds B"
    );
    assert!(registry[2].is_active, "shard 2 is the active shard");
    assert_eq!(
        count_rows_in_shard(&shards, &registry[2], &fact_a.id).await,
        0,
        "shard 2 is empty"
    );
    assert_eq!(
        count_rows_across_shards(&shards, SCOPE_HASH, &fact_a.id).await,
        1,
        "exactly one copy of A across the whole scope"
    );
}

/// AC7 — promotion fails the run closed and writes nothing.
///
/// The ONE mechanism able to produce the `Err` in this fixture: shard 0 is
/// registered in the metadata registry and its `.db` was deliberately
/// removed. Every other step of the promote (validation, dimensions, the
/// active shard's file, the insert target) is healthy.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_fails_the_run_and_writes_nothing_when_a_registered_shard_is_unreadable() {
    let dir = TestDir::new("promotion-scope-shard-missing");
    let mut config = dir.store_config();
    config.embedding_dimensions = 2;
    config.max_vectors_per_shard = 1;
    let shards = shard_manager(config.clone());
    let sink = ShardPermanentMemorySink::new(shards.clone(), 2);

    let fact_a = record("fact-a", "use redis", 2);
    let fact_b = record("fact-b", "use postgres", 2);
    for fact in [&fact_a, &fact_b] {
        assert_eq!(
            sink.promote(SCOPE_HASH, fact).await.expect("promote"),
            PromoteOutcome::Inserted
        );
    }

    let registry = registry_rows(&shards, SCOPE_HASH).await;
    assert_eq!(
        registry.len(),
        2,
        "A in shard 0, B in shard 1: {registry:?}"
    );
    let shard_zero = registry[0].db_path.clone();
    assert!(shard_zero.exists());

    // Drop every handle to shard 0 (a cached connection would still read the
    // unlinked file), delete its database, then build a FRESH manager over
    // the same root.
    drop(sink);
    drop(shards);
    std::fs::remove_file(&shard_zero).expect("remove shard 0's database");

    let fresh = shard_manager(config);
    let sink = ShardPermanentMemorySink::new(fresh.clone(), 2);
    let fact_c = record("fact-c", "use sqlite", 2);

    let error = sink
        .promote(SCOPE_HASH, &fact_c)
        .await
        .expect_err("an unreadable scope shard aborts the run");
    assert!(
        matches!(error, mem_episodic::EpisodicError::Store(_)),
        "the seam's error must surface as `EpisodicError::Store`: {error}"
    );

    let registry = registry_rows(&fresh, SCOPE_HASH).await;
    assert_eq!(
        registry.len(),
        3,
        "the resolution retired the full shard 1 and created shard 2 BEFORE the scan: {registry:?}"
    );
    assert!(registry[2].is_active, "shard 2 is the active shard");
    // AC7's "0 rows across all three shards", realised: shard 0's file is gone
    // (0 rows by construction, absence asserted below), so the countable set
    // is the two surviving shards.
    assert!(
        !shard_zero.exists(),
        "the scan must never re-create a missing shard file"
    );
    assert_eq!(
        count_rows_across_shards(&fresh, SCOPE_HASH, &fact_c.id).await,
        0,
        "no row with C's id in the two surviving shards"
    );
}

// ---------------------------------------------------------------------------
// W3 — the absolute count authority on the promotion path
// ---------------------------------------------------------------------------

/// AC17 — promote twice: both count authorities stay exact at 1 and the
/// `AlreadyPresent` path never resyncs.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_keeps_both_count_authorities_exact() {
    let dir = TestDir::new("promotion-count-authority");
    let shards = shard_manager(dir.store_config());
    let sink = ShardPermanentMemorySink::new(shards.clone(), 8);
    let record = record("fact-1", "use redis", 8);

    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("first promote"),
        PromoteOutcome::Inserted
    );
    let info = registry_rows(&shards, SCOPE_HASH)
        .await
        .into_iter()
        .next()
        .expect("a shard registry row");
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    assert_eq!(
        mem_store::shard_facts::read_stored_count(db.as_ref())
            .await
            .expect("shard_stats count"),
        1,
        "the insert path writes the absolute count"
    );
    assert_eq!(
        shards
            .get_shard_by_id(info.id)
            .await
            .expect("registry read")
            .expect("registry row")
            .vector_count,
        1,
        "the registry adopted the same value"
    );
    drop(db);

    assert_eq!(
        sink.promote(SCOPE_HASH, &record)
            .await
            .expect("second promote"),
        PromoteOutcome::AlreadyPresent
    );
    let info = registry_rows(&shards, SCOPE_HASH)
        .await
        .into_iter()
        .next()
        .expect("a shard registry row");
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    assert_eq!(
        mem_store::shard_facts::read_stored_count(db.as_ref())
            .await
            .expect("shard_stats count"),
        1,
        "the AlreadyPresent path leaves the count alone"
    );
    assert_eq!(
        shards
            .get_shard_by_id(info.id)
            .await
            .expect("registry read")
            .expect("registry row")
            .vector_count,
        1
    );
    drop(db);
    assert_eq!(
        memory_rows(&shards, SCOPE_HASH, CONTAINER_TAG).await.len(),
        1
    );
}
