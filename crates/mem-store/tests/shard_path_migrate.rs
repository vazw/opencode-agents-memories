//! Shard path-migration battery (blueprint W1.2 step 12): the forward
//! `migrate()` flow, dry-run, refusals/conflicts, partial-swap rollback,
//! recovery-by-gate and pinned upstream error fragments
//! (upstream shard-path-migration-service.ts).

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{FakeTagSource, FaultingRegistry, RecordingPromptUpdater, write_swap_fixture};
use mem_core::tags::TagInfo;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, ConnectOptions};
use mem_store::error::StoreError;
use mem_store::operation_lock::OPERATION_LOCK_FILE;
use mem_store::platform::Platform;
use mem_store::scope::Scope;
use mem_store::shard_inventory::ShardInventory;
use mem_store::shard_manager::{ShardManager, ShardRegistry};
use mem_store::shard_path_migration::{
    PathMigrateSwapState, PathMigrationError, PathMigrationOptions, PathMigrationService,
    ShardMove, swap_state_path,
};
use mem_store::vector::{MemoryInsert, count_all_vectors, insert_vector};

const OLD_HASH: &str = "a1b2c3d4e5f60001";
const TARGET_HASH: &str = "ffffffffffffffff";

struct Rig {
    config: StoreConfig,
    shards: ShardManager,
    registry: Arc<dyn ShardRegistry>,
    service: PathMigrationService,
    prompts: Arc<RecordingPromptUpdater>,
    current_dir: PathBuf,
}

async fn rig(prefix: &str) -> Rig {
    let config = common::test_store_config(prefix, 2, 100);
    let current_dir = config.storage_path.join("current-workdir");
    std::fs::create_dir_all(&current_dir).expect("cwd");
    let target_tag = format!("opencode_project_{TARGET_HASH}");
    let tags: Arc<dyn mem_store::project_tags::ProjectTagSource> = {
        let mut map = std::collections::HashMap::new();
        map.insert(
            current_dir.to_string_lossy().to_string(),
            TagInfo {
                tag: target_tag,
                display_name: current_dir.to_string_lossy().to_string(),
                user_name: None,
                user_email: None,
                project_path: Some(current_dir.clone()),
                project_name: Some("TargetProject".to_string()),
                git_repo_url: None,
            },
        );
        Arc::new(FakeTagSource {
            map: std::sync::Mutex::new(map),
            fallback: None,
        })
    };
    rig_with_tags(prefix, tags, current_dir).await
}

async fn rig_with_tags(
    prefix: &str,
    tags: Arc<dyn mem_store::project_tags::ProjectTagSource>,
    current_dir: PathBuf,
) -> Rig {
    let config = common::test_store_config(prefix, 2, 100);
    let (config, connections, shards) = common::real_components(config).await;
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());
    let inventory = Arc::new(ShardInventory::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        registry.clone(),
        tags.clone(),
    ));
    let prompts = Arc::new(RecordingPromptUpdater::default());
    let service = PathMigrationService::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        registry.clone(),
        inventory.clone(),
        tags.clone(),
        prompts.clone(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    Rig {
        config,
        shards,
        registry,
        service,
        prompts,
        current_dir,
    }
}

fn record(hash: &str, id: &str, created: i64, project_path: Option<&Path>) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: format!("content-{id}"),
        vector: vec![1.0f32, 0.0],
        tags_vector: None,
        container_tag: format!("opencode_project_{hash}"),
        tags: None,
        memory_type: None,
        created_at: created,
        updated_at: created,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: project_path.map(|p| p.to_string_lossy().to_string()),
        project_name: Some("FixtureProject".to_string()),
        git_repo_url: None,
    }
}

async fn seed_shard(
    rig: &Rig,
    hash: &str,
    index: u32,
    count: usize,
    project_path: Option<&Path>,
) -> mem_store::shard_manager::ShardInfo {
    let shard = rig
        .shards
        .create_shard(Scope::Project, hash, index)
        .await
        .expect("create shard");
    let db = rig.shards.open_shard(&shard.db_path).await.expect("open");
    for i in 0..count {
        insert_vector(
            db.as_ref(),
            &record(hash, &format!("m{index}_{i}"), i as i64, project_path),
        )
        .await
        .expect("insert");
    }
    shard
}

fn shard_file(storage: &Path, hash: &str, index: u32) -> PathBuf {
    storage
        .join("projects")
        .join(format!("project_{hash}_shard_{index}.db"))
}

fn backups(storage: &Path) -> Vec<String> {
    std::fs::read_dir(storage.join("projects"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.contains(".pre-path-migrate-") && name.ends_with(".bak"))
                .collect()
        })
        .unwrap_or_default()
}

fn staged_leftovers(storage: &Path) -> Vec<String> {
    std::fs::read_dir(storage.join("projects"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.contains(".path-migrate-") && name.ends_with(".tmp"))
                .collect()
        })
        .unwrap_or_default()
}

fn aborted(error: PathMigrationError) -> String {
    match error {
        PathMigrationError::Aborted(message) => message,
        other => panic!("expected Aborted, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 1/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_migrates_orphaned_shards_and_remaps_container_tags() {
    let gone = std::env::temp_dir().join("mem-pm-gone-0xa1-must-not-exist");
    assert!(!gone.exists());
    let rig = rig("pm-migrate").await;
    seed_shard(&rig, OLD_HASH, 0, 2, Some(&gone)).await;

    let result = rig
        .service
        .migrate(
            &rig.current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                dry_run: false,
                allow_linked_source: false,
                ..Default::default()
            },
        )
        .await
        .expect("migrate");

    assert_eq!(result.migrated_shards, 1);
    assert_eq!(result.migrated_memories, 2);
    assert_eq!(result.old_hash, OLD_HASH);
    assert_eq!(result.new_hash, TARGET_HASH);

    let new_file = shard_file(&rig.config.storage_path, TARGET_HASH, 0);
    assert!(
        !shard_file(&rig.config.storage_path, OLD_HASH, 0).exists(),
        "old file moved"
    );
    assert!(new_file.exists(), "remapped shard at the new path");
    assert!(
        staged_leftovers(&rig.config.storage_path).is_empty(),
        "staged copies consumed"
    );
    assert!(
        !backups(&rig.config.storage_path).is_empty(),
        "the pre-swap backup REMAINS (upstream keeps it for recovery)"
    );

    let db = rig.shards.open_shard(&new_file).await.expect("reopen");
    assert_eq!(count_all_vectors(db.as_ref()).await, 2);
    let rows = mem_store::vector::get_distinct_tags(db.as_ref()).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].text("container_tag").expect("tag"),
        format!("opencode_project_{TARGET_HASH}"),
        "container tag remapped"
    );
    assert_eq!(
        rows[0].text("project_path").expect("path"),
        rig.current_dir.to_string_lossy().to_string(),
        "project path remapped"
    );

    assert!(
        rig.registry
            .get_all_shards(Scope::Project, Some(OLD_HASH))
            .await
            .expect("old rows")
            .is_empty(),
        "source hash empty after move"
    );
    let moved = rig
        .registry
        .get_all_shards(Scope::Project, Some(TARGET_HASH))
        .await
        .expect("new rows");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].db_path, new_file);
    assert!(moved[0].is_active, "active preserved");

    assert!(
        !swap_state_path(&rig.config.storage_path).exists(),
        "journal removed"
    );
    let calls = rig.prompts.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one prompt update");
    assert_eq!(calls[0].1, rig.current_dir.to_string_lossy().to_string());
}

// ---------------------------------------------------------------------------
// 2/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_aborts_unchanged_when_target_project_already_has_memories() {
    let rig = rig("pm-conflict").await;
    seed_shard(&rig, TARGET_HASH, 0, 1, Some(&rig.current_dir)).await;
    seed_shard(&rig, OLD_HASH, 0, 1, None).await;

    let error = aborted(
        rig.service
            .migrate(
                &rig.current_dir,
                &PathMigrationOptions {
                    from_hash: Some(OLD_HASH.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect_err("conflict"),
    );
    assert!(
        error.contains("Migration aborted: target project already has 1 memories"),
        "{error}"
    );
    assert!(error.contains("export/import"), "{error}");

    // Zero mutations: both files untouched, no backups, no journal.
    let target_db = rig
        .shards
        .open_shard(&shard_file(&rig.config.storage_path, TARGET_HASH, 0))
        .await
        .expect("target open");
    assert_eq!(
        count_all_vectors(target_db.as_ref()).await,
        1,
        "target untouched"
    );
    assert!(
        shard_file(&rig.config.storage_path, OLD_HASH, 0).exists(),
        "source untouched"
    );
    assert!(backups(&rig.config.storage_path).is_empty());
    assert!(!swap_state_path(&rig.config.storage_path).exists());
    assert!(rig.prompts.calls.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 3/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_archives_empty_target_shard_and_completes_migration() {
    let rig = rig("pm-archive").await;
    let target = seed_shard(&rig, TARGET_HASH, 0, 0, None).await; // registered + empty
    seed_shard(&rig, OLD_HASH, 0, 1, None).await;

    let result = rig
        .service
        .migrate(
            &rig.current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("archive path succeeds");

    let target_file = shard_file(&rig.config.storage_path, TARGET_HASH, 0);
    let base = target_file
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let leftovers = backups(&rig.config.storage_path);
    assert!(
        leftovers.iter().any(|name| {
            name.starts_with(&format!("{base}.pre-path-migrate-")) && name.ends_with(".bak")
        }),
        "archived target: {leftovers:?}"
    );
    // The archived file is the OLD empty target; the target path now holds
    // the migrated source content.
    assert!(target_file.exists(), "migrated content at the target path");
    assert!(result.actions.iter().any(|action| {
        matches!(
            action,
            mem_store::shard_path_migration::PathMigrationAction::ArchiveEmptyTarget { file }
                if file.file_name().map(|n| n.to_string_lossy().to_string()).as_deref() == Some(base.as_str())
        )
    }), "archive action listed");
    assert!(
        rig.registry
            .get_shard_by_id(target.id)
            .await
            .expect("archived row")
            .is_none(),
        "the archived EMPTY target's row is removed by the archive"
    );

    let db = rig.shards.open_shard(&target_file).await.expect("open");
    assert_eq!(count_all_vectors(db.as_ref()).await, 1, "migrated content");
}

// ---------------------------------------------------------------------------
// 4/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_supports_dry_run_without_mutating_shards() {
    let rig = rig("pm-dry").await;
    seed_shard(&rig, OLD_HASH, 0, 1, None).await;

    let result = rig
        .service
        .migrate(
            &rig.current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                dry_run: true,
                ..Default::default()
            },
        )
        .await
        .expect("dry run");

    assert!(result.dry_run);
    assert_eq!(result.migrated_shards, 1);
    assert_eq!(result.migrated_memories, 1);
    assert!(
        result.actions.iter().any(|action| {
            matches!(
                action,
                mem_store::shard_path_migration::PathMigrationAction::UpdateMemories { count: 1 }
            )
        }),
        "actions describe the plan"
    );

    assert!(shard_file(&rig.config.storage_path, OLD_HASH, 0).exists());
    assert!(!shard_file(&rig.config.storage_path, TARGET_HASH, 0).exists());
    assert!(
        !rig.config
            .storage_path
            .join(".turso-operation.lock")
            .exists(),
        "dry run never took the storage lock"
    );
    assert!(!swap_state_path(&rig.config.storage_path).exists());
    assert!(rig.prompts.calls.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 5/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_refuses_still_linked_source_unless_explicitly_allowed() {
    let live = std::env::temp_dir().join("mem-pm-live-0xa5");
    let _ = std::fs::remove_dir_all(&live);
    std::fs::create_dir_all(&live).expect("linked dir");
    let rig = rig("pm-linked").await;
    seed_shard(&rig, OLD_HASH, 0, 1, Some(&live)).await;

    let refused = aborted(
        rig.service
            .migrate(
                &rig.current_dir,
                &PathMigrationOptions {
                    from_hash: Some(OLD_HASH.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect_err("refused"),
    );
    assert!(
        refused.contains("Migration refused: source project path still exists"),
        "{refused}"
    );

    let result = rig
        .service
        .migrate(
            &rig.current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                allow_linked_source: true,
                ..Default::default()
            },
        )
        .await
        .expect("allowed");
    assert_eq!(result.migrated_shards, 1);
    assert!(
        !shard_file(&rig.config.storage_path, OLD_HASH, 0).exists(),
        "source moved"
    );
    assert_eq!(
        count_all_vectors(
            rig.shards
                .open_shard(&shard_file(&rig.config.storage_path, TARGET_HASH, 0))
                .await
                .expect("new")
                .as_ref()
        )
        .await,
        1
    );
    let _ = std::fs::remove_dir_all(&live);
}

// ---------------------------------------------------------------------------
// 6/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_restores_exact_source_files_and_registry_metadata_after_partial_swap_failure() {
    let rig = rig("pm-fault").await;
    seed_shard(&rig, OLD_HASH, 0, 1, None).await;
    seed_shard(&rig, OLD_HASH, 1, 1, None).await;

    let faulting = FaultingRegistry {
        inner: rig.registry.clone(),
        old_hash: OLD_HASH.to_string(),
        fail_nth: std::sync::atomic::AtomicUsize::new(2),
        forward_calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let registry: Arc<dyn ShardRegistry> = Arc::new(faulting);
    let tags = rig_inventory_tags(&rig);
    let inventory = Arc::new(ShardInventory::new(
        rig.config.storage_path.clone(),
        rig.config.clone(),
        Arc::new(ConnectionManager::new(
            rig.config.storage_path.clone(),
            rig.config.clone(),
            Platform::current(),
            Arc::new(mem_store::db::TursoConnectFactory),
        )),
        registry.clone(),
        tags.clone(),
    ));
    let prompts = Arc::new(RecordingPromptUpdater::default());
    let service = PathMigrationService::new(
        rig.config.storage_path.clone(),
        rig.config.clone(),
        Arc::new(ConnectionManager::new(
            rig.config.storage_path.clone(),
            rig.config.clone(),
            Platform::current(),
            Arc::new(mem_store::db::TursoConnectFactory),
        )),
        registry.clone(),
        inventory.clone(),
        tags.clone(),
        prompts.clone(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );

    let error = aborted(
        service
            .migrate(
                &rig.current_dir,
                &PathMigrationOptions {
                    from_hash: Some(OLD_HASH.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect_err("second reassign faults"),
    );
    assert!(error.contains("forced reassign fault"), "{error}");

    // Both source files back with their original bytes and placement.
    for index in [0u32, 1] {
        let old = shard_file(&rig.config.storage_path, OLD_HASH, index);
        let db = rig.shards.open_shard(&old).await.expect("restored");
        assert_eq!(
            count_all_vectors(db.as_ref()).await,
            1,
            "shard {index} content restored"
        );
        let rows = mem_store::vector::get_distinct_tags(db.as_ref()).await;
        assert_eq!(
            rows[0].text("container_tag").expect("tag"),
            format!("opencode_project_{OLD_HASH}"),
            "shard {index} kept the ORIGINAL container tag"
        );
        drop(db);
    }
    assert!(
        !shard_file(&rig.config.storage_path, TARGET_HASH, 0).exists(),
        "new shard file removed"
    );
    assert!(
        !shard_file(&rig.config.storage_path, TARGET_HASH, 1).exists(),
        "new shard file (index 1) removed"
    );
    assert!(
        backups(&rig.config.storage_path).is_empty(),
        "backups restored onto the source"
    );
    let rows = rig
        .registry
        .get_all_shards(Scope::Project, Some(OLD_HASH))
        .await
        .expect("rows");
    assert_eq!(rows.len(), 2, "both rows back on the old hash");
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.shard_index, index as u32);
        assert_eq!(
            row.db_path,
            shard_file(&rig.config.storage_path, OLD_HASH, index as u32),
            "row {index} path restored"
        );
    }
    assert!(
        rig.registry
            .get_all_shards(Scope::Project, Some(TARGET_HASH))
            .await
            .expect("target rows")
            .is_empty()
    );
    assert!(
        !swap_state_path(&rig.config.storage_path).exists(),
        "journal removed after a clean rollback"
    );
    assert!(
        prompts.calls.lock().unwrap().is_empty(),
        "prompts never ran"
    );
}

fn rig_inventory_tags(rig: &Rig) -> Arc<dyn mem_store::project_tags::ProjectTagSource> {
    let mut map = std::collections::HashMap::new();
    map.insert(
        rig.current_dir.to_string_lossy().to_string(),
        TagInfo {
            tag: format!("opencode_project_{TARGET_HASH}"),
            display_name: rig.current_dir.to_string_lossy().to_string(),
            user_name: None,
            user_email: None,
            project_path: Some(rig.current_dir.clone()),
            project_name: Some("TargetProject".to_string()),
            git_repo_url: None,
        },
    );
    Arc::new(FakeTagSource {
        map: std::sync::Mutex::new(map),
        fallback: None,
    })
}

// ---------------------------------------------------------------------------
// 7/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_migrates_valid_disk_only_shard_discovered_after_startup() {
    let rig = rig("pm-disk").await;
    let file = shard_file(&rig.config.storage_path, OLD_HASH, 0);
    std::fs::create_dir_all(file.parent().expect("projects")).expect("projects dir");
    let opts = ConnectOptions::for_config(&rig.config, Platform::current());
    let db = mem_store::db::turso_connect_factory()
        .connect(file.clone(), opts.clone())
        .await
        .expect("raw open");
    mem_store::schema_migrations::apply_schema_migrations(
        db.as_ref(),
        &mem_store::schema_migrations::memory_shard_migrations(2),
        &mem_store::schema_migrations::MigrationOptions {
            db_path: None,
            label: "disk-only",
        },
    )
    .await
    .expect("schema");
    insert_vector(db.as_ref(), &record(OLD_HASH, "disk", 1, None))
        .await
        .expect("insert");

    let result = rig
        .service
        .migrate(
            &rig.current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("disk-only migrate");
    assert_eq!(result.migrated_shards, 1);
    assert_eq!(result.migrated_memories, 1);
    let new_file = shard_file(&rig.config.storage_path, TARGET_HASH, 0);
    assert!(new_file.exists());
    let db = rig.shards.open_shard(&new_file).await.expect("open");
    assert_eq!(count_all_vectors(db.as_ref()).await, 1);
}

// ---------------------------------------------------------------------------
// 8/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_recovers_interrupted_swap_during_storage_ready_gate() {
    let config = common::store_config("pm-recover");
    let (config, connections, shards) = common::real_components(config).await;
    let storage = config.storage_path.clone();
    let old_path = shard_file(&storage, OLD_HASH, 0);
    let new_path = shard_file(&storage, TARGET_HASH, 0);
    std::fs::create_dir_all(old_path.parent().expect("projects")).expect("projects dir");

    let info = shards
        .register_existing_shard(Scope::Project, OLD_HASH, 0, &old_path, 1, true)
        .await
        .expect("row");
    // World at crash time: old renamed to backup; new (remapped copy)
    // in place; the reassign never ran.
    let baseline = mem_store::db::turso_connect_factory()
        .connect(
            old_path.clone(),
            ConnectOptions::for_config(&config, Platform::current()),
        )
        .await
        .expect("old open");
    mem_store::schema_migrations::apply_schema_migrations(
        baseline.as_ref(),
        &mem_store::schema_migrations::memory_shard_migrations(2),
        &mem_store::schema_migrations::MigrationOptions {
            db_path: None,
            label: "recover-fixture",
        },
    )
    .await
    .expect("schema");
    insert_vector(baseline.as_ref(), &record(OLD_HASH, "crash", 1, None))
        .await
        .expect("insert");
    // Byte-copies follow: fold the WAL into the main db first. (The
    // checkpoint PRAGMA produces a row, so it goes through `query`.)
    let _ = baseline
        .query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
        .await;
    drop(baseline);
    mem_store::sqlite_files::copy_sqlite_database(&old_path, &new_path).expect("copy to new");
    let backup = old_path.with_file_name(format!(
        "{}.pre-path-migrate-crash.bak",
        old_path.file_name().unwrap().to_string_lossy()
    ));
    mem_store::sqlite_files::copy_sqlite_database(&old_path, &backup).expect("copy to backup");
    std::fs::remove_file(&old_path).expect("simulate old->backup rename");

    write_swap_fixture(
        &storage,
        &PathMigrateSwapState {
            operation: "path-migrate".to_string(),
            old_hash: OLD_HASH.to_string(),
            new_hash: TARGET_HASH.to_string(),
            moves: vec![ShardMove {
                shard_id: info.id,
                shard_index: 0,
                old_path: old_path.clone(),
                new_path: new_path.clone(),
                staged_path: old_path.with_file_name(format!(
                    "{}.path-migrate-crash.tmp",
                    old_path.file_name().unwrap().to_string_lossy()
                )),
                backup_path: backup.clone(),
                is_active: true,
                vector_count: 1,
                old_container_tag: format!("opencode_project_{OLD_HASH}"),
            }],
            archived_targets: Vec::new(),
        },
    );

    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());
    let gate = Arc::new(mem_store::ready::build_ready_gate(
        &storage,
        connections.clone(),
        Arc::new(shards.clone()),
        registry.clone(),
    ));
    gate.ensure_ready().await.expect("gate recovers the swap");

    assert!(old_path.exists(), "old restored from backup");
    assert!(!new_path.exists(), "half-swapped new removed");
    assert!(!backup.exists(), "backup consumed by the restore");
    assert!(!swap_state_path(&storage).exists(), "journal gone");
    let db = mem_store::db::turso_connect_factory()
        .connect(
            old_path.clone(),
            ConnectOptions::for_config(&config, Platform::current()),
        )
        .await
        .expect("reopen");
    assert_eq!(count_all_vectors(db.as_ref()).await, 1);
    let rows = mem_store::vector::get_distinct_tags(db.as_ref()).await;
    assert_eq!(
        rows[0].text("container_tag").expect("tag"),
        format!("opencode_project_{OLD_HASH}"),
        "original bytes restored"
    );
}

// ---------------------------------------------------------------------------
// 9/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_same_hash_source_is_noop_success() {
    // The seed shard's stored project path exists on disk: a DIFFERENT hash
    // would be refused as linked, but the same-hash no-op returns first.
    let live = std::env::temp_dir().join("mem-pm-same-0xa9");
    let _ = std::fs::remove_dir_all(&live);
    std::fs::create_dir_all(&live).expect("linked dir");
    let tags: Arc<dyn mem_store::project_tags::ProjectTagSource> = {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "/fixture-cwd".to_string(),
            TagInfo {
                tag: format!("opencode_project_{OLD_HASH}"),
                display_name: "/fixture-cwd".to_string(),
                user_name: None,
                user_email: None,
                project_path: Some(PathBuf::from("/fixture-cwd")),
                project_name: Some("TargetProject".to_string()),
                git_repo_url: None,
            },
        );
        Arc::new(FakeTagSource {
            map: std::sync::Mutex::new(map),
            fallback: None,
        })
    };
    let config = common::test_store_config("pm-same", 2, 100);
    let (config, connections, shards) = common::real_components(config).await;
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());
    let inventory = Arc::new(ShardInventory::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        registry.clone(),
        tags.clone(),
    ));
    let prompts = Arc::new(RecordingPromptUpdater::default());
    let service = PathMigrationService::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        registry.clone(),
        inventory.clone(),
        tags.clone(),
        prompts.clone(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );

    let shard = shards
        .create_shard(Scope::Project, OLD_HASH, 0)
        .await
        .expect("shard");
    {
        let db = shards.open_shard(&shard.db_path).await.expect("open");
        for i in 0..2 {
            insert_vector(
                db.as_ref(),
                &record(OLD_HASH, &format!("same{i}"), i, Some(&live)),
            )
            .await
            .expect("insert");
        }
    }

    let result = service
        .migrate(
            Path::new("/fixture-cwd"),
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                allow_linked_source: false, // would be refused for a different hash
                ..Default::default()
            },
        )
        .await
        .expect("no-op success");
    assert_eq!(result.migrated_shards, 0);
    assert_eq!(result.migrated_memories, 0);
    assert!(result.actions.is_empty());
    assert!(prompts.calls.lock().unwrap().is_empty(), "no prompt churn");
    assert!(
        shard_file(&config.storage_path, OLD_HASH, 0).exists(),
        "file untouched"
    );
    let _ = std::fs::remove_dir_all(&live);
}

#[tokio::test]
async fn test_recovery_rejects_journal_paths_outside_storage() {
    let config = common::store_config("pm-escape");
    let (config, connections, shards) = common::real_components(config).await;
    let storage = config.storage_path.clone();
    let outside_dir = std::env::temp_dir().join("mem-pm-escape-0xaa");
    std::fs::create_dir_all(&outside_dir).expect("outside dir");
    let outside = outside_dir.join("hostile.db");
    std::fs::write(&outside, b"untouched").expect("outside file");

    write_swap_fixture(
        &storage,
        &PathMigrateSwapState {
            operation: "path-migrate".to_string(),
            old_hash: OLD_HASH.to_string(),
            new_hash: TARGET_HASH.to_string(),
            moves: vec![ShardMove {
                shard_id: 1,
                shard_index: 0,
                old_path: outside.clone(),
                new_path: shard_file(&storage, TARGET_HASH, 0),
                staged_path: storage.join("projects").join("staged.tmp"),
                backup_path: storage.join("projects").join("backup.bak"),
                is_active: true,
                vector_count: 1,
                old_container_tag: format!("opencode_project_{OLD_HASH}"),
            }],
            archived_targets: Vec::new(),
        },
    );

    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());
    let error = mem_store::shard_path_migration::recover_interrupted_swap(
        &storage,
        &connections,
        registry.as_ref(),
    )
    .await
    .expect_err("outside storage rejected");
    let message = error.to_string();
    assert!(
        message.contains("Failed to recover path-migrate swap"),
        "{message}"
    );
    assert!(
        message.contains("Path-migrate recovery path escapes storage"),
        "{message}"
    );
    assert_eq!(
        std::fs::read(&outside).expect("outside intact"),
        b"untouched",
        "ZERO file ops: the outside file survives"
    );
    assert!(
        swap_state_path(&storage).exists(),
        "journal kept for diagnostics"
    );
    let _ = std::fs::remove_dir_all(&outside_dir);
}

// ---------------------------------------------------------------------------
// 11/11
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_prompt_update_failure_does_not_rollback_completed_migration() {
    let config = common::test_store_config("pm-promptfail", 2, 100);
    let current_dir = config.storage_path.join("current-workdir");
    std::fs::create_dir_all(&current_dir).expect("cwd");
    let tags: Arc<dyn mem_store::project_tags::ProjectTagSource> = {
        let mut map = std::collections::HashMap::new();
        map.insert(
            current_dir.to_string_lossy().to_string(),
            TagInfo {
                tag: format!("opencode_project_{TARGET_HASH}"),
                display_name: current_dir.to_string_lossy().to_string(),
                user_name: None,
                user_email: None,
                project_path: Some(current_dir.clone()),
                project_name: Some("TargetProject".to_string()),
                git_repo_url: None,
            },
        );
        Arc::new(FakeTagSource {
            map: std::sync::Mutex::new(map),
            fallback: None,
        })
    };
    let (config, connections, shards) = common::real_components(config).await;
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());
    let inventory = Arc::new(ShardInventory::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        registry.clone(),
        tags.clone(),
    ));
    let prompts = Arc::new(RecordingPromptUpdater {
        fail: std::sync::atomic::AtomicBool::new(true),
        ..Default::default()
    });
    let service = PathMigrationService::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        registry.clone(),
        inventory.clone(),
        tags.clone(),
        prompts.clone(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );

    let shard = shards
        .create_shard(Scope::Project, OLD_HASH, 0)
        .await
        .expect("shard");
    {
        let db = shards.open_shard(&shard.db_path).await.expect("open");
        let gone = std::env::temp_dir().join("mem-pm-promptfail-gone");
        insert_vector(db.as_ref(), &record(OLD_HASH, "m0", 1, Some(&gone)))
            .await
            .expect("insert");
    }

    let result = service
        .migrate(
            &current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("migration succeeds despite prompt fault");
    assert_eq!(result.migrated_shards, 1);
    assert_eq!(
        prompts.calls.lock().unwrap().len(),
        1,
        "the update WAS attempted"
    );
    let new_file = shard_file(&config.storage_path, TARGET_HASH, 0);
    assert!(new_file.exists(), "files stay migrated");
    let moved = registry
        .get_all_shards(Scope::Project, Some(TARGET_HASH))
        .await
        .expect("rows");
    assert_eq!(moved.len(), 1, "registry stays migrated");
}

// ---------------------------------------------------------------------------
// Rework: the operation-lock producer in migrate()
// ---------------------------------------------------------------------------

/// Decorator that, on the FIRST `reassign_shard_scope`, records whether the
/// storage operation lock exists — proving the lock is HELD during the
/// rename/registry mutation — then delegates.
struct LockObservingRegistry {
    inner: Arc<dyn ShardRegistry>,
    storage_path: PathBuf,
    lock_seen: Arc<std::sync::atomic::AtomicBool>,
    observed: std::sync::atomic::AtomicUsize,
}

impl ShardRegistry for LockObservingRegistry {
    fn get_all_shards<'a>(
        &'a self,
        scope: Scope,
        scope_hash: Option<&'a str>,
    ) -> mem_store::db::BoxFuture<'a, Result<Vec<mem_store::shard_manager::ShardInfo>, StoreError>>
    {
        self.inner.get_all_shards(scope, scope_hash)
    }

    fn register_existing_shard<'a>(
        &'a self,
        scope: Scope,
        scope_hash: &'a str,
        shard_index: u32,
        db_path: &'a Path,
        vector_count: u64,
        is_active: bool,
    ) -> mem_store::db::BoxFuture<'a, Result<mem_store::shard_manager::ShardInfo, StoreError>> {
        self.inner.register_existing_shard(
            scope,
            scope_hash,
            shard_index,
            db_path,
            vector_count,
            is_active,
        )
    }

    fn archive_shard<'a>(
        &'a self,
        shard_id: i64,
        reason: &'a str,
        archive_path_override: Option<&'a Path>,
    ) -> mem_store::db::BoxFuture<'a, Result<Option<PathBuf>, StoreError>> {
        self.inner
            .archive_shard(shard_id, reason, archive_path_override)
    }

    fn reassign_shard_scope<'a>(
        &'a self,
        shard_id: i64,
        new_scope_hash: &'a str,
        new_db_path: &'a Path,
        vector_count: u64,
        is_active: bool,
    ) -> mem_store::db::BoxFuture<'a, Result<mem_store::shard_manager::ShardInfo, StoreError>> {
        use std::sync::atomic::Ordering;
        let call = self.observed.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let lock = self.storage_path.join(OPERATION_LOCK_FILE);
            self.lock_seen.store(lock.exists(), Ordering::SeqCst);
        }
        self.inner.reassign_shard_scope(
            shard_id,
            new_scope_hash,
            new_db_path,
            vector_count,
            is_active,
        )
    }

    fn delete_shard<'a>(
        &'a self,
        shard_id: i64,
    ) -> mem_store::db::BoxFuture<'a, Result<(), StoreError>> {
        self.inner.delete_shard(shard_id)
    }

    fn get_shard_by_id<'a>(
        &'a self,
        shard_id: i64,
    ) -> mem_store::db::BoxFuture<'a, Result<Option<mem_store::shard_manager::ShardInfo>, StoreError>>
    {
        self.inner.get_shard_by_id(shard_id)
    }
}

#[tokio::test]
async fn test_migrate_honors_preseeded_live_operation_lock() {
    let gone = std::env::temp_dir().join("mem-pm-lock-gone-0xa1-must-not-exist");
    assert!(!gone.exists());
    let rig = rig("pm-lock").await;
    seed_shard(&rig, OLD_HASH, 0, 1, Some(&gone)).await;

    // A LIVE foreign operation lock (owned by the current process) must be
    // honored at mutation-phase entry, BEFORE any rename.
    let lock_path = common::write_lock_state(
        &rig.config.storage_path,
        OPERATION_LOCK_FILE,
        std::process::id(),
        Some("some-other-operation"),
    );
    assert!(lock_path.exists());

    let error = rig
        .service
        .migrate(
            &rig.current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                dry_run: false,
                ..Default::default()
            },
        )
        .await
        .expect_err("a live foreign lock must block the migration");

    match error {
        PathMigrationError::Store(StoreError::MigrationLocked { pid, .. }) => {
            assert_eq!(pid, std::process::id(), "the live owner is reported");
        }
        other => panic!("expected Store(MigrationLocked), got {other:?}"),
    }

    // Zero mutations: source present, target absent, no journal, lock kept.
    assert!(shard_file(&rig.config.storage_path, OLD_HASH, 0).exists());
    assert!(!shard_file(&rig.config.storage_path, TARGET_HASH, 0).exists());
    assert!(!swap_state_path(&rig.config.storage_path).exists());
    assert!(lock_path.exists(), "a foreign lock is never removed");
    assert!(rig.prompts.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_migrate_acquires_and_releases_operation_lock() {
    let gone = std::env::temp_dir().join("mem-pm-locklife-gone-0xa1-must-not-exist");
    assert!(!gone.exists());
    let rig = rig("pm-locklife").await;
    seed_shard(&rig, OLD_HASH, 0, 1, Some(&gone)).await;

    let lock_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observing: Arc<dyn ShardRegistry> = Arc::new(LockObservingRegistry {
        inner: rig.registry.clone(),
        storage_path: rig.config.storage_path.clone(),
        lock_seen: lock_seen.clone(),
        observed: std::sync::atomic::AtomicUsize::new(0),
    });

    let tags = rig_inventory_tags(&rig);
    let connections = Arc::new(ConnectionManager::new(
        rig.config.storage_path.clone(),
        rig.config.clone(),
        Platform::current(),
        Arc::new(mem_store::db::TursoConnectFactory),
    ));
    let inventory = Arc::new(ShardInventory::new(
        rig.config.storage_path.clone(),
        rig.config.clone(),
        connections.clone(),
        observing.clone(),
        tags.clone(),
    ));
    let prompts = Arc::new(RecordingPromptUpdater::default());
    let service = PathMigrationService::new(
        rig.config.storage_path.clone(),
        rig.config.clone(),
        connections,
        observing.clone(),
        inventory,
        tags,
        prompts,
        Arc::new(mem_store::platform::SystemProcessProbe),
    );

    assert!(
        !rig.config.storage_path.join(OPERATION_LOCK_FILE).exists(),
        "no lock before the migration"
    );
    let result = service
        .migrate(
            &rig.current_dir,
            &PathMigrationOptions {
                from_hash: Some(OLD_HASH.to_string()),
                dry_run: false,
                ..Default::default()
            },
        )
        .await
        .expect("migration succeeds");
    assert_eq!(result.migrated_shards, 1);

    assert!(
        lock_seen.load(std::sync::atomic::Ordering::SeqCst),
        "the operation lock was HELD during the registry mutation"
    );
    assert!(
        !rig.config.storage_path.join(OPERATION_LOCK_FILE).exists(),
        "the operation lock is RELEASED after migrate() returns"
    );
}

#[tokio::test]
async fn test_recovery_rejects_deep_traversal_journal_paths() {
    let config = common::store_config("pm-deep");
    let (config, connections, shards) = common::real_components(config).await;
    let storage = config.storage_path.clone();

    // A sibling dir of the storage root, so the crafted chain
    // `{storage}/sub/../../{sibling}/victim.db` resolves to a REAL outside
    // file at the OS level — the wall must still reject it lexically.
    let outside_dir = common::temp_dir("pm-deep-outside");
    let victim = outside_dir.join("victim.db");
    std::fs::write(&victim, b"untouched").expect("outside victim");
    std::fs::create_dir_all(storage.join("sub")).expect("sub dir");
    let sibling = outside_dir
        .file_name()
        .expect("sibling name")
        .to_string_lossy()
        .to_string();
    let outside_chain = storage
        .join("sub")
        .join("..")
        .join("..")
        .join(&sibling)
        .join("victim.db");
    assert!(
        outside_chain.exists(),
        "the crafted chain resolves to the outside victim: {outside_chain:?}"
    );
    let chain_text = outside_chain.to_string_lossy().to_string();

    write_swap_fixture(
        &storage,
        &PathMigrateSwapState {
            operation: "path-migrate".to_string(),
            old_hash: OLD_HASH.to_string(),
            new_hash: TARGET_HASH.to_string(),
            moves: vec![ShardMove {
                shard_id: 1,
                shard_index: 0,
                old_path: outside_chain,
                new_path: shard_file(&storage, TARGET_HASH, 0),
                staged_path: storage.join("projects").join("staged.tmp"),
                backup_path: storage.join("projects").join("backup.bak"),
                is_active: true,
                vector_count: 1,
                old_container_tag: format!("opencode_project_{OLD_HASH}"),
            }],
            archived_targets: Vec::new(),
        },
    );

    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());
    let error = mem_store::shard_path_migration::recover_interrupted_swap(
        &storage,
        &connections,
        registry.as_ref(),
    )
    .await
    .expect_err("deep traversal must be rejected before any mutation");
    let message = error.to_string();
    assert!(
        message.contains("Failed to recover path-migrate swap"),
        "{message}"
    );
    assert!(
        message.contains("Path-migrate recovery path escapes storage"),
        "{message}"
    );
    assert!(
        message.contains(chain_text.as_str()),
        "the journal's own path is named: {message}"
    );

    // ZERO mutations: outside victim intact, no target/backup, journal kept.
    assert_eq!(std::fs::read(&victim).expect("victim intact"), b"untouched");
    assert!(
        !shard_file(&storage, TARGET_HASH, 0).exists(),
        "the target file was never created"
    );
    assert!(
        !storage.join("projects").join("backup.bak").exists(),
        "no backup was written"
    );
    assert!(
        swap_state_path(&storage).exists(),
        "journal kept for diagnostics"
    );
    let _ = std::fs::remove_dir_all(&outside_dir);
}
