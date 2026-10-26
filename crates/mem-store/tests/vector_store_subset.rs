//! Vector subset battery (blueprint W1.2 step 7): the WRITE + aggregation
//! half of upstream `vector-search.ts` (search/filters complete in W1.3).
//! Schema prepared via `memory_shard_migrations` (upstream's
//! `prepareShardDb` moves to the manager layer here — recorded divergence).

mod common;

use mem_store::db::{BatchMode, Value};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::vector::{
    MemoryInsert, ProjectAssociation, count_all_vectors, count_vectors_by_container_tag,
    get_all_memories, get_distinct_tags, get_project_path_counts, insert_vector,
    insert_vector_statement, resolve_session_id, update_project_association, vector_to_text,
};

use mem_store::config::StoreConfig;

async fn prepared_db(config: &StoreConfig, file: &str, dims: u32) -> mem_store::db::DbHandle {
    let db = common::open_real_db(config, file).await.expect("open");
    apply_schema_migrations(
        db.as_ref(),
        &memory_shard_migrations(dims),
        &MigrationOptions {
            db_path: None,
            label: "vector-battery",
        },
    )
    .await
    .expect("shard schema");
    db
}

fn record(id: &str, created: i64, with_tags: bool, project_path: Option<&str>) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: format!("content-{id}"),
        vector: vec![1.0f32, 0.0],
        tags_vector: if with_tags {
            Some(vec![0.25f32, -0.75])
        } else {
            None
        },
        container_tag: "opencode_project_a1b2c3d4e5f60001".to_string(),
        tags: if with_tags {
            Some("rust,testing".to_string())
        } else {
            None
        },
        memory_type: Some("note".to_string()),
        created_at: created,
        updated_at: created,
        metadata: Some(r#"{"kind":"test"}"#.to_string()),
        session_id: None,
        display_name: Some("Display".to_string()),
        user_name: Some("assoc@example.invalid".to_string()),
        user_email: Some("assoc@example.invalid".to_string()),
        project_path: project_path.map(|p| p.to_string()),
        project_name: Some("AggregateProject".to_string()),
        git_repo_url: None,
    }
}

// ---------------------------------------------------------------------------
// 1/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_insert_with_tags_roundtrips_vector_and_tags_blob() {
    let config = common::store_config("vec-tags");
    let db = prepared_db(&config, "shard.db", 2).await;
    let value = record("mem_tags", 10, true, Some("/proj/a"));
    insert_vector(db.as_ref(), &value).await.expect("insert");

    assert_eq!(count_all_vectors(db.as_ref()).await, 1);
    let rows = db
        .query(
            "SELECT content, vector_extract(vector) AS v, vector_extract(tags_vector) AS tv,\n                    tags, type FROM memories WHERE id = ?",
            vec![Value::Text("mem_tags".to_string())],
        )
        .await
        .expect("probe");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text("v").as_deref(), Some("[1,0]"));
    assert_eq!(rows[0].text("tv").as_deref(), Some("[0.25,-0.75]"));
    assert_eq!(rows[0].text("tags").as_deref(), Some("rust,testing"));
}

// ---------------------------------------------------------------------------
// 2/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_insert_without_tags_stores_null_tags_vector() {
    let config = common::store_config("vec-notags");
    let db = prepared_db(&config, "shard.db", 2).await;
    let value = record("mem_plain", 10, false, None);
    insert_vector(db.as_ref(), &value).await.expect("insert");

    let rows = db
        .query(
            "SELECT tags_vector IS NULL AS null_tags, tags IS NULL AS null_tags_text,
                    session_id FROM memories WHERE id = ?",
            vec![Value::Text("mem_plain".to_string())],
        )
        .await
        .expect("probe");
    assert_eq!(rows[0].i64("null_tags"), Some(1));
    assert_eq!(rows[0].i64("null_tags_text"), Some(1));
}

// ---------------------------------------------------------------------------
// 3/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_insert_rejects_duplicate_primary_key() {
    let config = common::store_config("vec-dup");
    let db = prepared_db(&config, "shard.db", 2).await;
    let value = record("mem_dup", 10, false, None);
    insert_vector(db.as_ref(), &value).await.expect("first");
    let error = insert_vector(db.as_ref(), &value)
        .await
        .expect_err("duplicate PK rejected");
    assert!(
        matches!(error, StoreError::Turso { .. }),
        "turso passthrough: {error}"
    );
    assert!(
        error.is_unique_violation(),
        "unique-violation classification: {error}"
    );
}

// ---------------------------------------------------------------------------
// 4/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_count_all_vectors_and_by_container_tag() {
    let config = common::store_config("vec-count");
    let db = prepared_db(&config, "shard.db", 2).await;
    insert_vector(db.as_ref(), &record("m1", 1, false, None))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("m2", 2, false, None))
        .await
        .unwrap();
    assert_eq!(count_all_vectors(db.as_ref()).await, 2);
    assert_eq!(
        count_vectors_by_container_tag(db.as_ref(), "opencode_project_a1b2c3d4e5f60001").await,
        2
    );
    assert_eq!(
        count_vectors_by_container_tag(db.as_ref(), "opencode_none").await,
        0
    );
    assert_eq!(count_all_vectors(db.as_ref()).await, 2);
}

// ---------------------------------------------------------------------------
// 5/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_all_memories_orders_by_created_at_desc() {
    let config = common::store_config("vec-order");
    let db = prepared_db(&config, "shard.db", 2).await;
    insert_vector(db.as_ref(), &record("old", 10, false, None))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("new", 99, false, None))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("mid", 50, false, None))
        .await
        .unwrap();
    let rows = get_all_memories(db.as_ref())
        .await
        .expect("the fixture rows read");
    let ids: Vec<String> = rows.iter().filter_map(|row| row.text("id")).collect();
    assert_eq!(ids, vec!["new", "mid", "old"]);
}

#[tokio::test]
async fn test_get_all_memories_reports_a_query_failure() {
    let config = common::store_config("vec-unreadable");
    let db = prepared_db(&config, "shard.db", 2).await;
    db.execute("DROP TABLE memories", Vec::new())
        .await
        .expect("the table drops");
    assert!(
        get_all_memories(db.as_ref()).await.is_err(),
        "a failed SELECT * is a returned error, never an empty list"
    );
}

// ---------------------------------------------------------------------------
// 6/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_distinct_tags_deduplicates_project_associations() {
    let config = common::store_config("vec-distinct");
    let db = prepared_db(&config, "shard.db", 2).await;
    // Two rows sharing the whole association tuple collapse to one row.
    insert_vector(db.as_ref(), &record("a1", 1, true, Some("/proj/a")))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("a2", 2, true, Some("/proj/a")))
        .await
        .unwrap();
    // Distinct container/association tuple.
    insert_vector(db.as_ref(), &record("b1", 3, true, Some("/proj/b")))
        .await
        .unwrap();
    let rows = get_distinct_tags(db.as_ref()).await;
    let tags: Vec<String> = rows
        .iter()
        .filter_map(|row| row.text("container_tag"))
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "a1+a2 collapse (3 inserts, 2 tuples): {tags:?}"
    );
    let paths: Vec<Option<String>> = rows.iter().map(|row| row.text("project_path")).collect();
    assert!(paths.contains(&Some("/proj/a".to_string())));
    assert!(paths.contains(&Some("/proj/b".to_string())));
}

// ---------------------------------------------------------------------------
// 7/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_project_path_counts_groups_and_orders() {
    let config = common::store_config("vec-counts");
    let db = prepared_db(&config, "shard.db", 2).await;
    insert_vector(db.as_ref(), &record("a2", 20, true, Some("/proj/a")))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("a1", 10, true, Some("/proj/a")))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("b1", 30, true, Some("/proj/b")))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("none", 40, true, None))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("empty", 50, true, Some("")))
        .await
        .unwrap();

    let counts = get_project_path_counts(db.as_ref()).await;
    assert_eq!(counts.len(), 2, "NULL/empty project paths excluded");
    assert_eq!(counts[0].project_path, "/proj/a");
    assert_eq!(counts[0].count, 2);
    assert_eq!(
        counts[0].container_tag.as_deref(),
        Some("opencode_project_a1b2c3d4e5f60001")
    );
    assert_eq!(counts[1].project_path, "/proj/b");
    assert_eq!(counts[1].count, 1);
}

// ---------------------------------------------------------------------------
// 8/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_update_project_association_remaps_matching_rows_only() {
    let config = common::store_config("vec-remap");
    let db = prepared_db(&config, "shard.db", 2).await;
    insert_vector(db.as_ref(), &record("m1", 1, true, Some("/old")))
        .await
        .unwrap();
    insert_vector(db.as_ref(), &record("m2", 2, true, Some("/old")))
        .await
        .unwrap();
    // Same project, DIFFERENT container tag: the UPDATE matches on
    // container_tag only, so this row must stay untouched.
    let mut m3 = record("m3", 3, true, Some("/old"));
    m3.container_tag = "opencode_project_fffffffffffff001".to_string();
    insert_vector(db.as_ref(), &m3).await.unwrap();

    let changes = update_project_association(
        db.as_ref(),
        "opencode_project_a1b2c3d4e5f60001",
        &ProjectAssociation {
            container_tag: "opencode_project_fffffffffffff002".to_string(),
            project_path: Some("/new".to_string()),
            project_name: Some("Renamed".to_string()),
            display_name: Some("New Display".to_string()),
            git_repo_url: None,
        },
    )
    .await
    .expect("remap");
    assert_eq!(changes, 2, "two matching rows migrated");

    let rows = db
        .query(
            "SELECT container_tag FROM memories WHERE project_path = ?",
            vec![Value::Text("/new".to_string())],
        )
        .await
        .expect("probe");
    assert_eq!(rows.len(), 2, "only matching rows moved");
    let untouched = db
        .query(
            "SELECT id, container_tag FROM memories WHERE project_path = ?",
            vec![Value::Text("/old".to_string())],
        )
        .await
        .expect("probe");
    assert_eq!(untouched.len(), 1, "different container stays untouched");
    assert_eq!(
        untouched[0].text("container_tag").as_deref(),
        Some("opencode_project_fffffffffffff001")
    );
}

// ---------------------------------------------------------------------------
// helpers under test (pure functions)
// ---------------------------------------------------------------------------

#[test]
fn test_vector_to_text_serializes_and_maps_nonfinite_to_null() {
    assert_eq!(
        vector_to_text(&[1.0f32, 0.0]).expect("vector serializes"),
        "[1.0,0.0]"
    );
    let nonfinite = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.5f32];
    assert_eq!(
        vector_to_text(&nonfinite).expect("vector serializes"),
        "[null,null,null,1.5]"
    );
}

#[test]
fn test_resolve_session_id_explicit_wins_then_metadata() {
    assert_eq!(
        resolve_session_id(
            Some("s-explicit".to_string()),
            Some(r#"{"sessionID":"s-meta"}"#)
        ),
        Some("s-explicit".to_string())
    );
    assert_eq!(
        resolve_session_id(None, Some(r#"{"sessionID":"s-meta"}"#)),
        Some("s-meta".to_string())
    );
    assert_eq!(
        resolve_session_id(Some(String::new()), Some(r#"{"sessionID":"s-meta"}"#)),
        Some("s-meta".to_string()),
        "empty explicit loses to metadata"
    );
    assert_eq!(resolve_session_id(None, Some(r#"{"other":1}"#)), None);
    assert_eq!(resolve_session_id(None, Some("not-json")), None);
    assert_eq!(resolve_session_id(None, None), None);
    let _: Option<mem_store::db::Row> = None;
    let _ = std::sync::Arc::new(0u8);
}

// ---------------------------------------------------------------------------
// statement-form helper (blueprint W1.3 step 2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_insert_vector_statement_batch_matches_single_insert() {
    let config = common::store_config("vec-stmt-batch");
    let db = prepared_db(&config, "shard.db", 2).await;
    let value = record("mem_stmt_batch", 10, true, Some("/proj/a"));

    // Commit the statement through one write batch (the seam the migrators
    // and re-embed staging use).
    db.batch(
        vec![insert_vector_statement(&value).expect("statement builds")],
        BatchMode::Write,
    )
    .await
    .expect("statement batch");

    // A reference row inserted through `insert_vector` (single execute).
    let mut reference = record("mem_stmt_single", 10, true, Some("/proj/a"));
    reference.id = "mem_stmt_single".to_string();
    insert_vector(db.as_ref(), &reference)
        .await
        .expect("single insert");

    let rows = db
        .query(
            "SELECT id, content, vector_extract(vector) AS v, vector_extract(tags_vector) AS tv,\
                    container_tag, tags, type, created_at, updated_at, metadata, display_name, \
                    user_name, user_email, project_path, project_name, git_repo_url \
             FROM memories ORDER BY id",
            vec![],
        )
        .await
        .expect("probe");
    assert_eq!(rows.len(), 2);
    let batched = &rows[0];
    let single = &rows[1];
    for column in [
        "v",
        "tv",
        "container_tag",
        "tags",
        "type",
        "metadata",
        "display_name",
        "user_name",
        "user_email",
        "project_path",
        "project_name",
        "git_repo_url",
    ] {
        assert_eq!(
            batched.text(column).as_deref(),
            single.text(column).as_deref(),
            "batch statement row matches single insert on {column}"
        );
    }
    assert_eq!(batched.i64("created_at"), single.i64("created_at"));
    assert_eq!(batched.i64("updated_at"), single.i64("updated_at"));
}
