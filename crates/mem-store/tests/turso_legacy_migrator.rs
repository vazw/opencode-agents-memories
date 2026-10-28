//! Legacy-migrator battery (blueprint W1.3 step 5): port of upstream
//! `tests/turso-legacy-migrator.test.ts` (7 cases) + the pinned legacy-lock
//! retry case. Every run drives the real seam
//! (`run_legacy_turso_migration`) over real turso components in a temp
//! storage root; retry delays go through `RecordingSleeper`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{RecordingSleeper, cleanup_temp_dir, real_components};
use mem_store::connection::ConnectionManager;
use mem_store::db::{Value, query_one};
use mem_store::error::StoreError;
use mem_store::legacy_migrator::run_legacy_turso_migration;
use mem_store::scope::Scope;
use mem_store::shard_manager::ShardManager;
use mem_store::sql;
use mem_store::vector::{MemoryInsert, insert_vector};

/// Upstream `createLegacyShard` schema (pre-turso shard shape: plain BLOB
/// vectors — raw F32 LE bytes, no header — and NO shard_metadata table).
const LEGACY_SCHEMA: &str = r#"CREATE TABLE memories (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  vector BLOB NOT NULL,
  tags_vector BLOB,
  container_tag TEXT NOT NULL,
  tags TEXT,
  type TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  metadata TEXT,
  display_name TEXT,
  user_name TEXT,
  user_email TEXT,
  project_path TEXT,
  project_name TEXT,
  git_repo_url TEXT,
  is_pinned INTEGER DEFAULT 0
)"#;

struct LegacyRow {
    id: &'static str,
    content: &'static str,
    /// Raw BLOB payload for `vector` (usually `f32_blob(...)`; the corrupt
    /// case passes a 3-byte non-multiple-of-4 payload).
    blob: Vec<u8>,
    container_tag: String,
}

impl LegacyRow {
    fn new(id: &'static str, content: &'static str, vector: Vec<f32>, container_tag: &str) -> Self {
        Self {
            id,
            content,
            blob: f32_blob(&vector),
            container_tag: container_tag.to_string(),
        }
    }
}

/// Raw F32 little-endian bytes (upstream `new Uint8Array(f32.buffer)`).
fn f32_blob(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Upstream `new Float32Array(768); vector[0] = 1`.
fn vector768() -> Vec<f32> {
    let mut vector = vec![0.0f32; 768];
    vector[0] = 1.0;
    vector
}

/// Hermetic rig: real components over a temp storage root at the given
/// embedding dimensions (upstream flips `CONFIG.embeddingDimensions`).
struct Rig {
    storage: PathBuf,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    sleeper: RecordingSleeper,
}

async fn rig(prefix: &str, dims: u32, model: &str) -> Rig {
    let mut config = common::store_config(prefix);
    config.embedding_dimensions = dims;
    config.embedding_model = model.to_string();
    let (config, connections, manager) = real_components(config).await;
    Rig {
        storage: config.storage_path.clone(),
        connections,
        shards: Arc::new(manager),
        sleeper: RecordingSleeper::new(),
    }
}

impl Rig {
    async fn run(&self) -> Result<(), StoreError> {
        let sleep = self.sleeper.as_sleep_fn();
        run_legacy_turso_migration(&self.storage, &self.shards, &self.connections, &sleep).await
    }

    /// Upstream `createLegacyShard`: legacy schema + raw-f32 rows, written
    /// through a real (turso-engine) connection at `dir/name`.
    async fn create_legacy_shard(&self, dir: &Path, name: &str, rows: &[LegacyRow]) -> PathBuf {
        std::fs::create_dir_all(dir).expect("scope dir");
        let db_path = dir.join(name);
        let db = self
            .connections
            .get_connection(&db_path)
            .await
            .expect("legacy shard open");
        db.execute(LEGACY_SCHEMA, Vec::new())
            .await
            .expect("legacy table");
        for row in rows {
            db.execute(
                "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                vec![
                    Value::Text(row.id.to_string()),
                    Value::Text(row.content.to_string()),
                    Value::Blob(row.blob.clone()),
                    Value::Text(row.container_tag.clone()),
                    Value::Integer(1_700_000_000_000),
                    Value::Integer(1_700_000_000_000),
                ],
            )
            .await
            .expect("legacy insert");
        }
        drop(db);
        self.connections
            .close_connection(&db_path)
            .await
            .expect("connection closes");
        db_path
    }

    fn projects(&self) -> PathBuf {
        let dir = self.storage.join("projects");
        std::fs::create_dir_all(&dir).expect("projects dir");
        dir
    }

    async fn read_json(&self, path: &Path) -> serde_json::Value {
        let body = std::fs::read_to_string(path).expect("json read");
        serde_json::from_str(&body).expect("json parse")
    }
}

// ---------------------------------------------------------------------------
// 1/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_migrates_legacy_shard_and_writes_sidecar_plus_global_marker() {
    let rig = rig("legacy-basic", 768, "test-model").await;
    let scope_hash = "a1b2c3d4e5f67890";
    rig.create_legacy_shard(
        &rig.projects(),
        &format!("project_{scope_hash}_shard_0.db"),
        &[LegacyRow::new(
            "mem_legacy_1",
            "legacy memory",
            vector768(),
            &format!("opencode_project_{scope_hash}"),
        )],
    )
    .await;

    rig.run().await.expect("migration succeeds");

    let sidecar_path = rig.storage.join("projects").join(format!(
        "project_{scope_hash}_shard_0.db.turso-migrate.json"
    ));
    assert!(sidecar_path.exists(), "sidecar written");
    let sidecar = rig.read_json(&sidecar_path).await;
    assert_eq!(sidecar["status"].as_str(), Some("complete"));
    assert_eq!(sidecar["expectedCount"].as_u64(), Some(1));
    assert_eq!(sidecar["sourceCount"].as_u64(), Some(1));
    assert_eq!(sidecar["skippedCount"].as_u64(), Some(0));

    let marker_path = rig.storage.join(".turso-migrated");
    assert!(marker_path.exists(), "global marker written");
    let marker = rig.read_json(&marker_path).await;
    assert_eq!(marker["shards"].as_array().map(Vec::len), Some(1));

    let registered = rig
        .shards
        .get_all_shards(Scope::Project, None)
        .await
        .expect("registry read");
    assert_eq!(registered.len(), 1, "shard registered by reconcile");
    assert_eq!(registered[0].scope_hash, scope_hash);
    assert_eq!(registered[0].vector_count, 1);

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 2/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_resumes_from_backup_after_interrupted_migration() {
    let rig = rig("legacy-resume", 768, "test-model").await;
    let db_path = rig
        .create_legacy_shard(
            &rig.projects(),
            "project_resume_shard_0.db",
            &[LegacyRow::new(
                "mem_resume_1",
                "resume me",
                vector768(),
                "opencode_project_resume",
            )],
        )
        .await;

    // Simulate a crash after the backup rename but before the import: the
    // backup sits at `.legacy.bak`, the active path holds a fresh (empty)
    // turso shard, and the sidecar still says `pending`. The move goes
    // through the PRODUCTION rename primitive (main + wal/shm/tshm sidecars
    // together) because multiprocess-WAL keeps committed rows in `-wal`
    // after close — a raw `fs::rename` would orphan the data the backup
    // must still hold (upstream needed no such care: their fixture closes
    // with a plain checkpointed WAL).
    let backup = PathBuf::from(format!("{}.legacy.bak", db_path.display()));
    let proof = common::exclusive_root(&rig.storage);
    let from_claim = proof
        .claim(&db_path)
        .expect("the source is inside the root");
    let to_claim = proof.claim(&backup).expect("the backup is inside the root");
    mem_store::sqlite_files::rename_sqlite_database(from_claim, to_claim).expect("move to backup");
    let fresh_db = rig
        .connections
        .get_connection(&db_path)
        .await
        .expect("fresh db");
    rig.shards
        .init_shard_db(&*fresh_db, 768, "test-model")
        .await
        .expect("fresh init");
    drop(fresh_db);
    std::fs::write(
        format!("{}.turso-migrate.json", db_path.display()),
        r#"{"expectedCount":1,"importedCount":0,"status":"pending"}"#,
    )
    .expect("pending sidecar");

    rig.run().await.expect("resume succeeds");

    let migrated_db = rig
        .connections
        .get_connection(&db_path)
        .await
        .expect("migrated connection");
    let row = query_one(&*migrated_db, sql::COUNT_ALL_VECTORS, Vec::new())
        .await
        .expect("count")
        .expect("row");
    assert_eq!(row.i64("count"), Some(1), "resume imported from backup");
    let restored = query_one(
        &*migrated_db,
        "SELECT content FROM memories WHERE id = ?",
        vec![Value::Text("mem_resume_1".to_string())],
    )
    .await
    .expect("content read")
    .expect("row");
    assert_eq!(
        restored.text("content").as_deref(),
        Some("resume me"),
        "content restored from backup"
    );

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 3/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_does_not_skip_migration_when_marker_exists_but_shard_sidecar_is_incomplete() {
    let rig = rig("legacy-marker", 768, "test-model").await;
    rig.create_legacy_shard(
        &rig.projects(),
        "project_marker_shard_0.db",
        &[LegacyRow::new(
            "mem_marker_1",
            "marker test",
            vector768(),
            "opencode_project_marker",
        )],
    )
    .await;
    std::fs::write(
        rig.storage.join(".turso-migrated"),
        r#"{"completedAt":"2026-01-01T00:00:00.000Z","shards":[]}"#,
    )
    .expect("marker write");

    rig.run().await.expect("marker never skips real work");

    let sidecar_path = rig
        .storage
        .join("projects")
        .join("project_marker_shard_0.db.turso-migrate.json");
    assert!(
        sidecar_path.exists(),
        "sidecar written despite stale marker"
    );
    let sidecar = rig.read_json(&sidecar_path).await;
    assert_eq!(sidecar["status"].as_str(), Some("complete"));

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 4/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_aborts_migration_when_legacy_vector_blob_is_unreadable() {
    let rig = rig("legacy-corrupt", 768, "test-model").await;
    let db_path = rig
        .create_legacy_shard(
            &rig.projects(),
            "project_corrupt_shard_0.db",
            &[LegacyRow {
                id: "mem_corrupt_1",
                content: "bad vector",
                // 3 bytes: not a whole number of f32 lanes (upstream
                // `new Uint8Array([1, 2, 3])`).
                blob: vec![1, 2, 3],
                container_tag: "opencode_project_corrupt".to_string(),
            }],
        )
        .await;

    let error = rig.run().await.expect_err("unreadable blob aborts");
    assert!(
        error.message().contains("unreadable vectors"),
        "abort names the unreadable vectors, got {}",
        error.message()
    );

    assert!(
        !rig.storage.join(".turso-migrated").exists(),
        "no marker after abort"
    );
    let sidecar_path = PathBuf::from(format!("{}.turso-migrate.json", db_path.display()));
    assert!(!sidecar_path.exists(), "no sidecar (complete or otherwise)");
    let backup = PathBuf::from(format!("{}.legacy.bak", db_path.display()));
    assert!(!backup.exists(), "original never renamed to backup");
    let db = rig
        .connections
        .get_connection(&db_path)
        .await
        .expect("original still openable");
    let row = query_one(
        &*db,
        "SELECT content FROM memories WHERE id = ?",
        vec![Value::Text("mem_corrupt_1".to_string())],
    )
    .await
    .expect("row read")
    .expect("row");
    assert_eq!(
        row.text("content").as_deref(),
        Some("bad vector"),
        "original shard still holds its row (never mutated away)"
    );

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 5/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_does_not_skip_migration_when_complete_sidecar_lacks_vector_index() {
    let rig = rig("legacy-fake-sidecar", 768, "test-model").await;
    let db_path = rig
        .create_legacy_shard(
            &rig.projects(),
            "project_fake_sidecar_shard_0.db",
            &[LegacyRow::new(
                "mem_fake_1",
                "needs real migration",
                vector768(),
                "opencode_project_fakesidecar",
            )],
        )
        .await;
    // Fabricated "complete" sidecar: counts match, but the shard holds no
    // turso vector metadata at all — readiness must see through it.
    std::fs::write(
        format!("{}.turso-migrate.json", db_path.display()),
        r#"{"sourceCount":1,"expectedCount":1,"importedCount":1,"skippedCount":0,"status":"complete"}"#,
    )
    .expect("fake sidecar");
    std::fs::write(
        rig.storage.join(".turso-migrated"),
        r#"{"completedAt":"2026-01-01T00:00:00.000Z","shards":[]}"#,
    )
    .expect("marker write");

    rig.run().await.expect("migration still runs");

    let db = rig
        .connections
        .get_connection(&db_path)
        .await
        .expect("migrated connection");
    let index_row = query_one(
        &*db,
        "SELECT name FROM sqlite_master WHERE type='index' AND name='memories_vec_idx'",
        Vec::new(),
    )
    .await
    .expect("index probe");
    assert!(index_row.is_none(), "no libSQL memories_vec_idx on turso");
    let probe = query_one(
        &*db,
        "SELECT vector_extract(vector) AS extracted FROM memories LIMIT 1",
        Vec::new(),
    )
    .await
    .expect("vector probe")
    .expect("row");
    let extracted = probe.text("extracted").unwrap_or_default();
    assert!(!extracted.is_empty(), "vector_extract yields readable text");
    let meta = query_one(
        &*db,
        sql::GET_SHARD_METADATA_VALUE,
        vec![Value::Text("embedding_dimensions".to_string())],
    )
    .await
    .expect("meta read")
    .expect("row");
    assert_eq!(
        meta.text("value").as_deref(),
        Some("768"),
        "dims metadata written"
    );

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 6/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_does_not_skip_migration_when_index_exists_but_embedding_dimensions_mismatch() {
    let rig = rig("legacy-dims-mismatch", 768, "test-model").await;
    let scope_hash = "d1d2d3d4e5f67890";
    let shard = rig
        .shards
        .create_shard(Scope::Project, scope_hash, 0)
        .await
        .expect("create shard");
    let db = rig
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("shard connection");
    insert_vector(
        &*db,
        &MemoryInsert {
            id: "mem_dims_mismatch_1".to_string(),
            content: "turso shard with wrong metadata dims".to_string(),
            vector: vector768(),
            tags_vector: None,
            container_tag: format!("opencode_project_{scope_hash}"),
            tags: None,
            memory_type: Some("project".to_string()),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
        },
    )
    .await
    .expect("insert vector");
    // Corrupt the metadata: 384 declared, 768 actually stored.
    db.execute(
        "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_dimensions', ?)",
        vec![Value::Text("384".to_string())],
    )
    .await
    .expect("corrupt metadata");
    drop(db);
    rig.connections
        .close_connection(&shard.db_path)
        .await
        .expect("connection closes");
    let sidecar_path = PathBuf::from(format!("{}.turso-migrate.json", shard.db_path.display()));
    if sidecar_path.exists() {
        std::fs::remove_file(&sidecar_path).expect("sidecar clear");
    }

    rig.run().await.expect("dims mismatch re-migrates");

    let backup = PathBuf::from(format!("{}.legacy.bak", shard.db_path.display()));
    assert!(backup.exists(), "mismatched shard backed up");
    let sidecar = rig.read_json(&sidecar_path).await;
    assert_eq!(sidecar["status"].as_str(), Some("complete"));

    let migrated_db = rig
        .connections
        .get_connection(&shard.db_path)
        .await
        .expect("migrated connection");
    let meta = query_one(
        &*migrated_db,
        sql::GET_SHARD_METADATA_VALUE,
        vec![Value::Text("embedding_dimensions".to_string())],
    )
    .await
    .expect("meta read")
    .expect("row");
    assert_eq!(meta.text("value").as_deref(), Some("768"), "dims repaired");

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 7/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_repairs_the_active_shard_flag_while_reconciling_an_existing_registry() {
    let rig = rig("legacy-registry", 4, "test-model").await;
    let scope_hash = "1122334455667788";
    let shard = rig
        .shards
        .create_shard(Scope::Project, scope_hash, 0)
        .await
        .expect("create shard");
    let metadata_db = rig
        .connections
        .get_connection(&rig.storage.join("metadata.db"))
        .await
        .expect("metadata connection");
    metadata_db
        .execute(sql::MARK_SHARD_INACTIVE, vec![Value::Integer(shard.id)])
        .await
        .expect("deactivate");
    drop(metadata_db);

    rig.run().await.expect("migration repairs registry");

    let active = rig
        .shards
        .get_active_shard(Scope::Project, scope_hash)
        .await
        .expect("active read")
        .expect("active shard present");
    assert_eq!(active.id, shard.id, "same registry row repaired");
    assert!(active.is_active, "active flag repaired");

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 8/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_migration_lock_retries_three_attempts_then_errors_locked_by_another_process() {
    let rig = rig("legacy-lock", 768, "test-model").await;
    // A foreign holder that is definitely alive (PID 1) and definitely not
    // this process: the lock must NEVER be stolen, only retried, then fail.
    common::write_lock_state(&rig.storage, ".turso-migrate.lock", 1, None);

    let error = rig.run().await.expect_err("live foreign lock blocks");
    assert!(
        error.message().contains("locked by another process"),
        "final error names the lock, got {}",
        error.message()
    );
    assert_eq!(
        rig.sleeper.recorded(),
        vec![
            std::time::Duration::from_millis(200),
            std::time::Duration::from_millis(350),
        ],
        "exactly the upstream between-attempt delays for 3 attempts"
    );
    assert!(
        rig.storage.join(".turso-migrate.lock").exists(),
        "foreign lock file never unlinked"
    );

    cleanup_temp_dir(&rig.storage).expect("cleanup");
}
