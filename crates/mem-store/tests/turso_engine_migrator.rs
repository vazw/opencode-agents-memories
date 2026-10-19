//! Engine-migrator battery (blueprint W1.3 step 4): port of upstream
//! `tests/turso-engine-migrator.test.ts` marker/no-op/skip cases + the
//! loud-refusal case (DIVERGENCE, recorded: the turso 0.8.3 crate cannot
//! parse a DiskANN-indexed file at all, so the upstream in-place rewrite
//! becomes a precise failure — never a marker, never a backup, never a
//! byte touched on the shard).

mod common;

use std::path::Path;

use common::{copy_diskann_fixture, real_components};
use mem_store::db::{Value, query_one};
use mem_store::engine_migrator::{ENGINE_MARKER, run_engine_migration};

/// Recursively collects file names containing `.pre-tursodb-` under `dir`
/// (the upstream rewrite backup prefix — its absence is asserted).
fn collect_pre_tursodb(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return found,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            found.extend(collect_pre_tursodb(&path));
        } else if name.contains(".pre-tursodb-") {
            found.push(name);
        }
    }
    found
}

// ---------------------------------------------------------------------------
// 1/4
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_is_a_no_op_when_the_engine_marker_already_exists() {
    let config = common::store_config("engine-marker-noop");
    let (config, connections, _shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let projects = storage.join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");
    // A candidate that would refuse if it were ever opened: the marker
    // must stop the run BEFORE any probe.
    copy_diskann_fixture(&projects, "project_a1a1a1a1a1a1a1a1_shard_0.db");
    let marker = storage.join(ENGINE_MARKER);
    let body = r#"{"migratedAt":"already"}"#;
    std::fs::write(&marker, body).expect("marker write");

    run_engine_migration(&storage, &connections)
        .await
        .expect("marker present -> no-op");

    assert_eq!(
        std::fs::read_to_string(&marker).expect("marker survives"),
        body,
        "existing marker content untouched"
    );
    assert!(
        collect_pre_tursodb(&storage).is_empty(),
        "no .pre-tursodb-* backups"
    );
}

// ---------------------------------------------------------------------------
// 2/4
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_writes_marker_when_storage_has_no_databases() {
    let config = common::store_config("engine-empty");
    let (config, connections, _shards) = real_components(config).await;
    let storage = config.storage_path.clone();

    run_engine_migration(&storage, &connections)
        .await
        .expect("empty storage marks the engine migrated");

    let body = std::fs::read_to_string(storage.join(ENGINE_MARKER)).expect("marker written");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("marker json");
    let migrated_at = parsed
        .get("migratedAt")
        .and_then(|value| value.as_str())
        .expect("migratedAt string");
    assert!(
        migrated_at.ends_with('Z') && migrated_at.contains('T'),
        "RFC3339 UTC timestamp, got {migrated_at}"
    );
    assert!(
        collect_pre_tursodb(&storage).is_empty(),
        "no .pre-tursodb-* backups"
    );
}

/// Names of every file in `dir` whose name starts with `stem` (a database
/// file plus whatever sidecars an open left behind), sorted.
fn stem_artifacts(dir: &Path, stem: &str) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .expect("read artifacts dir")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(stem))
        .collect();
    found.sort();
    found
}

// ---------------------------------------------------------------------------
// 3/4
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_refuses_diskann_indexed_shard_loudly_without_marker_or_backup() {
    let config = common::store_config("engine-refuse");
    let (config, connections, _shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let projects = storage.join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");
    let shard = copy_diskann_fixture(&projects, "project_b2b2b2b2b2b2b2b2_shard_0.db");

    // Control: the same fixture copy opened through the same
    // ConnectionManager with the same probe sequence, minus the migrator.
    // Opening a libsql-shaped file through the engine performs its own
    // WAL bookkeeping (measured: two header bytes flip, `-wal`/`-tshm`
    // sidecars appear), so the migrator's "never a byte touched" contract
    // is asserted against this open-identical control, not the raw fixture.
    // The control lives INSIDE storage (the manager refuses paths outside
    // `storagePath`) but in a subdirectory the candidate scan (storage
    // root, `users/`, `projects/`) never lists.
    let control_dir = storage.join("open-control");
    std::fs::create_dir_all(&control_dir).expect("control dir");
    let control = copy_diskann_fixture(&control_dir, "project_b2b2b2b2b2b2b2b2_shard_0.db");
    if let Ok(db) = connections.get_connection(&control).await {
        let _ = db.query("SELECT 1", Vec::new()).await;
    }
    connections.close_connection(&control).await;

    let error = run_engine_migration(&storage, &connections)
        .await
        .expect_err("DiskANN shard must refuse loudly");
    let message = error.message();
    assert!(
        message.contains("invalid expression in CREATE INDEX"),
        "refusal carries the turso parse error, got {message}"
    );
    assert!(
        message.contains("libsql_vector_idx"),
        "refusal names the offending index, got {message}"
    );
    assert!(
        message.contains(&shard.to_string_lossy().to_string()),
        "refusal names the shard path, got {message}"
    );
    assert!(
        !storage.join(ENGINE_MARKER).exists(),
        "refusal never writes the marker"
    );
    assert!(
        collect_pre_tursodb(&storage).is_empty(),
        "refusal never creates a .pre-tursodb-* backup"
    );

    // Byte-identity: the refused shard equals the open-identical control.
    let shard_bytes = std::fs::read(&shard).expect("shard read");
    let control_bytes = std::fs::read(&control).expect("control read");
    let first_diff = shard_bytes
        .iter()
        .zip(control_bytes.iter())
        .position(|(left, right)| left != right);
    assert!(
        shard_bytes.len() == control_bytes.len() && first_diff.is_none(),
        "refused shard diverges from open-identical control: lens {} vs {}, \
         first differing offsets {:?} ({} vs {})",
        shard_bytes.len(),
        control_bytes.len(),
        first_diff,
        first_diff.map(|i| shard_bytes[i]).unwrap_or(0),
        first_diff.map(|i| control_bytes[i]).unwrap_or(0),
    );

    // Artifact-identity: the refused shard carries exactly the sidecars an
    // open-identical control carries — the migrator added nothing.
    assert_eq!(
        stem_artifacts(&projects, "project_b2b2b2b2b2b2b2b2_shard_0"),
        stem_artifacts(&control_dir, "project_b2b2b2b2b2b2b2b2_shard_0"),
        "refused shard carries no artifacts beyond an open-identical control"
    );
}

// ---------------------------------------------------------------------------
// 4/4
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_skips_shards_that_already_open_with_the_turso_engine() {
    let config = common::store_config("engine-skip");
    let (config, connections, _shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let projects = storage.join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");
    let shard = projects.join("project_c3c3c3c3c3c3c3c3_shard_0.db");

    // A real turso-engine shard with one row.
    {
        let db = connections
            .get_connection(&shard)
            .await
            .expect("create turso shard");
        db.execute(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT NOT NULL)",
            Vec::new(),
        )
        .await
        .expect("table");
        db.execute(
            "INSERT INTO memories (id, content) VALUES (?, ?)",
            vec![
                Value::Text("mem_turso".to_string()),
                Value::Text("hello".to_string()),
            ],
        )
        .await
        .expect("insert");
    }
    connections.close_connection(&shard).await;

    run_engine_migration(&storage, &connections)
        .await
        .expect("turso-openable shards pass through");

    assert!(
        storage.join(ENGINE_MARKER).exists(),
        "marker written after all candidates opened"
    );
    assert!(
        collect_pre_tursodb(&storage).is_empty(),
        "no .pre-tursodb-* backups"
    );
    let db = connections
        .get_connection(&shard)
        .await
        .expect("reopen after migration");
    let row = query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM memories",
        Vec::new(),
    )
    .await
    .expect("count")
    .expect("row");
    assert_eq!(row.i64("count"), Some(1), "shard content preserved");
}
