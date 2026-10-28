//! Schema-migrations mirror (blueprint W1.3 step 6): ports the upstream
//! `tests/turso-schema-migrations.test.ts` idempotency + session-index v2
//! cases onto the W1.2 runner (`schema_migrations.rs`).

mod common;

use mem_store::db::{Statement, Value};
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, ensure_user_prompt_columns, user_prompts_migrations,
};
use mem_store::sql;

use common::{cleanup_temp_dir, open_real_db, store_config};

/// `sqlite_master` index names for `table` (sorted for stable asserts).
async fn index_names(db: &dyn mem_store::db::Db, table: &str) -> Vec<String> {
    let sql =
        format!("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = '{table}'");
    let rows = db
        .query(sql.as_str(), Vec::new())
        .await
        .expect("index_list");
    let mut names: Vec<String> = rows.iter().filter_map(|row| row.text("name")).collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// 1/2 — upstream `applies user prompts migrations idempotently`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_applies_user_version_migrations_idempotently() {
    let config = store_config("schema-user-prompts");
    let db = open_real_db(&config, "user-prompts.db")
        .await
        .expect("open");
    let db_path = config.storage_path.join("user-prompts.db");
    let options = MigrationOptions {
        db_path: Some(db_path.clone()),
        label: "user-prompts",
    };
    let migrations = user_prompts_migrations();
    let proof = common::exclusive_root(&config.storage_path);

    let first = apply_schema_migrations(&*db, &migrations, &options, Some(&proof))
        .await
        .expect("first apply");
    ensure_user_prompt_columns(&*db).await.expect("columns");
    let second = apply_schema_migrations(&*db, &migrations, &options, Some(&proof))
        .await
        .expect("second apply");

    assert_eq!(first, 1, "first apply reaches v1");
    assert_eq!(second, 1, "second apply is a no-op at v1");
    let row = mem_store::db::query_one(&*db, sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("pragma")
        .expect("row");
    assert_eq!(row.i64("user_version").expect("version"), 1);

    // Extra columns landed and the table is writable with them.
    db.execute(
        "INSERT INTO user_prompts (id, session_id, message_id, content, created_at, captured, \
         capture_attempts, provider_id, model_id) \
         VALUES ('p1', 's1', 'm1', 'hi', 1, 0, 0, 'prov', 'mod')",
        Vec::new(),
    )
    .await
    .expect("insert with extra columns");

    drop(db);
    // Multiprocess-WAL sidecars outlive the handle; recursive cleanup of
    // the whole storage root removes main + sidecars together.
    cleanup_temp_dir(&config.storage_path).ok();
}

// ---------------------------------------------------------------------------
// 2/2 — upstream `migrates memory shards to session_id index (v2)`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_migrates_memory_shards_to_session_id_index_v2() {
    let config = store_config("schema-shard-v2");
    let db = open_real_db(&config, "shard_v2.db").await.expect("open");
    let db_path = config.storage_path.join("shard_v2.db");
    let options = MigrationOptions {
        db_path: Some(db_path.clone()),
        label: "shard",
    };

    let all = mem_store::schema_migrations::memory_shard_migrations(2);
    let proof = common::exclusive_root(&config.storage_path);
    let v1: Vec<_> = all
        .iter()
        .filter(|migration| migration.version == 1)
        .cloned()
        .collect();
    let applied_v1 = apply_schema_migrations(&*db, &v1, &options, Some(&proof))
        .await
        .expect("apply v1");
    assert_eq!(applied_v1, 1);

    // A v1-era row: session lives only inside the metadata JSON.
    let vector = vec![1.0f32, 2.0];
    db.execute(
        "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at, metadata)
         VALUES ('m1', 'c1', ?1, 'tag', 10, 10, '{\"sessionID\":\"sess_from_meta\"}')",
        vec![Value::Text(
            mem_store::vector::vector_to_text(&vector).expect("vector serializes"),
        )],
    )
    .await
    .expect("insert v1 row");

    let applied_v2 = apply_schema_migrations(&*db, &all, &options, Some(&proof))
        .await
        .expect("apply v1+v2");
    assert_eq!(applied_v2, 2);
    let row = mem_store::db::query_one(&*db, sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("pragma")
        .expect("row");
    assert_eq!(row.i64("user_version").expect("version"), 2);

    // Backfill copied metadata.sessionID into the new column.
    let row = mem_store::db::query_one(
        &*db,
        "SELECT session_id FROM memories WHERE id = 'm1'",
        Vec::new(),
    )
    .await
    .expect("select")
    .expect("row");
    assert_eq!(row.text("session_id").as_deref(), Some("sess_from_meta"));

    // DEVIATION from the blueprint text (`memories_session_idx`): the
    // frozen `sql.rs` v2 statement names the index `idx_session_id`.
    let names = index_names(&*db, "memories").await;
    assert!(
        names.iter().any(|name| name == "idx_session_id"),
        "idx_session_id present in {names:?}"
    );

    // A v2-era insert can write the column directly.
    let statement = Statement::new(
        "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at, session_id)
         VALUES ('m2', 'c2', ?1, 'tag', 20, 20, 'sess_direct')",
        vec![Value::Text(
            mem_store::vector::vector_to_text(&vector).expect("vector serializes"),
        )],
    );
    db.execute(&statement.sql, statement.params)
        .await
        .expect("v2 insert");

    drop(db);
    cleanup_temp_dir(&config.storage_path).ok();
}
