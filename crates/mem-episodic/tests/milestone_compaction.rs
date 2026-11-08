//! Issue 43 W6c — the milestone dead-image counter (Step 16).
//!
//! The sweep never selects a row by deletion-eligibility (D-19-5): the physical
//! rewrite is `VACUUM`'s. This module only **counts** the milestones whose
//! `contributors` JSON already carries a scrubbed (empty-summary) contributor,
//! so the sweep report can name `milestone_compacted` truthfully.
//!
//! Invariants (invariant-guard): rows are read in rowid order; the loop
//! invariant is "counted so far = scrubbed-contributor rows among rows[0..i)";
//! the scan advances by the page size and terminates when a page returns fewer
//! than `PAGE` rows (bounded by the table's row count).
//!
//! A corrupt `contributors` row is SKIPPED (fail-soft, the shipped pre-scan
//! rule in `purge.rs`): a count must never refuse the sweep.

mod common;

use mem_episodic::milestone::Contributor;
use mem_episodic::store::EpisodicStore;
use mem_store::db::Value;
use mem_store::vector::vector_to_text;

fn contributor(summary: &str) -> Contributor {
    Contributor {
        session_row_id: "row-1".to_string(),
        session_id: "s-1".to_string(),
        parent_session_id: None,
        agent_id: "agent-a".to_string(),
        agent_name: "Agent A".to_string(),
        created_at: 11,
        summary: summary.to_string(),
    }
}

fn contributors_json(summaries: &[&str]) -> String {
    let contributors: Vec<Contributor> = summaries.iter().map(|text| contributor(text)).collect();
    mem_episodic::milestone::encode_contributors(&contributors).expect("encode contributors")
}

async fn insert_milestone(store: &EpisodicStore, id: &str, contributors: &str) {
    let db = store.ready().await.expect("ready");
    db.execute(
        "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)",
        vec![
            Value::Text(id.to_string()),
            Value::Text("project-a".to_string()),
            Value::Text("agent-a".to_string()),
            Value::Text("merged summary".to_string()),
            Value::Text(contributors.to_string()),
            Value::Integer(1),
            Value::Integer(11),
            Value::Integer(11),
            Value::Text(vector_to_text(&[0.5_f32; 8]).expect("vector serializes")),
            Value::Integer(99),
        ],
    )
    .await
    .expect("insert milestone row");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_counts_only_milestones_with_a_scrubbed_contributor() {
    let dir = common::TestDir::new("milestone-compaction-count");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    insert_milestone(
        &store,
        "scrubbed-1",
        &contributors_json(&["", "still here"]),
    )
    .await;
    insert_milestone(&store, "scrubbed-2", &contributors_json(&[""])).await;
    insert_milestone(
        &store,
        "clean",
        &contributors_json(&["retained text", "more text"]),
    )
    .await;

    let db = store.ready().await.expect("ready");
    let count = mem_episodic::milestone_compaction::count_scrubbed_milestones(db.as_ref())
        .await
        .expect("count");
    assert_eq!(count, 2, "two rows carry an empty-summary contributor");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_empty_table_counts_zero() {
    let dir = common::TestDir::new("milestone-compaction-empty");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    let db = store.ready().await.expect("ready");

    let count = mem_episodic::milestone_compaction::count_scrubbed_milestones(db.as_ref())
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_corrupt_contributors_row_is_skipped() {
    let dir = common::TestDir::new("milestone-compaction-corrupt");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    insert_milestone(&store, "corrupt", "not json").await;
    insert_milestone(&store, "scrubbed", &contributors_json(&[""])).await;

    let db = store.ready().await.expect("ready");
    let count = mem_episodic::milestone_compaction::count_scrubbed_milestones(db.as_ref())
        .await
        .expect("a corrupt row must not refuse the count");
    assert_eq!(
        count, 1,
        "the corrupt row is skipped, the scrubbed one counts"
    );
}
