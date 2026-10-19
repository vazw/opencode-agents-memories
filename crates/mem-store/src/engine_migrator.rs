//! Turso engine migrator — port of upstream
//! `src/services/turso/engine-migrator.ts` detection/marker half
//! (blueprint W1.3 step 4).
//!
//! Upstream `runTursoEngineMigration` (L275-318): marker present → return;
//! collect candidate shards (storage root + `users/` + `projects/`,
//! `listDbFiles` L40-45); zero candidates → write the marker; otherwise
//! classify every shard through `needsEngineRewrite` (L56-93) and rewrite
//! the ones the turso engine cannot read.
//!
//! RECORDED DIVERGENCE (blueprint W1.3, live experiment): the `turso`
//! 0.8.3 crate cannot even parse a DiskANN-indexed file —
//! `Parse error: Error: invalid expression in CREATE INDEX:
//! libsql_vector_idx (vector)`. libsql/rusqlite readers are disallowed by
//! roadmap §0b D1/D3, so the rewrite half is replaced by a LOUD REFUSAL:
//! the precise [`StoreError::Internal`] names the path, the engine error
//! and the fix path, and the run NEVER writes the marker, NEVER takes the
//! operation lock, NEVER creates a `.pre-tursodb-` backup, and never
//! modifies a file byte. Upstream's libsql-probe fallback in
//! `needsEngineRewrite` (L78-91) is N-A for the same reason: an unknown
//! open failure classifies as "no rewrite needed".

use std::path::{Path, PathBuf};

use crate::connection::ConnectionManager;
use crate::error::StoreError;
use crate::operation_lock::now_rfc3339_millis;

/// Upstream `ENGINE_MARKER` (L24).
pub const ENGINE_MARKER: &str = ".tursodb-engine-v1";

/// Upstream `markerPath` (L36-38) with the global `CONFIG.storagePath`
/// becoming the explicit argument (W1.2 divergence pattern).
fn marker_path(storage_path: &Path) -> PathBuf {
    storage_path.join(ENGINE_MARKER)
}

/// Upstream `listDbFiles` (L40-45): `.db` names excluding any name
/// containing `.bak` or `.tmp`; missing directories list empty.
/// (Determinism divergence: entries are sorted so the candidate order is
/// stable across filesystems.)
fn list_db_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".db") && !name.contains(".bak") && !name.contains(".tmp"))
        .collect();
    names.sort();
    names.into_iter().map(|name| dir.join(name)).collect()
}

/// Upstream `collectCandidateDbs` (L47-54).
fn collect_candidate_dbs(storage_path: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for dir in [
        storage_path.to_path_buf(),
        storage_path.join("users"),
        storage_path.join("projects"),
    ] {
        candidates.extend(list_db_files(&dir));
    }
    candidates
}

/// Upstream `needsEngineRewrite` classifier (L66-77): the two DiskANN
/// signatures need (would need) a rewrite; encryption-mismatch failures
/// are handled elsewhere; everything else is unknown → no rewrite
/// (divergence: upstream's libsql probe fallback L78-91 is N-A).
fn classify_open_failure(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("libsql_vector_idx") || lowered.contains("invalid expression in create index")
}

/// Open the shard through the manager and probe it (`SELECT 1`, upstream
/// L59-62), closing the probe connection afterwards (upstream `finally
/// db.close()` — a cached handle must never outlive the probe across the
/// legacy migration's renames). `Ok(())` = readable by the turso engine;
/// `Err(engine error text)` = the open/probe failure message.
async fn probe_shard(db_path: &Path, connections: &ConnectionManager) -> Result<(), String> {
    match connections.get_connection(db_path).await {
        Ok(db) => {
            let probed = db.query("SELECT 1", Vec::new()).await;
            connections.close_connection(db_path).await;
            probed.map(|_| ()).map_err(|error| error.message())
        }
        Err(error) => Err(error.message()),
    }
}

/// Upstream `needsEngineRewrite` (L56-93); `Some(engine error text)` when
/// the shard uses libSQL DiskANN indexes the pure-turso engine cannot
/// parse.
async fn needs_engine_rewrite(db_path: &Path, connections: &ConnectionManager) -> Option<String> {
    match probe_shard(db_path, connections).await {
        Ok(()) => None,
        Err(message) => classify_open_failure(&message).then_some(message),
    }
}

/// Upstream marker write (`JSON.stringify({ migratedAt: ISO })`).
fn write_marker(marker: &Path) -> Result<(), StoreError> {
    let body = serde_json::json!({ "migratedAt": now_rfc3339_millis() }).to_string();
    std::fs::write(marker, body)?;
    Ok(())
}

/// Upstream `runTursoEngineMigration` (L275-318), refusal half (see
/// module divergences): marker/no-op, empty-storage marker, per-candidate
/// classification, and a loud `Internal` failure on any DiskANN shard
/// before any marker/backup/byte mutation.
pub async fn run_engine_migration(
    storage_path: &Path,
    connections: &ConnectionManager,
) -> Result<(), StoreError> {
    let marker = marker_path(storage_path);
    if marker.exists() {
        return Ok(());
    }
    let candidates = collect_candidate_dbs(storage_path);
    if candidates.is_empty() {
        return write_marker(&marker);
    }
    for candidate in &candidates {
        if let Some(engine_error) = needs_engine_rewrite(candidate, connections).await {
            return Err(StoreError::internal(format!(
                "Turso engine migration cannot rewrite {candidate}: this shard uses libSQL \
                 DiskANN indexes ({engine_error}). Convert it with a libsql-capable tool \
                 before using this storage; the in-place DiskANN rewrite is not available \
                 in the pure-turso engine.",
                candidate = candidate.display()
            )));
        }
    }
    write_marker(&marker)
}
