//! Path-migration SWAP-STATE half (blueprint W1.2 step 9) — port of
//! upstream `src/services/shard-path-migration-service.ts` journal types +
//! `rollbackSwap` + `recoverInterruptedSwap`. The FORWARD migrator
//! (journal WRITER) arrives in a later wave; recovery is what makes
//! crashed path migrations safe at startup, so it ships with the store.
//!
//! Divergences (reviewer notes): upstream reads `CONFIG.storagePath` —
//! the storage root travels explicitly through every function here;
//! `tursoShardManager` singleton calls go through the injected
//! [`ShardRegistry`]; per-move/target failure logging stays (warn) +
//! `rollbackFailed` semantics are upstream verbatim.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::connection::ConnectionManager;
use crate::db::{Db, Value, assert_path_inside_storage};
use crate::error::StoreError;
use crate::scope::Scope;
use crate::shard_manager::ShardRegistry;
use crate::sqlite_files::{
    FILE_LOCK_RETRY_DELAYS_MS, remove_sqlite_database, rename_sqlite_database,
    with_sqlite_file_lock_retry,
};

/// Upstream `interface ShardMove` (camelize — TS journal keys).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardMove {
    pub shard_id: i64,
    pub shard_index: u32,
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    pub staged_path: PathBuf,
    pub backup_path: PathBuf,
    pub is_active: bool,
    pub vector_count: u64,
    pub old_container_tag: String,
}

/// Upstream `interface ArchivedTarget`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedTarget {
    pub shard_id: i64,
    pub shard_index: u32,
    pub original_path: PathBuf,
    pub archive_path: Option<PathBuf>,
    pub is_active: bool,
    pub vector_count: u64,
}

/// Upstream `interface PathMigrateSwapState`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathMigrateSwapState {
    pub operation: String,
    pub old_hash: String,
    pub new_hash: String,
    pub moves: Vec<ShardMove>,
    pub archived_targets: Vec<ArchivedTarget>,
}

/// Upstream `swapStatePath()`.
pub fn swap_state_path(storage_path: &Path) -> PathBuf {
    storage_path.join(".path-migrate-swap.json")
}

/// Upstream `assertRecoveryPathInsideStorage`: relative-to-storage must be
/// non-empty (not the storage root itself), never start with `..`, and
/// never contain NUL.
pub fn assert_recovery_path_inside_storage(
    storage_path: &Path,
    path: &Path,
) -> Result<(), StoreError> {
    if path.to_string_lossy().contains('\0') {
        return Err(StoreError::RecoveryPathEscape {
            path: path.to_path_buf(),
        });
    }
    if !is_inside_storage_lexical(storage_path, path) || same_normalized(storage_path, path) {
        return Err(StoreError::RecoveryPathEscape {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Lexical `..`/`.` resolution for the recovery root-equality check
/// ([`same_normalized`]). Pops the last real component on `..`; a relative
/// `..` with nothing to pop is kept as a marker and an anchor (root/prefix)
/// is never climbed. Consistent with `db::normalize_lexical`.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if matches!(output.components().next_back(), Some(Component::Normal(_))) {
                    output.pop();
                } else {
                    output.push(component.as_os_str());
                }
            }
            Component::CurDir => {}
            other => output.push(other.as_os_str()),
        }
    }
    output
}

fn same_normalized(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

/// Lexical containment (upstream `relative(root, target)` semantics — the
/// empty/`..`-prefixed/NUL cases are judged separately by the caller).
fn is_inside_storage_lexical(storage_path: &Path, path: &Path) -> bool {
    // assert_path_inside_storage implements the same relative() semantics.
    assert_path_inside_storage(storage_path, path).is_ok()
}

/// Upstream `removeFile`: close any open handle first, then remove under the
/// file-lock retry (existsSync guard). The unlink runs only under the root's
/// exclusivity proof (issue 46): the probe is inside `remove_sqlite_database`,
/// so every retry attempt re-probes.
async fn remove_file_helper(
    connections: &ConnectionManager,
    path: &Path,
) -> Result<(), StoreError> {
    connections.close_connection(path).await?;
    if path.exists() {
        let Some(proof) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot {
                db_path: path.to_path_buf(),
            });
        };
        let claim = proof.claim(path)?;
        with_sqlite_file_lock_retry(
            || remove_sqlite_database(claim),
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &crate::sqlite_files::system_retry_context(),
        )?;
    }
    Ok(())
}

async fn rollback_move(
    connections: &ConnectionManager,
    registry: &dyn ShardRegistry,
    old_hash: &str,
    mv: &ShardMove,
) -> Result<(), StoreError> {
    connections.close_connection(&mv.new_path).await?;
    connections.close_connection(&mv.staged_path).await?;
    if mv.backup_path.exists() {
        remove_file_helper(connections, &mv.new_path).await?;
        remove_file_helper(connections, &mv.old_path).await?;
        let Some(proof) = connections.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot {
                db_path: mv.old_path.clone(),
            });
        };
        let from_claim = proof.claim(&mv.backup_path)?;
        let to_claim = proof.claim(&mv.old_path)?;
        with_sqlite_file_lock_retry(
            || rename_sqlite_database(from_claim, to_claim),
            FILE_LOCK_RETRY_DELAYS_MS.len(),
            &crate::sqlite_files::system_retry_context(),
        )?;
    }
    remove_file_helper(connections, &mv.staged_path).await?;
    if mv.old_path.exists() {
        registry
            .reassign_shard_scope(
                mv.shard_id,
                old_hash,
                &mv.old_path,
                mv.vector_count,
                mv.is_active,
            )
            .await?;
    }
    Ok(())
}

async fn rollback_archived(
    connections: &ConnectionManager,
    registry: &dyn ShardRegistry,
    new_hash: &str,
    archived: &ArchivedTarget,
) -> Result<(), StoreError> {
    let Some(archive) = archived.archive_path.as_deref() else {
        return Ok(());
    };
    if !archive.exists() {
        return Ok(());
    }
    remove_file_helper(connections, &archived.original_path).await?;
    let Some(proof) = connections.exclusive_root() else {
        return Err(StoreError::NoExclusiveRoot {
            db_path: archived.original_path.clone(),
        });
    };
    let from_claim = proof.claim(archive)?;
    let to_claim = proof.claim(&archived.original_path)?;
    with_sqlite_file_lock_retry(
        || rename_sqlite_database(from_claim, to_claim),
        FILE_LOCK_RETRY_DELAYS_MS.len(),
        &crate::sqlite_files::system_retry_context(),
    )?;
    registry
        .register_existing_shard(
            Scope::Project,
            new_hash,
            archived.shard_index,
            &archived.original_path,
            archived.vector_count,
            archived.is_active,
        )
        .await?;
    Ok(())
}

/// Upstream `rollbackSwap` — `Ok(true)` when any per-item failure was
/// logged (`rollbackFailed`), `Ok(false)` for a clean rollback.
async fn rollback_swap(
    connections: &ConnectionManager,
    registry: &dyn ShardRegistry,
    state: &PathMigrateSwapState,
    state_path: &Path,
) -> Result<bool, StoreError> {
    let mut rollback_failed = false;

    for mv in state.moves.iter().rev() {
        if let Err(error) = rollback_move(connections, registry, &state.old_hash, mv).await {
            tracing::warn!(error = %error.message(), "Path migration source rollback failed");
            rollback_failed = true;
        }
    }

    for archived in &state.archived_targets {
        if let Err(error) =
            rollback_archived(connections, registry, &state.new_hash, archived).await
        {
            tracing::warn!(error = %error.message(), "Path migration target rollback failed");
            rollback_failed = true;
        }
    }

    // Startup reconciliation may have registered a newly renamed shard
    // before recovery ran. Remove only registry rows whose files are gone.
    match registry
        .get_all_shards(Scope::Project, Some(&state.new_hash))
        .await
    {
        Ok(stale_targets) => {
            for shard in stale_targets {
                if !shard.db_path.exists()
                    && let Err(error) = registry.delete_shard(shard.id).await
                {
                    tracing::warn!(error = %error.message(), shard_id = shard.id, "Path migration stale target cleanup failed");
                    rollback_failed = true;
                }
            }
        }
        Err(error) => {
            tracing::warn!(error = %error.message(), "Path migration stale target cleanup failed");
            rollback_failed = true;
        }
    }

    if !rollback_failed && state_path.exists() {
        std::fs::remove_file(state_path).map_err(StoreError::from)?;
    } else if rollback_failed && !state_path.exists() {
        let body = serde_json::to_string(state).map_err(|error| StoreError::Json {
            message: error.to_string(),
        })?;
        std::fs::write(state_path, body).map_err(StoreError::from)?;
    }
    Ok(rollback_failed)
}

/// Upstream `recoverInterruptedSwap`: journal missing -> Ok; parse +
/// validate + guard EVERY path BEFORE any mutation; rollback; journal
/// still present -> `Internal` (upstream error text pinned).
///
/// 46.5: the recovery acquires the path-migrate-recovery operation lock **before**
/// `rollback_from_journal` and releases it immediately after, so a live
/// operation lock (one file shared by every operation name) refuses the
/// recovery before its first unlink and leaves the journal and every shard
/// file byte-identical. The operation *name* records who holds the lock; a
/// refusal reports the **holder's** name, never ours.
pub async fn recover_interrupted_swap(
    storage_path: &Path,
    connections: &ConnectionManager,
    registry: &dyn ShardRegistry,
    process_probe: Arc<dyn crate::platform::ProcessProbe>,
) -> Result<(), StoreError> {
    let state_path = swap_state_path(storage_path);
    if !state_path.exists() {
        return Ok(());
    }

    let recovery_lock = crate::operation_lock::acquire_turso_operation_lock(
        storage_path,
        process_probe,
        "path-migrate-recovery",
    )?;
    let outcome = rollback_from_journal(storage_path, connections, registry, &state_path).await;
    recovery_lock.release();
    match outcome {
        Ok(()) => {
            if state_path.exists() {
                Err(shard_recover_error(
                    &state_path,
                    "rollback could not restore every shard",
                ))
            } else {
                Ok(())
            }
        }
        Err(error) => Err(shard_recover_error(&state_path, &error.message())),
    }
}

fn shard_recover_error(state_path: &Path, cause: &str) -> StoreError {
    StoreError::Internal {
        message: format!(
            "Failed to recover path-migrate swap {}: {}",
            state_path.display(),
            cause
        ),
    }
}

async fn rollback_from_journal(
    storage_path: &Path,
    connections: &ConnectionManager,
    registry: &dyn ShardRegistry,
    state_path: &Path,
) -> Result<(), StoreError> {
    let body = std::fs::read_to_string(state_path).map_err(StoreError::from)?;
    let state: PathMigrateSwapState =
        serde_json::from_str(&body).map_err(|error| StoreError::Json {
            message: error.to_string(),
        })?;
    if state.operation != "path-migrate" {
        return Err(StoreError::Internal {
            message: "invalid path-migrate swap state".to_string(),
        });
    }
    // Validate EVERY path BEFORE any mutation (upstream loop spacing).
    for mv in &state.moves {
        for path in [&mv.old_path, &mv.new_path, &mv.staged_path, &mv.backup_path] {
            assert_recovery_path_inside_storage(storage_path, path)?;
        }
    }
    for archived in &state.archived_targets {
        assert_recovery_path_inside_storage(storage_path, &archived.original_path)?;
        if let Some(archive) = &archived.archive_path {
            assert_recovery_path_inside_storage(storage_path, archive)?;
        }
    }
    rollback_swap(connections, registry, &state, state_path).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Forward migration half (upstream `migrate()`)
// ---------------------------------------------------------------------------

use crate::config::StoreConfig;
use crate::operation_lock::acquire_turso_operation_lock;
use crate::platform::ProcessProbe;
use crate::project_tags::ProjectTagSource;
use crate::prompts::PromptPathUpdater;
use crate::shard_inventory::{
    MatchedBy, ResolveMigrationSourceOptions, ResolveSourceError, ShardInventory,
};
use crate::vector::{ProjectAssociation, count_all_vectors, update_project_association};

/// Upstream `PathMigrationOptions` (`currentDirectory` travels as an
/// explicit `migrate` argument — no CONFIG global).
#[derive(Debug, Clone, Default)]
pub struct PathMigrationOptions {
    pub from_path: Option<String>,
    pub from_hash: Option<String>,
    pub dry_run: bool,
    pub allow_linked_source: bool,
}

/// Upstream `PathMigrationAction.detail()` strings, pinned by tests.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum PathMigrationAction {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    ArchiveEmptyTarget {
        file: PathBuf,
    },
    UpdateMemories {
        count: u64,
    },
    UpdatePrompts {
        from: String,
        to: String,
    },
    ReassignMetadata {
        shard_id: i64,
        new_hash: String,
        shard_index: u32,
    },
}

impl PathMigrationAction {
    pub fn detail(&self) -> String {
        match self {
            Self::Rename { from, to } => format!("{} -> {}", file_base(from), file_base(to)),
            Self::ArchiveEmptyTarget { file } => file_base(file).to_string(),
            Self::UpdateMemories { count } => {
                format!("Remap container_tag and project metadata for {count} memories")
            }
            Self::UpdatePrompts { from, to } => format!("{from} -> {to}"),
            Self::ReassignMetadata {
                shard_id,
                new_hash,
                shard_index,
            } => format!("shard {shard_id} -> {new_hash}#{shard_index}"),
        }
    }
}

fn file_base(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Upstream `PathMigrationResult` (errors travel as typed
/// `PathMigrationError` instead of `{success: false}`).
#[derive(Debug, Clone)]
pub struct PathMigrationResult {
    pub dry_run: bool,
    pub migrated_shards: usize,
    pub migrated_memories: u64,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_hash: String,
    pub new_hash: String,
    pub matched_by: MatchedBy,
    pub actions: Vec<PathMigrationAction>,
}

/// Typed failure set (blueprint step 12).
#[derive(Debug, Clone)]
pub enum PathMigrationError {
    ResolveSource(ResolveSourceError),
    Aborted(String),
    Store(StoreError),
}

impl PathMigrationError {
    pub fn message(&self) -> String {
        match self {
            Self::ResolveSource(inner) => inner.message.clone(),
            Self::Aborted(message) => message.clone(),
            Self::Store(inner) => inner.to_string(),
        }
    }
}

impl std::fmt::Display for PathMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// Upstream `countLiveMemories` (missing file -> 0).
async fn count_live_memories(connections: &ConnectionManager, db_path: &Path) -> u64 {
    if !db_path.exists() {
        return 0;
    }
    let Ok(db) = connections.get_connection(db_path).await else {
        return 0;
    };
    count_all_vectors(db.as_ref() as &dyn Db).await
}

/// Upstream `writeSwapState` (private).
fn write_swap_state(
    storage_path: &Path,
    state: &PathMigrateSwapState,
) -> Result<PathBuf, StoreError> {
    let path = swap_state_path(storage_path);
    let body = serde_json::to_string(state).map_err(|error| StoreError::Json {
        message: error.to_string(),
    })?;
    std::fs::write(&path, body).map_err(StoreError::from)?;
    Ok(path)
}

/// The path-migration service (blueprint step 12 wiring).
pub struct PathMigrationService {
    storage_path: PathBuf,
    config: StoreConfig,
    connections: Arc<ConnectionManager>,
    registry: Arc<dyn ShardRegistry>,
    inventory: Arc<ShardInventory>,
    tags: Arc<dyn ProjectTagSource>,
    prompts: Arc<dyn PromptPathUpdater>,
    probe: Arc<dyn ProcessProbe>,
}

async fn count_connected(connections: &ConnectionManager, db_path: &Path) -> u64 {
    count_live_memories(connections, db_path).await
}

impl PathMigrationService {
    /// 8 injected dependencies are the upstream constructor shape — every
    /// layer (store/registry/inventory/tags/prompts/probe) stays swappable.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage_path: PathBuf,
        config: StoreConfig,
        connections: Arc<ConnectionManager>,
        registry: Arc<dyn ShardRegistry>,
        inventory: Arc<ShardInventory>,
        tags: Arc<dyn ProjectTagSource>,
        prompts: Arc<dyn PromptPathUpdater>,
        probe: Arc<dyn ProcessProbe>,
    ) -> Self {
        Self {
            storage_path,
            config,
            connections,
            registry,
            inventory,
            tags,
            prompts,
            probe,
        }
    }

    /// Upstream `migrate` (full flow, blueprint-pinned strings).
    pub async fn migrate(
        &self,
        current_directory: &Path,
        options: &PathMigrationOptions,
    ) -> Result<PathMigrationResult, PathMigrationError> {
        let target = self.tags.project_tag_info(current_directory);
        let new_hash = crate::scope::try_extract_scope_from_container_tag(&target.tag)
            .and_then(|scope| scope.hash)
            .map(|hash| hash.as_str().to_string())
            .ok_or_else(|| {
                PathMigrationError::Aborted("Invalid target tag: missing scope hash".to_string())
            })?;

        let resolved = self
            .inventory
            .resolve_migration_source(
                current_directory,
                &ResolveMigrationSourceOptions {
                    from_path: options.from_path.clone(),
                    from_hash: options.from_hash.clone(),
                },
            )
            .await
            .map_err(PathMigrationError::ResolveSource)?;

        let old_hash = resolved.scope_hash.clone();
        let old_path = resolved.group.project_path.clone();
        let matched_by = resolved.matched_by;
        let new_path = target
            .project_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());

        let noop = |actions: Vec<PathMigrationAction>, migrated: usize, memories: u64| {
            PathMigrationResult {
                dry_run: options.dry_run,
                migrated_shards: migrated,
                migrated_memories: memories,
                old_path: old_path.clone(),
                new_path: new_path.clone(),
                old_hash: old_hash.clone(),
                new_hash: new_hash.clone(),
                matched_by,
                actions,
            }
        };

        if old_hash == new_hash {
            return Ok(noop(Vec::new(), 0, 0));
        }

        if resolved.group.path_exists && !options.allow_linked_source {
            return Err(PathMigrationError::Aborted(format!(
                "Migration refused: source project path still exists ({}). \
                 Set allowLinkedSource: true only after confirming that this \
                 active project should be reassociated.",
                old_path.clone().unwrap_or_default()
            )));
        }

        // Inventory may discover a valid shard added after startup
        // reconciliation: register it so list-shards and migrate operate
        // on the same set.
        for descriptor in &resolved.group.shards {
            if descriptor.shard_id.is_none() && descriptor.file_exists {
                self.registry
                    .register_existing_shard(
                        Scope::Project,
                        &old_hash,
                        descriptor.shard_index,
                        &descriptor.db_path,
                        descriptor.memory_count,
                        descriptor.is_active,
                    )
                    .await
                    .map_err(PathMigrationError::Store)?;
            }
        }

        let source_shards = self
            .registry
            .get_all_shards(Scope::Project, Some(&old_hash))
            .await
            .map_err(PathMigrationError::Store)?;
        if source_shards.is_empty() {
            return Err(PathMigrationError::Aborted(format!(
                "No usable shards found for source hash {old_hash}"
            )));
        }

        // Strict conflict policy: any non-empty target aborts unchanged.
        let target_shards = self
            .registry
            .get_all_shards(Scope::Project, Some(&new_hash))
            .await
            .map_err(PathMigrationError::Store)?;
        for shard in &target_shards {
            let live_count = count_live_memories(&self.connections, &shard.db_path).await;
            if live_count > 0 || shard.vector_count > 0 {
                return Err(PathMigrationError::Aborted(format!(
                    "Migration aborted: target project already has {} memories \
                     (shard {}). Delete or forget those memories first, or use export/import.",
                    live_count.max(shard.vector_count),
                    file_base(&shard.db_path)
                )));
            }
        }

        let old_container_tag =
            resolved.group.container_tag.clone().unwrap_or_else(|| {
                format!("{}_project_{old_hash}", self.config.container_tag_prefix)
            });
        let nonce = format!("{}-{}", std::process::id(), now_millis_u64());
        // shard_path validates the scope hash and containment, so its refusal is a real error to propagate (ADR-0013 D-3);
        // a for loop, because propagation cannot leave the `.map()` closure
        let mut moves: Vec<ShardMove> = Vec::with_capacity(source_shards.len());
        for shard in &source_shards {
            let new_path_shard = crate::shard_manager::ShardManager::shard_path(
                &self.storage_path,
                Scope::Project,
                &new_hash,
                shard.shard_index,
            )
            .map_err(PathMigrationError::Store)?;
            let staged_basis = new_path_shard.clone();
            let inventory_shard = resolved
                .group
                .shards
                .iter()
                .find(|candidate| candidate.shard_index == shard.shard_index);
            moves.push(ShardMove {
                shard_id: shard.id,
                shard_index: shard.shard_index,
                old_path: shard.db_path.clone(),
                new_path: new_path_shard,
                staged_path: PathBuf::from(format!(
                    "{}.path-migrate-{nonce}.tmp",
                    staged_basis.display()
                )),
                backup_path: PathBuf::from(format!(
                    "{}.pre-path-migrate-{nonce}.bak",
                    shard.db_path.display()
                )),
                is_active: shard.is_active,
                vector_count: inventory_shard
                    .map(|candidate| candidate.memory_count)
                    .unwrap_or(shard.vector_count),
                old_container_tag: old_container_tag.clone(),
            });
        }

        for mv in &moves {
            if mv.new_path.exists()
                && !target_shards
                    .iter()
                    .any(|shard| shard.db_path == mv.new_path)
                && count_live_memories(&self.connections, &mv.new_path).await > 0
            {
                return Err(PathMigrationError::Aborted(format!(
                    "Migration aborted: target shard file already contains memories ({}).",
                    file_base(&mv.new_path)
                )));
            }
        }

        let mut actions: Vec<PathMigrationAction> = moves
            .iter()
            .map(|mv| PathMigrationAction::Rename {
                from: mv.old_path.clone(),
                to: mv.new_path.clone(),
            })
            .collect();
        for shard in &target_shards {
            actions.push(PathMigrationAction::ArchiveEmptyTarget {
                file: shard.db_path.clone(),
            });
        }
        let migrated_memories: u64 = {
            let mut total = 0u64;
            for mv in &moves {
                total += count_live_memories(&self.connections, &mv.old_path).await;
            }
            total
        };
        actions.push(PathMigrationAction::UpdateMemories {
            count: migrated_memories,
        });
        if let (Some(from), Some(to)) = (resolved.group.project_path.clone(), new_path.clone()) {
            actions.push(PathMigrationAction::UpdatePrompts { from, to });
        }

        if options.dry_run {
            return Ok(PathMigrationResult {
                dry_run: true,
                migrated_shards: moves.len(),
                migrated_memories,
                actions,
                ..noop(Vec::new(), 0, 0)
            });
        }

        // Operation lock: the mutation phase is exclusive. Acquired after the
        // dry-run early return (dry runs never take the lock) and held for the
        // whole swap + rollback + journal removal + prompt update (RAII
        // `Drop` releases it; `get_write_shard` blocks while it is held).
        let _lock =
            acquire_turso_operation_lock(&self.storage_path, self.probe.clone(), "path-migrate")
                .map_err(PathMigrationError::Store)?;

        // ----- mutation begins below; every failure rolls the swap back ----
        let target_association = ProjectAssociation {
            container_tag: target.tag.clone(),
            project_path: target
                .project_path
                .clone()
                .map(|p| p.to_string_lossy().to_string()),
            project_name: target.project_name.clone(),
            display_name: Some(target.display_name.clone()),
            git_repo_url: target.git_repo_url.clone(),
        };

        let archived_targets: Vec<ArchivedTarget> = target_shards
            .iter()
            .map(|shard| ArchivedTarget {
                shard_id: shard.id,
                shard_index: shard.shard_index,
                original_path: shard.db_path.clone(),
                archive_path: Some(PathBuf::from(format!(
                    "{}.pre-path-migrate-{nonce}.bak",
                    shard.db_path.display()
                ))),
                is_active: shard.is_active,
                vector_count: shard.vector_count,
            })
            .collect();
        let state = PathMigrateSwapState {
            operation: "path-migrate".to_string(),
            old_hash: old_hash.clone(),
            new_hash: new_hash.clone(),
            moves: moves.clone(),
            archived_targets,
        };

        let mutation = self
            .swap(&moves, &state, &target, &target_association)
            .await;
        match mutation {
            Ok(()) => {}
            Err(failure) => {
                let state_path = swap_state_path(&self.storage_path);
                let _ = rollback_swap(
                    &self.connections,
                    self.registry.as_ref(),
                    &state,
                    &state_path,
                )
                .await;
                return Err(PathMigrationError::Aborted(failure));
            }
        }

        let state_path = swap_state_path(&self.storage_path);
        let _ = std::fs::remove_file(&state_path);

        // Prompt metadata is secondary and must not invalidate a completed,
        // recoverable memory migration (best-effort, warn on failure).
        // Upstream guard: `source.group.projectPath && target.projectPath`.
        let prompt_paths = resolved
            .group
            .project_path
            .as_deref()
            .zip(new_path.as_deref())
            .map(|(from, to)| (from.to_string(), to.to_string()));
        if let Some((from, to)) = prompt_paths
            && let Err(error) = self.prompts.update_project_paths(&from, &to).await
        {
            tracing::warn!(error = %error, "Path migration prompt metadata update failed");
        }

        let mut final_actions = actions.clone();
        for mv in &moves {
            final_actions.push(PathMigrationAction::ReassignMetadata {
                shard_id: mv.shard_id,
                new_hash: new_hash.clone(),
                shard_index: mv.shard_index,
            });
        }

        Ok(PathMigrationResult {
            dry_run: false,
            migrated_shards: moves.len(),
            migrated_memories,
            old_path,
            new_path,
            old_hash: old_hash.clone(),
            new_hash: new_hash.clone(),
            matched_by,
            actions: final_actions,
        })
    }

    /// The stretch between `writeSwapState` and journal removal.
    async fn swap(
        &self,
        moves: &[ShardMove],
        state: &PathMigrateSwapState,
        target: &mem_core::tags::TagInfo,
        target_association: &ProjectAssociation,
    ) -> Result<(), String> {
        // Stage complete, updated copies while originals remain untouched.
        for mv in moves {
            self.connections
                .close_connection(&mv.old_path)
                .await
                .map_err(|error| error.to_string())?;
            if !mv.old_path.exists() {
                return Err(format!(
                    "Source shard file missing: {}",
                    mv.old_path.display()
                ));
            }
            remove_file_helper(&self.connections, &mv.staged_path)
                .await
                .map_err(|error| error.to_string())?;
            let Some(proof) = self.connections.exclusive_root() else {
                return Err(StoreError::NoExclusiveRoot {
                    db_path: mv.staged_path.clone(),
                }
                .to_string());
            };
            let staged_claim = proof
                .claim(&mv.staged_path)
                .map_err(|error| error.to_string())?;
            crate::sqlite_files::copy_sqlite_database(&mv.old_path, staged_claim)
                .map_err(|error| error.to_string())?;

            let staged = self
                .connections
                .get_connection(&mv.staged_path)
                .await
                .map_err(|error| error.to_string())?;
            let staged_db = staged.as_ref() as &dyn Db;
            update_project_association(staged_db, &mv.old_container_tag, target_association)
                .await
                .map_err(|error| error.to_string())?;
            staged_db
                .execute(
                    crate::sql::UPDATE_MEMORIES_PROJECT_ASSOCIATION,
                    vec![
                        Value::Text(target.tag.clone()),
                        target
                            .project_path
                            .as_ref()
                            .map(|p| Value::Text(p.to_string_lossy().to_string()))
                            .unwrap_or(Value::Null),
                        target.project_name.clone().map_or(Value::Null, Value::Text),
                        Value::Text(target.display_name.clone()),
                        target.git_repo_url.clone().map_or(Value::Null, Value::Text),
                        Value::Text(target.tag.clone()),
                    ],
                )
                .await
                .map_err(|error| error.to_string())?;
            let staged_count = count_all_vectors(staged_db).await;
            if staged_count != mv.vector_count {
                return Err(format!(
                    "Staged shard count mismatch for {}: expected {}, got {}",
                    file_base(&mv.old_path),
                    mv.vector_count,
                    staged_count
                ));
            }
            drop(staged);
            self.connections
                .close_connection(&mv.staged_path)
                .await
                .map_err(|error| error.to_string())?;
        }

        // The journal only exists once renames may start.
        write_swap_state(&self.storage_path, state).map_err(|error| error.to_string())?;

        // Empty targets remain recoverable until the complete swap succeeds.
        for shard in &state.archived_targets {
            self.registry
                .archive_shard(
                    shard.shard_id,
                    "pre-path-migrate",
                    shard.archive_path.as_deref(),
                )
                .await
                .map_err(|error| error.to_string())?;
        }

        for mv in moves {
            self.connections
                .close_connection(&mv.old_path)
                .await
                .map_err(|error| error.to_string())?;
            let Some(proof) = self.connections.exclusive_root() else {
                return Err(StoreError::NoExclusiveRoot {
                    db_path: mv.old_path.clone(),
                }
                .to_string());
            };
            let old_claim = proof
                .claim(&mv.old_path)
                .map_err(|error| error.to_string())?;
            let backup_claim = proof
                .claim(&mv.backup_path)
                .map_err(|error| error.to_string())?;
            let staged_claim = proof
                .claim(&mv.staged_path)
                .map_err(|error| error.to_string())?;
            let new_claim = proof
                .claim(&mv.new_path)
                .map_err(|error| error.to_string())?;
            with_sqlite_file_lock_retry(
                || rename_sqlite_database(old_claim, backup_claim),
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &crate::sqlite_files::system_retry_context(),
            )
            .map_err(|error| error.to_string())?;
            if let Err(restore_error) = with_sqlite_file_lock_retry(
                || rename_sqlite_database(staged_claim, new_claim),
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &crate::sqlite_files::system_retry_context(),
            ) {
                // Rename staged -> new failed; restore the backup then throw.
                let _ = with_sqlite_file_lock_retry(
                    || rename_sqlite_database(backup_claim, old_claim),
                    FILE_LOCK_RETRY_DELAYS_MS.len(),
                    &crate::sqlite_files::system_retry_context(),
                );
                return Err(restore_error.to_string());
            }

            self.registry
                .reassign_shard_scope(
                    mv.shard_id,
                    &state.new_hash,
                    &mv.new_path,
                    mv.vector_count,
                    mv.is_active,
                )
                .await
                .map_err(|error| error.to_string())?;
        }

        Ok(())
    }
}

/// Upstream `Date.now()` (no chrono dependency).
fn now_millis_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_millis() as u64)
        .unwrap_or(0)
}
