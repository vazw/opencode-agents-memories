//! Turso legacy migrator — port of upstream
//! `src/services/turso/legacy-migrator.ts` (blueprint W1.3 step 5).
//!
//! Upstream order (`runLegacyTursoMigration`, L621-672): mkdir storage →
//! `recoverInterruptedReembedSwaps` → `runTursoEngineMigration` (convert
//! DiskANN BEFORE any legacy verification opens a shard with the turso
//! engine, and before the legacy lock) → marker && all-shards-complete →
//! reconcile + return → lock-with-retry → open the auxiliary dbs →
//! migrate `users/` + `projects/` → verify all shards → reconcile the
//! registry → write the marker; the lock is released in all paths.
//!
//! Divergences (recorded):
//! - `CONFIG.*` globals become explicit `storage_path` / `shards.config()`
//!   reads (W1.2 pattern); `process.kill` staleness goes through the
//!   manager's injected [`crate::platform::ProcessProbe`]; retry sleeps go
//!   through the injected [`crate::platform::SleepFn`].
//! - The import transaction (upstream `db.transaction("write", …)` batching
//!   50-row slices) becomes ONE `BatchMode::Write` batch (this port's
//!   transactional `Immediate` form — a larger atomicity window, same
//!   all-or-nothing guarantee); pin updates join the same batch.
//! - Shard-filename parsing keeps the upstream regex semantics
//!   (`^(user|project)_([a-f0-9]{16})_shard_(\d+)\.db$`) via manual
//!   decomposition (no regex dependency).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::connection::ConnectionManager;
use crate::db::{BatchMode, Db, Statement, Value, query_one};
use crate::engine_migrator::run_engine_migration;
use crate::error::StoreError;
use crate::operation_lock::{acquire_legacy_migration_lock_with_retry, now_rfc3339_millis};
use crate::scope::Scope;
use crate::shard_manager::ShardManager;
use crate::sql;
use crate::sqlite_files::{
    FILE_LOCK_RETRY_DELAYS_MS, RetryContext, remove_sqlite_database, rename_sqlite_database,
    with_sqlite_file_lock_retry,
};
use crate::vector::{MemoryInsert, insert_vector_statement};
use crate::vector_utils::{blob_to_float32_array, parse_extracted_vector};

/// Upstream `MIGRATION_MARKER` (L25).
pub const MIGRATION_MARKER: &str = ".turso-migrated";
/// Upstream `SIDECAR_SUFFIX` (L27).
pub const SIDECAR_SUFFIX: &str = ".turso-migrate.json";
/// Upstream `BACKUP_SUFFIX` (L28).
pub const BACKUP_SUFFIX: &str = ".legacy.bak";
/// Upstream `REEMBED_SWAP_SUFFIX` (L29).
pub const REEMBED_SWAP_SUFFIX: &str = ".reembed-swap.json";
/// Upstream `BATCH_SIZE` (L433).
const BATCH_SIZE: usize = 50;
/// Upstream `acquireMigrationLockWithRetry(..., maxAttempts = 3)`.
const LOCK_MAX_ATTEMPTS: u32 = 3;
/// Upstream inline readiness probe (L189; `sql.rs` is frozen, no const).
const VECTOR_EXTRACT_PROBE: &str =
    "SELECT vector_extract(vector) AS extracted FROM memories LIMIT 1";

/// Upstream `ShardMigrationSidecar`. `#[serde(default)]` on the counts the
/// upstream pending fixture omits (`sourceCount`/`skippedCount` absent in
/// `turso-legacy-migrator.test.ts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardMigrationSidecar {
    #[serde(default)]
    pub source_count: u64,
    pub expected_count: u64,
    pub imported_count: u64,
    #[serde(default)]
    pub skipped_count: u64,
    pub status: SidecarStatus,
}

/// Upstream `status: "pending" | "complete"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidecarStatus {
    Pending,
    Complete,
}

/// Upstream `MigrationMarker` (written pretty, `JSON.stringify(_, null, 2)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationMarker {
    completed_at: String,
    shards: Vec<MarkerShard>,
}

/// Upstream marker shard entry `{ path, expectedCount, importedCount }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkerShard {
    path: String,
    expected_count: u64,
    imported_count: u64,
}

fn append_suffix(base: &Path, suffix: &str) -> PathBuf {
    let mut text = base.as_os_str().to_os_string();
    text.push(suffix);
    PathBuf::from(text)
}

/// Upstream `sidecarPath` (L49-51).
fn sidecar_path(db_path: &Path) -> PathBuf {
    append_suffix(db_path, SIDECAR_SUFFIX)
}

/// Upstream `backupPath` (L53-55).
fn backup_path(db_path: &Path) -> PathBuf {
    append_suffix(db_path, BACKUP_SUFFIX)
}

/// Upstream `readSidecar` (L57-65): missing/corrupt → `None`.
fn read_sidecar(db_path: &Path) -> Option<ShardMigrationSidecar> {
    let contents = std::fs::read_to_string(sidecar_path(db_path)).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Upstream `writeSidecar` (L67-69, pretty JSON).
fn write_sidecar(db_path: &Path, sidecar: &ShardMigrationSidecar) -> Result<(), StoreError> {
    let body = serde_json::to_string_pretty(sidecar).map_err(|error| StoreError::Json {
        message: error.to_string(),
    })?;
    std::fs::write(sidecar_path(db_path), body)?;
    Ok(())
}

/// Upstream `readMarker` (L71-84): missing/corrupt/missing-`completedAt` →
/// `None` (upstream validation `parsed.completedAt && Array.isArray(...)`).
fn read_marker(storage_path: &Path) -> Option<MigrationMarker> {
    let contents = std::fs::read_to_string(storage_path.join(MIGRATION_MARKER)).ok()?;
    let parsed: MigrationMarker = serde_json::from_str(&contents).ok()?;
    (!parsed.completed_at.is_empty()).then_some(parsed)
}

/// Upstream `writeMarker` (L86-92).
fn write_marker(storage_path: &Path, shards: Vec<MarkerShard>) -> Result<(), StoreError> {
    let marker = MigrationMarker {
        completed_at: now_rfc3339_millis(),
        shards,
    };
    let body = serde_json::to_string_pretty(&marker).map_err(|error| StoreError::Json {
        message: error.to_string(),
    })?;
    std::fs::write(storage_path.join(MIGRATION_MARKER), body)?;
    Ok(())
}

/// Upstream `countMemories` (L158-161).
async fn count_memories(db: &dyn Db) -> Result<u64, StoreError> {
    let row = query_one(db, sql::COUNT_ALL_VECTORS, Vec::new()).await?;
    Ok(row.and_then(|row| row.i64("count")).unwrap_or(0).max(0) as u64)
}

/// Upstream `hasMemoriesTable` (L163-166).
async fn has_memories_table(db: &dyn Db) -> Result<bool, StoreError> {
    let row = query_one(db, sql::SQLITE_MASTER_MEMORIES_TABLE, Vec::new()).await?;
    Ok(row.is_some())
}

/// Upstream `isTursoVectorShardReady` (L168-196): metadata dims +
/// `vector_extract` length agreement; any failure reads as "not ready".
async fn is_turso_vector_shard_ready(db: &dyn Db) -> bool {
    is_turso_vector_shard_ready_inner(db).await.unwrap_or(false)
}

async fn is_turso_vector_shard_ready_inner(db: &dyn Db) -> Result<bool, StoreError> {
    let meta_row = query_one(
        db,
        sql::GET_SHARD_METADATA_VALUE,
        vec![Value::Text("embedding_dimensions".to_string())],
    )
    .await?;
    let Some(value) = meta_row.as_ref().and_then(|row| row.get("value")) else {
        return Ok(false); // metaRow?.value falsy
    };
    let stored_text = match value {
        Value::Text(text) => text.clone(),
        Value::Integer(number) => number.to_string(),
        _ => return Ok(false),
    };
    let Ok(stored_dimensions) = stored_text.parse::<u32>() else {
        return Ok(false);
    };
    if stored_dimensions == 0 {
        return Ok(false);
    }
    let count = count_memories(db).await?;
    if count == 0 {
        return Ok(true);
    }
    let probe = query_one(db, VECTOR_EXTRACT_PROBE, Vec::new()).await?;
    let Some(row) = probe else {
        return Ok(false);
    };
    let Some(extracted_value) = row.get("extracted") else {
        return Ok(false);
    };
    if extracted_value.is_null() {
        return Ok(false);
    }
    let Some(extracted) = parse_extracted_vector(extracted_value) else {
        return Ok(false);
    };
    Ok(extracted.len() == usize::try_from(stored_dimensions).unwrap_or(usize::MAX))
}

/// Upstream `isShardMigrationComplete` (L198-222): sidecar alone is not
/// enough — the shard must also open, count-match, and read ready.
async fn is_shard_migration_complete(db_path: &Path, connections: &ConnectionManager) -> bool {
    let Some(sidecar) = read_sidecar(db_path) else {
        return false;
    };
    if sidecar.status != SidecarStatus::Complete || !db_path.exists() {
        return false;
    }
    is_shard_migration_complete_inner(db_path, connections, &sidecar)
        .await
        .unwrap_or(false)
}

async fn is_shard_migration_complete_inner(
    db_path: &Path,
    connections: &ConnectionManager,
    sidecar: &ShardMigrationSidecar,
) -> Result<bool, StoreError> {
    let db = connections.get_connection(db_path).await?;
    if !has_memories_table(&*db).await? {
        return Ok(sidecar.expected_count == 0);
    }
    let count = count_memories(&*db).await?;
    if count != sidecar.expected_count {
        return Ok(false);
    }
    Ok(is_turso_vector_shard_ready(&*db).await)
}

/// Upstream `restoreFromBackup` (L224-242).
async fn restore_from_backup(
    db_path: &Path,
    connections: &ConnectionManager,
) -> Result<(), StoreError> {
    let backup = backup_path(db_path);
    if !backup.exists() {
        return Ok(());
    }
    connections.close_connection(db_path).await?;
    if db_path.exists() {
        let Some(proof) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot {
                db_path: db_path.to_path_buf(),
            });
        };
        let claim = proof.claim(db_path)?;
        with_sqlite_file_lock_retry(
            || remove_sqlite_database(claim),
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &RetryContext::system(),
        )?;
    }
    let from = backup.clone();
    let to = db_path.to_path_buf();
    let Some(proof) = connections.exclusive_root() else {
        return Err(StoreError::NoExclusiveRoot { db_path: to });
    };
    let from_claim = proof.claim(&from)?;
    let to_claim = proof.claim(&to)?;
    with_sqlite_file_lock_retry(
        || rename_sqlite_database(from_claim, to_claim),
        FILE_LOCK_RETRY_DELAYS_MS.len(),
        &RetryContext::system(),
    )?;
    let sidecar = sidecar_path(db_path);
    if sidecar.exists() {
        std::fs::remove_file(&sidecar)?;
    }
    tracing::info!(db_path = %db_path.display(), backup = %backup.display(), "Legacy migration restored shard from backup");
    Ok(())
}

/// Upstream `shouldRestoreFromBackup` (L244-289); any probe failure restores.
async fn should_restore_from_backup(db_path: &Path, connections: &ConnectionManager) -> bool {
    let backup = backup_path(db_path);
    if !backup.exists() {
        return false;
    }
    if !db_path.exists() {
        return true;
    }
    should_restore_from_backup_inner(db_path, connections)
        .await
        .unwrap_or(true)
}

async fn should_restore_from_backup_inner(
    db_path: &Path,
    connections: &ConnectionManager,
) -> Result<bool, StoreError> {
    let db = connections.get_connection(db_path).await?;
    let sidecar = read_sidecar(db_path);
    // A healthy Turso vector shard must not be overwritten by .legacy.bak,
    // except when a pending rewrite clearly did not finish importing all rows.
    if has_memories_table(&*db).await? && is_turso_vector_shard_ready(&*db).await {
        if let Some(sidecar) = sidecar.filter(|s| s.status == SidecarStatus::Pending) {
            let count = count_memories(&*db).await?;
            if count < sidecar.expected_count {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if sidecar
        .as_ref()
        .is_some_and(|s| s.status == SidecarStatus::Complete)
    {
        return Ok(!is_shard_migration_complete(db_path, connections).await);
    }
    if !has_memories_table(&*db).await? {
        return Ok(true);
    }
    let count = count_memories(&*db).await?;
    if let Some(expected) = sidecar.as_ref().map(|s| s.expected_count)
        && count < expected
    {
        return Ok(true);
    }
    if sidecar
        .as_ref()
        .is_some_and(|s| s.status == SidecarStatus::Pending)
    {
        return Ok(true);
    }
    Ok(false)
}

/// Upstream `rowToRecord` (L291-315): `None` when the vector is unreadable.
/// The upstream record carries no `sessionId` (legacy schema has none);
/// `insert_vector_statement` still resolves it from metadata, matching the
/// upstream insert path.
fn row_to_record(row: &crate::db::Row) -> Option<MemoryInsert> {
    let vector = blob_to_float32_array(row.get("vector")?)?;
    let tags_vector = row.get("tags_vector").and_then(blob_to_float32_array);
    Some(MemoryInsert {
        id: row.text("id").unwrap_or_default(),
        content: row.text("content").unwrap_or_default(),
        vector,
        tags_vector,
        container_tag: row.text("container_tag").unwrap_or_default(),
        tags: truthy_text(row, "tags"),
        memory_type: truthy_text(row, "type"),
        created_at: row.i64("created_at").unwrap_or(0),
        updated_at: row.i64("updated_at").unwrap_or(0),
        metadata: truthy_text(row, "metadata"),
        session_id: None,
        display_name: truthy_text(row, "display_name"),
        user_name: truthy_text(row, "user_name"),
        user_email: truthy_text(row, "user_email"),
        project_path: truthy_text(row, "project_path"),
        project_name: truthy_text(row, "project_name"),
        git_repo_url: truthy_text(row, "git_repo_url"),
    })
}

/// `row.col ? String(row.col) : undefined` — empty strings are falsy upstream.
fn truthy_text(row: &crate::db::Row, column: &str) -> Option<String> {
    row.text(column).filter(|text| !text.is_empty())
}

/// Upstream `migrateMemoryShard` (L317-472).
async fn migrate_memory_shard(
    db_path: &Path,
    connections: &ConnectionManager,
    shards: &ShardManager,
) -> Result<Option<ShardMigrationSidecar>, StoreError> {
    if is_shard_migration_complete(db_path, connections).await {
        return Ok(read_sidecar(db_path));
    }
    if should_restore_from_backup(db_path, connections).await {
        restore_from_backup(db_path, connections).await?;
    }
    if is_shard_migration_complete(db_path, connections).await {
        return Ok(read_sidecar(db_path));
    }

    let db = connections.get_connection(db_path).await?;
    let config = shards.config();

    if !has_memories_table(&*db).await? {
        shards
            .init_shard_db(&*db, config.embedding_dimensions, &config.embedding_model)
            .await?;
        let sidecar = zero_sidecar();
        write_sidecar(db_path, &sidecar)?;
        return Ok(Some(sidecar));
    }

    if is_turso_vector_shard_ready(&*db).await {
        let count = count_memories(&*db).await?;
        let sidecar = ShardMigrationSidecar {
            source_count: count,
            expected_count: count,
            imported_count: count,
            skipped_count: 0,
            status: SidecarStatus::Complete,
        };
        write_sidecar(db_path, &sidecar)?;
        return Ok(Some(sidecar));
    }

    let rows = db.query(sql::GET_MEMORIES_UNORDERED, Vec::new()).await?;
    let mut records: Vec<MemoryInsert> = Vec::new();
    let mut pinned_ids: Vec<String> = Vec::new();
    let mut skipped_ids: Vec<String> = Vec::new();
    for row in &rows {
        match row_to_record(row) {
            Some(record) => {
                if row.i64("is_pinned") == Some(1) {
                    pinned_ids.push(record.id.clone());
                }
                records.push(record);
            }
            None => {
                let id = row.text("id").unwrap_or_default();
                skipped_ids.push(id);
                tracing::warn!(memory_id = %row.text("id").unwrap_or_default(), db_path = %db_path.display(), "Legacy migration skipped memory without vector");
            }
        }
    }

    let source_count = rows.len() as u64;
    let skipped_count = skipped_ids.len() as u64;
    if skipped_count > 0 {
        // Abort BEFORE any backup rename or sidecar write: the original
        // shard stays byte-intact (pinned by the corrupt-blob test).
        return Err(StoreError::internal(format!(
            "Legacy migration aborted for {db}: {skipped_count} of {source_count} memories have \
             unreadable vectors ({skipped})",
            db = db_path.display(),
            skipped = skipped_ids.join(", "),
        )));
    }

    let source_dimensions = records
        .first()
        .map(|record| record.vector.len())
        .unwrap_or(config.embedding_dimensions as usize);
    if !(1..=65536).contains(&source_dimensions) {
        return Err(StoreError::internal(format!(
            "Legacy migration aborted for {}: invalid source vector dimensions {source_dimensions}",
            db_path.display()
        )));
    }
    let dimension_mismatches: Vec<&MemoryInsert> = records
        .iter()
        .filter(|record| {
            record.vector.len() != source_dimensions
                || record
                    .tags_vector
                    .as_ref()
                    .is_some_and(|tags| tags.len() != source_dimensions)
        })
        .collect();
    if !dimension_mismatches.is_empty() {
        let sample = dimension_mismatches
            .iter()
            .take(5)
            .map(|record| {
                let tags = record
                    .tags_vector
                    .as_ref()
                    .map(|tags| format!(",tags={}", tags.len()))
                    .unwrap_or_default();
                format!("{}(vector={}{tags})", record.id, record.vector.len())
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(StoreError::internal(format!(
            "Legacy migration aborted for {db}: {count} memories have inconsistent vector \
             dimensions; expected {source_dimensions} ({sample})",
            db = db_path.display(),
            count = dimension_mismatches.len(),
        )));
    }

    let expected_count = records.len() as u64;
    write_sidecar(
        db_path,
        &ShardMigrationSidecar {
            source_count,
            expected_count,
            imported_count: 0,
            skipped_count,
            status: SidecarStatus::Pending,
        },
    )?;

    let backup = backup_path(db_path);
    drop(db);
    connections.close_connection(db_path).await?;
    if db_path.exists() {
        let from = db_path.to_path_buf();
        let to = backup.clone();
        let Some(proof) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot { db_path: from });
        };
        let from_claim = proof.claim(&from)?;
        let to_claim = proof.claim(&to)?;
        with_sqlite_file_lock_retry(
            || rename_sqlite_database(from_claim, to_claim),
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &RetryContext::system(),
        )?;
    }

    let fresh_db = connections.get_connection(db_path).await?;
    let source_model = if source_dimensions == config.embedding_dimensions as usize {
        config.embedding_model.as_str()
    } else {
        "legacy-unknown"
    };
    shards
        .init_shard_db(
            &*fresh_db,
            u32::try_from(source_dimensions).unwrap_or(u32::MAX),
            source_model,
        )
        .await?;

    // Divergence (recorded): ONE transactional batch (inserts in
    // BATCH_SIZE groups, pins last) instead of upstream's chunked
    // `transaction("write")` loops.
    let mut statements: Vec<Statement> = Vec::with_capacity(records.len() + pinned_ids.len());
    for chunk in records.chunks(BATCH_SIZE) {
        for record in chunk {
            statements.push(insert_vector_statement(record)?);
        }
    }
    for memory_id in pinned_ids {
        statements.push(Statement::new(
            sql::PIN_MEMORY,
            vec![Value::Text(memory_id)],
        ));
    }
    fresh_db.batch(statements, BatchMode::Write).await?;

    let imported_count = count_memories(&*fresh_db).await?;
    if imported_count != expected_count {
        return Err(StoreError::internal(format!(
            "Legacy migration count mismatch for {db}: expected {expected_count}, got {imported_count}",
            db = db_path.display(),
        )));
    }

    let completed = ShardMigrationSidecar {
        source_count,
        expected_count,
        imported_count,
        skipped_count,
        status: SidecarStatus::Complete,
    };
    write_sidecar(db_path, &completed)?;
    tracing::info!(db_path = %db_path.display(), memories = expected_count, backup = %backup.display(), "Legacy memory shard migrated to Turso vectors");
    Ok(Some(completed))
}

fn zero_sidecar() -> ShardMigrationSidecar {
    ShardMigrationSidecar {
        source_count: 0,
        expected_count: 0,
        imported_count: 0,
        skipped_count: 0,
        status: SidecarStatus::Complete,
    }
}

/// Upstream `readdirSync` scans (L481-488, L493-504): `.db` files excluding
/// any name containing the backup suffix; sorted for determinism (recorded
/// divergence, engine-migrator pattern).
fn list_db_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".db") && !name.contains(BACKUP_SUFFIX))
        .collect();
    names.sort();
    names.into_iter().map(|name| dir.join(name)).collect()
}

/// Upstream `listAllShardDbPaths` (L493-504).
fn list_all_shard_db_paths(storage_path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir_name in ["users", "projects"] {
        paths.extend(list_db_files(&storage_path.join(dir_name)));
    }
    paths
}

/// Upstream `allShardsComplete` (L506-515).
async fn all_shards_complete(
    storage_path: &Path,
    connections: &ConnectionManager,
) -> Result<bool, StoreError> {
    let paths = list_all_shard_db_paths(storage_path);
    if paths.is_empty() {
        return Ok(true);
    }
    for db_path in paths {
        if !is_shard_migration_complete(&db_path, connections).await {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Upstream swap-state shape (L527-531).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReembedSwapState {
    #[serde(default)]
    db_path: Option<String>,
    #[serde(default)]
    staged_path: Option<String>,
    #[serde(default)]
    backup_path: Option<String>,
}

/// Upstream `recoverInterruptedReembedSwaps` (L517-574).
async fn recover_interrupted_reembed_swaps(
    storage_path: &Path,
    connections: &Arc<ConnectionManager>,
) -> Result<(), StoreError> {
    for dir_name in ["users", "projects"] {
        let dir = storage_path.join(dir_name);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(REEMBED_SWAP_SUFFIX))
            .collect();
        names.sort();
        for name in names {
            let state_path = dir.join(name);
            recover_one_swap(&state_path, connections)
                .await
                .map_err(|error| {
                    tracing::error!(state_path = %state_path.display(), error = %error.message(), "Re-embed swap recovery failed");
                    StoreError::internal(format!(
                        "Failed to recover re-embed swap {}: {}",
                        state_path.display(),
                        error.message()
                    ))
                })?;
        }
    }
    Ok(())
}

async fn recover_one_swap(
    state_path: &Path,
    connections: &Arc<ConnectionManager>,
) -> Result<(), StoreError> {
    let state_text = state_path.to_string_lossy().to_string();
    let Some(expected_db) = state_text
        .strip_suffix(REEMBED_SWAP_SUFFIX)
        .map(PathBuf::from)
    else {
        return Err(StoreError::internal("invalid re-embed swap state paths"));
    };
    let body = std::fs::read_to_string(state_path)?;
    let state: ReembedSwapState = serde_json::from_str(&body).map_err(|error| {
        StoreError::internal(format!("invalid re-embed swap state paths: {error}"))
    })?;
    let staged = state.staged_path.filter(|staged| {
        staged.starts_with(&format!("{}.reembed-", expected_db.display()))
            && staged.ends_with(".tmp")
    });
    let backup = state.backup_path.filter(|backup| {
        backup.starts_with(&format!("{}.pre-reembed-", expected_db.display()))
            && backup.ends_with(".bak")
    });
    if state.db_path.as_deref() != Some(expected_db.to_string_lossy().as_ref()) {
        return Err(StoreError::internal("invalid re-embed swap state paths"));
    }
    // the filtered pair is consumed once instead of re-asserted after an is_none guard (ADR-0013 D-3)
    let (Some(staged), Some(backup)) = (staged, backup) else {
        return Err(StoreError::internal("invalid re-embed swap state paths"));
    };
    let staged = PathBuf::from(staged);
    let backup = PathBuf::from(backup);

    if expected_db.exists() {
        if staged.exists() {
            let Some(proof) = connections.exclusive_root() else {
                return Err(StoreError::NoExclusiveRoot {
                    db_path: staged.clone(),
                });
            };
            let claim = proof.claim(&staged)?;
            with_sqlite_file_lock_retry(
                || remove_sqlite_database(claim),
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &RetryContext::system(),
            )?;
        }
    } else if staged.exists() {
        // Target is absent after a crash mid-swap; the retry covers Windows
        // EPERM while native handles release (engine/encryption pattern).
        let Some(proof) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot {
                db_path: staged.clone(),
            });
        };
        let expected_claim = proof.claim(&expected_db)?;
        let expected = expected_db.clone();
        let staged_claim = proof.claim(&staged)?;
        with_sqlite_file_lock_retry(
            || -> Result<(), StoreError> {
                if expected.exists() {
                    remove_sqlite_database(expected_claim)?;
                }
                rename_sqlite_database(staged_claim, expected_claim)?;
                Ok(())
            },
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &RetryContext::system(),
        )?;
    } else if backup.exists() {
        let Some(proof) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot {
                db_path: backup.clone(),
            });
        };
        let expected_claim = proof.claim(&expected_db)?;
        let expected = expected_db.clone();
        let backup_for_rename = backup.clone();
        let backup_claim = proof.claim(&backup_for_rename)?;
        with_sqlite_file_lock_retry(
            || -> Result<(), StoreError> {
                if expected.exists() {
                    remove_sqlite_database(expected_claim)?;
                }
                rename_sqlite_database(backup_claim, expected_claim)?;
                Ok(())
            },
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &RetryContext::system(),
        )?;
    } else {
        return Err(StoreError::internal(
            "neither staged replacement nor source backup exists",
        ));
    }
    std::fs::remove_file(state_path)?;
    tracing::info!(db_path = %expected_db.display(), "Recovered interrupted re-embed shard swap");
    Ok(())
}

/// Upstream shard-filename regex `^(user|project)_([a-f0-9]{16})_shard_(\d+)\.db$`
/// via manual decomposition (no regex dependency, same accept set).
fn parse_shard_file_name(name: &str) -> Option<(Scope, String, u32)> {
    let stem = name.strip_suffix(".db")?;
    let (prefix, index) = stem.rsplit_once("_shard_")?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let shard_index: u32 = index.parse().ok()?;
    let (scope, hash) = prefix.split_once('_')?;
    let scope = match scope {
        "user" => Scope::User,
        "project" => Scope::Project,
        _ => return None,
    };
    if hash.len() != 16
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    Some((scope, hash.to_string(), shard_index))
}

/// Upstream `reconcileShardRegistry` (L576-619): re-register every parsed
/// shard file, the highest shard index per scope keyed active.
async fn reconcile_shard_registry(
    storage_path: &Path,
    connections: &ConnectionManager,
    shards: &ShardManager,
) -> Result<(), StoreError> {
    struct ParsedShard {
        path: PathBuf,
        scope: Scope,
        scope_hash: String,
        shard_index: u32,
        vector_count: u64,
    }
    let mut parsed_shards: Vec<ParsedShard> = Vec::new();
    for path in list_all_shard_db_paths(storage_path) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        let Some((scope, scope_hash, shard_index)) =
            name.as_deref().and_then(parse_shard_file_name)
        else {
            tracing::warn!(path = %path.display(), "Legacy migration left unrecognized shard filename unregistered");
            continue;
        };
        let db = connections.get_connection(&path).await?;
        parsed_shards.push(ParsedShard {
            path,
            scope,
            scope_hash,
            shard_index,
            vector_count: count_memories(&*db).await?,
        });
    }
    let mut highest_index_by_scope = std::collections::BTreeMap::new();
    for shard in &parsed_shards {
        let key = (shard.scope.as_str().to_string(), shard.scope_hash.clone());
        let entry = highest_index_by_scope
            .entry(key)
            .or_insert(i64::from(shard.shard_index));
        *entry = (*entry).max(i64::from(shard.shard_index));
    }
    for shard in &parsed_shards {
        let key = (shard.scope.as_str().to_string(), shard.scope_hash.clone());
        // the highest-index map is keyed from this same loop, so a missing key is an internal defect, not a panic (ADR-0013 D-3)
        let is_active = i64::from(shard.shard_index)
            == *highest_index_by_scope.get(&key).ok_or_else(|| {
                StoreError::internal("shard scope missing from the highest-index map")
            })?;
        shards
            .register_existing_shard(
                shard.scope,
                &shard.scope_hash,
                shard.shard_index,
                &shard.path,
                shard.vector_count,
                is_active,
            )
            .await?;
    }
    Ok(())
}

/// Upstream `migrateDirectory` (L474-491).
async fn migrate_directory(
    storage_path: &Path,
    dir_name: &str,
    connections: &ConnectionManager,
    shards: &ShardManager,
) -> Result<Vec<MarkerShard>, StoreError> {
    let mut results = Vec::new();
    for db_path in list_db_files(&storage_path.join(dir_name)) {
        if let Some(sidecar) = migrate_memory_shard(&db_path, connections, shards).await? {
            results.push(MarkerShard {
                path: db_path.to_string_lossy().to_string(),
                expected_count: sidecar.expected_count,
                imported_count: sidecar.imported_count,
            });
        }
    }
    Ok(results)
}

/// Upstream `runLegacyTursoMigration` (L621-672).
pub async fn run_legacy_turso_migration(
    storage_path: &Path,
    shards: &Arc<ShardManager>,
    connections: &Arc<ConnectionManager>,
    sleep: &crate::platform::SleepFn,
) -> Result<(), StoreError> {
    if !storage_path.exists() {
        std::fs::create_dir_all(storage_path)?;
    }

    // 46.5: the recovery mutates shard files, so it runs under the
    // legacy-reembed-recover operation lock — acquired immediately before
    // the recovery and released immediately after, before the engine
    // conversion and long before the legacy lock. One lock file is shared by
    // every operation name; a live holder (any name) refuses the recovery
    // before its first unlink.
    let recovery_lock = crate::operation_lock::acquire_turso_operation_lock(
        storage_path,
        shards.process_probe(),
        "legacy-reembed-recover",
    )?;
    let recovery = recover_interrupted_reembed_swaps(storage_path, connections).await;
    recovery_lock.release();
    recovery?;

    // Recovery may put a libSQL shard back at its active path. Convert DiskANN
    // indexes before any legacy verification opens it with the Turso engine,
    // and before taking the legacy lock that would block the engine migration.
    run_engine_migration(storage_path, connections).await?;

    if read_marker(storage_path).is_some() && all_shards_complete(storage_path, connections).await?
    {
        reconcile_shard_registry(storage_path, connections, shards).await?;
        return Ok(());
    }

    let lock = acquire_legacy_migration_lock_with_retry(
        storage_path,
        shards.process_probe(),
        sleep,
        LOCK_MAX_ATTEMPTS,
    )?;

    let outcome = migrate_storage(storage_path, connections, shards).await;
    // Upstream releases in `finally`; the Drop fallback covers unwinding.
    lock.release();
    outcome
}

async fn migrate_storage(
    storage_path: &Path,
    connections: &ConnectionManager,
    shards: &ShardManager,
) -> Result<(), StoreError> {
    for file in [
        "metadata.db",
        "user-prompts.db",
        "user-profiles.db",
        "ai-sessions.db",
    ] {
        let path = storage_path.join(file);
        if !path.exists() {
            continue;
        }
        connections.get_connection(&path).await?;
    }

    let mut migrated_shards: Vec<MarkerShard> = Vec::new();
    for dir_name in ["users", "projects"] {
        migrated_shards
            .extend(migrate_directory(storage_path, dir_name, connections, shards).await?);
    }

    if !all_shards_complete(storage_path, connections).await? {
        return Err(StoreError::internal(
            "Turso legacy migration incomplete: not all shards verified",
        ));
    }

    reconcile_shard_registry(storage_path, connections, shards).await?;
    write_marker(storage_path, migrated_shards)?;
    tracing::info!(storage_path = %storage_path.display(), "Turso legacy migration complete");
    Ok(())
}
