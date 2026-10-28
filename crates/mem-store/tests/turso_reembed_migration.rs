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

use common::{
    FakeReEmbedder, RecordingConnectFactory, cleanup_temp_dir, real_components_with_identity,
    store_config,
};
use mem_core::embedding::EmbeddingIdentity;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, Value, query_one};
use mem_store::legacy_migrator;
use mem_store::migration_service::{MigrationService, MigrationStrategy, ReEmbedder};
use mem_store::platform::SystemProcessProbe;
use mem_store::ready::build_ready_gate;
use mem_store::scope::Scope;
use mem_store::shard_facts;
use mem_store::shard_manager::{ShardInfo, ShardManager, ShardRegistry};
use mem_store::sql;
use mem_store::sqlite_files::copy_sqlite_database;
use mem_store::vector::{MemoryInsert, insert_vector_statement};

const SCOPE_HASH: &str = "0123456789abcdef";

/// The identity a `StoreConfig` describes (a test-side bridge: production
/// derives it from the resolved `mem_core::Config`).
fn identity_for_store(config: &mem_store::config::StoreConfig) -> EmbeddingIdentity {
    EmbeddingIdentity {
        model: config.embedding_model.clone(),
        dimensions: config.embedding_dimensions,
        pooling: mem_core::config::EmbeddingPooling::Mean,
        dtype: None,
        use_task_prefixes: false,
        query_prefix: None,
        document_prefix: None,
    }
}

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
    // Upstream flips CONFIG.embeddingDimensions = 4 before migrating. The
    // manager carries that same target identity (W2): the StoreConfig keeps
    // the base dims so the source shard stays valid until the swap.
    let mut service_config = base.clone();
    service_config.embedding_dimensions = 4;
    service_config.embedding_model = "text-embedding-mock".to_string();
    let identity = identity_for_store(&service_config);
    let (config, connections, manager) = real_components_with_identity(base, Some(identity)).await;
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

// ---------------------------------------------------------------------------
// W2 — embedding-identity detection and scope re-embed (AC11–AC16)
// ---------------------------------------------------------------------------

/// A populated, equal-dimensions shard rig: created and filled with the
/// identity **unwired**, then read through a manager that carries (or does
/// not carry) `Some(identity)` — the pre-W2 population of R-12.
struct IdentityRig {
    config: mem_store::config::StoreConfig,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    service: MigrationService,
    reembedder: Arc<FakeReEmbedder>,
}

#[allow(clippy::too_many_arguments)]
async fn identity_rig(
    prefix: &str,
    ids: &[&str],
    stored_model: &str,
    fingerprint: Option<&str>,
    wire_identity: bool,
    recording: Option<&Arc<RecordingConnectFactory>>,
) -> (IdentityRig, ShardInfo) {
    let config = store_config(prefix); // dims = 2, model "test-model"
    let factory: Arc<dyn ConnectFactory> = match recording {
        Some(recording) => recording.clone(),
        None => Arc::new(mem_store::db::TursoConnectFactory),
    };
    let (config, connections, manager) =
        common::real_components_with_factory(config, None, factory).await;
    let unwired = Arc::new(manager);
    let registry: Arc<dyn ShardRegistry> = unwired.clone();
    let ready = Arc::new(build_ready_gate(
        &config.storage_path,
        Arc::clone(&connections),
        Arc::clone(&unwired),
        registry,
    ));
    ready.ensure_ready().await.expect("ready gate closes");

    let shard = unwired
        .create_shard(Scope::Project, SCOPE_HASH, 0)
        .await
        .expect("create the pre-W2 shard");
    let db = connections
        .get_connection(&shard.db_path)
        .await
        .expect("source connection");
    for (index, id) in ids.iter().enumerate() {
        let record = MemoryInsert {
            id: (*id).to_string(),
            content: format!("content {index}"),
            vector: vec![1.0, index as f32],
            tags_vector: None,
            container_tag: format!("opencode_project_{SCOPE_HASH}"),
            tags: None,
            memory_type: None,
            created_at: 100 + index as i64,
            updated_at: 200 + index as i64,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
        };
        let statement = insert_vector_statement(&record).expect("statement builds");
        db.execute(&statement.sql, statement.params)
            .await
            .expect("insert memory");
        unwired
            .increment_vector_count(shard.id)
            .await
            .expect("vector count");
        db.execute(
            "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_model', ?1)",
            vec![Value::Text(stored_model.to_string())],
        )
        .await
        .expect("stored model");
    }
    if let Some(fingerprint) = fingerprint {
        db.execute(
            "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_fingerprint', ?1)",
            vec![Value::Text(fingerprint.to_string())],
        )
        .await
        .expect("stored fingerprint");
    }
    // Leave a truthful count behind (the fixture bypassed the manager's sync).
    shard_facts::sync_count(db.as_ref())
        .await
        .expect("seed shard_stats");
    drop(db);

    let identity = wire_identity.then(|| identity_for_store(&config));
    let shards = Arc::new(ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        Arc::clone(&connections),
        Arc::new(SystemProcessProbe),
        identity,
    ));
    let reembedder = Arc::new(FakeReEmbedder::new());
    let service = MigrationService::new(
        config.clone(),
        ready,
        Arc::clone(&shards),
        Arc::clone(&connections),
        Arc::clone(&reembedder) as Arc<dyn ReEmbedder>,
    );
    (
        IdentityRig {
            config,
            connections,
            shards,
            service,
            reembedder,
        },
        shard,
    )
}

async fn metadata_value(rig: &IdentityRig, shard: &ShardInfo, key: &str) -> Option<String> {
    let db = rig
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("shard connection");
    query_one(
        &*db,
        "SELECT value FROM shard_metadata WHERE key = ?1",
        vec![Value::Text(key.to_string())],
    )
    .await
    .expect("metadata read")
    .and_then(|row| row.text("value"))
}

async fn memory_count(rig: &IdentityRig, shard: &ShardInfo) -> i64 {
    let db = rig
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("shard connection");
    query_one(&*db, sql::COUNT_ALL_VECTORS, Vec::new())
        .await
        .expect("count")
        .and_then(|row| row.i64("count"))
        .unwrap_or(-1)
}

async fn stats_value(rig: &IdentityRig, shard: &ShardInfo) -> Option<i64> {
    let db = rig
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("shard connection");
    query_one(
        &*db,
        "SELECT vector_count FROM shard_stats WHERE id = 1",
        Vec::new(),
    )
    .await
    .expect("stats read")
    .and_then(|row| row.i64("vector_count"))
}

/// AC11 — a model-only change (equal dims) is detected, in two arms:
/// (a) a present fingerprint that differs; (b) a populated lost-registry shard
/// whose preserved `embedding_model` is foreign (D-16).
#[tokio::test]
async fn test_a_model_change_with_equal_dimensions_is_detected() {
    // Arm (a): fingerprint present and different.
    let (rig, shard) = identity_rig(
        "identity-detect-fp",
        &["mem_one", "mem_two"],
        "test-model",
        Some("foreign-fingerprint"),
        true,
        None,
    )
    .await;
    let mismatches = rig
        .service
        .detect_embedding_mismatch()
        .await
        .expect("detect");
    assert_eq!(
        mismatches.len(),
        1,
        "one shard carries the foreign fingerprint"
    );
    assert_eq!(mismatches[0].shard_id, shard.id);
    assert_eq!(
        mismatches[0].stored_fingerprint.as_deref(),
        Some("foreign-fingerprint")
    );
    assert_eq!(mismatches[0].stored_model.as_deref(), Some("test-model"));
    cleanup_temp_dir(&rig.config.storage_path).expect("cleanup");

    // Arm (b): registry row lost; the populated file's foreign model must
    // survive create_shard (INSERT OR IGNORE, D-16) and be detected.
    let (rig, shard) = identity_rig(
        "identity-detect-lost-row",
        &["mem_one", "mem_two"],
        "foreign-model",
        None,
        true,
        None,
    )
    .await;
    let metadata_path = rig
        .config
        .storage_path
        .join(mem_store::operation_lock::METADATA_DB_NAME);
    let metadata = rig
        .connections
        .get_connection(&metadata_path)
        .await
        .expect("metadata connection");
    metadata
        .execute(
            "DELETE FROM shards WHERE id = ?1",
            vec![Value::Integer(shard.id)],
        )
        .await
        .expect("delete only the registry row");
    drop(metadata);

    let recreated = rig
        .shards
        .create_shard(Scope::Project, SCOPE_HASH, 0)
        .await
        .expect("re-register the lost row over the populated file");
    assert_eq!(
        metadata_value(&rig, &recreated, "embedding_model")
            .await
            .as_deref(),
        Some("foreign-model"),
        "D-16: a populated file's stored model is never rewritten"
    );
    assert_eq!(
        metadata_value(&rig, &recreated, "embedding_fingerprint").await,
        None,
        "the non-empty shard stays unclaimed"
    );
    let mismatches = rig
        .service
        .detect_embedding_mismatch()
        .await
        .expect("detect");
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0].stored_model.as_deref(), Some("foreign-model"));
    assert_eq!(mismatches[0].stored_fingerprint, None);

    // Control: a fresh create_shard writes the configured model (OR IGNORE
    // writes when the key is absent).
    let fresh = rig
        .shards
        .create_shard(Scope::Project, "abcdef0123456789", 0)
        .await
        .expect("fresh shard");
    assert_eq!(
        metadata_value(&rig, &fresh, "embedding_model")
            .await
            .as_deref(),
        Some("test-model")
    );
    cleanup_temp_dir(&rig.config.storage_path).expect("cleanup");
}

/// AC12 — re-embedding a scope rewrites a populated, un-fingerprinted,
/// model-equal shard and stamps the new fingerprint + count in the same file
/// (R-12's supported way out).
#[tokio::test]
async fn test_reembed_scope_rewrites_the_scope_and_stamps_the_new_fingerprint() {
    let (rig, shard) = identity_rig(
        "identity-reembed-scope",
        &["mem_one", "mem_two"],
        "test-model",
        None,
        true,
        None,
    )
    .await;
    // The fixture is NOT selected by identity detection: no fingerprint and an
    // equal model. A short-circuit on `detect_embedding_mismatch().is_empty()`
    // returns zero rewrites and fails here.
    assert!(
        rig.service
            .detect_embedding_mismatch()
            .await
            .expect("detect")
            .is_empty(),
        "the fixture must be invisible to identity detection"
    );

    let identity = identity_for_store(&rig.config);
    let expected = identity.fingerprint().expect("identity serializes");
    let result = rig
        .service
        .reembed_scope(Scope::Project, SCOPE_HASH)
        .await
        .expect("scope re-embed");
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(result.re_embedded_memories, 2);
    assert_eq!(
        rig.reembedder.calls.lock().unwrap().len(),
        2,
        "both memories were re-embedded"
    );
    assert_eq!(
        metadata_value(&rig, &shard, "embedding_fingerprint")
            .await
            .as_deref(),
        Some(expected.as_str()),
        "the configured fingerprint is stamped"
    );
    assert_eq!(memory_count(&rig, &shard).await, 2);
    assert_eq!(stats_value(&rig, &shard).await, Some(2));
    cleanup_temp_dir(&rig.config.storage_path).expect("cleanup");
}

/// AC13 — a shard already at the target identity is accepted, not rewritten:
/// no embed, no warmup, no staged temp.
#[tokio::test]
async fn test_a_matching_fingerprint_is_accepted_without_a_rebuild() {
    let recording = Arc::new(RecordingConnectFactory::new());
    let config = store_config("identity-short-circuit");
    let identity = identity_for_store(&config);
    let fingerprint = identity.fingerprint().expect("identity serializes");
    let (rig, shard) = identity_rig(
        "identity-short-circuit",
        &["mem_one", "mem_two"],
        "test-model",
        Some(&fingerprint),
        true,
        Some(&recording),
    )
    .await;

    let result = rig
        .service
        .reembed_scope(Scope::Project, SCOPE_HASH)
        .await
        .expect("scope re-embed");
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(result.re_embedded_memories, 0);
    assert!(
        rig.reembedder.calls.lock().unwrap().is_empty(),
        "no embedding call may happen on the short-circuit"
    );
    assert_eq!(
        rig.reembedder
            .warmups
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "even warmup is skipped"
    );

    let opens = recording.opens();
    assert!(
        !opens.iter().any(|path| {
            let text = path.to_string_lossy();
            text.contains(".reembed-") && text.ends_with(".tmp")
        }),
        "no staged temp may be opened: {opens:?}"
    );
    let batches = recording.write_batches();
    assert!(
        !batches.iter().any(|batch| {
            let text = batch.path.to_string_lossy();
            text.contains(".reembed-") && text.ends_with(".tmp")
        }),
        "no write batch may hit a staged temp"
    );
    // The values are unchanged.
    assert_eq!(memory_count(&rig, &shard).await, 2);
    assert_eq!(stats_value(&rig, &shard).await, Some(2));
    assert_eq!(
        metadata_value(&rig, &shard, "embedding_fingerprint")
            .await
            .as_deref(),
        Some(fingerprint.as_str())
    );
    cleanup_temp_dir(&rig.config.storage_path).expect("cleanup");
}

/// AC14 — without a configured identity the scope re-embed refuses and
/// mutates nothing.
#[tokio::test]
async fn test_reembed_without_a_configured_identity_is_refused() {
    let (rig, shard) = identity_rig(
        "identity-refused",
        &["mem_one", "mem_two"],
        "test-model",
        None,
        false,
        None,
    )
    .await;
    let result = rig
        .service
        .reembed_scope(Scope::Project, SCOPE_HASH)
        .await
        .expect("a refusal is a typed outcome, not an outer error");
    assert!(!result.success);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("identity"),
        "the refusal must name the missing identity: {:?}",
        result.error
    );
    assert_eq!(memory_count(&rig, &shard).await, 2);
    assert_eq!(
        metadata_value(&rig, &shard, "embedding_fingerprint").await,
        None
    );
    let projects = rig.config.storage_path.join("projects");
    let names: Vec<String> = std::fs::read_dir(&projects)
        .expect("projects dir")
        .filter_map(|entry| {
            entry
                .ok()
                .map(|e| e.file_name().to_string_lossy().to_string())
        })
        .collect();
    assert!(
        !names.iter().any(|name| name.contains(".reembed-")),
        "no staged file may exist: {names:?}"
    );
    cleanup_temp_dir(&rig.config.storage_path).expect("cleanup");
}

/// AC15 — the staged **vector** batch, identified by content: the claim is
/// first, the N inserts follow, the count is last; no other batch claims.
#[tokio::test]
async fn test_the_staged_rebuild_writes_the_fingerprint_in_the_vector_batch() {
    let recording = Arc::new(RecordingConnectFactory::new());
    let (rig, _shard) = identity_rig(
        "identity-vector-batch",
        &["mem_one", "mem_two"],
        "test-model",
        None,
        true,
        Some(&recording),
    )
    .await;

    let result = rig
        .service
        .reembed_scope(Scope::Project, SCOPE_HASH)
        .await
        .expect("scope re-embed");
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(result.re_embedded_memories, 2);

    let staged: Vec<common::RecordedBatch> = recording
        .write_batches()
        .into_iter()
        .filter(|batch| {
            let text = batch.path.to_string_lossy();
            text.contains(".reembed-") && text.ends_with(".tmp")
        })
        .collect();
    assert_eq!(
        staged.len(),
        5,
        "a fresh staged file: v1 + v2 + v3 + metadata + vector batches"
    );

    let claiming: Vec<&common::RecordedBatch> = staged
        .iter()
        .filter(|batch| {
            batch
                .statements
                .iter()
                .any(|statement| statement.sql.contains("'embedding_fingerprint'"))
        })
        .collect();
    assert_eq!(claiming.len(), 1, "exactly one batch claims");
    let batch = claiming[0];
    let first = batch.statements.first().expect("non-empty batch");
    assert!(
        first.sql.contains("INSERT OR IGNORE INTO shard_metadata")
            && first.sql.contains("'embedding_fingerprint'"),
        "the claim must be the FIRST statement: {}",
        first.sql
    );
    assert_eq!(
        batch
            .statements
            .iter()
            .filter(|statement| statement.sql.contains("INSERT INTO memories"))
            .count(),
        2,
        "the batch carries exactly the N vector inserts"
    );
    assert_eq!(
        batch.statements.last().expect("non-empty batch").sql,
        shard_facts::SYNC_COUNT_SQL,
        "the sync count must be the LAST statement"
    );
    assert_eq!(
        recording
            .write_batches()
            .iter()
            .filter(|candidate| candidate
                .statements
                .iter()
                .any(|statement| statement.sql.contains("'embedding_fingerprint'")))
            .count(),
        1,
        "no batch outside the staged vector batch may claim"
    );
    cleanup_temp_dir(&rig.config.storage_path).expect("cleanup");
}

/// AC16 — `migrate_to_new_model(ReEmbed)` re-embeds identity mismatches too.
#[tokio::test]
async fn test_migrate_to_new_model_reembed_includes_identity_mismatches() {
    let (rig, shard) = identity_rig(
        "identity-migrate-union",
        &["mem_one", "mem_two"],
        "test-model",
        Some("foreign-fingerprint"),
        true,
        None,
    )
    .await;
    let dims_before = rig
        .service
        .detect_dimension_mismatch()
        .await
        .expect("dims detect");
    assert!(
        !dims_before.needs_migration,
        "the fixture must differ in identity only"
    );

    let result = rig
        .service
        .migrate_to_new_model(MigrationStrategy::ReEmbed)
        .await
        .expect("migration call");
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(result.re_embedded_memories, 2);
    let identity = identity_for_store(&rig.config);
    assert_eq!(
        metadata_value(&rig, &shard, "embedding_fingerprint")
            .await
            .as_deref(),
        Some(identity.fingerprint().expect("serializes").as_str())
    );
    cleanup_temp_dir(&rig.config.storage_path).expect("cleanup");
}
