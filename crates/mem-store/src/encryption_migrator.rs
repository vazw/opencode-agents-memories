//! Plaintext → encrypted shard migrator — port of upstream
//! `src/services/turso/encryption-migrator.ts` (blueprint W1.5 step 4, lane
//! sE).
//!
//! Scaffold stub created by the captain (step 2); lane sE fills it RED-first.
//! Public API (pinned by the blueprint): `ENCRYPT_MARKER`, `marker_path`,
//! `list_db_files`, `collect_candidate_dbs`, `is_already_encrypted`,
//! `can_open_plain`, `dump_and_reload_encrypted`,
//! `run_database_encryption_migration`, `encryption_migration_step`. The
//! ready-gate builder `build_ready_gate_with_encryption` lives in
//! [`crate::ready`] (the composition locus).
//!
//! DIVERGENCE (recorded): upstream reads the process-global `CONFIG` here
//! (`markerPath()`, `collectCandidateDbs()`); this port threads the storage
//! root explicitly. Like upstream, `run_database_encryption_migration` always
//! resets the shard manager and releases the operation lock (its `finally`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::StoreConfig;
use crate::connection::ConnectionManager;
use crate::db::{ConnectFactory, ConnectOptions, DbHandle, EncryptionOptions, Row, Value};
use crate::error::StoreError;
use crate::platform::Platform;
use crate::ready::ReadyStep;
use crate::shard_manager::ShardManager;

/// Upstream `ENCRYPT_MARKER`.
pub const ENCRYPT_MARKER: &str = ".tursodb-encrypted-v1";

/// Upstream `markerPath()` rooted at the caller's storage path.
pub fn marker_path(storage_path: &Path) -> PathBuf {
    crate::encryption_key::encryption_marker_path(storage_path)
}

/// Upstream `listDbFiles`: `*.db` entries, skipping `.bak`/`.tmp` artifacts
/// (a missing directory is empty, never an error).
pub fn list_db_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".db") && !name.contains(".bak") && !name.contains(".tmp") {
            files.push(entry.path());
        }
    }
    files
}

/// Upstream `collectCandidateDbs`: the storage root plus `users/` and
/// `projects/`.
pub fn collect_candidate_dbs(storage_path: &Path) -> Vec<PathBuf> {
    let mut candidates = list_db_files(storage_path);
    candidates.extend(list_db_files(&storage_path.join("users")));
    candidates.extend(list_db_files(&storage_path.join("projects")));
    candidates
}

/// The one classifier both probes use: a guard refusal is a **diagnosis** and
/// must reach the caller; any other open failure is "not openable with these
/// options", which is the caller's decision; a successful open is `true` (and the
/// handle is dropped here — a probe holds nothing).
///
/// Without the first arm a guard refusal is re-labelled "Cannot open … as
/// plaintext for encryption migration", which is the mis-diagnosis issue 45's
/// `45.1` row 4 flags.
fn probe_opened(result: Result<DbHandle, StoreError>) -> Result<bool, StoreError> {
    match result {
        Ok(handle) => {
            drop(handle);
            Ok(true)
        }
        Err(error) if error.is_open_guard_refusal() => Err(error),
        Err(_) => Ok(false),
    }
}

/// `true` when the shard opens with the configured encryption options (i.e. it
/// is already encrypted with the active key). A guard refusal is `Err`.
pub async fn is_already_encrypted(
    db_path: &Path,
    config: &StoreConfig,
) -> Result<bool, StoreError> {
    // Use the non-provisioning resolver so probing candidate shards never
    // mints a key file as a side effect.
    let Some(hexkey) = crate::encryption_key::resolve_database_encryption_key_if_present(config)
    else {
        return Ok(false);
    };
    let mut options = ConnectOptions::for_config(config, Platform::current());
    options.encryption = Some(EncryptionOptions {
        cipher: config.database_encryption_cipher.clone(),
        hexkey,
    });
    probe_opened(
        crate::db::turso_connect_factory()
            .connect(db_path.to_path_buf(), options)
            .await,
    )
}

/// `true` when the shard still opens as plaintext (the migration precondition).
/// A guard refusal is `Err`.
pub async fn can_open_plain(db_path: &Path, config: &StoreConfig) -> Result<bool, StoreError> {
    let mut options = ConnectOptions::for_config(config, Platform::current());
    options.encryption = None;
    probe_opened(
        crate::db::turso_connect_factory()
            .connect(db_path.to_path_buf(), options)
            .await,
    )
}

// ---------------------------------------------------------------------------
// Table copy (upstream `dumpAndReloadEncrypted` body)
// ---------------------------------------------------------------------------

/// Upstream table scan: skips sqlite/turso-internal/FTS/virtual tables.
const TABLE_LIST_SQL: &str = "SELECT name FROM sqlite_master \
    WHERE type = 'table' \
      AND name NOT LIKE 'sqlite_%' \
      AND name NOT LIKE '__turso_internal_%' \
      AND name NOT LIKE '%_fts%' \
      AND sql NOT LIKE 'CREATE VIRTUAL TABLE%'";

/// Upstream per-table DDL lookup.
const TABLE_DDL_SQL: &str = "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?";

/// Upstream index scan (recreated after the tables).
const INDEX_LIST_SQL: &str = "SELECT sql FROM sqlite_master \
    WHERE type = 'index' \
      AND sql IS NOT NULL \
      AND name NOT LIKE 'sqlite_%' \
      AND name NOT LIKE '__turso_internal_%'";

/// Upstream `memories` read (vectors are extracted to JSON text so the
/// staged DB can re-encode them with `vector32`). No `session_id` — the
/// ready-gate schema migration backfills it later, exactly like upstream.
const MEMORIES_SELECT_SQL: &str = "SELECT \
    id, content, \
    vector_extract(vector) AS vector, \
    CASE WHEN tags_vector IS NOT NULL THEN vector_extract(tags_vector) ELSE NULL END AS tags_vector, \
    container_tag, tags, type, created_at, updated_at, metadata, \
    display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned \
    FROM memories";

/// Upstream `memories` insert for rows without a tags vector.
const MEMORIES_INSERT_WITHOUT_TAGS_SQL: &str = "INSERT OR REPLACE INTO memories (\
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at, \
    metadata, display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned \
    ) VALUES (?, ?, vector32(?), NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

/// Upstream `memories` insert for rows carrying a tags vector.
const MEMORIES_INSERT_WITH_TAGS_SQL: &str = "INSERT OR REPLACE INTO memories (\
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at, \
    metadata, display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned \
    ) VALUES (?, ?, vector32(?), vector32(?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

fn value_or_null(row: &Row, column: &str) -> Value {
    row.get(column).cloned().unwrap_or(Value::Null)
}

/// Upstream `asSqlValue(row.is_pinned ?? 0)`.
fn int_or_zero(row: &Row, column: &str) -> Value {
    match row.get(column) {
        Some(Value::Null) | None => Value::Integer(0),
        Some(value) => value.clone(),
    }
}

/// The upstream `memories` binding order (vector as extracted JSON text).
fn memory_params(row: &Row) -> Vec<Value> {
    vec![
        value_or_null(row, "id"),
        value_or_null(row, "content"),
        Value::Text(row.text("vector").unwrap_or_default()),
        value_or_null(row, "container_tag"),
        value_or_null(row, "tags"),
        value_or_null(row, "type"),
        value_or_null(row, "created_at"),
        value_or_null(row, "updated_at"),
        value_or_null(row, "metadata"),
        value_or_null(row, "display_name"),
        value_or_null(row, "user_name"),
        value_or_null(row, "user_email"),
        value_or_null(row, "project_path"),
        value_or_null(row, "project_name"),
        value_or_null(row, "git_repo_url"),
        int_or_zero(row, "is_pinned"),
    ]
}

async fn copy_memories(
    source: &dyn crate::db::Db,
    staged: &dyn crate::db::Db,
) -> Result<(), StoreError> {
    let rows = source.query(MEMORIES_SELECT_SQL, Vec::new()).await?;
    for row in rows {
        if row.is_null("tags_vector") {
            staged
                .execute(MEMORIES_INSERT_WITHOUT_TAGS_SQL, memory_params(&row))
                .await?;
        } else {
            let mut params = memory_params(&row);
            // `tags_vector` follows `vector` in the upstream column list.
            params.insert(3, Value::Text(row.text("tags_vector").unwrap_or_default()));
            staged
                .execute(MEMORIES_INSERT_WITH_TAGS_SQL, params)
                .await?;
        }
    }
    Ok(())
}

/// Quote a SQL identifier per SQLite rules: wrap in double quotes and double
/// any embedded double quote. Prevents breakage/injection on crafted or
/// quoting-needy table/column names.
fn quote_identifier(identifier: &str) -> String {
    let mut out = String::from("\"");
    for ch in identifier.chars() {
        if ch == '"' {
            out.push('"');
            out.push('"');
        } else {
            out.push(ch);
        }
    }
    out.push('"');
    out
}

async fn copy_generic_table(
    source: &dyn crate::db::Db,
    staged: &dyn crate::db::Db,
    table: &str,
) -> Result<(), StoreError> {
    let rows = source
        .query(
            &format!("SELECT * FROM {}", quote_identifier(table)),
            Vec::new(),
        )
        .await?;
    let Some(first) = rows.first() else {
        return Ok(());
    };
    let columns: Vec<String> = first.column_names().to_vec();
    let placeholders = vec!["?"; columns.len()].join(", ");
    let insert_sql = format!(
        "INSERT OR REPLACE INTO {} ({}) VALUES ({placeholders})",
        quote_identifier(table),
        columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for row in &rows {
        let params: Vec<Value> = columns
            .iter()
            .map(|column| value_or_null(row, column))
            .collect();
        staged.execute(&insert_sql, params).await?;
    }
    Ok(())
}

/// Upstream table loop + index recreation.
async fn copy_source_into_staged(source: &DbHandle, staged: &DbHandle) -> Result<(), StoreError> {
    let tables = source.query(TABLE_LIST_SQL, Vec::new()).await?;
    let names: Vec<String> = tables.iter().filter_map(|row| row.text("name")).collect();

    for table in names {
        let ddl = source
            .query(TABLE_DDL_SQL, vec![Value::Text(table.clone())])
            .await?;
        let Some(sql) = ddl.first().and_then(|row| row.text("sql")) else {
            continue;
        };
        staged.execute(&sql, Vec::new()).await?;

        if table == "memories" {
            copy_memories(source.as_ref(), staged.as_ref()).await?;
        } else {
            copy_generic_table(source.as_ref(), staged.as_ref(), &table).await?;
        }
    }

    let indexes = source.query(INDEX_LIST_SQL, Vec::new()).await?;
    for index in indexes {
        if let Some(sql) = index.text("sql") {
            // May already exist — failures are ignored (upstream catch).
            let _ = staged.execute(&sql, Vec::new()).await;
        }
    }
    Ok(())
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// Upstream `dumpAndReloadEncrypted`: copy the plaintext shard into an
/// encrypted staged file, swap it in (copy → remove → rename under the
/// retrying file lock), then verify the encrypted open.
pub async fn dump_and_reload_encrypted(
    storage_path: &Path,
    db_path: &Path,
    connections: &Arc<ConnectionManager>,
    config: &StoreConfig,
) -> Result<(), StoreError> {
    let Some(encryption) = crate::db::resolve_database_encryption(config)? else {
        return Err(StoreError::internal(
            "databaseEncryptionKey is required for encryption migration",
        ));
    };

    crate::db::assert_path_inside_storage(storage_path, db_path)?;
    connections.close_connection(db_path).await?;

    let millis = now_millis();
    let staged = PathBuf::from(format!("{}.encrypt-{millis}.tmp", db_path.display()));
    let backup = PathBuf::from(format!("{}.pre-encrypt-{millis}.bak", db_path.display()));

    let factory = crate::db::turso_connect_factory();

    let mut plain_options = ConnectOptions::for_config(config, Platform::current());
    plain_options.encryption = None;
    let mut encrypted_options = ConnectOptions::for_config(config, Platform::current());
    encrypted_options.encryption = Some(encryption.clone());

    let source = factory
        .connect(db_path.to_path_buf(), plain_options)
        .await?;
    let staged_db = factory.connect(staged.clone(), encrypted_options).await?;

    let copy = copy_source_into_staged(&source, &staged_db).await;
    drop(source);
    drop(staged_db);
    if let Err(error) = copy {
        // Best-effort cleanup of the staged file created by the turso builder
        // so a failed copy does not leave an orphaned `.encrypt-*.tmp`.
        let _ = crate::sqlite_files::remove_sqlite_database(&staged);
        return Err(error);
    }

    crate::sqlite_files::with_sqlite_file_lock_retry(
        || -> std::io::Result<()> {
            crate::sqlite_files::copy_sqlite_database(db_path, &backup)?;
            crate::sqlite_files::remove_sqlite_database(db_path)?;
            crate::sqlite_files::rename_sqlite_database(&staged, db_path)?;
            Ok(())
        },
        crate::sqlite_files::FILE_LOCK_RETRY_DELAYS_MS.len(),
        &crate::sqlite_files::system_retry_context(),
    )?;

    // Test-only fault-injection seam: allows the restore-on-verify-failure
    // path to be exercised deterministically. Defaults to false; production
    // behaviour is unaffected. Named to make its test-only intent explicit.
    use std::sync::atomic::Ordering;

    // Verify the encrypted open of the swapped-in file. On failure restore the
    // plaintext backup to the active path (backup -> active) before rethrowing,
    // so a verification failure never strands the original shard.
    let mut verify_options = ConnectOptions::for_config(config, Platform::current());
    verify_options.encryption = Some(encryption);
    let verify_result: Result<(), StoreError> = if FORCE_VERIFY_FAIL.load(Ordering::SeqCst) {
        Err(StoreError::internal("forced verify failure for testing"))
    } else {
        match factory.connect(db_path.to_path_buf(), verify_options).await {
            Ok(handle) => {
                drop(handle);
                Ok(())
            }
            Err(error) => Err(error),
        }
    };
    if let Err(error) = verify_result {
        // Remove the unverifiable encrypted file, then restore the backup.
        let _ = crate::sqlite_files::remove_sqlite_database(db_path);
        let _ = crate::sqlite_files::rename_sqlite_database(&backup, db_path);
        return Err(error);
    }

    tracing::info!(
        "Encrypted database at rest (db: {}, backup: {})",
        db_path.display(),
        backup.display()
    );
    Ok(())
}

/// Upstream `runDatabaseEncryptionMigration`: no-op when encryption is off or
/// the marker already exists; otherwise convert every plaintext candidate
/// under the operation lock and write the marker.
pub async fn run_database_encryption_migration(
    storage_path: &Path,
    connections: &Arc<ConnectionManager>,
    shards: &Arc<ShardManager>,
    config: &StoreConfig,
) -> Result<(), StoreError> {
    let resolved = crate::db::resolve_database_encryption(config)?;
    let Some(encryption) = resolved else {
        return Ok(());
    };
    if marker_path(storage_path).exists() {
        return Ok(());
    }

    let candidates = collect_candidate_dbs(storage_path);
    let lock = crate::operation_lock::acquire_turso_operation_lock(
        storage_path,
        shards.process_probe(),
        "tursodb-encrypt",
    )?;

    let outcome =
        migrate_candidates(storage_path, &candidates, connections, config, &encryption).await;

    // Upstream `finally`: the shard manager always resets and the lock is
    // always released, on success and on failure alike.
    // the reset must not skip the release: an op-lock leak blocks every later write
    let reset = shards.reset();
    lock.release();
    reset?;
    outcome
}

async fn migrate_candidates(
    storage_path: &Path,
    candidates: &[PathBuf],
    connections: &Arc<ConnectionManager>,
    config: &StoreConfig,
    encryption: &EncryptionOptions,
) -> Result<(), StoreError> {
    for db_path in candidates {
        if is_already_encrypted(db_path, config).await? {
            continue;
        }
        if !can_open_plain(db_path, config).await? {
            return Err(StoreError::internal(format!(
                "Cannot open {} as plaintext for encryption migration. \
                 If it is already encrypted with a different key, update databaseEncryptionKey.",
                db_path.display()
            )));
        }
        dump_and_reload_encrypted(storage_path, db_path, connections, config).await?;
    }

    let marker = serde_json::json!({
        "migratedAt": crate::operation_lock::now_rfc3339_millis(),
        "cipher": encryption.cipher,
    });
    std::fs::write(marker_path(storage_path), marker.to_string())?;
    Ok(())
}

/// Upstream `ready.ts` encryption step (name `encryption-migration`).
pub fn encryption_migration_step(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    config: &StoreConfig,
) -> ReadyStep {
    let storage: PathBuf = storage_path.to_path_buf();
    let config = config.clone();
    ReadyStep {
        name: "encryption-migration",
        run: Arc::new(move || {
            let (storage, connections, shards, config) = (
                storage.clone(),
                connections.clone(),
                shards.clone(),
                config.clone(),
            );
            Box::pin(async move {
                run_database_encryption_migration(&storage, &connections, &shards, &config).await
            })
        }),
    }
}

/// Test-only fault-injection flag for the verify step in
/// [`dump_and_reload_encrypted`]. Defaults to `false`. Production code must
/// never set this. Exposed (`#[doc(hidden)]`) solely so integration tests can
/// exercise the restore-on-verify-failure path deterministically.
#[doc(hidden)]
pub static FORCE_VERIFY_FAIL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Test-only setter for [`FORCE_VERIFY_FAIL`].
#[doc(hidden)]
pub fn set_force_verify_fail(value: bool) {
    FORCE_VERIFY_FAIL.store(value, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::OpenDirection;

    /// 45.1 row 4's diagnosis gap: a guard refusal must **propagate**, not be
    /// relabelled "Cannot open … as plaintext" (the mis-diagnosis O4's grounds call
    /// unacceptable); any other open error is "not openable with these options",
    /// which is the caller's decision, and a successful open is `true`.
    ///
    /// Inverted world: collapsing every error to `false` (the pre-fix behaviour)
    /// makes the first assertion fail, and the operator is told the wrong cause.
    #[tokio::test]
    async fn test_encryption_probe_propagates_a_guard_refusal() {
        let blocked = probe_opened(Err(StoreError::OpenBlocked {
            db_path: PathBuf::from("/tmp/guard-refused.db"),
            direction: OpenDirection::LegacyAuthority,
        }));
        assert!(
            matches!(blocked, Err(StoreError::OpenBlocked { .. })),
            "a guard refusal must survive the encryption probe: {blocked:?}"
        );

        let plain_failure = probe_opened(Err(StoreError::turso_message("file is not a database")));
        assert_eq!(
            plain_failure.ok(),
            Some(false),
            "a non-guard open error stays 'not openable with these options'"
        );

        // `Ok(handle)` ⇒ `Ok(true)`, through a real handle so the drop path is real.
        let dir = std::env::temp_dir().join(format!(
            "mem-store-encryption-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create the temp dir");
        let options = ConnectOptions {
            experimental: vec![
                crate::db::ExperimentalFeature::Encryption,
                crate::db::ExperimentalFeature::MultiprocessWal,
            ],
            encryption: None,
            read_only: false,
            allow_unguarded_open: false,
            storage_root: Some(dir.clone()),
        };
        let handle = crate::db::turso_connect_factory()
            .connect(dir.join("opens.db"), options)
            .await
            .expect("open through the guarded seam");
        assert_eq!(
            probe_opened(Ok(handle)).ok(),
            Some(true),
            "a handle that opened is `true` (and is dropped inside the classifier)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
