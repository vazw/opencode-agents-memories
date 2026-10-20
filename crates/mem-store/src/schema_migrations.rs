//! Schema-migration runner — port of upstream
//! `src/services/turso/schema-migrations.ts` apply/backup/restore semantics
//! (blueprint W1.2 step 6).
//!
//! Upstream apply order, verbatim: sort by version, read `PRAGMA
//! user_version`, filter pending, backup before destructive (existing file,
//! current > 0); per pending migration write-batch statements then
//! `PRAGMA user_version = <v>`. On failure the close is handled by the
//! caller (DIVERGENCE, see below): delete originals, rename the backup
//! back under the file-lock retry, and re-raise the ORIGINAL error (a
//! restore failure replaces it, exactly like the upstream catch block).
//!
//! DIVERGENCE (reviewer note): upstream closes its own `TursoDb` handle
//! before restoring; this runner borrows the db (`&dyn Db`) and cannot
//! close it — the caller drops/reopens (wired in W1.5). On POSIX a rename
//! over an open file stays valid, so the restore path is behaviorally
//! equivalent on the test platform.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::db::{BatchMode, Db, Statement, query_one};
use crate::error::StoreError;
use crate::sqlite_files::{
    FILE_LOCK_RETRY_DELAYS_MS, RetryContext, copy_sqlite_database, remove_sqlite_database,
    with_sqlite_file_lock_retry,
};

/// A single versioned schema step; statements run as ONE write batch.
#[derive(Debug, Clone)]
pub struct SchemaMigration {
    pub version: i64,
    pub description: &'static str,
    pub statements: Vec<Statement>,
}

/// Apply-time options (`options?.dbPath` / `options?.label` upstream; the
/// label is for tracing only).
#[derive(Debug, Clone, Default)]
pub struct MigrationOptions {
    pub db_path: Option<PathBuf>,
    pub label: &'static str,
}

/// Upstream `getUserVersion`.
async fn get_user_version(db: &dyn Db) -> Result<i64, StoreError> {
    let row = query_one(db, crate::sql::PRAGMA_USER_VERSION, Vec::new()).await?;
    Ok(row.and_then(|row| row.i64("user_version")).unwrap_or(0))
}

/// Upstream `setUserVersion` (`Math.trunc` is implicit for `i64`).
fn set_user_version_statement(version: i64) -> Statement {
    Statement::new(crate::sql::set_user_version_sql(version), Vec::new())
}

/// Upstream `backupBeforeDestructive`.
fn backup_before_destructive(
    db_path: Option<&Path>,
    from_version: i64,
) -> Result<Option<PathBuf>, StoreError> {
    let Some(db_path) = db_path else {
        return Ok(None);
    };
    if !db_path.exists() || from_version <= 0 {
        return Ok(None);
    }
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let backup_path = PathBuf::from(format!(
        "{}.schema-v{}-{}.bak",
        db_path.display(),
        from_version,
        now_millis
    ));
    if let Some(parent) = backup_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    copy_sqlite_database(db_path, &backup_path)?;
    Ok(Some(backup_path))
}

/// Upstream restore body inside `withSqliteFileLockRetry`: unlink originals
/// (main + sidecars) then rename the backup back.
fn restore_from_backup(db_path: &Path, backup_path: &Path) -> Result<(), StoreError> {
    remove_sqlite_database(db_path)?;
    std::fs::rename(backup_path, db_path)?;
    Ok(())
}

/// Upstream `applySchemaMigrations`. Returns the applied target version, or
/// the current version when nothing is pending.
pub async fn apply_schema_migrations(
    db: &dyn Db,
    migrations: &[SchemaMigration],
    options: &MigrationOptions,
) -> Result<i64, StoreError> {
    let mut sorted = migrations.to_vec();
    sorted.sort_by_key(|migration| migration.version);
    let current = get_user_version(db).await?;
    let pending: Vec<i64> = sorted
        .iter()
        .filter(|migration| migration.version > current)
        .map(|migration| migration.version)
        .collect();
    if pending.is_empty() {
        return Ok(current);
    }
    let target = *pending.last().expect("pending non-empty checked above");
    // DIVERGENCE (reviewer note): upstream closes the handle before the
    // backup copy; a borrowed `&dyn Db` cannot. Without a checkpoint the
    // seeded `PRAGMA user_version` write may still live in the live WAL,
    // so the copied main-file header can read stale. A truncating
    // checkpoint makes the backup self-contained (tolerated failure: the
    // sidecar copy below already covers the committed WAL).
    let _ = db
        .execute("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
        .await;
    let backup = backup_before_destructive(options.db_path.as_deref(), current)?;
    if backup.is_some() {
        tracing::info!(
            label = options.label,
            from = current,
            to = target,
            "Schema migration backup created"
        );
    }

    let mut failure: Option<StoreError> = None;
    for migration in &sorted {
        if migration.version <= current {
            continue;
        }
        match db
            .batch(migration.statements.clone(), BatchMode::Write)
            .await
        {
            Ok(_) => {
                let user_version = set_user_version_statement(migration.version);
                match db.execute(&user_version.sql, Vec::new()).await {
                    Ok(_) => {
                        tracing::info!(
                            label = options.label,
                            version = migration.version,
                            description = migration.description,
                            "Applied schema migration"
                        );
                    }
                    Err(error) => {
                        failure = Some(error);
                    }
                }
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
        if failure.is_some() {
            break;
        }
    }

    if let Some(original) = failure {
        if let (Some(backup_path), Some(db_path)) = (&backup, options.db_path.as_deref()) {
            let restore = || restore_from_backup(db_path, backup_path);
            let restore_result: Result<(), StoreError> = with_sqlite_file_lock_retry(
                restore,
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &RetryContext::system(),
            );
            // Upstream catch block: a restore failure replaces the original;
            // success re-raises the original error.
            return match restore_result {
                Ok(()) => Err(original),
                Err(restore_error) => Err(restore_error),
            };
        }
        return Err(original);
    }
    Ok(target)
}

// ---------------------------------------------------------------------------
// Migration sets (byte-equal statements from `sql.rs` builders)
// ---------------------------------------------------------------------------

/// Upstream `memoryShardMigrations(dimensions)`.
pub fn memory_shard_migrations(dimensions: u32) -> Vec<SchemaMigration> {
    let mut v1 = vec![
        Statement::new(crate::sql::shard_metadata_ddl(), Vec::new()),
        Statement::new(crate::sql::memories_ddl(dimensions), Vec::new()),
    ];
    for index in crate::sql::MEMORIES_INDEXES {
        v1.push(Statement::new(index, Vec::new()));
    }
    let mut v2 = Vec::new();
    for statement in crate::sql::memory_shard_v2_statements() {
        v2.push(Statement::new(statement, Vec::new()));
    }
    vec![
        SchemaMigration {
            version: 1,
            description: "Baseline memories + shard_metadata for @tursodatabase/database",
            statements: v1,
        },
        SchemaMigration {
            version: 2,
            description: "Indexed session_id for session lookup (FTS5 unavailable on @tursodatabase/database)",
            statements: v2,
        },
    ]
}

/// Upstream `METADATA_DB_MIGRATIONS`.
pub fn metadata_db_migrations() -> Vec<SchemaMigration> {
    vec![SchemaMigration {
        version: 1,
        description: "Baseline shards registry",
        statements: [crate::sql::SHARDS_DDL, crate::sql::SHARDS_ACTIVE_INDEX]
            .iter()
            .map(|sql| Statement::new(*sql, Vec::new()))
            .collect(),
    }]
}

/// Upstream `USER_PROMPTS_MIGRATIONS` (data-only until W3.1).
pub fn user_prompts_migrations() -> Vec<SchemaMigration> {
    vec![SchemaMigration {
        version: 1,
        description: "Baseline user_prompts with provider/model columns",
        statements: std::iter::once(crate::sql::USER_PROMPTS_DDL)
            .chain(crate::sql::USER_PROMPTS_INDEXES.iter().copied())
            .map(|sql| Statement::new(sql, Vec::new()))
            .collect(),
    }]
}

/// Upstream `USER_PROFILES_MIGRATIONS` (data-only until W3.1 wired the
/// profile store): baseline `user_profiles` + `user_profile_changelogs` with
/// their indexes, DDL order preserved.
pub fn user_profiles_migrations() -> Vec<SchemaMigration> {
    vec![SchemaMigration {
        version: 1,
        description: "Baseline user_profiles + changelogs",
        statements: [
            crate::sql::USER_PROFILES_DDL,
            crate::sql::USER_PROFILE_CHANGELOGS_DDL,
        ]
        .into_iter()
        .chain(crate::sql::USER_PROFILES_INDEXES.iter().copied())
        .map(|sql| Statement::new(sql, Vec::new()))
        .collect(),
    }]
}

/// NET-NEW (R10): the episodic `agent_sessions` baseline (W4.1) plus the
/// W4.2 soft-delete/orphan set (v2) and the W4.3 milestone table (v3). The
/// DDL lives in `episodic_sql` (no upstream oracle) and is never added to the
/// golden-SQL locus. The label is set-level since v2 (D-10); per-version
/// identity lives in `description`.
pub fn episodic_migrations(dimensions: u32) -> Vec<SchemaMigration> {
    let mut statements = vec![Statement::new(
        crate::episodic_sql::agent_sessions_ddl(dimensions),
        Vec::new(),
    )];
    for index in crate::episodic_sql::EPISODIC_INDEXES {
        statements.push(Statement::new(index, Vec::new()));
    }
    let mut v2_statements: Vec<Statement> = crate::episodic_sql::EPISODIC_V2_ALTERS
        .iter()
        .map(|sql| Statement::new(*sql, Vec::new()))
        .collect();
    for index in crate::episodic_sql::EPISODIC_V2_INDEXES {
        v2_statements.push(Statement::new(index, Vec::new()));
    }
    let mut v3_statements = vec![Statement::new(
        crate::episodic_sql::episodic_milestones_ddl(dimensions),
        Vec::new(),
    )];
    for index in crate::episodic_sql::EPISODIC_V3_INDEXES {
        v3_statements.push(Statement::new(index, Vec::new()));
    }
    vec![
        SchemaMigration {
            version: 1,
            description: "episodic-v1: Baseline agent_sessions with embedding",
            statements,
        },
        SchemaMigration {
            version: 2,
            description: "episodic-v2: Soft-delete and orphan tracking for session timelines",
            statements: v2_statements,
        },
        SchemaMigration {
            version: 3,
            description: "episodic-v3: Consolidated milestone records (W4.3)",
            statements: v3_statements,
        },
    ]
}

// ---------------------------------------------------------------------------
// user_prompt column upgrade
// ---------------------------------------------------------------------------

/// Upstream `ensureUserPromptColumns` (L340-355): three ADD COLUMN attempts;
/// duplicate-column failures are silently tolerated, every other failure is
/// logged (`tracing::warn!`) and tolerated — startup never aborts. Always
/// `Ok(())`.
pub async fn ensure_user_prompt_columns(db: &dyn Db) -> Result<(), StoreError> {
    for column in USER_PROMPT_EXTRA_COLUMNS {
        let sql = format!("ALTER TABLE user_prompts ADD COLUMN {column}");
        match db.execute(&sql, Vec::new()).await {
            Ok(_) => {}
            Err(error) => {
                if !is_duplicate_column_error(&error) {
                    tracing::warn!(
                        column,
                        error = %error.message(),
                        "Failed to add user_prompts column"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Upstream column list, order preserved.
pub const USER_PROMPT_EXTRA_COLUMNS: [&str; 3] = [
    "capture_attempts INTEGER DEFAULT 0",
    "provider_id TEXT",
    "model_id TEXT",
];

/// Duplicate-column detection (upstream checks the lowercased message for
/// `duplicate column`; SQLite variants (e.g. `column already exists`) are
/// matched the same way as turso's catch-all — recorded N-A: upstream pins
/// ITS turso dialect via the exact substring).
pub fn is_duplicate_column_error(error: &StoreError) -> bool {
    let message = error.message().to_ascii_lowercase();
    message.contains("duplicate column")
}
