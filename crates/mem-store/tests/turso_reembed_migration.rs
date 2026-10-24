//! Re-embed migration battery (blueprint W1.3 step 7, sB2): mirrors
//! upstream `tests/turso-reembed-migration.test.ts` — staging swap safety,
//! untouched-source-on-embedding-failure, and fresh-start archiving.
//!
//! Case 4 ("recovers a crash between source backup and replacement rename")
//! calls sB1's `legacy_migrator::run_legacy_turso_migration`; it lands in a
//! second append per the cross-lane protocol (the file must keep compiling
//! while that sibling step is in flight).

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{FakeReEmbedder, cleanup_temp_dir, real_components, store_config};
use mem_store::connection::ConnectionManager;
use mem_store::db::Value;
use mem_store::legacy_migrator;
use mem_store::migration_service::{MigrationService, MigrationStrategy, ReEmbedder};
use mem_store::ready::build_ready_gate;
use mem_store::scope::Scope;
use mem_store::shard_manager::{ShardInfo, ShardManager, ShardRegistry};
use mem_store::sql;
use mem_store::sqlite_files::copy_sqlite_database;
use mem_store::vector::{MemoryInsert, insert_vector_statement};

const SCOPE_HASH: &str = "0123456789abcdef";

struct Fixture {
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    service: MigrationService,
    #[allow(dead_code)]
    reembedder: Arc<FakeReEmbedder>,
    storage: PathBuf,
}

/// Upstream `createSourceShard`: a dims-2 project shard with `mem_one`
/// (pinned) + `mem_two`, then a `MigrationService` whose config demands
/// dims 4 (the mismatch the migration must resolve).
async fn fixture(prefix: &str) -> (Fixture, ShardInfo) {
    let base = store_config(prefix); // dims = 2, model "test-model"
    let (config, connections, manager) = real_components(base).await;
    let shards = Arc::new(manager);
    let registry: Arc<dyn ShardRegistry> = shards.clone();
    let ready = Arc::new(build_ready_gate(
        &config.storage_path,
        Arc::clone(&connections),
        Arc::clone(&shards),
        registry,
    ));
    ready.ensure_ready().await.expect("ready gate closes");

    let shard = shards
        .create_shard(Scope::Project, SCOPE_HASH, 0)
        .await
        .expect("create shard");
    let db = connections
        .get_connection(&shard.db_path)
        .await
        .expect("source connection");
    for (index, id) in ["mem_one", "mem_two"].iter().enumerate() {
        let record = MemoryInsert {
            id: (*id).to_string(),
            content: format!("content {index}"),
            vector: vec![1.0, index as f32],
            tags_vector: Some(vec![0.0, 1.0]),
            container_tag: format!("opencode_project_{SCOPE_HASH}"),
            tags: Some("alpha,beta".to_string()),
            memory_type: Some("project".to_string()),
            created_at: 100 + index as i64,
            updated_at: 200 + index as i64,
            metadata: Some(format!(r#"{{"source":"test","index":{index}}}"#)),
            session_id: None,
            display_name: Some(format!("Memory {index}")),
            user_name: None,
            user_email: None,
            project_path: Some(format!("/project/{index}")),
            project_name: None,
            git_repo_url: None,
        };
        let statement = insert_vector_statement(&record).expect("statement builds");
        db.execute(&statement.sql, statement.params)
            .await
            .expect("insert memory");
        shards
            .increment_vector_count(shard.id)
            .await
            .expect("vector count");
    }
    db.execute(sql::PIN_MEMORY, vec![Value::Text("mem_one".to_string())])
        .await
        .expect("pin mem_one");
    drop(db);
    connections
        .close_connection(&shard.db_path)
        .await
        .expect("connection closes");

    // Upstream flips CONFIG.embeddingDimensions = 4 before migrating.
    let mut service_config = config.clone();
    service_config.embedding_dimensions = 4;
    service_config.embedding_model = "text-embedding-mock".to_string();
    let reembedder = Arc::new(FakeReEmbedder::new()); // deterministic [1,0,0,0]
    let service = MigrationService::new(
        service_config,
        ready,
        Arc::clone(&shards),
        Arc::clone(&connections),
        Arc::clone(&reembedder) as Arc<dyn ReEmbedder>,
    );
    let storage = config.storage_path.clone();
    (
        Fixture {
            connections,
            shards,
            service,
            reembedder,
            storage,
        },
        shard,
    )
}

fn list_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read projects dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn test_stages_a_complete_replacement_and_preserves_metadata_tags_and_pins() {
    let (fixture, shard) = fixture("reembed-swap").await;

    let result = fixture
        .service
        .migrate_to_new_model(MigrationStrategy::ReEmbed)
        .await
        .expect("migration call");

    assert!(result.success, "migration error: {:?}", result.error);
    assert_eq!(result.deleted_shards, 0);
    assert_eq!(result.re_embedded_memories, 2);
    let projects = fixture.storage.join("projects");
    assert!(
        list_names(&projects)
            .iter()
            .any(|name| name.contains(".pre-reembed-")),
        "expected a .pre-reembed-* backup beside the shard"
    );

    let db = fixture
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("replacement connection");
    let rows = db
        .query(
            "SELECT *, vector_extract(vector) AS vector_json FROM memories ORDER BY id",
            Vec::new(),
        )
        .await
        .expect("read replacement");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].text("tags").as_deref(), Some("alpha,beta"));
    assert_eq!(rows[0].text("display_name").as_deref(), Some("Memory 0"));
    assert_eq!(rows[0].i64("is_pinned"), Some(1));
    let vector_json = rows[0].text("vector_json").expect("vector_extract text");
    let vector: Vec<f32> = serde_json::from_str(&vector_json).expect("vector json");
    assert_eq!(vector, vec![1.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        rows[0].text("metadata").as_deref(),
        Some(r#"{"source":"test","index":0}"#)
    );
    let metadata = mem_store::db::query_one(
        &*db,
        sql::GET_SHARD_METADATA_VALUE,
        vec![Value::Text("embedding_dimensions".to_string())],
    )
    .await
    .expect("shard metadata")
    .expect("dimensions row");
    assert_eq!(metadata.text("value").as_deref(), Some("4"));

    // Swap journal consumed; staged temp gone.
    let staged_left = list_names(&projects)
        .iter()
        .any(|name| name.contains(".reembed-") && name.ends_with(".tmp"));
    assert!(!staged_left, "staged temp must not survive a clean swap");
    assert!(
        !projects
            .join(format!(
                "{}.reembed-swap.json",
                shard.db_path.file_name().unwrap().to_string_lossy()
            ))
            .exists()
    );

    drop(db);
    fixture
        .connections
        .close_connection(&shard.db_path)
        .await
        .expect("connection closes");
    cleanup_temp_dir(&fixture.storage).expect("cleanup");
}

#[tokio::test]
async fn test_leaves_the_source_shard_untouched_when_any_embedding_fails() {
    let (fixture, shard) = fixture("reembed-fail").await;
    fixture
        .reembedder
        .fail_on
        .lock()
        .unwrap()
        .replace("content 1".to_string());

    let result = fixture
        .service
        .migrate_to_new_model(MigrationStrategy::ReEmbed)
        .await
        .expect("migration call");

    assert!(!result.success);
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains(FakeReEmbedder::failure_message()),
        "error was: {error}"
    );
    let projects = fixture.storage.join("projects");
    let names = list_names(&projects);
    assert!(
        !names.iter().any(|name| name.contains(".pre-reembed-")),
        "source must never be moved when staging fails: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|name| name.contains(".reembed-") && name.ends_with(".tmp")),
        "staged temp must be removed on failure: {names:?}"
    );

    let db = fixture
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("source connection");
    let count = mem_store::db::query_one(&*db, sql::COUNT_ALL_VECTORS, Vec::new())
        .await
        .expect("count source")
        .expect("count row");
    assert_eq!(count.i64("count"), Some(2));
    let metadata = mem_store::db::query_one(
        &*db,
        sql::GET_SHARD_METADATA_VALUE,
        vec![Value::Text("embedding_dimensions".to_string())],
    )
    .await
    .expect("shard metadata")
    .expect("dimensions row");
    assert_eq!(metadata.text("value").as_deref(), Some("2"));

    drop(db);
    fixture
        .connections
        .close_connection(&shard.db_path)
        .await
        .expect("connection closes");
    cleanup_temp_dir(&fixture.storage).expect("cleanup");
}

#[tokio::test]
async fn test_archives_source_shards_instead_of_deleting_them_during_fresh_start() {
    let (fixture, shard) = fixture("reembed-fresh").await;
    let original_name = shard
        .db_path
        .file_name()
        .expect("shard file name")
        .to_string_lossy()
        .to_string();

    let result = fixture
        .service
        .migrate_to_new_model(MigrationStrategy::FreshStart)
        .await
        .expect("migration call");

    assert!(result.success, "migration error: {:?}", result.error);
    assert_eq!(result.deleted_shards, 1);
    assert_eq!(result.re_embedded_memories, 0);
    let names = list_names(&fixture.storage.join("projects"));
    assert!(
        names
            .iter()
            .any(|name| name.contains(".fresh-start-") && name.ends_with(".bak")),
        "expected a .fresh-start-*.bak archive: {names:?}"
    );
    assert!(
        !names.contains(&original_name),
        "original shard file must be gone"
    );

    let remaining = fixture
        .shards
        .get_all_shards(Scope::Project, Some(SCOPE_HASH))
        .await
        .expect("registry read");
    assert!(remaining.is_empty(), "registry entry must be archived away");

    cleanup_temp_dir(&fixture.storage).expect("cleanup");
}

#[tokio::test]
async fn test_recovers_a_crash_between_source_backup_and_replacement_rename() {
    let (fixture, shard) = fixture("reembed-crash").await;
    fixture
        .connections
        .close_connection(&shard.db_path)
        .await
        .expect("connection closes");

    // Emulate the crash window: staged replacement exists, the source has
    // already been moved to the backup, the journal is on disk.
    let staged = PathBuf::from(format!("{}.reembed-crash.tmp", shard.db_path.display()));
    let backup = PathBuf::from(format!("{}.pre-reembed-crash.bak", shard.db_path.display()));
    copy_sqlite_database(&shard.db_path, &staged).expect("copy staged replacement");
    mem_store::sqlite_files::rename_sqlite_database(&shard.db_path, &backup)
        .expect("move source to backup");
    let state = common::write_reembed_swap_state(&shard.db_path, &staged, &backup);

    let sleeper = common::RecordingSleeper::new();
    let sleep = sleeper.as_sleep_fn();
    legacy_migrator::run_legacy_turso_migration(
        &fixture.storage,
        &fixture.shards,
        &fixture.connections,
        &sleep,
    )
    .await
    .expect("legacy migration recovers the interrupted swap");

    assert!(shard.db_path.exists(), "active shard restored at its path");
    assert!(!state.exists(), "swap journal consumed by recovery");
    let db = fixture
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("restored connection");
    let count = mem_store::db::query_one(&*db, sql::COUNT_ALL_VECTORS, Vec::new())
        .await
        .expect("count restored shard")
        .expect("count row");
    assert_eq!(count.i64("count"), Some(2));

    drop(db);
    fixture
        .connections
        .close_connection(&shard.db_path)
        .await
        .expect("connection closes");
    cleanup_temp_dir(&fixture.storage).expect("cleanup");
}
