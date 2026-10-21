//! W4.3 lane — milestone consolidation (AC-6).
//!
//! The seams under test are `Consolidator::run`, the store's milestone
//! accessors (`EpisodicStore::list_milestones`) and the pure
//! `plan_milestone`. Everything runs against a real turso database in a temp
//! dir; time comes from the injected clock, never from the wall clock.

mod common;

use std::sync::{Arc, Mutex};

use common::{ConsolidationFixture, DAY_MILLIS, SummarizerHook, TestDir};
use mem_episodic::milestone::{
    MAX_CONTRIBUTORS_FRAMING_BYTES_PER_ROW, encode_contributors, max_contributors_bytes,
    plan_milestone,
};
use mem_episodic::record::AgentSession;
use mem_episodic::store::EpisodicStore;
use mem_store::db::{Value, query_one};
use mem_store::vector::vector_to_text;

/// The stored contributors JSON of one source session (camelCase wire shape).
const CONTRIBUTORS_JSON: &str = r#"[{"sessionRowId":"row-1","sessionId":"s-1","agentId":"agent-a","agentName":"Agent A","createdAt":11,"summary":"first source summary"}]"#;

/// Raw milestone insert (the write path lands with the consolidator).
async fn raw_insert_milestone(
    store: &EpisodicStore,
    id: &str,
    project_id: &str,
    agent_id: &str,
    contributors: &str,
    created_at: i64,
) {
    let db = store.ready().await.expect("ready");
    db.execute(
        "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(project_id.to_string()),
            Value::Text(agent_id.to_string()),
            Value::Text("merged summary".to_string()),
            Value::Text(contributors.to_string()),
            Value::Integer(1),
            Value::Integer(11),
            Value::Integer(11),
            Value::Text(vector_to_text(&[0.5_f32; 8])),
            Value::Integer(created_at),
        ],
    )
    .await
    .expect("insert milestone row");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_milestones_decodes_contributors_and_excludes_the_embedding() {
    let dir = TestDir::new("milestone-read");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    raw_insert_milestone(
        &store,
        "milestone-1",
        "project-a",
        "agent-a",
        CONTRIBUTORS_JSON,
        99,
    )
    .await;

    let milestones = store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    let milestone = &milestones[0];
    assert_eq!(milestone.id, "milestone-1");
    assert_eq!(milestone.project_id, "project-a");
    assert_eq!(milestone.agent_id, "agent-a");
    assert_eq!(milestone.summary, "merged summary");
    assert_eq!(milestone.source_count, 1);
    assert_eq!(milestone.window_start, 11);
    assert_eq!(milestone.window_end, 11);
    assert_eq!(milestone.created_at, 99);
    assert!(
        milestone.embedding.is_empty(),
        "the embedding is never read back"
    );
    assert_eq!(milestone.contributors.len(), 1);
    assert_eq!(milestone.contributors[0].session_row_id, "row-1");
    assert_eq!(milestone.contributors[0].session_id, "s-1");
    assert_eq!(milestone.contributors[0].agent_id, "agent-a");
    assert_eq!(milestone.contributors[0].agent_name, "Agent A");
    assert_eq!(milestone.contributors[0].created_at, 11);
    assert_eq!(
        milestone.contributors[0].summary, "first source summary",
        "the retained source summary survives the round trip"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_milestones_is_project_scoped() {
    let dir = TestDir::new("milestone-project-scope");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    raw_insert_milestone(
        &store,
        "milestone-a",
        "project-a",
        "agent-a",
        CONTRIBUTORS_JSON,
        10,
    )
    .await;
    raw_insert_milestone(
        &store,
        "milestone-b",
        "project-b",
        "agent-a",
        CONTRIBUTORS_JSON,
        11,
    )
    .await;

    let milestones = store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].id, "milestone-a");
}

// ---------------------------------------------------------------------------
// AC-6 — the milestone write, the retained summaries and the contributors
// ---------------------------------------------------------------------------

/// The default window: 30 days.
const DEFAULT_AGE_DAYS: u32 = 30;

/// A session row fixture for the pure `plan_milestone` test.
fn plan_row(
    id: &str,
    session_id: &str,
    parent: Option<&str>,
    agent_id: &str,
    agent_name: &str,
    summary: &str,
    created_at: i64,
) -> AgentSession {
    AgentSession {
        id: id.to_string(),
        project_id: "project-a".to_string(),
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        parent_session_id: parent.map(str::to_string),
        summary: summary.to_string(),
        metadata: mem_episodic::payload::SessionMetadata {
            timestamp: created_at,
            agent_name: agent_name.to_string(),
            ..Default::default()
        },
        embedding: Vec::new(),
        created_at,
        updated_at: created_at,
        deleted_at: None,
        capture_id: None,
        orphaned: false,
    }
}

#[test]
fn test_contributors_preserve_agent_identity_and_parent_links() {
    let rows = vec![
        plan_row(
            "row-parent",
            "session-parent",
            None,
            "agent-a",
            "Agent A",
            "the parent session",
            10,
        ),
        plan_row(
            "row-child",
            "session-child",
            Some("row-parent"),
            "agent-b",
            "Agent B",
            "the child session",
            20,
        ),
    ];

    let milestone = plan_milestone("milestone-1", &rows, "merged", vec![0.5; 8], 99);

    assert_eq!(milestone.id, "milestone-1");
    assert_eq!(milestone.project_id, "project-a");
    assert_eq!(milestone.agent_id, "agent-a");
    assert_eq!(milestone.summary, "merged");
    assert_eq!(milestone.source_count, 2);
    assert_eq!(milestone.window_start, 10);
    assert_eq!(milestone.window_end, 20);
    assert_eq!(milestone.created_at, 99);
    assert_eq!(milestone.embedding, vec![0.5_f32; 8]);

    assert_eq!(milestone.contributors.len(), 2);
    let parent = &milestone.contributors[0];
    assert_eq!(parent.session_row_id, "row-parent");
    assert_eq!(parent.session_id, "session-parent");
    assert_eq!(parent.agent_id, "agent-a");
    assert_eq!(parent.agent_name, "Agent A");
    assert_eq!(parent.created_at, 10);
    assert_eq!(parent.summary, "the parent session");
    assert_eq!(parent.parent_session_id, None);

    let child = &milestone.contributors[1];
    assert_eq!(child.session_row_id, "row-child");
    assert_eq!(child.agent_id, "agent-b");
    assert_eq!(child.agent_name, "Agent B");
    assert_eq!(
        child.parent_session_id.as_deref(),
        Some("row-parent"),
        "the parent link survives the parent's deletion"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_consolidation_writes_one_milestone_and_removes_source_rows() {
    let fixture = ConsolidationFixture::new();
    let row = fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.milestones_written, 1);
    assert_eq!(report.sessions_consolidated, 1);

    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].summary, "a merged milestone summary");
    assert_eq!(milestones[0].agent_id, "agent-a");
    assert_eq!(milestones[0].source_count, 1);

    // The source row is gone (hard delete, decision 1).
    assert!(
        fixture
            .store
            .get_session(&row)
            .await
            .expect("get")
            .is_none(),
        "the source row was removed"
    );
    // The row is HARD deleted, not soft deleted: a raw count proves the row
    // itself is gone (a soft delete would still be counted here).
    let db = fixture.store.ready().await.expect("ready");
    let counted = query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM agent_sessions WHERE id = ?",
        vec![Value::Text(row.clone())],
    )
    .await
    .expect("count")
    .expect("count row");
    assert_eq!(
        counted.i64("count").unwrap_or(0),
        0,
        "the source row was HARD deleted (decision 1)"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_milestone_contributors_retain_source_summary_text() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the verbatim source summary",
            &[],
        )
        .await;

    fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].contributors.len(), 1);
    assert_eq!(
        milestones[0].contributors[0].summary, "the verbatim source summary",
        "the source summary text is retained verbatim"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_milestone_contributor_summary_is_privacy_stripped() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the secret is <private>hunter2</private> inside",
            &[],
        )
        .await;

    fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    let milestone = &milestones[0];
    assert!(
        !milestone.contributors[0].summary.contains("hunter2"),
        "the raw private text never survives: {}",
        milestone.contributors[0].summary
    );
    assert!(
        milestone.contributors[0].summary.contains("[REDACTED]"),
        "{}",
        milestone.contributors[0].summary
    );

    // The raw tag appears NOWHERE in the milestone row.
    let db = fixture.store.ready().await.expect("ready");
    let row = query_one(
        db.as_ref(),
        "SELECT summary, contributors FROM episodic_milestones WHERE id = ?",
        vec![Value::Text(milestone.id.clone())],
    )
    .await
    .expect("select milestone")
    .expect("milestone row");
    let summary = row.text("summary").unwrap_or_default();
    let contributors = row.text("contributors").unwrap_or_default();
    assert!(!summary.contains("hunter2"), "{summary}");
    assert!(!contributors.contains("hunter2"), "{contributors}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_milestone_contributors_retain_summary_for_every_consolidated_row() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -41 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the first summary",
            &[],
        )
        .await;
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-a",
            "   ",
            &[],
        )
        .await;

    fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    let contributors = &milestones[0].contributors;
    assert_eq!(contributors.len(), 2, "no contributor is dropped");
    assert_eq!(contributors[0].summary, "the first summary");
    assert_eq!(
        contributors[1].summary, "",
        "a source summary that strips to empty is still listed, with empty text"
    );
    assert_eq!(contributors[1].session_id, "session-b");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_milestone_contributors_json_stays_within_its_bound() {
    let fixture = ConsolidationFixture::new();
    for index in 0..3 {
        fixture
            .insert_at(
                -(40 - index) * DAY_MILLIS,
                "project-a",
                &format!("session-{index}"),
                "agent-a",
                "a source summary",
                &[],
            )
            .await;
    }

    fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    let contributors = &milestones[0].contributors;
    let encoded = encode_contributors(contributors).expect("encode");
    assert!(
        encoded.len() <= max_contributors_bytes(contributors.len()),
        "the assembled JSON is within its bound ({} > {})",
        encoded.len(),
        max_contributors_bytes(contributors.len())
    );
    assert_eq!(MAX_CONTRIBUTORS_FRAMING_BYTES_PER_ROW, 1_536);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_oversize_contributors_json_skips_agent_and_writes_nothing() {
    let fixture = ConsolidationFixture::new();
    // Two 100_000-byte summaries: each transcript chunk still fits under the
    // transcript cap (2 × ~100_100 bytes), but the JSON encoding ESCAPES the
    // quotes 2×, so the assembled contributors JSON (~402_000 bytes) exceeds
    // `max_contributors_bytes(2)` (= 265_216) even though the RAW retained
    // text (~200_000 bytes) does not.
    let quotes = "\"".repeat(100_000);
    fixture
        .insert_at(
            -41 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            &quotes,
            &[],
        )
        .await;
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-a",
            &quotes,
            &[],
        )
        .await;

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let report = fixture
        .consolidator(DEFAULT_AGE_DAYS, writer)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(
        report.agents_skipped[0].reason,
        mem_episodic::consolidation::SkipReason::ContributorsTooLarge
    );
    assert_eq!(fixture.sink.call_count(), 0, "no promotion was attempted");
    assert_eq!(fixture.recorder.call_count(), 0, "no batch was attempted");
    assert_eq!(report.milestones_written, 0);

    // Both source rows are still present.
    let remaining = fixture
        .store
        .list_sessions("project-a", 10)
        .await
        .expect("list sessions");
    assert_eq!(
        remaining.len(),
        2,
        "the skipped agent's rows stay untouched"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_surviving_child_of_consolidated_parent_is_flagged_orphaned() {
    let fixture = ConsolidationFixture::new();
    let parent = fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-parent",
            "agent-a",
            "the parent session",
            &[],
        )
        .await;
    // The child is too young to be consolidated, so it SURVIVES the parent's
    // removal.
    let child = fixture
        .insert_at(
            -DAY_MILLIS,
            "project-a",
            "session-child",
            "agent-a",
            "the child session",
            &[],
        )
        .await;
    // Bind the child to the parent through the public write path.
    let db = fixture.store.ready().await.expect("ready");
    db.execute(
        "UPDATE agent_sessions SET parent_session_id = ? WHERE id = ?",
        vec![Value::Text(parent.clone()), Value::Text(child.clone())],
    )
    .await
    .expect("link child");

    fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    let survivor = fixture
        .store
        .get_session(&child)
        .await
        .expect("get child")
        .expect("the child survived");
    assert!(survivor.orphaned, "the surviving child is flagged orphaned");
    assert_eq!(
        survivor.parent_session_id, None,
        "the FK nulled the severed link"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_soft_delete_between_claim_and_batch_yields_zero_changes_and_keeps_contributor() {
    let fixture = ConsolidationFixture::new();
    let soft_deleted = fixture
        .insert_at(
            -41 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the soft-deleted session",
            &[],
        )
        .await;
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-a",
            "the surviving session",
            &[],
        )
        .await;

    // The summarizer hook soft-deletes one claimed row BETWEEN the claim and
    // the batch.
    let store = fixture.store.clone();
    let target = soft_deleted.clone();
    let hook: SummarizerHook = Arc::new(move || {
        let store = store.clone();
        let target = target.clone();
        Box::pin(async move {
            store
                .soft_delete_session("project-a", &target)
                .await
                .expect("soft delete");
        })
    });
    let summarizer =
        Arc::new(common::FakeSummarizer::returning("a merged milestone summary").with_hook(hook));
    let deps = mem_episodic::consolidation::ConsolidationDeps {
        summarizer,
        embedder: fixture.embedder.clone(),
        sink: fixture.sink.clone(),
        batch_writer: Arc::new(mem_episodic::consolidation::StoreBatchWriter::new(
            fixture.store.clone(),
        )),
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: 200,
        probe: Arc::new(mem_store::platform::SystemProcessProbe),
    };
    let consolidator = mem_episodic::consolidation::Consolidator::new(
        fixture.store.clone(),
        mem_episodic::consolidation::ConsolidationPolicy::new(DEFAULT_AGE_DAYS).expect("policy"),
        deps,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    // The soft-deleted row matched zero rows: it is not an error and it is
    // not counted.
    assert_eq!(report.sessions_consolidated, 1);
    assert_eq!(report.milestones_written, 1);
    assert_eq!(report.agents_skipped.len(), 0);

    // The soft-deleted row is still listed as a contributor.
    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    let contributors = &milestones[0].contributors;
    assert_eq!(contributors.len(), 2);
    let kept = contributors
        .iter()
        .find(|contributor| contributor.session_row_id == soft_deleted)
        .expect("the soft-deleted row is still a contributor");
    assert_eq!(kept.summary, "the soft-deleted session");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_precedes_the_episodic_write_batch() {
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(common::RecordingSink::with_events(events.clone()));
    let writer = Arc::new(common::RecordingBatchWriter::with_events(events.clone()));

    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;

    let deps = mem_episodic::consolidation::ConsolidationDeps {
        summarizer: fixture.summarizer.clone(),
        embedder: fixture.embedder.clone(),
        sink,
        batch_writer: writer,
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: 200,
        probe: Arc::new(mem_store::platform::SystemProcessProbe),
    };
    let consolidator = mem_episodic::consolidation::Consolidator::new(
        fixture.store.clone(),
        mem_episodic::consolidation::ConsolidationPolicy::new(DEFAULT_AGE_DAYS).expect("policy"),
        deps,
    );
    consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    let log = events.lock().expect("event log").clone();
    assert_eq!(
        log,
        vec!["promote".to_string(), "batch".to_string()],
        "the promotion precedes the episodic batch"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_milestone_embedding_matches_store_dimensions() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &[],
        )
        .await;

    fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    let db = fixture.store.ready().await.expect("ready");
    let row = query_one(
        db.as_ref(),
        "SELECT vector_extract(embedding) AS embedding FROM episodic_milestones",
        Vec::new(),
    )
    .await
    .expect("select embedding")
    .expect("milestone row");
    let raw = row.get("embedding").cloned().expect("embedding column");
    let parsed = mem_store::vector_utils::parse_extracted_vector(&raw).expect("vector");
    assert_eq!(
        parsed.len(),
        fixture.store.dimensions() as usize,
        "the milestone embedding carries the store's dimension count"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_each_milestone_row_has_a_distinct_id() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "agent a session",
            &[],
        )
        .await;
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-b",
            "agent b session",
            &[],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");
    assert_eq!(report.milestones_written, 2);

    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 2);
    assert_ne!(
        milestones[0].id, milestones[1].id,
        "every milestone row has its own id"
    );
    assert!(!milestones[0].id.is_empty());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_second_run_after_consolidation_sees_no_source_rows_and_calls_no_summarizer() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &[],
        )
        .await;

    let first = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("first run");
    assert_eq!(first.milestones_written, 1);
    assert_eq!(fixture.summarizer.call_count(), 1);

    let second = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("second run");
    assert_eq!(second.agents_considered, 0);
    assert_eq!(second.milestones_written, 0);
    assert_eq!(
        fixture.summarizer.call_count(),
        1,
        "the second run calls no summarizer"
    );
    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1, "no duplicate milestone");
}

// ---------------------------------------------------------------------------
// The C3 pin — the selection sees ONLY the consolidated prefix `rows[..k]`
// ---------------------------------------------------------------------------

/// The promotion selection must see ONLY the rows the milestone actually
/// consolidated — never the whole claimed set.
///
/// The transcript cap is what creates the gap: when the claimed rows do not
/// all fit, the consolidator merges only the FIRST `k` of them and leaves
/// the rest live for a later run. A selection that saw the whole claimed set
/// would promote decisions from rows that were neither in the milestone nor
/// deleted — into the PERMANENT tier, with no trace in the milestone.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_selection_sees_only_the_consolidated_prefix() {
    let fixture = ConsolidationFixture::new();
    // Row A's chunk is ~200 KB (a 100_000-byte summary plus a 100_006-byte
    // decision), row B's is ~100 KB; together they exceed the 262_144-byte
    // transcript cap, so only row A is consolidated.
    let big_summary = "a".repeat(100_000);
    let long_decision = format!("alpha {}", "b".repeat(100_000));
    let row_a = fixture
        .insert_at(
            -41 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            &big_summary,
            &[&long_decision],
        )
        .await;
    // A DISTINCT decision text: identical texts would land in one dedup
    // group and a selection mutation would still yield ONE promoted fact.
    let row_b = fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-a",
            &"c".repeat(100_000),
            &["beta decision"],
        )
        .await;

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    // Only row A's decision is promoted.
    assert_eq!(
        report.facts_promoted, 1,
        "only row A's decision is promoted"
    );
    assert_eq!(fixture.sink.call_count(), 1, "exactly one promotion");
    assert_eq!(
        fixture.sink.calls()[0].1.source_row_ids,
        vec![row_a.clone()],
        "the promoted fact's only source is row A"
    );

    // The milestone covers row A only.
    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].source_count, 1);
    assert_eq!(milestones[0].contributors.len(), 1);
    assert_eq!(milestones[0].contributors[0].session_row_id, row_a);
    assert_eq!(report.sessions_consolidated, 1);

    // Row A was consolidated away; row B did not fit and stays live.
    assert!(
        fixture
            .store
            .get_session(&row_a)
            .await
            .expect("get row A")
            .is_none(),
        "row A was consolidated away"
    );
    assert!(
        fixture
            .store
            .get_session(&row_b)
            .await
            .expect("get row B")
            .is_some(),
        "row B was NOT consolidated: it did not fit the transcript cap"
    );
}
