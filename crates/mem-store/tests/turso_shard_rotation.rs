//! Shard rotation + operation-lock battery (blueprint W1.2 step 8).
//! Ports the upstream operation-lock.ts semantics (stale locks, corrupt
//! locks, legacy migration lock, acquire/release) and rotation
//! (mark-read-only + create next) from shard-manager.ts.

mod common;

use std::sync::Arc;

use common::{real_components, write_lock_state};
use mem_store::error::StoreError;
use mem_store::operation_lock::{
    LEGACY_MIGRATION_LOCK_FILE, OPERATION_LOCK_FILE, acquire_turso_operation_lock, read_live_lock,
};
use mem_store::platform::SystemProcessProbe;
use mem_store::scope::Scope;
use mem_store::shard_manager::{ShardManager, get_shard_path};
use mem_store::vector::{MemoryInsert, count_all_vectors, insert_vector};

fn scope_hash(tag: u8) -> String {
    format!("a1b2c3d4e5f60{tag:03x}")
}

fn seed_record(id: &str, created: i64) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: "rotation".to_string(),
        vector: vec![1.0f32, 0.0],
        tags_vector: None,
        container_tag: "opencode_project_a1b2c3d4e5f60001".to_string(),
        tags: None,
        memory_type: None,
        created_at: created,
        updated_at: created,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
        source_prompt_created_at: None,
    }
}

async fn fill(
    shard: &ShardManager,
    index: u32,
    hash: &str,
    count: usize,
    config: mem_store::config::StoreConfig,
) {
    let full = get_shard_path(&config.storage_path, Scope::Project, hash, index).expect("path");
    let db = shard.open_shard(&full).await.expect("open");
    for i in 0..count {
        insert_vector(
            db.as_ref(),
            &seed_record(&format!("m{index}_{i}"), i as i64),
        )
        .await
        .expect("insert");
    }
}

// ---------------------------------------------------------------------------
// 1/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_marks_full_shard_read_only_and_creates_next_shard() {
    let config = common::test_store_config("rot-full", 2, 1);
    let (config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(1);
    let first = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 0");
    assert_eq!(first.shard_index, 0);

    fill(&manager, 0, &hash, 1, config.clone()).await;

    // increment isn't called by our rotation test directly: sync-on-read covers
    // the count through the COUNT(*) drift path.
    let second = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 1");
    assert_eq!(second.shard_index, 1, "rotation to the next index");
    assert!(second.is_active);

    let first_after = manager
        .get_active_shard(Scope::Project, &hash)
        .await
        .expect("read")
        .expect("active shard present");
    assert_eq!(first_after.shard_index, 1, "active shard is the NEW shard");
    let old = manager
        .get_shard_by_id(first.id)
        .await
        .expect("old shard row")
        .expect("row present");
    assert!(!old.is_active, "old shard marked read-only");
}

// ---------------------------------------------------------------------------
// 2/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_blocks_writes_while_migration_operation_owns_storage_lock() {
    let config = common::test_store_config("rot-lock-block", 2, 100);
    let (config, _connections, manager) = real_components(config).await;

    let lock = acquire_turso_operation_lock(
        &config.storage_path,
        Arc::new(SystemProcessProbe),
        "manual-test",
    )
    .expect("lock acquired");

    let error = manager
        .get_write_shard(Scope::Project, &scope_hash(2))
        .await
        .expect_err("blocked");
    let text = error.to_string();
    assert!(
        text.contains("writes are temporarily blocked"),
        "upstream wording: {text}"
    );

    lock.release();
    let shard = manager
        .get_write_shard(Scope::Project, &scope_hash(2))
        .await
        .expect("resumes after release");
    assert_eq!(shard.shard_index, 0);
}

// ---------------------------------------------------------------------------
// 3/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stale_operation_lock_from_dead_process_is_removed_then_write_proceeds() {
    let config = common::test_store_config("rot-stale", 2, 100);
    let (config, _connections, manager) = real_components(config).await;
    // A registered but DEAD pid (probe says nobody home).
    write_lock_state(
        &config.storage_path,
        OPERATION_LOCK_FILE,
        999_999,
        Some("migration"),
    );
    let lock_path = config.storage_path.join(OPERATION_LOCK_FILE);
    assert!(lock_path.exists());

    let shard = manager
        .get_write_shard(Scope::Project, &scope_hash(3))
        .await
        .expect("stale lock removed; write proceeds");
    assert_eq!(shard.shard_index, 0);
    assert!(!lock_path.exists(), "stale lock removed");
}

// ---------------------------------------------------------------------------
// 4/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_corrupt_operation_lock_is_treated_as_stale_and_removed() {
    let config = common::test_store_config("rot-corrupt", 2, 100);
    let (config, _connections, manager) = real_components(config).await;
    std::fs::write(
        config.storage_path.join(OPERATION_LOCK_FILE),
        "{{{ not json",
    )
    .expect("corrupt write");

    let shard = manager
        .get_write_shard(Scope::Project, &scope_hash(4))
        .await
        .expect("corrupt lock cleared");
    assert_eq!(shard.shard_index, 0);
    assert!(!config.storage_path.join(OPERATION_LOCK_FILE).exists());
    // sanity: read_live_lock itself reports None after the cleanup
    assert!(
        read_live_lock(
            &config.storage_path,
            OPERATION_LOCK_FILE,
            &SystemProcessProbe
        )
        .is_none()
    );
}

// ---------------------------------------------------------------------------
// 5/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_legacy_migration_lock_file_also_blocks_writes() {
    let config = common::test_store_config("rot-legacy", 2, 100);
    let (config, _connections, manager) = real_components(config).await;
    write_lock_state(
        &config.storage_path,
        LEGACY_MIGRATION_LOCK_FILE,
        std::process::id(),
        Some("legacy-migration"),
    );

    let error = manager
        .get_write_shard(Scope::Project, &scope_hash(5))
        .await
        .expect_err("blocked by legacy lock");
    assert!(
        matches!(error, StoreError::MigrationLocked { .. }),
        "typed: {error}"
    );
    let text = error.to_string();
    assert!(text.contains("legacy-migration"), "operation named: {text}");
    assert!(text.contains("writes are temporarily blocked"));
}

// ---------------------------------------------------------------------------
// 6/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_acquire_operation_lock_blocks_self_and_release_unblocks() {
    let config = common::test_store_config("rot-self", 2, 100);
    let (config, _connections, manager) = real_components(config).await;
    let lock = acquire_turso_operation_lock(
        &config.storage_path,
        Arc::new(SystemProcessProbe),
        "rotation",
    )
    .expect("acquire");

    let first = manager
        .get_write_shard(Scope::Project, &scope_hash(6))
        .await
        .expect_err("self-blocked");
    assert!(matches!(first, StoreError::MigrationLocked { .. }));

    lock.release();
    manager
        .get_write_shard(Scope::Project, &scope_hash(6))
        .await
        .expect("unblocked after release");

    // double-release is a no-op (lock already gone)
    lock.release();
    assert!(!config.storage_path.join(OPERATION_LOCK_FILE).exists());
}

// ---------------------------------------------------------------------------
// 7/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sync_vector_count_drift_triggers_rotation() {
    let config = common::test_store_config("rot-drift", 2, 1);
    let (config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(7);
    let first = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 0");

    // Registry says 0; the DB actually holds 1 (count drift covered by sync).
    {
        let full = get_shard_path(&config.storage_path, Scope::Project, &hash, 0).expect("path");
        let db = manager.open_shard(&full).await.expect("open");
        insert_vector(db.as_ref(), &seed_record("drift", 1))
            .await
            .expect("insert");
    }

    let second = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("rotate");
    assert_eq!(second.shard_index, 1, "drift pushed over max -> rotation");

    let old = manager
        .get_shard_by_id(first.id)
        .await
        .expect("old row")
        .expect("row present");
    assert!(!old.is_active, "full shard inactive");
    assert_eq!(old.vector_count, 1, "synced count recorded");
    drop(first);
    let _ = Arc::new(0u8);
}

// ---------------------------------------------------------------------------
// 8/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_shard_is_idempotent_via_unique_constraint_dedup() {
    let config = common::test_store_config("rot-idem", 2, 100);
    let (_config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(8);
    let first = manager
        .create_shard(Scope::Project, &hash, 0)
        .await
        .expect("create");
    let second = manager
        .create_shard(Scope::Project, &hash, 0)
        .await
        .expect("recreate");
    assert_eq!(first.id, second.id, "unique violation dedup re-fetches");
    let all = manager
        .get_all_shards(Scope::Project, Some(&hash))
        .await
        .expect("list");
    assert_eq!(all.len(), 1, "no duplicate rows");
}

// ---------------------------------------------------------------------------
// 9/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_register_existing_shard_upserts_registry_row() {
    let config = common::test_store_config("rot-register", 2, 100);
    let storage = config.storage_path.clone();
    let (_config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(9);
    let first = manager
        .register_existing_shard(Scope::Project, &hash, 0, &storage.join("x.db"), 5, true)
        .await
        .expect("register");
    assert_eq!(first.vector_count, 5);

    let updated = manager
        .register_existing_shard(Scope::Project, &hash, 0, &storage.join("y.db"), 9, false)
        .await
        .expect("upsert");
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.vector_count, 9);
    assert!(!updated.is_active);
    let all = manager
        .get_all_shards(Scope::Project, Some(&hash))
        .await
        .expect("list");
    assert_eq!(all.len(), 1);
}

// ---------------------------------------------------------------------------
// 10/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_shard_by_path_escapes_like_metacharacters() {
    let config = common::test_store_config("rot-escape", 2, 100);
    let (_config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(10);
    let first = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard");
    let full = get_shard_path(&_config.storage_path, Scope::Project, &hash, 0).expect("path");

    let found = manager
        .get_shard_by_path(&full)
        .await
        .expect("query")
        .expect("exact path found");
    assert_eq!(found.id, first.id);

    // Unescaped '_' would match any single char — the escaped suffix
    // probe must NOT match a hypothetical longer name.
    let decoy = full
        .to_string_lossy()
        .replace("_shard_0.db", "X_shard_0.db");
    assert!(decoy != full.to_string_lossy(), "decoy path differs");
    let decoy_hit = manager
        .get_shard_by_path(std::path::Path::new(decoy.as_str()))
        .await
        .expect("query");
    assert!(decoy_hit.is_none(), "metachar escaping confines the match");
}

// ---------------------------------------------------------------------------
// 11/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_shard_closes_connection_removes_row_and_file() {
    let config = common::test_store_config("rot-delete", 2, 100);
    let (config, connections, manager) = real_components(config).await;
    let hash = scope_hash(11);
    let shard = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard");
    let full = get_shard_path(
        &config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path");
    assert!(full.exists());

    manager.delete_shard(shard.id).await.expect("delete");
    assert!(!full.exists(), "file removed");
    assert!(
        !connections.has_open_connection(&full).expect("has open"),
        "manager connection closed"
    );
    assert!(
        manager
            .get_shard_by_id(shard.id)
            .await
            .expect("row")
            .is_none(),
        "row gone"
    );
    let _ = count_all_vectors;
    let _: Option<StoreError> = None;
}

// ---------------------------------------------------------------------------
// 12/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_archive_shard_renames_file_and_rejects_escape_overrides() {
    let config = common::test_store_config("rot-archive", 2, 100);
    let (config, _connections, manager) = real_components(config).await;
    // archive_shard renames under the root's proof.
    let _proof = common::exclusive_root(&config.storage_path);
    let hash = scope_hash(12);
    let shard = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard");
    let full = get_shard_path(
        &config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path");
    assert!(full.exists());

    let escaped_override = std::path::PathBuf::from(format!(
        "{}/escape-target.bak",
        std::env::temp_dir().display()
    ));
    let escape = manager
        .archive_shard(shard.id, "migrated", Some(&escaped_override))
        .await
        .expect_err("override outside scope dir rejected");
    assert!(
        matches!(escape, StoreError::ArchivePathEscape { .. }),
        "typed: {escape}"
    );

    let archived_anchor = manager
        .archive_shard(shard.id, "migrated", None)
        .await
        .expect("archive")
        .expect("path returned");
    let archived = archived_anchor.clone();
    assert!(archived.to_string_lossy().ends_with(".bak"), "bak suffix");
    assert!(archived.exists(), "file renamed to archive");
    assert!(!full.exists(), "original gone");
    assert!(
        manager
            .get_shard_by_id(shard.id)
            .await
            .expect("row")
            .is_none(),
        "row deleted"
    );
    let text = archived_anchor.to_string_lossy();
    assert!(
        text.contains("migrated"),
        "reason stamped in the archive name: {text}"
    );
}

// ---------------------------------------------------------------------------
// Rework: deep-traversal archive override (defense-in-depth)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_archive_shard_rejects_deep_traversal_override() {
    let config = common::test_store_config("rot-archive-deep", 2, 100);
    let (config, _connections, manager) = real_components(config).await;
    let hash = scope_hash(13);
    let shard = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard");
    let scope_dir = get_shard_path(
        &config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path")
    .parent()
    .expect("scope dir")
    .to_path_buf();

    // `{scope}/sub/../../outside.bak` resolves OUTSIDE the scope dir; the
    // archive containment wall must reject it before any rename.
    let override_path = scope_dir
        .join("sub")
        .join("..")
        .join("..")
        .join("outside.bak");
    let error = manager
        .archive_shard(shard.id, "pre-path-migrate", Some(&override_path))
        .await
        .expect_err("deep traversal override rejected");
    assert!(
        matches!(error, StoreError::ArchivePathEscape { .. }),
        "typed: {error}"
    );

    let full = get_shard_path(
        &config.storage_path,
        Scope::Project,
        &hash,
        shard.shard_index,
    )
    .expect("path");
    assert!(full.exists(), "the shard file was never archived");
    assert!(!override_path.exists(), "no file escaped the scope dir");
}

// ---------------------------------------------------------------------------
// 14/14 — the own-lock-tolerant write path (W4.3, ADR-0006 D-25)
// ---------------------------------------------------------------------------

/// `get_write_shard_allowing_own_lock` tolerates OUR OWN operation lock and
/// refuses a FOREIGN one; `get_write_shard` refuses both ("a lock blocks
/// even its own holder").
///
/// The foreign-pid refusal is the property the W4.3 own-lock bypass rests
/// on: without it, a live lock held by another process would be treated as
/// our own and the write would proceed into a migrating shard.
#[tokio::test]
async fn test_get_write_shard_allowing_own_lock_refuses_a_foreign_lock_but_allows_our_own() {
    let config = common::test_store_config("rot-own-lock", 2, 100);
    let storage = config.storage_path.clone();
    // `FakeProbe` answers `true` for EVERY pid: a foreign pid reads as live.
    let (_config, _connections, manager) =
        common::real_components_with_probe(config, Arc::new(common::FakeProbe::default())).await;
    let hash = scope_hash(14);

    // A FOREIGN live lock blocks the tolerant path...
    write_lock_state(
        &storage,
        OPERATION_LOCK_FILE,
        4242,
        Some("foreign-migration"),
    );
    let error = manager
        .get_write_shard_allowing_own_lock(Scope::Project, &hash)
        .await
        .expect_err("a foreign lock is refused");
    match error {
        StoreError::MigrationLocked { pid, .. } => {
            assert_eq!(pid, 4242, "the foreign owner's pid is reported");
        }
        other => panic!("unexpected error: {other}"),
    }
    // ...and the strict path refuses it too.
    let error = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect_err("a foreign lock is refused by the strict path");
    assert!(
        matches!(error, StoreError::MigrationLocked { .. }),
        "typed: {error}"
    );

    // OUR OWN lock is tolerated by the tolerant path...
    write_lock_state(
        &storage,
        OPERATION_LOCK_FILE,
        std::process::id(),
        Some("episodic-consolidation"),
    );
    let shard = manager
        .get_write_shard_allowing_own_lock(Scope::Project, &hash)
        .await
        .expect("our own lock is tolerated by the tolerant path");
    assert_eq!(shard.shard_index, 0);

    // ...but NOT by the strict path: a lock blocks even its own holder.
    let error = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect_err("the strict path still blocks our own lock");
    assert!(
        matches!(error, StoreError::MigrationLocked { .. }),
        "typed: {error}"
    );
}
