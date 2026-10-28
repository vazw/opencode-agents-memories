//! Exact-cosine fallback battery (blueprint W1.3 step 3): port of upstream
//! `tests/turso-exact-fallback.test.ts` — search works on a shard with NO
//! vector (DiskANN) indexes at all, plain `vector_distance_cos` scans only.
//! Schema prepared via `memory_shard_migrations` (manager-owned
//! `prepareShardDb` — recorded divergence).

mod common;

use mem_store::vector::{MemoryInsert, insert_vector, search_in_shard};

const NOW_MS: i64 = 1_700_000_000_000;

#[tokio::test]
async fn test_finds_memories_via_exact_cosine_scan_without_vector_indexes() {
    let config = common::test_store_config("exact-scan-fallback", 8, 1000);
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open");
    mem_store::schema_migrations::apply_schema_migrations(
        db.as_ref(),
        &mem_store::schema_migrations::memory_shard_migrations(8),
        &mem_store::schema_migrations::MigrationOptions {
            db_path: None,
            label: "exact-fallback",
            max_known: mem_store::schema_migrations::MEMORY_SHARD_MAX_KNOWN,
        },
        None,
    )
    .await
    .expect("shard schema");

    let ann = db
        .query(
            "SELECT name FROM sqlite_schema WHERE type = 'index' AND name IN ('memories_vec_idx', 'memories_tags_vec_idx')",
            vec![],
        )
        .await
        .expect("ann probe");
    let names: Vec<String> = ann.iter().filter_map(|row| row.text("name")).collect();
    assert!(
        names.is_empty(),
        "expected zero DiskANN indexes, found {names:?}"
    );

    let container_tag = "opencode_project_e1e2e3e4e5f67890";
    let vector = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let row = MemoryInsert {
        id: "mem_exact_1".to_string(),
        content: "exact cosine memory".to_string(),
        vector: vector.clone(),
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
    };
    insert_vector(db.as_ref(), &row).await.expect("insert");

    let results = search_in_shard(db.as_ref(), &vector, container_tag, 5, None)
        .await
        .expect("exact-scan search");
    assert!(!results.is_empty(), "exact scan finds the memory");
    assert_eq!(results[0].id, "mem_exact_1");
    // No query text: 0.6*1 + 0.4*0 (tags_vector NULL).
    assert!(
        results[0].similarity > 0.55,
        "similarity {} not > 0.55",
        results[0].similarity
    );
}
