//! Shard-inventory battery (blueprint W1.2 step 11): disk + registry union
//! listing, derive_status precedence, resolve_migration_source ordering
//! and error fragments (upstream shard-inventory-service.ts).

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::FakeTagSource;
use mem_core::tags::TagInfo;
use mem_store::config::StoreConfig;
use mem_store::scope::Scope;
use mem_store::shard_inventory::{ShardInventory, ShardInventoryStatus};
use mem_store::shard_manager::{ShardManager, ShardRegistry};
use mem_store::vector::{MemoryInsert, insert_vector};

const CUR_HASH: &str = "d4e5f6a1b2c3d4e5";

fn fake_source(entries: Vec<(PathBuf, &str, Option<PathBuf>)>) -> Arc<FakeTagSource> {
    let mut map = HashMap::new();
    for (directory, tag, project_path) in entries {
        map.insert(
            directory.to_string_lossy().to_string(),
            TagInfo {
                tag: tag.to_string(),
                display_name: directory.to_string_lossy().to_string(),
                user_name: None,
                user_email: None,
                project_path,
                project_name: Some("FixtureProject".to_string()),
                git_repo_url: None,
            },
        );
    }
    Arc::new(FakeTagSource {
        map: std::sync::Mutex::new(map),
        fallback: None,
    })
}

fn current_tag(hash: &str) -> String {
    format!("opencode_project_{hash}")
}

struct Rig {
    config: StoreConfig,
    shards: ShardManager,
    inventory: ShardInventory,
    current_dir: PathBuf,
}

async fn rig(prefix: &str, entries: Vec<(PathBuf, &str, Option<PathBuf>)>) -> Rig {
    let config = common::test_store_config(prefix, 2, 100);
    let current_dir = config.storage_path.join("current-workdir");
    std::fs::create_dir_all(&current_dir).expect("cwd dir");
    let tags = fake_source(entries);
    let (config, connections, shards) = common::real_components(config).await;
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());
    let inventory = ShardInventory::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        registry.clone(),
        tags.clone(),
    );
    Rig {
        config,
        shards,
        inventory,
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

/// Creates a real shard file (dims metadata included) with `count` memories.
async fn seed_shard(
    rig: &Rig,
    hash: &str,
    count: usize,
    project_path: Option<&Path>,
    index: u32,
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
            &record(hash, &format!("m{i}"), i as i64, project_path),
        )
        .await
        .expect("insert");
    }
    shard
}

/// Registers a row whose FILE will never exist.
async fn register_missing(
    rig: &Rig,
    hash: &str,
    index: u32,
) -> mem_store::shard_manager::ShardInfo {
    let full = mem_store::shard_manager::get_shard_path(
        &rig.config.storage_path,
        Scope::Project,
        hash,
        index,
    )
    .expect("path");
    rig.shards
        .register_existing_shard(Scope::Project, hash, index, &full, 0, false)
        .await
        .expect("register")
}

fn scope_hashes(result: &mem_store::shard_inventory::ListShardsResult) -> Vec<String> {
    result.shards.iter().map(|g| g.scope_hash.clone()).collect()
}

// ---------------------------------------------------------------------------
// 1/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lists_orphaned_shards_when_stored_project_path_is_gone() {
    let gone = std::env::temp_dir().join("mem-inv-gone-does-not-exist-0x91");
    assert!(!gone.exists(), "fixture path must not exist");
    let rig = rig(
        "inv-orphaned",
        vec![(PathBuf::from(""), &current_tag(CUR_HASH), None)],
    )
    .await;
    seed_shard(&rig, "a1b2c3d4e5f60001", 1, Some(&gone), 0).await;

    let result = rig
        .inventory
        .list_shards(&rig.current_dir)
        .await
        .expect("list");
    assert_eq!(scope_hashes(&result), vec!["a1b2c3d4e5f60001"]);
    let group = &result.shards[0];
    assert!(matches!(group.status, ShardInventoryStatus::Orphaned));
    assert!(!group.path_exists);
    assert_eq!(group.memory_count, 1);
    assert_eq!(
        group.project_path.as_deref(),
        gone.to_str().map(str::to_string).as_deref()
    );
    assert_eq!(
        result.summary.orphaned, 1,
        "summary counts the orphaned group"
    );
}

// ---------------------------------------------------------------------------
// 2/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_resolves_migration_source_by_stored_project_path_and_by_from_hash() {
    let live = std::env::temp_dir().join("mem-inv-live-0x92-new");
    let _ = std::fs::remove_dir_all(&live);
    std::fs::create_dir_all(&live).expect("live dir");
    let rig = rig(
        "inv-resolve",
        vec![(PathBuf::new(), &current_tag(CUR_HASH), None)],
    )
    .await;
    seed_shard(&rig, "a1b2c3d4e5f60002", 2, Some(&live), 0).await;

    let source = rig
        .inventory
        .resolve_migration_source(
            &rig.current_dir,
            &mem_store::shard_inventory::ResolveMigrationSourceOptions {
                from_path: Some(live.to_string_lossy().to_string()),
                from_hash: None,
            },
        )
        .await
        .expect("by stored path");
    assert!(matches!(
        source.matched_by,
        mem_store::shard_inventory::MatchedBy::StoredProjectPath
    ));
    assert_eq!(source.scope_hash, "a1b2c3d4e5f60002");

    // Deleted directory: from_path falls to the final missing-source hint;
    // from_hash still works.
    std::fs::remove_dir_all(&live).expect("delete dir");
    let by_hash = rig
        .inventory
        .resolve_migration_source(
            &rig.current_dir,
            &mem_store::shard_inventory::ResolveMigrationSourceOptions {
                from_path: None,
                from_hash: Some("a1b2c3d4e5f60002".to_string()),
            },
        )
        .await
        .expect("by hash after deletion");
    assert!(matches!(
        by_hash.matched_by,
        mem_store::shard_inventory::MatchedBy::FromHash
    ));

    // The stored candidate path still matches AFTER the directory is gone
    // (inventory merges on STORED paths, not filesystem state).
    let still_matches = rig
        .inventory
        .resolve_migration_source(
            &rig.current_dir,
            &mem_store::shard_inventory::ResolveMigrationSourceOptions {
                from_path: Some(live.to_string_lossy().to_string()),
                from_hash: None,
            },
        )
        .await
        .expect("still matches by stored path");
    assert!(matches!(
        still_matches.matched_by,
        mem_store::shard_inventory::MatchedBy::StoredProjectPath
    ));

    let missing = rig
        .inventory
        .resolve_migration_source(
            &rig.current_dir,
            &mem_store::shard_inventory::ResolveMigrationSourceOptions {
                from_path: Some("/tmp/mem-inv-never-ever-0x95".to_string()),
                from_hash: None,
            },
        )
        .await
        .expect_err("missing source");
    assert!(
        missing.message.contains("list-shards"),
        "hint present: {}",
        missing.message
    );
    assert!(
        missing.message.contains("fromHash"),
        "fromHash hint present: {}",
        missing.message
    );
}

// ---------------------------------------------------------------------------
// 3/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rejects_ambiguous_and_invalid_from_hash_values() {
    let shared = std::env::temp_dir().join("mem-inv-shared-0x93");
    std::fs::create_dir_all(&shared).expect("shared dir");
    let rig = rig("inv-bad", vec![]).await;
    seed_shard(&rig, "a1b2c3d4e5f60003", 1, Some(&shared), 0).await;
    seed_shard(&rig, "a1b2c3d4e5f60004", 1, Some(&shared), 0).await;

    for (hash, fragment) in [
        (
            "NOPE",
            "Invalid fromHash: expected 16 lowercase hex characters",
        ),
        ("aaaaaaaaaaaaaaaa", "No project shards found for hash"),
    ] {
        let error = rig
            .inventory
            .resolve_migration_source(
                &rig.current_dir,
                &mem_store::shard_inventory::ResolveMigrationSourceOptions {
                    from_path: None,
                    from_hash: Some(hash.to_string()),
                },
            )
            .await
            .expect_err(hash);
        assert!(
            error.message.contains(fragment),
            "{hash}: {}",
            error.message
        );
    }

    let ambiguous = rig
        .inventory
        .resolve_migration_source(
            &rig.current_dir,
            &mem_store::shard_inventory::ResolveMigrationSourceOptions {
                from_path: Some(shared.to_string_lossy().to_string()),
                from_hash: None,
            },
        )
        .await
        .expect_err("ambiguous");
    assert!(
        ambiguous
            .message
            .starts_with("Ambiguous fromPath match for "),
        "{}",
        ambiguous.message
    );
    assert_eq!(
        ambiguous.candidates.len(),
        2,
        "matched-by-path groups are the candidates"
    );

    let neither = rig
        .inventory
        .resolve_migration_source(
            &rig.current_dir,
            &mem_store::shard_inventory::ResolveMigrationSourceOptions {
                from_path: None,
                from_hash: None,
            },
        )
        .await
        .expect_err("neither");
    assert_eq!(
        neither.message,
        "fromPath or fromHash is required for migrate"
    );
}

// ---------------------------------------------------------------------------
// 4/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lists_current_project_group_as_current_with_summary_counts() {
    let live = std::env::temp_dir().join("mem-inv-current-0x94");
    std::fs::create_dir_all(&live).expect("live dir");
    let rig = rig("inv-current", vec![]).await;
    // The current directory's fixture map must key on the CURRENT dir; the
    // rig built a source without entries: rebuild by registering a canned
    // entry ourselves.
    seed_shard(&rig, CUR_HASH, 3, Some(&live), 0).await;

    let source = Arc::new(FakeTagSource {
        map: {
            let mut map = HashMap::new();
            map.insert(
                rig.current_dir.to_string_lossy().to_string(),
                TagInfo {
                    tag: current_tag(CUR_HASH),
                    display_name: "current-workdir".to_string(),
                    user_name: None,
                    user_email: None,
                    project_path: Some(live.clone()),
                    project_name: Some("CurrentProject".to_string()),
                    git_repo_url: None,
                },
            );
            std::sync::Mutex::new(map)
        },
        fallback: None,
    });
    let registry: Arc<dyn ShardRegistry> = Arc::new(rig.shards.clone());
    let inventory = ShardInventory::new(
        rig.config.storage_path.clone(),
        rig.config.clone(),
        std::sync::Arc::new(mem_store::connection::ConnectionManager::new(
            rig.config.storage_path.clone(),
            rig.config.clone(),
            mem_store::platform::Platform::current(),
            Arc::new(mem_store::db::TursoConnectFactory),
        )),
        registry.clone(),
        source.clone(),
    );

    let result = inventory.list_shards(&rig.current_dir).await.expect("list");
    assert_eq!(scope_hashes(&result), vec![CUR_HASH], "current first");
    let group = &result.shards[0];
    assert!(matches!(group.status, ShardInventoryStatus::Current));
    assert!(group.matches_current_project);
    assert_eq!(result.summary.total, 1);
    assert_eq!(result.summary.current, 1);
    assert_eq!(result.summary.linked, 0);
    assert_eq!(result.summary.orphaned, 0);
    assert_eq!(result.summary.missing_file, 0);
    assert_eq!(result.summary.empty, 0);
    assert_eq!(result.summary.ambiguous, 0);
    assert_eq!(
        result.current_project.tag,
        current_tag(CUR_HASH),
        "current project tag surfaced"
    );
}

// ---------------------------------------------------------------------------
// 5/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_status_precedence_covers_empty_and_missing_file_groups() {
    let rig = rig("inv-precedence", vec![]).await;
    // EMPTY group: file exists, zero memories.
    seed_shard(&rig, "e000000000000001", 0, None, 0).await;
    // MISSING-FILE group: registered but the file never materialized.
    register_missing(&rig, "e000000000000002", 0).await;
    // ANY-FILE-MISSING group: index 0 content, index 1 registered-missing.
    seed_shard(&rig, "e000000000000003", 1, None, 0).await;
    register_missing(&rig, "e000000000000003", 1).await;

    let result = rig
        .inventory
        .list_shards(&rig.current_dir)
        .await
        .expect("list");
    let status_of = |hash: &str| {
        result
            .shards
            .iter()
            .find(|g| g.scope_hash == hash)
            .map(|g| g.status.clone())
            .expect("group")
    };
    assert!(matches!(
        status_of("e000000000000001"),
        ShardInventoryStatus::Empty
    ));
    assert!(matches!(
        status_of("e000000000000002"),
        ShardInventoryStatus::MissingFile
    ));
    assert!(matches!(
        status_of("e000000000000003"),
        ShardInventoryStatus::MissingFile
    ));
    assert_eq!(result.summary.empty, 1);
    assert_eq!(result.summary.missing_file, 2);
    assert_eq!(result.summary.total, 3);
}

// ---------------------------------------------------------------------------
// 6/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_inventory_discovers_disk_only_shard_files() {
    let live = std::env::temp_dir().join("mem-inv-disk-0x95");
    std::fs::create_dir_all(&live).expect("live dir");
    let rig = rig("inv-disk", vec![]).await;
    let hash = "d000000000000001";
    let file = rig
        .config
        .storage_path
        .join("projects")
        .join(format!("project_{hash}_shard_0.db"));
    std::fs::create_dir_all(file.parent().expect("projects dir")).expect("projects dir");

    // Build the shard file directly (no registry row).
    let opts = mem_store::db::ConnectOptions::for_config(
        &rig.config,
        mem_store::platform::Platform::current(),
    );
    let db = {
        use mem_store::db::ConnectFactory;
        mem_store::db::turso_connect_factory()
            .connect(file.clone(), opts.clone())
            .await
            .expect("raw open")
    };
    mem_store::schema_migrations::apply_schema_migrations(
        db.as_ref(),
        &mem_store::schema_migrations::memory_shard_migrations(2),
        &mem_store::schema_migrations::MigrationOptions {
            db_path: None,
            label: "disk-only",
            max_known: mem_store::schema_migrations::MEMORY_SHARD_MAX_KNOWN,
        },
    )
    .await
    .expect("schema");
    insert_vector(db.as_ref(), &record(hash, "only", 1, Some(&live)))
        .await
        .expect("insert");
    drop(db);

    let result = rig
        .inventory
        .list_shards(&rig.current_dir)
        .await
        .expect("list");
    let group = result
        .shards
        .iter()
        .find(|g| g.scope_hash == hash)
        .expect("disk-only group");
    assert_eq!(group.shards.len(), 1);
    let descriptor = &group.shards[0];
    assert!(descriptor.shard_id.is_none(), "no registry row");
    assert!(
        descriptor.is_active,
        "disk-only shard index == max is active"
    );
    assert_eq!(descriptor.memory_count, 1);
    assert!(descriptor.file_exists);
    assert_eq!(descriptor.shard_index, 0);
}

// ---------------------------------------------------------------------------
// 7/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_groups_sort_current_first_then_memory_count_desc_then_hash_asc() {
    let rig = rig("inv-sort", vec![]).await;
    seed_shard(&rig, "a1b2c3d4e5f60005", 5, None, 0).await; // 5 memories
    seed_shard(&rig, "a1b2c3d4e5f60006", 2, None, 0).await; // 2
    seed_shard(&rig, CUR_HASH, 1, None, 0).await; // current

    // Current directory resolves to CUR_HASH via the fixture map.
    let source = Arc::new(FakeTagSource {
        map: {
            let mut map = HashMap::new();
            map.insert(
                rig.current_dir.to_string_lossy().to_string(),
                TagInfo {
                    tag: current_tag(CUR_HASH),
                    display_name: "sort-cwd".to_string(),
                    user_name: None,
                    user_email: None,
                    project_path: Some(rig.current_dir.clone()),
                    project_name: None,
                    git_repo_url: None,
                },
            );
            std::sync::Mutex::new(map)
        },
        fallback: None,
    });
    let registry: Arc<dyn ShardRegistry> = Arc::new(rig.shards.clone());
    let inventory = ShardInventory::new(
        rig.config.storage_path.clone(),
        rig.config.clone(),
        std::sync::Arc::new(mem_store::connection::ConnectionManager::new(
            rig.config.storage_path.clone(),
            rig.config.clone(),
            mem_store::platform::Platform::current(),
            Arc::new(mem_store::db::TursoConnectFactory),
        )),
        registry,
        source,
    );
    let result = inventory.list_shards(&rig.current_dir).await.expect("list");
    assert_eq!(
        scope_hashes(&result),
        vec![CUR_HASH, "a1b2c3d4e5f60005", "a1b2c3d4e5f60006"]
    );
}
