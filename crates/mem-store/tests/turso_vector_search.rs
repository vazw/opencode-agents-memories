//! Exact-scan vector search battery (blueprint W1.3 step 3): port of
//! upstream `tests/turso-vector-search.test.ts` — hybrid ranking over exact
//! cosine scans (NO ANN indexes), session lookup, and the zero-limit edge.
//! Schema prepared via `memory_shard_migrations` (upstream's
//! `prepareShardDb` is manager-owned since W1.2 — recorded divergence, same
//! seam as `vector_store_subset.rs`).

mod common;

use mem_store::db::{BatchMode, Value};
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::vector::{
    MemoryInsert, VectorColumn, exact_scan_kind, get_memories_by_session_id, insert_vector,
    insert_vector_statement, search_in_shard, search_memories_by_content,
};

const NOW_MS: i64 = 1_700_000_000_000;

async fn prepared_db(
    config: &mem_store::config::StoreConfig,
    file: &str,
    dims: u32,
) -> mem_store::db::DbHandle {
    let db = common::open_real_db(config, file).await.expect("open");
    apply_schema_migrations(
        db.as_ref(),
        &memory_shard_migrations(dims),
        &MigrationOptions {
            db_path: None,
            label: "search-battery",
            max_known: mem_store::schema_migrations::MEMORY_SHARD_MAX_KNOWN,
        },
        None,
    )
    .await
    .expect("shard schema");
    db
}

fn record(id: &str, content: &str, vector: &[f32], container_tag: &str) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: content.to_string(),
        vector: vector.to_vec(),
        tags_vector: None,
        container_tag: container_tag.to_string(),
        tags: None,
        memory_type: None,
        created_at: NOW_MS,
        updated_at: NOW_MS,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
    }
}

async fn assert_no_ann_indexes(db: &mem_store::db::DbHandle) {
    let rows = db
        .query(
            "SELECT name FROM sqlite_schema WHERE type = 'index' AND name IN ('memories_vec_idx', 'memories_tags_vec_idx')",
            vec![],
        )
        .await
        .expect("ann probe");
    let names: Vec<String> = rows.iter().filter_map(|row| row.text("name")).collect();
    assert!(
        names.is_empty(),
        "expected zero DiskANN indexes, found {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 1/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_inserts_and_searches_memories_with_exact_cosine_distance() {
    let config = common::store_config("vec-search-basic");
    let db = prepared_db(&config, "shard.db", 2).await;
    let container_tag = "opencode_project_a1b2c3d4e5f67890";

    assert_no_ann_indexes(&db).await;

    let vector = vec![1.0f32, 0.0];
    let tags_vector = vec![0.0f32, 1.0];

    let mut tagged = record(
        "mem_test_1",
        "Turso native vector search",
        &vector,
        container_tag,
    );
    tagged.tags_vector = Some(tags_vector.clone());
    tagged.tags = Some("turso,vector".to_string());
    insert_vector(db.as_ref(), &tagged)
        .await
        .expect("insert tagged");

    // Raw `vector_distance_cos` probe rows (upstream content/tags hits).
    let content_hit = db
        .query(
            "SELECT m.id AS id FROM memories m WHERE m.vector IS NOT NULL ORDER BY vector_distance_cos(m.vector, vector32(?)) ASC LIMIT 1",
            vec![Value::Text("[1,0]".to_string())],
        )
        .await
        .expect("content probe");
    let tags_hit = db
        .query(
            "SELECT m.id AS id FROM memories m WHERE m.tags_vector IS NOT NULL ORDER BY vector_distance_cos(m.tags_vector, vector32(?)) ASC LIMIT 1",
            vec![Value::Text("[0,1]".to_string())],
        )
        .await
        .expect("tags probe");
    assert_eq!(content_hit[0].text("id").as_deref(), Some("mem_test_1"));
    assert_eq!(tags_hit[0].text("id").as_deref(), Some("mem_test_1"));

    let plain = record(
        "mem_test_no_tags",
        "Content only vector",
        &vector,
        container_tag,
    );
    insert_vector(db.as_ref(), &plain)
        .await
        .expect("insert plain");

    let results = search_in_shard(db.as_ref(), &vector, container_tag, 5, Some("turso"))
        .await
        .expect("search");
    assert!(!results.is_empty(), "expected search hits");
    assert_eq!(results[0].id, "mem_test_1");
    assert!(
        results[0].similarity > 0.5,
        "similarity {} not > 0.5",
        results[0].similarity
    );
    assert_eq!(
        results[0].tags,
        vec!["turso".to_string(), "vector".to_string()]
    );
    assert_eq!(results[0].created_at, NOW_MS);
    assert_eq!(results[0].container_tag, container_tag);

    let limited = search_in_shard(db.as_ref(), &vector, container_tag, 1, Some("turso"))
        .await
        .expect("search limited");
    assert_eq!(limited.len(), 1, "limit respected");
}

// ---------------------------------------------------------------------------
// 2/5 — statement-form battery over the recording fake seam
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_uses_exact_cosine_scans_for_filtered_and_unfiltered_queries() {
    let core = std::sync::Arc::<common::FakeCore>::default();
    let fake = common::FakeDb::attach(
        core.clone(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    exact_scan_kind(&fake, "[1,0]", 10, "", VectorColumn::Content)
        .await
        .expect("unfiltered scan");
    exact_scan_kind(
        &fake,
        "[1,0]",
        10,
        "opencode_project_test",
        VectorColumn::Content,
    )
    .await
    .expect("filtered scan");

    let sqls = core.sqls.lock().unwrap();
    assert_eq!(sqls.len(), 2, "two observed statements");
    for sql in sqls.iter() {
        assert!(sql.contains("vector_distance_cos"), "exact scan SQL: {sql}");
        assert!(sql.contains("FROM memories m"), "aliased table: {sql}");
        assert!(!sql.contains("vector_top_k"), "never ANN search: {sql}");
    }
    assert!(
        sqls[1].contains("m.container_tag = ?"),
        "filtered variant: {}",
        sqls[1]
    );
    assert!(
        !sqls[0].contains("container_tag"),
        "unfiltered variant: {}",
        sqls[0]
    );
}

// ---------------------------------------------------------------------------
// 3/5 — 200 rows across two container tags (upstream rank case)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ranks_exact_cosine_hits_and_filters_other_container_tags() {
    let config = common::store_config("vec-search-rank");
    let db = prepared_db(&config, "shard.db", 2).await;
    let target_tag = "opencode_project_a1b2c3d4e5f67890";
    let other_tag = "opencode_project_0000000000000000";

    // mem_rank_0 is the exact query match; every other row is orthogonal.
    let mut statements = Vec::new();
    for i in 0..200usize {
        let vector = if i == 0 {
            vec![1.0f32, 0.0]
        } else {
            vec![0.0f32, 1.0]
        };
        let mut row = record(
            &format!("mem_rank_{i}"),
            &format!("Memory {i}"),
            &vector,
            if i < 150 { target_tag } else { other_tag },
        );
        row.created_at = NOW_MS - (200 - i as i64);
        row.updated_at = NOW_MS;
        row.tags = Some(String::new());
        statements.push(insert_vector_statement(&row).expect("statement builds"));
    }
    db.batch(statements, BatchMode::Write)
        .await
        .expect("seed batch");

    let query_vector = vec![1.0f32, 0.0];
    // No query text: ranking is pure vector similarity (classic 0.6/0.4).
    let results = search_in_shard(db.as_ref(), &query_vector, target_tag, 10, None)
        .await
        .expect("search");
    assert!(!results.is_empty());
    assert_eq!(results[0].id, "mem_rank_0");
    // tags_vector is NULL → 0.6*1 + 0.4*0 for the exact hit.
    assert!(
        results[0].similarity > 0.55,
        "similarity {} not > 0.55",
        results[0].similarity
    );
    for hit in &results {
        let idx: usize = hit.id.trim_start_matches("mem_rank_").parse().expect("idx");
        assert!(idx < 150, "other container tag leaked: {}", hit.id);
        assert!(
            hit.tags.is_empty(),
            "empty tags string → []: {:?}",
            hit.tags
        );
    }
    assert!(results.len() <= 10, "limit respected: {}", results.len());
    for pair in results.windows(2) {
        assert!(
            pair[0].similarity >= pair[1].similarity,
            "sorted desc: {} < {}",
            pair[0].similarity,
            pair[1].similarity
        );
    }
}

// ---------------------------------------------------------------------------
// 4/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_boosts_keyword_matches_in_hybrid_ranking_and_indexes_session_id() {
    let config = common::test_store_config("vec-search-hybrid", 8, 1000);
    let db = prepared_db(&config, "shard.db", 8).await;
    let container_tag = "opencode_project_b2c3d4e5f6789012";

    let query_vector = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let orthogonal = vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let mut keyword_row = record(
        "mem_kw",
        "unique-keyword-alpha appears here",
        &orthogonal,
        container_tag,
    );
    keyword_row.session_id = Some("sess_hybrid_1".to_string());
    keyword_row.metadata = Some(r#"{"sessionID":"sess_hybrid_1"}"#.to_string());
    insert_vector(db.as_ref(), &keyword_row)
        .await
        .expect("insert kw");

    let vector_row = record("mem_vec", "unrelated text", &query_vector, container_tag);
    insert_vector(db.as_ref(), &vector_row)
        .await
        .expect("insert vec");

    // Orthogonal filler with no keyword overlap — the candidate the keyword
    // boost must outrank.
    let filler = record(
        "mem_orth",
        "filler text nothing relevant",
        &orthogonal,
        container_tag,
    );
    insert_vector(db.as_ref(), &filler)
        .await
        .expect("insert filler");

    let by_session = get_memories_by_session_id(db.as_ref(), "sess_hybrid_1")
        .await
        .expect("session lookup");
    let ids: Vec<String> = by_session.iter().filter_map(|row| row.text("id")).collect();
    assert!(ids.contains(&"mem_kw".to_string()), "session rows: {ids:?}");
    assert_eq!(ids.len(), 1, "only the session row: {ids:?}");

    let session_probe = db
        .query(
            "SELECT session_id FROM memories WHERE id = ?",
            vec![Value::Text("mem_kw".to_string())],
        )
        .await
        .expect("session probe");
    assert_eq!(
        session_probe[0].text("session_id").as_deref(),
        Some("sess_hybrid_1"),
        "session_id column indexed on insert"
    );

    let hybrid = search_in_shard(
        db.as_ref(),
        &query_vector,
        container_tag,
        5,
        Some("unique-keyword-alpha"),
    )
    .await
    .expect("hybrid search");
    let keyword_hit = hybrid
        .iter()
        .find(|hit| hit.id == "mem_kw")
        .expect("keyword candidate present");
    let vector_hit = hybrid
        .iter()
        .find(|hit| hit.id == "mem_vec")
        .expect("vector candidate present");
    let filler_hit = hybrid
        .iter()
        .find(|hit| hit.id == "mem_orth")
        .expect("filler candidate present");
    // Keyword hybrid weight keeps the text match competitive.
    assert!(
        keyword_hit.similarity > 0.1,
        "keyword similarity {} not > 0.1",
        keyword_hit.similarity
    );
    // The keyword candidate outranks the unrelated orthogonal vector, and the
    // exact-vector candidate stays on top.
    assert!(
        keyword_hit.similarity > filler_hit.similarity,
        "keyword {} must outrank orthogonal {}",
        keyword_hit.similarity,
        filler_hit.similarity
    );
    assert!(
        vector_hit.similarity > keyword_hit.similarity,
        "exact-vector {} must outrank keyword {}",
        vector_hit.similarity,
        keyword_hit.similarity
    );
}

// ---------------------------------------------------------------------------
// 5/5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_returns_empty_for_zero_limit() {
    let config = common::store_config("vec-search-zero");
    let db = prepared_db(&config, "shard.db", 2).await;
    let container_tag = "opencode_project_a1b2c3d4e5f67890";
    let vector = vec![1.0f32, 0.0];
    insert_vector(
        db.as_ref(),
        &record("mem_zero", "zero limit memory", &vector, container_tag),
    )
    .await
    .expect("insert");

    let results = search_in_shard(db.as_ref(), &vector, container_tag, 0, Some("zero"))
        .await
        .expect("zero-limit search");
    assert!(results.is_empty(), "limit 0 yields no results");

    // Empty shard: no candidates at all.
    let empty = prepared_db(&config, "empty.db", 2).await;
    let empty_results = search_in_shard(empty.as_ref(), &vector, container_tag, 5, Some("zero"))
        .await
        .expect("empty search");
    assert!(empty_results.is_empty(), "no candidates → empty results");
}

// ---------------------------------------------------------------------------
// 6/8 — issue 50 (O9): the content-only read ignores the tags vector
// ---------------------------------------------------------------------------

/// Two rows whose content vectors are orthogonal to the query; the second one
/// carries the query direction in its TAGS vector. A content-only read must
/// score A at `1.0` and D at `0.0`; any hybrid blend would score them `0.6` /
/// `0.4` (or `0.5` / `0.3` with keyword text), and upstream's fabricated
/// `1.0` would put D at the top.
#[tokio::test]
async fn test_content_read_ignores_the_tags_vector() {
    let config = common::store_config("vec-content-read");
    let db = prepared_db(&config, "shard.db", 2).await;
    let container_tag = "opencode_project_a1b2c3d4e5f67890";

    insert_vector(
        db.as_ref(),
        &record("mem_A", "row A", &[1.0, 0.0], container_tag),
    )
    .await
    .expect("insert A");
    let mut d = record("mem_D", "row D", &[0.0, 1.0], container_tag);
    d.tags_vector = Some(vec![1.0, 0.0]);
    insert_vector(db.as_ref(), &d).await.expect("insert D");

    let results = search_memories_by_content(db.as_ref(), &[1.0, 0.0], container_tag, 10)
        .await
        .expect("content read");
    let a = results
        .iter()
        .find(|row| row.id == "mem_A")
        .expect("A present");
    let d = results
        .iter()
        .find(|row| row.id == "mem_D")
        .expect("D present");
    // MEASURED deviation from the blueprint's exact equality: turso's
    // `vector_distance_cos` returns 4.47e-8 of cosine error for the unit
    // vectors, so 1.0 arrives as 0.9999999552983359. A 1e-6 tolerance keeps
    // the oracle discriminating: the hybrid would score 0.6 / 0.4 (far),
    // and upstream's fabricated 1.0 would put D at the top.
    assert!(
        (a.similarity - 1.0).abs() < 1e-6,
        "the content direction scores 1.0 (measured {})",
        a.similarity
    );
    assert!(
        d.similarity.abs() < 1e-6,
        "the tags vector is ignored: an orthogonal content vector scores 0.0 (measured {})",
        d.similarity
    );
}

// ---------------------------------------------------------------------------
// 7/8 — issue-50 review: the comparator's three keys, each in conflict
// ---------------------------------------------------------------------------

#[test]
fn test_scored_order_is_similarity_then_recency_then_id() {
    use mem_store::vector::{ScoredMemory, scored_order};

    fn row(id: &str, similarity: f64, created_at: i64) -> ScoredMemory {
        ScoredMemory {
            id: id.to_string(),
            content: String::new(),
            similarity,
            created_at,
            metadata_text: None,
        }
    }
    fn ids(rows: &[ScoredMemory]) -> Vec<&str> {
        rows.iter().map(|row| row.id.as_str()).collect()
    }

    let mut rows = vec![
        row("mem_z", 0.9, 3_000),
        row("mem_a", 0.9, 2_000),
        row("mem_m", 1.0, 1_000),
    ];
    rows.sort_by(scored_order);
    assert_eq!(
        ids(&rows),
        vec!["mem_m", "mem_z", "mem_a"],
        "similarity first"
    );

    let mut rows = vec![row("mem_z", 0.9, 2_500), row("mem_a", 0.9, 500)];
    rows.sort_by(scored_order);
    assert_eq!(
        ids(&rows),
        vec!["mem_z", "mem_a"],
        "then recency descending"
    );

    let mut rows = vec![row("mem_z", 0.9, 1_000), row("mem_a", 0.9, 1_000)];
    rows.sort_by(scored_order);
    assert_eq!(ids(&rows), vec!["mem_a", "mem_z"], "then id ascending");
}

// ---------------------------------------------------------------------------
// 8/8 — issue-50 review: the content read honours the hydrate's 1000-id bound
// ---------------------------------------------------------------------------

/// 1001 rows in one tag with `limit = 600` → k = 1200 candidates. Before the
/// clamp the read bound 1004 params against 1003 placeholders and failed with
/// `Turso { message: "bind index 1004 is out of bounds", is_constraint: false }`;
/// the clamp keeps the nearest 1000 and the read returns the 600 requested rows.
#[tokio::test]
async fn test_content_read_clamps_the_hydrate_at_a_thousand_ids() {
    let config = common::store_config("vec-content-read-1000-bound");
    let db = prepared_db(&config, "shard.db", 2).await;
    let container_tag = "opencode_project_a1b2c3d4e5f67890";

    for index in 0..1001u32 {
        insert_vector(
            db.as_ref(),
            &record(
                &format!("mem_{index:04}"),
                &format!("row {index:04}"),
                &[1.0, 0.0],
                container_tag,
            ),
        )
        .await
        .expect("insert row");
    }

    let results = search_memories_by_content(db.as_ref(), &[1.0, 0.0], container_tag, 600)
        .await
        .expect("1001 candidates must not overflow the hydrate bind list");
    assert_eq!(results.len(), 600, "600 requested rows hydrate");
    assert!(
        results.windows(2).all(|pair| pair[0].id < pair[1].id),
        "equal-similarity rows tie-break by id ascending"
    );
}
