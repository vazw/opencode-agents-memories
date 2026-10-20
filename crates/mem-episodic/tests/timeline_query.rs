//! W4.2 Step 2 — chronological + filtered timeline (net-new, R10: there is no
//! upstream timeline service or test; the acceptance criteria define the
//! semantics).
//!
//! Every case runs against a real turso database through
//! `EpisodicStore::timeline`. Time boundaries are derived from the inserted
//! rows' own `created_at` values (read back through `get_session`), never from
//! a hardcoded clock; ordering between adjacent rows is separated by the
//! W4.1-proven `std::thread::sleep(Duration::from_millis(3))`.

mod common;

use std::time::Duration;

use common::TestDir;
use mem_episodic::error::EpisodicError;
use mem_episodic::store::EpisodicStore;
use mem_episodic::timeline::{
    DEFAULT_TIMELINE_LIMIT, MAX_TIMELINE_LIMIT, SessionSummary, TimelineFilter,
};
use mem_store::db::Value;

const DIMS: usize = 8;

fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMS];
    vector[index] = 1.0;
    vector
}

fn fixture(prefix: &str) -> (TestDir, EpisodicStore) {
    let dir = TestDir::new(prefix);
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    (dir, store)
}

fn assert_validation_field(error: EpisodicError, expected: &str) {
    match error {
        EpisodicError::Validation { field, .. } => assert_eq!(field, expected),
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// The three-row fixture shared by the ordering, time-range and matrix cases.
///
/// Newest-first order is `c, b, a`; `t_a < t_b < t_c` holds by construction.
struct Seeded {
    a: String,
    t_a: i64,
    b: String,
    t_b: i64,
    c: String,
    t_c: i64,
}

async fn seed_timeline(store: &EpisodicStore) -> Seeded {
    let (a, t_a) = common::insert_row(
        store,
        "project-a",
        "s-a",
        "agent-a",
        "did a",
        vec!["src/a.rs".to_string()],
        unit_vector(0),
    )
    .await;
    std::thread::sleep(Duration::from_millis(3));
    let (b, t_b) = common::insert_row(
        store,
        "project-a",
        "s-b",
        "agent-b",
        "did b",
        vec!["src/b.rs".to_string()],
        unit_vector(1),
    )
    .await;
    std::thread::sleep(Duration::from_millis(3));
    let (c, t_c) = common::insert_row(
        store,
        "project-a",
        "s-c",
        "agent-a",
        "did c",
        vec!["src/b.rs".to_string(), "src/c.rs".to_string()],
        unit_vector(2),
    )
    .await;
    assert!(
        t_a < t_b && t_b < t_c,
        "fixture ordering: {t_a} {t_b} {t_c}"
    );
    Seeded {
        a,
        t_a,
        b,
        t_b,
        c,
        t_c,
    }
}

async fn timeline_ids(
    store: &EpisodicStore,
    project_id: &str,
    filter: &TimelineFilter,
) -> Vec<String> {
    store
        .timeline(project_id, filter)
        .await
        .expect("timeline")
        .into_iter()
        .map(|summary| summary.id)
        .collect()
}

fn files(entries: &[&str]) -> Option<Vec<String>> {
    Some(entries.iter().map(|entry| entry.to_string()).collect())
}

// ---------------------------------------------------------------------------
// Ordering + scoping
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_returns_newest_first() {
    let (_dir, store) = fixture("timeline-newest-first");
    let seeded = seed_timeline(&store).await;

    let ids = timeline_ids(&store, "project-a", &TimelineFilter::default()).await;
    assert_eq!(ids, vec![seeded.c, seeded.b, seeded.a]);

    let listed: Vec<SessionSummary> = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("timeline");
    let summaries: Vec<&str> = listed
        .iter()
        .map(|summary| summary.summary.as_str())
        .collect();
    assert_eq!(summaries, vec!["did c", "did b", "did a"]);
    // The summary shape carries the v2/orphan surface too.
    assert!(listed.iter().all(|summary| summary.similarity.is_none()));
    assert!(listed.iter().all(|summary| !summary.orphaned));
    assert_eq!(listed[0].agent_name, "agent-a");
    assert_eq!(listed[0].files_touched, vec!["src/b.rs", "src/c.rs"]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_is_project_scoped() {
    let (_dir, store) = fixture("timeline-scope");
    let (_a, _t_a) = common::insert_row(
        &store,
        "project-a",
        "s-a",
        "agent-1",
        "a one",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (a_two, _t) = common::insert_row(
        &store,
        "project-a",
        "s-a2",
        "agent-1",
        "a two",
        Vec::new(),
        unit_vector(1),
    )
    .await;
    let (b_one, _t) = common::insert_row(
        &store,
        "project-b",
        "s-b",
        "agent-1",
        "b one",
        Vec::new(),
        unit_vector(2),
    )
    .await;

    let a_ids = timeline_ids(&store, "project-a", &TimelineFilter::default()).await;
    assert_eq!(a_ids.len(), 2, "no cross-project bleed: {a_ids:?}");
    assert!(a_ids.contains(&a_two));
    assert!(!a_ids.contains(&b_one));

    let b_ids = timeline_ids(&store, "project-b", &TimelineFilter::default()).await;
    assert_eq!(b_ids, vec![b_one]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_unknown_project_returns_empty() {
    let (_dir, store) = fixture("timeline-unknown-project");
    seed_timeline(&store).await;

    // A well-formed but unknown project id is an empty page, never an error.
    let ids = timeline_ids(&store, "project-unknown", &TimelineFilter::default()).await;
    assert!(ids.is_empty(), "{ids:?}");

    // A blank/malformed project id is rejected instead.
    let error = store
        .timeline("", &TimelineFilter::default())
        .await
        .expect_err("blank project rejected");
    assert_validation_field(error, "project_id");
}

// ---------------------------------------------------------------------------
// last_n_sessions
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_last_n_zero_returns_empty() {
    let (_dir, store) = fixture("timeline-last-n-zero");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        last_n_sessions: Some(0),
        ..Default::default()
    };
    let listed = store
        .timeline("project-a", &filter)
        .await
        .expect("zero last_n is an empty page, not an error");
    assert!(listed.is_empty(), "{listed:?}");

    // The rows are still there — zero is not a delete.
    assert!(store.get_session(&seeded.a).await.expect("get").is_some());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_last_n_truncates_after_filtering() {
    let (_dir, store) = fixture("timeline-last-n-truncate");
    let seeded = seed_timeline(&store).await;

    // agent-a matches a + c; the truncation applies AFTER the filter, so the
    // single newest matching entry (c) is returned — not a's row.
    let filter = TimelineFilter {
        last_n_sessions: Some(1),
        agent_id: Some("agent-a".to_string()),
        ..Default::default()
    };
    let ids = timeline_ids(&store, "project-a", &filter).await;
    assert_eq!(ids, vec![seeded.c.clone()], "filtered then truncated");

    let filter_all = TimelineFilter {
        last_n_sessions: Some(2),
        ..Default::default()
    };
    let ids = timeline_ids(&store, "project-a", &filter_all).await;
    assert_eq!(ids, vec![seeded.c, seeded.b]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_defaults_to_default_timeline_limit() {
    let (_dir, store) = fixture("timeline-default-limit");

    // The default page size is the pinned `DEFAULT_TIMELINE_LIMIT`.
    let total = DEFAULT_TIMELINE_LIMIT + 1;
    for index in 0..total {
        common::insert_row(
            &store,
            "project-a",
            &format!("s-{index}"),
            "agent-1",
            "row",
            Vec::new(),
            unit_vector(0),
        )
        .await;
    }

    let listed = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("timeline");
    assert_eq!(
        listed.len(),
        DEFAULT_TIMELINE_LIMIT,
        "default filter must cap at DEFAULT_TIMELINE_LIMIT"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_rejects_last_n_above_max() {
    let (_dir, store) = fixture("timeline-last-n-above-max");
    seed_timeline(&store).await;

    let filter = TimelineFilter {
        last_n_sessions: Some(MAX_TIMELINE_LIMIT + 1),
        ..Default::default()
    };
    let error = store
        .timeline("project-a", &filter)
        .await
        .expect_err("above-max last_n rejected");
    assert_validation_field(error, "last_n_sessions");
}

// ---------------------------------------------------------------------------
// Time range
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_since_is_inclusive() {
    let (_dir, store) = fixture("timeline-since-inclusive");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        since: Some(seeded.t_b),
        ..Default::default()
    };
    let ids = timeline_ids(&store, "project-a", &filter).await;
    assert_eq!(
        ids,
        vec![seeded.c.clone(), seeded.b.clone()],
        "since must include the boundary row"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_until_is_inclusive() {
    let (_dir, store) = fixture("timeline-until-inclusive");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        until: Some(seeded.t_b),
        ..Default::default()
    };
    let ids = timeline_ids(&store, "project-a", &filter).await;
    assert_eq!(
        ids,
        vec![seeded.b.clone(), seeded.a.clone()],
        "until must include the boundary row"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_rejects_inverted_time_range() {
    let (_dir, store) = fixture("timeline-inverted-range");
    seed_timeline(&store).await;

    let filter = TimelineFilter {
        since: Some(2_000),
        until: Some(1_000),
        ..Default::default()
    };
    let error = store
        .timeline("project-a", &filter)
        .await
        .expect_err("inverted range rejected");
    assert_validation_field(error, "since");

    let negative = TimelineFilter {
        since: Some(-1),
        ..Default::default()
    };
    let error = store
        .timeline("project-a", &negative)
        .await
        .expect_err("negative since rejected");
    assert_validation_field(error, "since");
}

// ---------------------------------------------------------------------------
// Column filters
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_agent_id_filter() {
    let (_dir, store) = fixture("timeline-agent-filter");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        agent_id: Some("agent-a".to_string()),
        ..Default::default()
    };
    let ids = timeline_ids(&store, "project-a", &filter).await;
    assert_eq!(ids, vec![seeded.c, seeded.a]);

    let missing = TimelineFilter {
        agent_id: Some("agent-missing".to_string()),
        ..Default::default()
    };
    assert!(timeline_ids(&store, "project-a", &missing).await.is_empty());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_session_id_filter() {
    let (_dir, store) = fixture("timeline-session-filter");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        session_id: Some("s-b".to_string()),
        ..Default::default()
    };
    let ids = timeline_ids(&store, "project-a", &filter).await;
    assert_eq!(ids, vec![seeded.b]);

    let missing = TimelineFilter {
        session_id: Some("s-missing".to_string()),
        ..Default::default()
    };
    assert!(timeline_ids(&store, "project-a", &missing).await.is_empty());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_rejects_blank_touched_file_entry() {
    let (_dir, store) = fixture("timeline-blank-file");
    seed_timeline(&store).await;

    let blank = TimelineFilter {
        touched_files: Some(vec!["  ".to_string()]),
        ..Default::default()
    };
    let error = store
        .timeline("project-a", &blank)
        .await
        .expect_err("blank entry rejected");
    assert_validation_field(error, "touched_files");

    let empty_list = TimelineFilter {
        touched_files: Some(Vec::new()),
        ..Default::default()
    };
    let error = store
        .timeline("project-a", &empty_list)
        .await
        .expect_err("empty list rejected");
    assert_validation_field(error, "touched_files");
}

// ---------------------------------------------------------------------------
// Corrupt metadata (fail-soft on the list path, F5)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_skips_corrupt_metadata_rows() {
    let (_dir, store) = fixture("timeline-corrupt-metadata");
    let (good, _t) = common::insert_row(
        &store,
        "project-a",
        "s-good",
        "agent-1",
        "good row",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (broken, t_broken) = common::insert_row(
        &store,
        "project-a",
        "s-broken",
        "agent-1",
        "broken row",
        Vec::new(),
        unit_vector(1),
    )
    .await;
    assert!(t_broken > 0);

    let db = store.ready().await.expect("ready");
    db.execute(
        "UPDATE agent_sessions SET metadata = ? WHERE id = ?",
        vec![
            Value::Text("{not valid json".to_string()),
            Value::Text(broken.clone()),
        ],
    )
    .await
    .expect("corrupt the row");

    let listed = store
        .timeline("project-a", &TimelineFilter::default())
        .await
        .expect("list path fails soft");
    let ids: Vec<String> = listed.iter().map(|summary| summary.id.clone()).collect();
    assert_eq!(ids, vec![good], "corrupt row skipped, good row served");
}

// ---------------------------------------------------------------------------
// Filter matrix (AND-only optional predicates)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_filter_matrix_time_range_only() {
    let (_dir, store) = fixture("timeline-matrix-time");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        since: Some(seeded.t_b),
        until: Some(seeded.t_c),
        ..Default::default()
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter).await,
        vec![seeded.c, seeded.b]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_filter_matrix_agent_only() {
    let (_dir, store) = fixture("timeline-matrix-agent");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        agent_id: Some("agent-b".to_string()),
        ..Default::default()
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter).await,
        vec![seeded.b]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_filter_matrix_touched_files_only() {
    let (_dir, store) = fixture("timeline-matrix-files");
    let seeded = seed_timeline(&store).await;

    // ANY-match (non-empty intersection), case-sensitive, exact equality.
    let filter = TimelineFilter {
        touched_files: files(&["src/b.rs"]),
        ..Default::default()
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter).await,
        vec![seeded.c, seeded.b]
    );

    let case_sensitive = TimelineFilter {
        touched_files: files(&["src/b.RS"]),
        ..Default::default()
    };
    assert!(
        timeline_ids(&store, "project-a", &case_sensitive)
            .await
            .is_empty(),
        "the match is case-sensitive"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_filter_matrix_time_and_agent() {
    let (_dir, store) = fixture("timeline-matrix-time-agent");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        since: Some(seeded.t_b),
        agent_id: Some("agent-a".to_string()),
        ..Default::default()
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter).await,
        vec![seeded.c]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_filter_matrix_time_and_files() {
    let (_dir, store) = fixture("timeline-matrix-time-files");
    let seeded = seed_timeline(&store).await;

    // until t_b AND files {a.rs, b.rs} ⇒ b (b.rs) and a (a.rs); c has b.rs
    // but is newer than the until bound.
    let filter = TimelineFilter {
        until: Some(seeded.t_b),
        touched_files: files(&["src/a.rs", "src/b.rs"]),
        ..Default::default()
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter).await,
        vec![seeded.b, seeded.a]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_filter_matrix_agent_and_files() {
    let (_dir, store) = fixture("timeline-matrix-agent-files");
    let seeded = seed_timeline(&store).await;

    let filter = TimelineFilter {
        agent_id: Some("agent-a".to_string()),
        touched_files: files(&["src/b.rs"]),
        ..Default::default()
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter).await,
        vec![seeded.c],
        "a is agent-a but never touched src/b.rs"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_timeline_filter_matrix_time_agent_files_and_last_n() {
    let (_dir, store) = fixture("timeline-matrix-all");
    let seeded = seed_timeline(&store).await;

    // time ≥ t_a AND agent-a AND files {a.rs, c.rs} ⇒ c, a; last_n = 1 ⇒ c.
    let filter = TimelineFilter {
        since: Some(seeded.t_a),
        agent_id: Some("agent-a".to_string()),
        touched_files: files(&["src/a.rs", "src/c.rs"]),
        last_n_sessions: Some(1),
        ..Default::default()
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter).await,
        vec![seeded.c.clone()]
    );

    // Dropping last_n exposes both matches in newest-first order.
    let filter_all = TimelineFilter {
        last_n_sessions: None,
        ..filter
    };
    assert_eq!(
        timeline_ids(&store, "project-a", &filter_all).await,
        vec![seeded.c, seeded.a]
    );
}
