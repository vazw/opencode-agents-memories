//! Dims preflight (blueprint W1.3 step 5, sB1): a dims-8 legacy shard must
//! land in `shard_metadata` as dims 8 under the live config (384 here), so
//! `MigrationService::detect_dimension_mismatch` reports the mismatch for
//! the operator instead of silently deleting the rows.

mod common;

use std::sync::Arc;

use common::{FakeReEmbedder, cleanup_temp_dir, real_components, store_config};
use mem_store::db::Value;
use mem_store::legacy_migrator::{MIGRATION_MARKER, run_legacy_turso_migration};
use mem_store::migration_service::{DimensionMismatch, MigrationService, ReEmbedder};
use mem_store::ready::build_ready_gate;
use mem_store::shard_manager::ShardRegistry;

const SCOPE_HASH: &str = "0123456789abcdef";

/// Raw F32 little-endian bytes, the pre-turso `vector` BLOB encoding.
fn f32_blob(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[tokio::test]
async fn test_legacy_import_preserves_source_dims_for_the_mismatch_detector() {
    // Live config demands 384; the legacy rows carry 8 dims.
    let mut base = store_config("dims_preflight");
    base.embedding_dimensions = 384;
    base.embedding_model = "new-model".to_string();
    let (config, connections, manager) = real_components(base).await;
    let storage = config.storage_path.clone();

    // Upstream `createLegacyShard` shape: plain BLOB vectors, no metadata.
    let users = storage.join("users");
    std::fs::create_dir_all(&users).expect("users dir");
    let db_path = users.join(format!("user_{SCOPE_HASH}_shard_0.db"));
    let legacy_schema = r#"CREATE TABLE memories (
        id TEXT PRIMARY KEY,
        content TEXT NOT NULL,
        vector BLOB NOT NULL,
        container_tag TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    )"#;
    {
        let db = connections
            .get_connection(&db_path)
            .await
            .expect("legacy shard open");
        db.execute(legacy_schema, Vec::new()).await.expect("table");
        let blob = Value::Blob(f32_blob(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]));
        for index in 0..2 {
            db.execute(
                "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                vec![
                    Value::Text(format!("mem_{index}")),
                    Value::Text(format!("legacy content {index}")),
                    blob.clone(),
                    Value::Text(SCOPE_HASH.to_string()),
                    Value::Integer(1_700_000_000_000),
                    Value::Integer(1_700_000_000_000),
                ],
            )
            .await
            .expect("insert");
        }
        drop(db);
        connections.close_connection(&db_path).await;
    }

    let shards = Arc::new(manager);
    let sleep = Arc::new(mem_store::platform::system_sleep)
        as Arc<dyn Fn(std::time::Duration) + Send + Sync>;
    run_legacy_turso_migration(&storage, &shards, &connections, &sleep)
        .await
        .expect("legacy migration");
    assert!(storage.join(MIGRATION_MARKER).exists());

    // Ready gate after the run: its engine step finds the fresh marker,
    // warm-up registers the converted shard for the detector sweep.
    let registry: Arc<dyn ShardRegistry> = shards.clone();
    let ready = Arc::new(build_ready_gate(
        &storage,
        Arc::clone(&connections),
        Arc::clone(&shards),
        registry,
    ));
    let reembedder = Arc::new(FakeReEmbedder::new());
    let service = MigrationService::new(
        config.clone(),
        ready,
        Arc::clone(&shards),
        Arc::clone(&connections),
        Arc::clone(&reembedder) as Arc<dyn ReEmbedder>,
    );

    let detection: DimensionMismatch = service
        .detect_dimension_mismatch()
        .await
        .expect("detection");

    // The 8 source dims are what the detector sees — never the 384 config
    // value, and never a silently dropped row.
    assert!(detection.needs_migration);
    assert_eq!(detection.config_dimensions, 384);
    assert_eq!(detection.config_model, "new-model");
    assert_eq!(detection.shard_mismatches.len(), 1);
    let mismatch = &detection.shard_mismatches[0];
    assert_eq!(mismatch.stored_dimensions, 8);
    assert_eq!(mismatch.stored_model, "legacy-unknown");
    assert_eq!(mismatch.vector_count, 2);

    drop(connections);
    cleanup_temp_dir(&storage).ok();
}
