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
use crate::user_prompts_sql::{USER_PROMPTS_V2_COLUMNS, user_prompts_v2_alter};

/// A single versioned schema step; statements run as ONE write batch.
#[derive(Debug, Clone)]
pub struct SchemaMigration {
    pub version: i64,
    pub description: &'static str,
    pub statements: Vec<Statement>,
}

/// Apply-time options (`options?.dbPath` / `options?.label` upstream; the
/// label is for tracing only).
///
/// `max_known` is the highest schema version this build knows for the
/// migration set being applied (ADR-0021). There is deliberately no
/// `Default`: a permissive default would fail open on a newer file, so every
/// call site names its set's maximum.
#[derive(Debug, Clone)]
pub struct MigrationOptions {
    pub db_path: Option<PathBuf>,
    pub label: &'static str,
    pub max_known: i64,
}

/// Highest `metadata.db` schema version this build knows.
pub const METADATA_MAX_KNOWN: i64 = 2;
/// Highest `user_prompts` schema version this build knows: 3 because
/// [`apply_user_prompts_schema`] applies v2 and v3 itself, so the migration
/// slice (`user_prompts_migrations`) stays v1 — the same split
/// [`EPISODIC_MAX_KNOWN`]` = 8` records.
pub const USER_PROMPTS_MAX_KNOWN: i64 = 3;
/// Highest `user_profiles` schema version this build knows: v2 is the sweep
/// stamp's `sweep_state` table (issue 43 W6c).
pub const USER_PROFILES_MAX_KNOWN: i64 = 2;
/// Highest memory-shard schema version this build knows.
pub const MEMORY_SHARD_MAX_KNOWN: i64 = 3;
/// Highest episodic schema version this build knows: `apply_episodic_schema`
/// applies v6, v7 and v8 itself while `episodic_migrations()` returns v1..v5,
/// so deriving this from the slice would refuse every v6/v7/v8 database.
pub const EPISODIC_MAX_KNOWN: i64 = 8;

/// The sweep stamp's table, created by every W6c bump (issue 43): the last
/// successful sweep of one tier. `tier TEXT PRIMARY KEY` so a re-sweep replaces
/// its row; `last_swept_unix_ms INTEGER NOT NULL` so a stamp always names a
/// time. No column changes, no row rewrite — `CREATE TABLE IF NOT EXISTS`
/// only, which is what makes the three bumps data-migration-free.
pub const SWEEP_STATE_DDL: &str = "CREATE TABLE IF NOT EXISTS sweep_state (tier TEXT PRIMARY KEY, last_swept_unix_ms INTEGER NOT NULL)";

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
    // Refuse a file stamped by a newer build BEFORE the pending check (ADR-0021):
    // a newer file has no pending migrations, so an in-loop check never runs.
    // The comparison is on `current`, never on `target` — a fresh database must
    // still apply v1..vN under a build that knows N.
    if current > options.max_known {
        return Err(StoreError::SchemaNewerThanBuild {
            label: options.label,
            found: current,
            supported: options.max_known,
        });
    }
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
    // pragma returns a row, so it goes through `query`. (`execute` also runs
    // the checkpoint, then reports the row as "unexpected row during
    // execution"; the old `let _ = execute` hid that error, not the
    // checkpoint.) The runner never unlinks a file it has open; on failure it
    // returns `MigrationRestorePending` for the caller to settle (issue 46.4).
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
        SchemaMigration {
            version: 3,
            description: "shard_stats: the shard's own absolute vector count (W2)",
            // The DDL alone: no count row is written by the migration — a
            // placeholder 0 is a W7 reaping trigger (ADR-0018).
            statements: vec![Statement::new(
                crate::shard_facts::SHARD_STATS_DDL,
                Vec::new(),
            )],
        },
    ]
}

/// Upstream `METADATA_DB_MIGRATIONS`.
///
/// v2 (W4, ADR-0022) makes "exactly one active shard per `(scope, scope_hash)`"
/// a database constraint. The order inside the batch is load-bearing: the
/// repair retires redundant active rows **first**, then the partial unique
/// index is created. Reversed, every root that already holds two actives
/// refuses the index and cannot open.
pub fn metadata_db_migrations() -> Vec<SchemaMigration> {
    vec![
        SchemaMigration {
            version: 1,
            description: "Baseline shards registry",
            statements: [crate::sql::SHARDS_DDL, crate::sql::SHARDS_ACTIVE_INDEX]
                .iter()
                .map(|sql| Statement::new(*sql, Vec::new()))
                .collect(),
        },
        SchemaMigration {
            version: 2,
            description: "One active shard per (scope, scope_hash): repair extra actives, then enforce",
            statements: [
                crate::shard_registry_sql::SHARDS_REPAIR_EXTRA_ACTIVE,
                crate::shard_registry_sql::SHARDS_ONE_ACTIVE_INDEX,
            ]
            .iter()
            .map(|sql| Statement::new(*sql, Vec::new()))
            .collect(),
        },
    ]
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
/// their indexes, DDL order preserved. v2 (issue 43 W6c) is the sweep stamp's
/// `sweep_state` table — one additive row, no column change.
pub fn user_profiles_migrations() -> Vec<SchemaMigration> {
    vec![
        SchemaMigration {
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
        },
        SchemaMigration {
            version: 2,
            description: "user-profiles-v2: the sweep stamp table (issue 43 W6c)",
            statements: vec![Statement::new(SWEEP_STATE_DDL, Vec::new())],
        },
    ]
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

/// Which path stamped the **v6** `PRAGMA user_version = 6`.
///
/// The v7 pragma is a separate `execute` of the `user_prompts`-v2 shape
/// (D-110-1), so it never reaches this classification: every v6 assertion
/// about this value stays true.
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
    /// The database's version after the apply (8 on success).
    pub version: i64,
    /// The measured path the **v6** version pragma took.
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

/// The v8 migration step (issue 43 W6c): the sweep stamp's `sweep_state`
/// table. It is applied ONLY by [`apply_episodic_schema`], after the v6/v7
/// convergence legs — putting it in `episodic_migrations()` would stamp a
/// fresh file 8 before the v6/v7 columns exist. One additive statement plus
/// one pragma: no backup, no column change, no row rewrite.
pub fn episodic_v8_migration() -> SchemaMigration {
    SchemaMigration {
        version: 8,
        description: "episodic-v8: the sweep stamp table (issue 43 W6c)",
        statements: vec![Statement::new(SWEEP_STATE_DDL, Vec::new())],
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

/// Applies the episodic schema fold: v1..v5 through the shared runner, then
/// v6 and v7 convergently.
///
/// # Convergence (M3)
///
/// The bricking state is a kill between a DDL commit and its version pragma:
/// a v5 file that already has `agent_role` and both retention tables, or a v6
/// file that already has `capture_inbox.not_before`. The pre-flight
/// `PRAGMA table_info` guards drop the non-re-runnable `ALTER`s when the
/// columns exist, and the tables use `CREATE … IF NOT EXISTS`, so a re-run
/// converges instead of failing on a duplicate column.
///
/// # The v6 pragma path (M14)
///
/// The v6 version pragma is the last statement of the v6 write batch. If turso
/// refuses it there, the code retries the batch without it and executes the
/// pragma separately, reporting [`PragmaOutcome::Fallback`]. The reopen path
/// reports [`PragmaOutcome::NotAttempted`] — it ran no v6 pragma at all.
///
/// # The v7 step (D-110-1)
///
/// `probe → ALTER → pragma → verify`, gated on `options.max_known >= 7`: a
/// build that knows only 6 must never hand back a 7-stamped file it cannot
/// open again. The `ALTER` runs only when `capture_inbox.not_before` is
/// absent, so the crash window between the batch and the pragma converges.
/// This step takes NO backup: one transactional batch plus one pragma
/// converge under the probe, so a restore would have nothing to roll back
/// beyond the batch the engine already rolls back itself (the
/// [`apply_user_prompts_schema`] precedent; the v6 apply, which does back up,
/// is not the model).
///
/// # Backup
///
/// One backup is taken for an EXISTING database before the v6 batch; a fresh
/// root takes none, and the v7 step takes none at all. The destination is
/// claimed from the caller's proof, and with no proof a needed backup refuses
/// **before** the copy ([`StoreError::NoExclusiveRoot`]). On final failure the
/// runner returns [`StoreError::MigrationRestorePending`] — it never unlinks a
/// file it has open. No code path deletes a `.bak`.
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
    let mut version =
        apply_schema_migrations(db, &episodic_migrations(dimensions), options, root).await?;
    let mut pragma = PragmaOutcome::NotAttempted;

    if version < 6 {
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

        let v6_pragma = set_user_version_statement(6);
        let mut with_pragma = statements.clone();
        with_pragma.push(v6_pragma.clone());

        let mut failure: Option<StoreError> = None;
        let mut pragma_outcome = PragmaOutcome::InBatch;

        match db.batch(with_pragma, BatchMode::Write).await {
            Ok(_) => {
                // Measure: a batch that "succeeded" but did not stamp the version
                // still needs the separate execute.
                if get_user_version(db).await? != 6 {
                    match db.execute(&v6_pragma.sql, v6_pragma.params.clone()).await {
                        Ok(_) => pragma_outcome = PragmaOutcome::Fallback,
                        Err(error) => failure = Some(error),
                    }
                }
            }
            Err(original) => {
                // The pragma is the only refused statement: retry the DDL alone.
                match db.batch(statements, BatchMode::Write).await {
                    Ok(_) => match db.execute(&v6_pragma.sql, v6_pragma.params.clone()).await {
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
                    MigrationRestorePending::new(
                        db_path.to_path_buf(),
                        backup_path.clone(),
                        original,
                    ),
                )));
            }
            return Err(original);
        }

        let stamped = get_user_version(db).await?;
        if stamped != 6 {
            return Err(StoreError::Internal {
                message: "episodic v6 apply did not stamp user_version = 6".to_string(),
            });
        }
        version = 6;
        pragma = pragma_outcome;
    }

    if options.max_known >= 7 && version < 7 {
        // The `ALTER` is not re-runnable: the probe is what makes a file whose
        // batch committed but whose pragma did not converge (D-110-1).
        let has_not_before = table_has_column(db, "capture_inbox", "not_before").await?;
        if !has_not_before {
            let statements: Vec<Statement> = crate::episodic_sql::EPISODIC_V7_ALTERS
                .iter()
                .map(|sql| Statement::new(*sql, Vec::new()))
                .collect();
            db.batch(statements, BatchMode::Write).await?;
        }
        // The v7 pragma is the separate `execute` of the user_prompts-v2 shape,
        // never part of the batch, so `PragmaOutcome` keeps describing v6.
        db.execute(&set_user_version_statement(7).sql, Vec::new())
            .await?;
        let stamped = get_user_version(db).await?;
        if stamped != 7 {
            return Err(StoreError::Internal {
                message: "episodic v7 apply did not stamp user_version = 7".to_string(),
            });
        }
        version = 7;
    }

    // The v8 leg (issue 43 W6c): the sweep stamp's table. `CREATE TABLE IF NOT
    // EXISTS` converges on its own, so no probe is needed; one additive
    // statement plus one pragma, and no backup (D-110-1's shape).
    if options.max_known >= 8 && version < 8 {
        db.batch(episodic_v8_migration().statements, BatchMode::Write)
            .await?;
        db.execute(&set_user_version_statement(8).sql, Vec::new())
            .await?;
        let stamped = get_user_version(db).await?;
        if stamped != 8 {
            return Err(StoreError::Internal {
                message: "episodic v8 apply did not stamp user_version = 8".to_string(),
            });
        }
        version = 8;
    }

    Ok(EpisodicSchemaOutcome { version, pragma })
}

// ---------------------------------------------------------------------------
// user_prompts v2 — the probe-then-ALTER apply (issue 43 W5, ADR-0021)
// ---------------------------------------------------------------------------

/// Applies the `user_prompts` schema fold: v1 through the shared runner, then
/// the v2 column step convergently.
///
/// # Contract
///
/// - **Precondition:** `options.max_known` is [`USER_PROMPTS_MAX_KNOWN`], this
///   set's maximum, and the file is not newer than it.
/// - **Postcondition:** `PRAGMA user_version == 3`, every column of
///   [`USER_PROMPTS_V2_COLUMNS`] exists.
/// - **Convergence** exists only for the crash window between the v2 ALTER
///   batch and the version pragma: a file already carrying the columns
///   converges silently. A file stamped 2 with columns missing is **not**
///   repaired — the version is the authority.
/// - **No backup** is taken by this function: one transactional batch plus one
///   pragma converge under the probe, so a restore would have nothing to roll
///   back beyond the batch the engine already rolls back itself (the shard-v3
///   precedent; the episodic v6 apply, which does back up, is not the model).
///
/// The newer-file refusal stays the runner's: `apply_schema_migrations` runs
/// first, so a v3 file is refused before any probe, before the pending check
/// and before any DDL.
pub async fn apply_user_prompts_schema(
    db: &dyn Db,
    options: &MigrationOptions,
    root: Option<&ExclusiveRoot>,
) -> Result<i64, StoreError> {
    let mut version =
        apply_schema_migrations(db, &user_prompts_migrations(), options, root).await?;
    if version < 2 {
        let mut missing: Vec<Statement> = Vec::new();
        for (name, declaration) in USER_PROMPTS_V2_COLUMNS {
            if !table_has_column(db, "user_prompts", name).await? {
                missing.push(Statement::new(
                    user_prompts_v2_alter(name, declaration),
                    Vec::new(),
                ));
            }
        }
        if !missing.is_empty() {
            db.batch(missing, BatchMode::Write).await?;
        }
        db.execute(&set_user_version_statement(2).sql, Vec::new())
            .await?;
        let stamped = get_user_version(db).await?;
        if stamped != 2 {
            return Err(StoreError::Internal {
                message: format!(
                    "user_prompts v2 apply did not stamp user_version = 2 (found {stamped})"
                ),
            });
        }
        version = 2;
    }

    // The v3 leg (issue 43 W6c): the sweep stamp's table. One additive
    // `CREATE TABLE IF NOT EXISTS` plus one pragma; no backup (the v2 shape,
    // not the episodic v6 one).
    if options.max_known >= 3 && version < 3 {
        db.batch(user_prompts_v3_migration().statements, BatchMode::Write)
            .await?;
        db.execute(&set_user_version_statement(3).sql, Vec::new())
            .await?;
        let stamped = get_user_version(db).await?;
        if stamped != 3 {
            return Err(StoreError::Internal {
                message: format!(
                    "user_prompts v3 apply did not stamp user_version = 3 (found {stamped})"
                ),
            });
        }
        version = 3;
    }
    Ok(version)
}

/// The v3 migration step (issue 43 W6c): the sweep stamp's `sweep_state`
/// table. It is applied ONLY by [`apply_user_prompts_schema`], after the v2
/// column leg — putting it in `user_prompts_migrations()` would stamp a fresh
/// file 3 before the v2 columns exist.
pub fn user_prompts_v3_migration() -> SchemaMigration {
    SchemaMigration {
        version: 3,
        description: "user-prompts-v3: the sweep stamp table (issue 43 W6c)",
        statements: vec![Statement::new(SWEEP_STATE_DDL, Vec::new())],
    }
}
