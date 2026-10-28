//! Schema-migration runner — port of upstream
//! `src/services/turso/schema-migrations.ts` apply/backup semantics
//! (blueprint W1.2 step 6).
//!
//! Upstream apply order, verbatim: sort by version, read `PRAGMA
//! user_version`, filter pending, backup before destructive (existing file,
//! current > 0); per pending migration write-batch statements then
//! `PRAGMA user_version = <v>`.
//!
//! **Issue 46.4 — the restore is the caller's step.** Upstream closes its own
//! `TursoDb` handle before restoring; this runner borrows the db (`&dyn Db`) and
//! must not unlink a file it has open. On failure with a backup it therefore
//! returns `StoreError::MigrationRestorePending`, and the caller (which owns the
//! handle) closes it and calls `MigrationRestorePending::restore`. That restore
//! moves the backup **and its sidecars** back with `rename_sqlite_database`
//! (the old bare `std::fs::rename` stranded `.bak-wal`/`-shm`/`-tshm` debris),
//! inside the same `with_sqlite_file_lock_retry` window the old call sites had.
//!
//! **Issue 46 — the proof.** A backup that is taken must be restorable, and only
//! a process that can prove root exclusivity may restore. So a destructive
//! upgrade with `db_path: Some(..)` requires the caller's
//! `Option<&ExclusiveRoot>`: with no proof the runner returns
//! `StoreError::NoExclusiveRoot` **before** its checkpoint and before the copy,
//! leaving the database byte-identical and creating no `.bak`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::connection::ConnectionManager;
use crate::db::{BatchMode, Db, DbHandle, Statement, query_one};
use crate::delete_guard::ExclusiveRoot;
use crate::error::{MigrationRestorePending, StoreError};
use crate::sqlite_files::{
    FILE_LOCK_RETRY_DELAYS_MS, RetryContext, copy_sqlite_database, remove_sqlite_database,
    rename_sqlite_database, with_sqlite_file_lock_retry,
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

/// The backup-needed rule: an existing database at a positive version.
fn requires_backup(db_path: Option<&Path>, from_version: i64) -> Option<&Path> {
    db_path.filter(|path| path.exists() && from_version > 0)
}

/// Upstream `backupBeforeDestructive`.
///
/// The destination is claimed from the caller's proof. With no proof and a
/// backup needed this refuses **before creating anything**
/// ([`StoreError::NoExclusiveRoot`]).
fn backup_before_destructive(
    db_path: Option<&Path>,
    from_version: i64,
    root: Option<&ExclusiveRoot>,
) -> Result<Option<PathBuf>, StoreError> {
    let Some(db_path) = requires_backup(db_path, from_version) else {
        return Ok(None);
    };
    let Some(root) = root else {
        return Err(StoreError::NoExclusiveRoot {
            db_path: db_path.to_path_buf(),
        });
    };
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
    let to_claim = root.claim(&backup_path)?;
    copy_sqlite_database(db_path, to_claim)?;
    Ok(Some(backup_path))
}

impl MigrationRestorePending {
    /// Settles the pending restore: **drop the caller's handle first**, close the
    /// manager's cached clone, resolve the proof, then unlink the failed file and
    /// rename the backup (main + sidecars) back — one retry window, like the old
    /// failure branches.
    ///
    /// The by-value handle is not a close by itself (`Arc<dyn Db>`): the literal
    /// `drop` is the first statement, and the manager's clone is dropped by
    /// `close_connection`, so the probe runs with no in-process holder. The
    /// retry window is belt-and-braces afterwards — external or transient
    /// holders (an indexer, AV) can still hold the path; no Linux test
    /// discriminates its presence, which is why it stays review-gated.
    pub async fn restore(
        self,
        connections: &ConnectionManager,
        handle: DbHandle,
    ) -> Result<(), StoreError> {
        drop(handle);
        let db_path = self.db_path().to_path_buf();
        let backup_path = self.backup_path().to_path_buf();
        connections.close_connection(&db_path).await?;
        let Some(root) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot { db_path });
        };
        let db_claim = root.claim(&db_path)?;
        let backup_claim = root.claim(&backup_path)?;
        with_sqlite_file_lock_retry(
            || -> Result<(), StoreError> {
                remove_sqlite_database(db_claim)?;
                rename_sqlite_database(backup_claim, db_claim)?;
                Ok(())
            },
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &RetryContext::system(),
        )
    }
}

/// Upstream `applySchemaMigrations`. Returns the applied target version, or
/// the current version when nothing is pending.
///
/// `root` is the caller's exclusivity proof: a destructive upgrade needs one
/// before its checkpoint and backup, and the failure path is returned as
/// [`StoreError::MigrationRestorePending`] for the caller to settle.
pub async fn apply_schema_migrations(
    db: &dyn Db,
    migrations: &[SchemaMigration],
    options: &MigrationOptions,
    root: Option<&ExclusiveRoot>,
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
    // pending is non-empty here: the last version is the migration target (ADR-0013 D-3)
    let Some(target) = pending.last().copied() else {
        return Ok(current);
    };
    // A backup that is taken must be restorable, and only a process that can
    // prove root exclusivity may restore: refuse BEFORE the checkpoint and the
    // copy, leaving the database and every sidecar byte-identical (46.4).
    if let Some(db_path) = requires_backup(options.db_path.as_deref(), current)
        && root.is_none()
    {
        return Err(StoreError::NoExclusiveRoot {
            db_path: db_path.to_path_buf(),
        });
    }
    // Truncating checkpoint: the backup wants a self-contained main file. The
    // pragma returns a row, so it must go through `query` — `execute` refuses
    // it ("unexpected row during execution"), and the old `let _ = execute`
    // silently skipped the checkpoint entirely. The runner never unlinks a file
    // it has open; on failure it returns `MigrationRestorePending` for the
    // caller to settle (issue 46.4).
    let _ = db
        .query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
        .await;
    let backup = backup_before_destructive(options.db_path.as_deref(), current, root)?;
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
            // The runner must not unlink the file it has open (46.4): hand the
            // restore to the caller, which drops its handle and settles it.
            return Err(StoreError::MigrationRestorePending(Box::new(
                MigrationRestorePending::new(db_path.to_path_buf(), backup_path.clone(), original),
            )));
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
    let v4_statements: Vec<Statement> = crate::episodic_sql::EPISODIC_V4_ALTERS
        .iter()
        .map(|sql| Statement::new(*sql, Vec::new()))
        .collect();
    let mut v5_statements = vec![Statement::new(
        crate::episodic_sql::CAPTURE_INBOX_DDL,
        Vec::new(),
    )];
    for index in crate::episodic_sql::CAPTURE_INBOX_V5_INDEXES {
        v5_statements.push(Statement::new(index, Vec::new()));
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
        SchemaMigration {
            version: 4,
            description: "episodic-v4: Additive client-minted capture_id on agent_sessions (W5.1)",
            statements: v4_statements,
        },
        SchemaMigration {
            version: 5,
            description: "episodic-v5: Durable capture inbox table and indexes (W5.3)",
            statements: v5_statements,
        },
    ]
}

// ---------------------------------------------------------------------------
// Episodic v6 — the convergent retention schema apply (W7.1)
// ---------------------------------------------------------------------------

/// Which path stamped `PRAGMA user_version = 6`.
///
/// This is a MEASURED value, never an assumption: the first-apply test pins
/// [`V6_PRAGMA_PATH_MEASURED`], which the implementer sets from one real run
/// on this turso version (M14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PragmaOutcome {
    /// The version pragma was the last statement of the v6 write batch.
    InBatch,
    /// turso refused the pragma inside the batch; the statements committed
    /// without it and the pragma was executed separately.
    Fallback,
    /// No pragma was attempted: the database was already at v6 (the reopen
    /// path never claims a path it did not run — M28).
    NotAttempted,
}

/// The result of [`apply_episodic_schema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodicSchemaOutcome {
    /// The database's version after the apply (6 on success).
    pub version: i64,
    /// The measured path the version pragma took.
    pub pragma: PragmaOutcome,
}

/// The v6 migration step. It is applied ONLY by [`apply_episodic_schema`];
/// `episodic_migrations()` deliberately stays v1..v5 so direct callers (and
/// `crates/mem-store/tests/capture_id_migration.rs`) keep their meaning
/// (M13).
pub fn episodic_v6_migration(_dimensions: u32) -> SchemaMigration {
    let mut statements: Vec<Statement> = crate::episodic_sql::EPISODIC_V6_ALTERS
        .iter()
        .map(|sql| Statement::new(*sql, Vec::new()))
        .collect();
    statements.push(Statement::new(
        crate::episodic_sql::RAW_CONTEXT_DDL,
        Vec::new(),
    ));
    statements.push(Statement::new(
        crate::episodic_sql::SEGMENTS_DDL,
        Vec::new(),
    ));
    statements.push(Statement::new(
        crate::episodic_sql::SEGMENTS_INDEX,
        Vec::new(),
    ));
    SchemaMigration {
        version: 6,
        description: "episodic-v6: Convergent raw-context retention schema (W7.1)",
        statements,
    }
}

/// True when `table` already has `column` (the pre-flight convergence guard).
async fn table_has_column(db: &dyn Db, table: &str, column: &str) -> Result<bool, StoreError> {
    // `table` is a crate constant, never user text.
    let rows = db
        .query(&format!("PRAGMA table_info({table})"), Vec::new())
        .await?;
    Ok(rows
        .iter()
        .any(|row| row.text("name").as_deref() == Some(column)))
}

/// Applies the episodic schema fold (v1..v5 through the shared runner) and
/// then v6 convergently.
///
/// # Convergence (M3)
///
/// The bricking state is a kill between the v6 DDL commit and the version
/// pragma: a v5 file that already has `agent_role` and both tables. The
/// pre-flight `PRAGMA table_info(agent_sessions)` guard drops the
/// non-re-runnable `ALTER` when the column exists, and the tables use
/// `CREATE … IF NOT EXISTS`, so a re-run converges instead of failing on a
/// duplicate column.
///
/// # The pragma path (M14)
///
/// The version pragma is the last statement of the v6 write batch. If turso
/// refuses it there, the code retries the batch without it and executes the
/// pragma separately, reporting [`PragmaOutcome::Fallback`]. The reopen path
/// reports [`PragmaOutcome::NotAttempted`] — it ran no pragma at all.
///
/// # Backup
///
/// One backup is taken for an EXISTING database before the v6 batch; a fresh
/// root takes none. The destination is claimed from the caller's proof, and
/// with no proof a needed backup refuses **before** the copy
/// ([`StoreError::NoExclusiveRoot`]). On final failure the runner returns
/// [`StoreError::MigrationRestorePending`] — it never unlinks a file it has
/// open. No code path deletes a `.bak`.
pub async fn apply_episodic_schema(
    db: &dyn Db,
    dimensions: u32,
    options: &MigrationOptions,
    root: Option<&ExclusiveRoot>,
) -> Result<EpisodicSchemaOutcome, StoreError> {
    let current_before = get_user_version(db).await?;

    // v1..v5 keep their exact existing semantics, backups included. This call
    // returns early when none are pending, so the v6 backup below is reachable
    // with `root: None` on a v5→v6 upgrade and repeats the same refusal.
    let version =
        apply_schema_migrations(db, &episodic_migrations(dimensions), options, root).await?;
    if version >= 6 {
        return Ok(EpisodicSchemaOutcome {
            version: version.max(6),
            pragma: PragmaOutcome::NotAttempted,
        });
    }

    // The v6 backup needs the proof before the copy; no checkpoint precedes it.
    let backup = if current_before > 0 {
        backup_before_destructive(options.db_path.as_deref(), current_before, root)?
    } else {
        None
    };

    let has_agent_role = table_has_column(db, "agent_sessions", "agent_role").await?;
    // The v6 statement list has ONE home (`episodic_v6_migration`); this
    // apply path composes it and drops the non-re-runnable `ALTER` when the
    // column already exists (M3/M13).
    let statements: Vec<Statement> = episodic_v6_migration(dimensions)
        .statements
        .into_iter()
        .filter(|statement| {
            let is_agent_role_alter =
                crate::episodic_sql::EPISODIC_V6_ALTERS.contains(&statement.sql.as_str());
            !is_agent_role_alter || !has_agent_role
        })
        .collect();

    let pragma = set_user_version_statement(6);
    let mut with_pragma = statements.clone();
    with_pragma.push(pragma.clone());

    let mut failure: Option<StoreError> = None;
    let mut pragma_outcome = PragmaOutcome::InBatch;

    match db.batch(with_pragma, BatchMode::Write).await {
        Ok(_) => {
            // Measure: a batch that "succeeded" but did not stamp the version
            // still needs the separate execute.
            if get_user_version(db).await? != 6 {
                match db.execute(&pragma.sql, pragma.params.clone()).await {
                    Ok(_) => pragma_outcome = PragmaOutcome::Fallback,
                    Err(error) => failure = Some(error),
                }
            }
        }
        Err(original) => {
            // The pragma is the only refused statement: retry the DDL alone.
            match db.batch(statements, BatchMode::Write).await {
                Ok(_) => match db.execute(&pragma.sql, pragma.params.clone()).await {
                    Ok(_) => pragma_outcome = PragmaOutcome::Fallback,
                    Err(_) => failure = Some(original),
                },
                Err(_) => failure = Some(original),
            }
        }
    }

    if let Some(original) = failure {
        if let (Some(backup_path), Some(db_path)) = (&backup, options.db_path.as_deref()) {
            return Err(StoreError::MigrationRestorePending(Box::new(
                MigrationRestorePending::new(db_path.to_path_buf(), backup_path.clone(), original),
            )));
        }
        return Err(original);
    }

    let version = get_user_version(db).await?;
    if version != 6 {
        return Err(StoreError::Internal {
            message: "episodic v6 apply did not stamp user_version = 6".to_string(),
        });
    }
    Ok(EpisodicSchemaOutcome {
        version,
        pragma: pragma_outcome,
    })
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
