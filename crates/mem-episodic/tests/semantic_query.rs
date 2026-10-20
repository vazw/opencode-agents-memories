//! W4.2 Step 3 — semantic timeline (net-new, R10: no upstream oracle).
//!
//! The semantic path embeds the query with [`mem_episodic::embedding::embed_query`]
//! (the W4.1 `PrefixOptions` with `EmbedTask::Query`) and ranks by the exact
//! `vector_distance_cos` scan — there is no vector index in this tier
//! (W1.3 exact-fallback semantics). Similarities are recomputed in-test from
//! the known inserted vectors, so the ranking assertion is not a tautology.

mod common;

use std::time::Duration;

use common::{FakeEmbedder, TestDir};
use mem_embed::embedder::EmbedTask;
use mem_embed::prefix::{PrefixOptions, apply_task_prefix};
use mem_episodic::error::EpisodicError;
use mem_episodic::store::EpisodicStore;
use mem_episodic::timeline::TimelineFilter;
use mem_store::db::Value;

const DIMS: usize = 8;

fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMS];
    vector[index] = 1.0;
    vector
}

fn diagonal_vector() -> Vec<f32> {
    vec![0.707_106_77, 0.707_106_77, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
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

/// Exact cosine similarity between two equally sized vectors (the in-test
/// oracle for the SQL ranking).
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum();
    let norm_a: f64 = a
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    let norm_b: f64 = b
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(0.0, 1.0)
}

async fn semantic_ids(
    store: &EpisodicStore,
    project_id: &str,
    query: &str,
    embedder: &dyn mem_embed::embedder::Embedder,
    filter: &TimelineFilter,
) -> Vec<String> {
    store
        .semantic_timeline(project_id, query, embedder, filter)
        .await
        .expect("semantic timeline")
        .into_iter()
        .map(|summary| summary.id)
        .collect()
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_ranks_by_similarity() {
    let (_dir, store) = fixture("semantic-ranking");
    let (exact, _t) = common::insert_row(
        &store,
        "project-a",
        "s-exact",
        "agent-1",
        "exact match",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (orthogonal, _t) = common::insert_row(
        &store,
        "project-a",
        "s-orthogonal",
        "agent-1",
        "orthogonal",
        Vec::new(),
        unit_vector(1),
    )
    .await;
    let (diagonal, _t) = common::insert_row(
        &store,
        "project-a",
        "s-diagonal",
        "agent-1",
        "diagonal",
        Vec::new(),
        diagonal_vector(),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let hits = store
        .semantic_timeline(
            "project-a",
            "find the exact row",
            &embedder,
            &TimelineFilter::default(),
        )
        .await
        .expect("semantic timeline");

    let ids: Vec<String> = hits.iter().map(|hit| hit.id.clone()).collect();
    assert_eq!(ids, vec![exact, diagonal, orthogonal], "hits: {hits:?}");

    let similarities: Vec<f64> = hits
        .iter()
        .map(|hit| hit.similarity.expect("semantic hits carry similarity"))
        .collect();
    assert!(similarities[0] > similarities[1], "{similarities:?}");
    assert!(similarities[1] > similarities[2], "{similarities:?}");
    assert!((similarities[0] - 1.0).abs() < 1e-6, "{similarities:?}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_is_exact_cosine_scan_without_vector_index() {
    let (_dir, store) = fixture("semantic-exact-scan");
    let query = unit_vector(0);
    let stored = [
        ("s-a", unit_vector(0)),
        ("s-b", diagonal_vector()),
        ("s-c", unit_vector(1)),
    ];
    let mut ids = Vec::new();
    for (session, vector) in &stored {
        let (id, _t) = common::insert_row(
            &store,
            "project-a",
            session,
            "agent-1",
            "row",
            Vec::new(),
            vector.clone(),
        )
        .await;
        ids.push(id);
    }

    // (a) No vector index exists on agent_sessions.
    let db = store.ready().await.expect("ready");
    let vector_indexes: Vec<String> = db
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'agent_sessions' AND name LIKE '%vec%'",
            Vec::new(),
        )
        .await
        .expect("sqlite_master")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect();
    assert!(
        vector_indexes.is_empty(),
        "the episodic tier has no vector index: {vector_indexes:?}"
    );

    // (b) The returned order/similarities equal the exact in-test computation.
    let embedder = FakeEmbedder::with_vector(query.clone());
    let hits = store
        .semantic_timeline(
            "project-a",
            "anything",
            &embedder,
            &TimelineFilter::default(),
        )
        .await
        .expect("semantic timeline");
    assert_eq!(hits.len(), 3);

    let mut expected: Vec<(f64, &String)> = stored
        .iter()
        .zip(ids.iter())
        .map(|((_session, vector), id)| (cosine_similarity(&query, vector), id))
        .collect();
    expected.sort_by(|left, right| right.0.partial_cmp(&left.0).expect("finite"));

    for (hit, (expected_similarity, expected_id)) in hits.iter().zip(expected.iter()) {
        assert_eq!(&hit.id, *expected_id);
        let actual = hit.similarity.expect("similarity present");
        assert!(
            (actual - expected_similarity).abs() < 1e-5,
            "similarity {actual} != exact cosine {expected_similarity}"
        );
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_applies_query_task_prefix() {
    let (_dir, store) = fixture("semantic-query-prefix");
    common::insert_row(
        &store,
        "project-a",
        "s-a",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let query = "where did we wire the store";
    store
        .semantic_timeline("project-a", query, &embedder, &TimelineFilter::default())
        .await
        .expect("semantic timeline");

    // The query side uses EmbedTask::Query with the same PrefixOptions as the
    // document path (D-15) — one call, exactly the composed query text.
    let calls = embedder.calls();
    assert_eq!(calls.len(), 1, "one query embedding per call: {calls:?}");
    assert_eq!(
        calls[0],
        mem_episodic::embedding::query_embedded_text(query),
        "the prefixed query text is pinned"
    );
    // With `PrefixOptions::default()` no prefix is applied (pinned no-op).
    assert_eq!(calls[0], query);
    assert_eq!(
        calls[0],
        apply_task_prefix(query, EmbedTask::Query, &PrefixOptions::default())
    );
}

// ---------------------------------------------------------------------------
// Rejections + failures
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_rejects_empty_query() {
    let (_dir, store) = fixture("semantic-empty-query");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let error = store
        .semantic_timeline("project-a", "", &embedder, &TimelineFilter::default())
        .await
        .expect_err("empty query rejected");
    assert_validation_field(error, "query");
    assert!(
        embedder.calls().is_empty(),
        "validation must run before the embedder"
    );

    let error = store
        .semantic_timeline("project-a", "  ", &embedder, &TimelineFilter::default())
        .await
        .expect_err("whitespace query rejected");
    assert_validation_field(error, "query");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_dimension_mismatch_is_validation_error() {
    let (_dir, store) = fixture("semantic-dim-mismatch");
    common::insert_row(
        &store,
        "project-a",
        "s-a",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(vec![0.5_f32; DIMS - 2]);
    let error = store
        .semantic_timeline("project-a", "query", &embedder, &TimelineFilter::default())
        .await
        .expect_err("dimension mismatch rejected");
    assert_validation_field(error, "embedding");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_embedder_failure_surfaces() {
    let (_dir, store) = fixture("semantic-embed-failure");
    common::insert_row(
        &store,
        "project-a",
        "s-a",
        "agent-1",
        "row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let embedder = FakeEmbedder::failing();
    let error = store
        .semantic_timeline("project-a", "query", &embedder, &TimelineFilter::default())
        .await
        .expect_err("embedder failure surfaces");
    assert!(matches!(error, EpisodicError::Embed(_)), "got {error:?}");
}

// ---------------------------------------------------------------------------
// Scoping + filters
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_is_project_scoped() {
    let (_dir, store) = fixture("semantic-scope");
    let (a_row, _t) = common::insert_row(
        &store,
        "project-a",
        "s-a",
        "agent-1",
        "a row",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (b_row, _t) = common::insert_row(
        &store,
        "project-b",
        "s-b",
        "agent-1",
        "b row",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let ids = semantic_ids(
        &store,
        "project-a",
        "query",
        &embedder,
        &TimelineFilter::default(),
    )
    .await;
    assert_eq!(ids, vec![a_row], "no cross-project bleed");
    assert!(!ids.contains(&b_row));

    let b_ids = semantic_ids(
        &store,
        "project-b",
        "query",
        &embedder,
        &TimelineFilter::default(),
    )
    .await;
    assert_eq!(b_ids, vec![b_row]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_respects_last_n_sessions() {
    let (_dir, store) = fixture("semantic-last-n");
    let (nearest, _t) = common::insert_row(
        &store,
        "project-a",
        "s-nearest",
        "agent-1",
        "nearest",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    common::insert_row(
        &store,
        "project-a",
        "s-diagonal",
        "agent-1",
        "diagonal",
        Vec::new(),
        diagonal_vector(),
    )
    .await;
    common::insert_row(
        &store,
        "project-a",
        "s-far",
        "agent-1",
        "far",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let filter = TimelineFilter {
        last_n_sessions: Some(1),
        ..Default::default()
    };
    let ids = semantic_ids(&store, "project-a", "query", &embedder, &filter).await;
    assert_eq!(ids, vec![nearest]);

    let zero = TimelineFilter {
        last_n_sessions: Some(0),
        ..Default::default()
    };
    let hits = store
        .semantic_timeline("project-a", "query", &embedder, &zero)
        .await
        .expect("zero last_n is an empty page");
    assert!(hits.is_empty());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_respects_time_and_agent_filters() {
    let (_dir, store) = fixture("semantic-time-agent");
    let (_a, t_a) = common::insert_row(
        &store,
        "project-a",
        "s-a",
        "agent-a",
        "a",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    std::thread::sleep(Duration::from_millis(3));
    let (b, _t_b) = common::insert_row(
        &store,
        "project-a",
        "s-b",
        "agent-b",
        "b",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    std::thread::sleep(Duration::from_millis(3));
    let (c, _t_c) = common::insert_row(
        &store,
        "project-a",
        "s-c",
        "agent-a",
        "c",
        Vec::new(),
        unit_vector(0),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    // agent-a AND created_at >= t_a ⇒ a and c (newest first by distance then id).
    let filter = TimelineFilter {
        since: Some(t_a),
        agent_id: Some("agent-a".to_string()),
        ..Default::default()
    };
    let ids = semantic_ids(&store, "project-a", "query", &embedder, &filter).await;
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&c));
    assert!(
        !ids.contains(&b),
        "agent-b is excluded by the agent filter: {ids:?}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_respects_touched_files_filter() {
    let (_dir, store) = fixture("semantic-touched-files");
    common::insert_row(
        &store,
        "project-a",
        "s-a",
        "agent-1",
        "a",
        vec!["src/a.rs".to_string()],
        unit_vector(0),
    )
    .await;
    let (b, _t) = common::insert_row(
        &store,
        "project-a",
        "s-b",
        "agent-1",
        "b",
        vec!["src/b.rs".to_string()],
        unit_vector(1),
    )
    .await;

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let filter = TimelineFilter {
        touched_files: Some(vec!["src/b.rs".to_string()]),
        ..Default::default()
    };
    let ids = semantic_ids(&store, "project-a", "query", &embedder, &filter).await;
    assert_eq!(ids, vec![b], "the Rust-side touched_files filter applies");
}

// ---------------------------------------------------------------------------
// Corrupt metadata (fail-soft, F5)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_semantic_timeline_skips_corrupt_metadata_rows() {
    let (_dir, store) = fixture("semantic-corrupt-metadata");
    let (good, _t) = common::insert_row(
        &store,
        "project-a",
        "s-good",
        "agent-1",
        "good",
        Vec::new(),
        unit_vector(0),
    )
    .await;
    let (broken, _t) = common::insert_row(
        &store,
        "project-a",
        "s-broken",
        "agent-1",
        "broken",
        Vec::new(),
        unit_vector(1),
    )
    .await;

    let db = store.ready().await.expect("ready");
    db.execute(
        "UPDATE agent_sessions SET metadata = ? WHERE id = ?",
        vec![
            Value::Text("{not valid json".to_string()),
            Value::Text(broken),
        ],
    )
    .await
    .expect("corrupt the row");

    let embedder = FakeEmbedder::with_vector(unit_vector(0));
    let hits = store
        .semantic_timeline("project-a", "query", &embedder, &TimelineFilter::default())
        .await
        .expect("semantic list path fails soft");
    let ids: Vec<String> = hits.iter().map(|hit| hit.id.clone()).collect();
    assert_eq!(ids, vec![good], "corrupt row skipped, good row served");
}
