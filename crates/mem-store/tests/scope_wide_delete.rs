//! Issue 72 — the scope-wide delete-by-id seam
//! [`ShardManager::delete_memory_in_scope`](mem_store::shard_manager::ShardManager::delete_memory_in_scope).
//!
//! `memories.id` is `TEXT PRIMARY KEY`, so uniqueness is enforced per SHARD
//! FILE while a scope owns many shard files. One id can therefore live in more
//! than one shard of the same `(scope, scope_hash)`; the seam deletes every
//! copy present at scan time, one `delete_memories` batch per touched shard,
//! and adopts each shard's statement-measured `COUNT(*)` into the registry.
//!
//! The fixtures use the real turso connections over a temp root
//! (`common::real_components`); the duplicate pre-assertion in
//! [`two_shard_duplicate`] keeps every delete assertion non-vacuous.

mod common;

use std::path::Path;

use mem_store::db::query_one;
use mem_store::error::StoreError;
use mem_store::scope::Scope;
use mem_store::shard_facts;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::sql;
use mem_store::vector::{MemoryInsert, get_memory_by_id, insert_vector};

/// The project hash every store fixture addresses.
const HASH: &str = "a1b2c3d4e5f60101";
/// The container tag of shard 0's rows.
const TAG_SHARD_0: &str = "opencode_project_a1b2c3d4e5f60102";
/// The container tag of shard 1's rows.
const TAG_SHARD_1: &str = "opencode_project_a1b2c3d4e5f60103";

fn scope_hash() -> String {
    HASH.to_string()
}

fn seed_record(id: &str, container_tag: &str) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: format!("row {id}"),
        vector: vec![1.0f32, 0.0],
        tags_vector: None,
        container_tag: container_tag.to_string(),
        tags: None,
        memory_type: None,
        created_at: 1,
        updated_at: 1,
        metadata: None,
        session_id: Some("session-1".to_string()),
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
    }
}

async fn insert_into_shard(manager: &ShardManager, path: &Path, record: &MemoryInsert) {
    let db = manager.open_shard(path).await.expect("open shard");
    insert_vector(db.as_ref(), record)
        .await
        .expect("insert row");
}

/// Inserts one row and syncs BOTH count authorities, so a later `0` is a
/// measured value rather than an absent stale row.
async fn insert_and_sync(manager: &ShardManager, shard: &ShardInfo, record: &MemoryInsert) {
    insert_into_shard(manager, &shard.db_path, record).await;
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    let count = shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync count");
    manager
        .set_vector_count(shard.id, count)
        .await
        .expect("adopt registry count");
}

/// `true` when `id` is readable in the shard at `path`.
async fn copy_exists(manager: &ShardManager, path: &Path, id: &str) -> bool {
    let db = manager.open_shard(path).await.expect("open shard");
    get_memory_by_id(db.as_ref(), id)
        .await
        .expect("lookup")
        .is_some()
}

/// The committed `shard_stats` value of the shard at `path`.
async fn stored_count(manager: &ShardManager, path: &Path) -> u64 {
    let db = manager.open_shard(path).await.expect("open shard");
    shard_facts::read_stored_count(db.as_ref())
        .await
        .expect("stored count")
}

/// The registry's cached vector count for `shard`.
async fn registry_count(manager: &ShardManager, shard: &ShardInfo) -> u64 {
    manager
        .get_shard_by_id(shard.id)
        .await
        .expect("registry read")
        .expect("registry row")
        .vector_count
}

/// A live manager over a fresh temp root.
struct Duplicate {
    manager: ShardManager,
    shard_zero: ShardInfo,
    shard_one: ShardInfo,
}

/// The issue-72 damage state, built directly (the per-file PK allows it):
/// project shards 0 and 1 both hold `dup-id`, with distinct neighbours.
///
/// Shard 0: `dup-id` + one neighbour (2 rows). Shard 1: `dup-id` + two
/// neighbours (3 rows). The fixture asserts BOTH copies exist BEFORE any
/// delete, so a delete test can never pass on an absent row.
async fn two_shard_duplicate(label: &str) -> Duplicate {
    let config = common::test_store_config(label, 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let shard_zero = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create shard 0");
    manager
        .mark_shard_read_only(shard_zero.id)
        .await
        .expect("retire shard 0 before creating shard 1");
    let shard_one = manager
        .create_shard(Scope::Project, HASH, 1)
        .await
        .expect("create shard 1");

    insert_into_shard(
        &manager,
        &shard_zero.db_path,
        &seed_record("dup-id", TAG_SHARD_0),
    )
    .await;
    insert_into_shard(
        &manager,
        &shard_zero.db_path,
        &seed_record("neighbour-0", TAG_SHARD_0),
    )
    .await;
    insert_into_shard(
        &manager,
        &shard_one.db_path,
        &seed_record("dup-id", TAG_SHARD_1),
    )
    .await;
    insert_into_shard(
        &manager,
        &shard_one.db_path,
        &seed_record("neighbour-1a", TAG_SHARD_1),
    )
    .await;
    insert_into_shard(
        &manager,
        &shard_one.db_path,
        &seed_record("neighbour-1b", TAG_SHARD_1),
    )
    .await;

    assert!(
        copy_exists(&manager, &shard_zero.db_path, "dup-id").await,
        "the fixture really seeds the retired copy"
    );
    assert!(
        copy_exists(&manager, &shard_one.db_path, "dup-id").await,
        "the fixture really seeds the active copy"
    );

    Duplicate {
        manager,
        shard_zero,
        shard_one,
    }
}

/// AC1: one call removes EVERY copy of the id in the scope and leaves every
/// differently-id'd neighbour in place.
#[tokio::test]
async fn test_delete_memory_in_scope_removes_every_copy_and_keeps_the_neighbour() {
    let duplicate = two_shard_duplicate("scope-delete-every-copy").await;

    let copies = duplicate
        .manager
        .delete_memory_in_scope(Scope::Project, &scope_hash(), "dup-id")
        .await
        .expect("a healthy scope deletes");
    assert_eq!(copies, 2, "both copies are counted and deleted");

    assert!(
        !copy_exists(&duplicate.manager, &duplicate.shard_zero.db_path, "dup-id").await,
        "the retired copy is gone"
    );
    assert!(
        !copy_exists(&duplicate.manager, &duplicate.shard_one.db_path, "dup-id").await,
        "the active copy is gone"
    );
    for (path, neighbour) in [
        (&duplicate.shard_zero.db_path, "neighbour-0"),
        (&duplicate.shard_one.db_path, "neighbour-1a"),
        (&duplicate.shard_one.db_path, "neighbour-1b"),
    ] {
        assert!(
            copy_exists(&duplicate.manager, path, neighbour).await,
            "the differently-id'd neighbour {neighbour} survives"
        );
    }
}

/// AC2: each touched shard's count is reconciled ABSOLUTELY — shard 0's and
/// shard 1's counts are different, so adopting one shard's count for every
/// shard is observable. The registry is planted with drift (99 / 0) so a
/// delta reads 98 / 0 instead of the real 1 / 2.
#[tokio::test]
async fn test_delete_memory_in_scope_reconciles_each_shard_count_absolutely() {
    let duplicate = two_shard_duplicate("scope-delete-absolute-count").await;
    duplicate
        .manager
        .set_vector_count(duplicate.shard_zero.id, 99)
        .await
        .expect("plant the drift");
    duplicate
        .manager
        .set_vector_count(duplicate.shard_one.id, 0)
        .await
        .expect("plant the drift");

    let copies = duplicate
        .manager
        .delete_memory_in_scope(Scope::Project, &scope_hash(), "dup-id")
        .await
        .expect("a healthy scope deletes");
    assert_eq!(copies, 2, "both copies are counted and deleted");

    assert_eq!(
        stored_count(&duplicate.manager, &duplicate.shard_zero.db_path).await,
        1,
        "shard 0 keeps only its neighbour"
    );
    assert_eq!(
        stored_count(&duplicate.manager, &duplicate.shard_one.db_path).await,
        2,
        "shard 1 keeps both neighbours"
    );
    assert_eq!(
        registry_count(&duplicate.manager, &duplicate.shard_zero).await,
        1,
        "shard 0's registry adopts its own committed count, never shard 1's"
    );
    assert_eq!(
        registry_count(&duplicate.manager, &duplicate.shard_one).await,
        2,
        "shard 1's registry adopts its own committed count"
    );
}

/// AC3: a retry after full success is a no-op — no copy means no adoption, so
/// the counts stay absolute instead of drifting under a delta. The first call
/// is where a delta would corrupt the count (a second call never reaches the
/// adoption), so both calls are measured.
#[tokio::test]
async fn test_delete_memory_in_scope_is_idempotent_and_keeps_the_absolute_count() {
    let duplicate = two_shard_duplicate("scope-delete-idempotent").await;

    let first = duplicate
        .manager
        .delete_memory_in_scope(Scope::Project, &scope_hash(), "dup-id")
        .await
        .expect("the first delete succeeds");
    assert_eq!(first, 2, "the first call sees both copies");
    assert_eq!(
        stored_count(&duplicate.manager, &duplicate.shard_zero.db_path).await,
        1,
        "the first call's shard 0 count is absolute"
    );
    assert_eq!(
        stored_count(&duplicate.manager, &duplicate.shard_one.db_path).await,
        2,
        "the first call's shard 1 count is absolute"
    );
    assert_eq!(
        registry_count(&duplicate.manager, &duplicate.shard_zero).await,
        1,
        "the first call's shard 0 registry is absolute, not committed - 1"
    );
    assert_eq!(
        registry_count(&duplicate.manager, &duplicate.shard_one).await,
        2,
        "the first call's shard 1 registry is absolute, not committed - 1"
    );

    let second = duplicate
        .manager
        .delete_memory_in_scope(Scope::Project, &scope_hash(), "dup-id")
        .await
        .expect("a retry after full success is a no-op");
    assert_eq!(second, 0, "no shard holds the id any more");
    assert_eq!(
        stored_count(&duplicate.manager, &duplicate.shard_zero.db_path).await,
        1,
        "the retry changes nothing (shard 0)"
    );
    assert_eq!(
        stored_count(&duplicate.manager, &duplicate.shard_one.db_path).await,
        2,
        "the retry changes nothing (shard 1)"
    );
    assert_eq!(
        registry_count(&duplicate.manager, &duplicate.shard_zero).await,
        1,
        "the retry changes nothing (shard 0 registry)"
    );
    assert_eq!(
        registry_count(&duplicate.manager, &duplicate.shard_one).await,
        2,
        "the retry changes nothing (shard 1 registry)"
    );
}

/// AC4: a registered shard whose file is gone is a TYPED failure, never a
/// silent skip — and the scan never re-creates the file.
///
/// The variant match alone is not discriminating (opening the re-created,
/// schema-less file fails and is wrapped in the same variant); the
/// `!db_path.exists()` assertion is what pins the stat-before-open.
#[tokio::test]
async fn test_delete_memory_in_scope_fails_closed_when_a_registered_shard_file_is_missing() {
    let config = common::test_store_config("scope-delete-missing", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;
    let shard_zero = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create shard 0");
    manager
        .mark_shard_read_only(shard_zero.id)
        .await
        .expect("retire shard 0 before creating shard 1");
    let shard_one = manager
        .create_shard(Scope::Project, HASH, 1)
        .await
        .expect("create shard 1");
    insert_into_shard(
        &manager,
        &shard_zero.db_path,
        &seed_record("dup-id", TAG_SHARD_0),
    )
    .await;
    insert_into_shard(
        &manager,
        &shard_one.db_path,
        &seed_record("present-id", TAG_SHARD_1),
    )
    .await;

    // Positive control: with every file present the very same call is Ok(0).
    assert_eq!(
        manager
            .delete_memory_in_scope(Scope::Project, &scope_hash(), "absent-id")
            .await
            .expect("a healthy scan"),
        0,
        "an id no shard holds deletes nothing"
    );

    // Drop every handle — a cached connection would read the UNLINKED file —
    // then delete shard 1's database (the DESC scan's first shard).
    drop(manager);
    drop(connections);
    std::fs::remove_file(&shard_one.db_path).expect("remove shard 1's database");
    assert!(!shard_one.db_path.exists());

    let (_config, _connections, fresh) = common::real_components(config).await;
    let error = fresh
        .delete_memory_in_scope(Scope::Project, &scope_hash(), "dup-id")
        .await
        .expect_err("a registered shard whose file is gone is fail-closed");
    match &error {
        StoreError::ScopeShardUnreadable {
            shard_index,
            db_path,
            ..
        } => {
            assert_eq!(*shard_index, 1, "the missing shard is named");
            assert_eq!(
                db_path, &shard_one.db_path,
                "the path is the one that is gone"
            );
        }
        other => panic!("expected ScopeShardUnreadable, got {other:?}"),
    }
    assert!(
        !shard_one.db_path.exists(),
        "the delete must never re-create a missing shard file"
    );
    assert!(
        copy_exists(&fresh, &shard_zero.db_path, "dup-id").await,
        "the DESC scan errors before touching the older shard's copy"
    );
}

/// AC5: the address space is exactly the anchor's `(scope, scope_hash)` — the
/// same id in another project hash and in the user scope is another tenant's
/// row and survives.
#[tokio::test]
async fn test_delete_memory_in_scope_does_not_cross_scope_or_scope_hash() {
    const HASH_B: &str = "b1b2c3d4e5f60101";
    const HASH_C: &str = "c1b2c3d4e5f60101";

    let config = common::test_store_config("scope-delete-boundary", 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let project_a = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create project A shard");
    let project_b = manager
        .create_shard(Scope::Project, HASH_B, 0)
        .await
        .expect("create project B shard");
    let user_c = manager
        .create_shard(Scope::User, HASH_C, 0)
        .await
        .expect("create user C shard");

    insert_and_sync(&manager, &project_a, &seed_record("dup-id", TAG_SHARD_0)).await;
    insert_and_sync(&manager, &project_b, &seed_record("dup-id", TAG_SHARD_1)).await;
    insert_and_sync(&manager, &user_c, &seed_record("dup-id", TAG_SHARD_1)).await;

    let copies = manager
        .delete_memory_in_scope(Scope::Project, HASH, "dup-id")
        .await
        .expect("the anchor scope deletes");
    assert_eq!(copies, 1, "only the anchor scope's copy is deleted");

    assert!(
        !copy_exists(&manager, &project_a.db_path, "dup-id").await,
        "project A's copy is gone"
    );
    assert_eq!(
        stored_count(&manager, &project_a.db_path).await,
        0,
        "project A's measured count drops to 0"
    );
    assert_eq!(
        registry_count(&manager, &project_a).await,
        0,
        "project A's registry drops to 0"
    );

    assert!(
        copy_exists(&manager, &project_b.db_path, "dup-id").await,
        "another project hash is another tenant"
    );
    assert!(
        copy_exists(&manager, &user_c.db_path, "dup-id").await,
        "the user copy is not deleted by a project-scope call"
    );
    assert_eq!(
        stored_count(&manager, &project_b.db_path).await,
        1,
        "project B's count is untouched"
    );
    assert_eq!(
        registry_count(&manager, &project_b).await,
        1,
        "project B's registry is untouched"
    );
    assert_eq!(
        stored_count(&manager, &user_c.db_path).await,
        1,
        "the user shard's count is untouched"
    );
    assert_eq!(
        registry_count(&manager, &user_c).await,
        1,
        "the user shard's registry is untouched"
    );
}

/// AC6: an unsafe hash is rejected BEFORE the registry query — `""` would
/// select `get_all_shards`' wildcard branch and delete across projects.
#[tokio::test]
async fn test_delete_memory_in_scope_rejects_an_invalid_scope_hash_without_deleting() {
    let config = common::test_store_config("scope-delete-invalid-hash", 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let shard = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create the shard");
    insert_and_sync(&manager, &shard, &seed_record("dup-id", TAG_SHARD_0)).await;

    let error = manager
        .delete_memory_in_scope(Scope::Project, "", "dup-id")
        .await
        .expect_err("an empty scope hash is refused");
    assert!(
        matches!(error, StoreError::Core { .. }),
        "the scope guard maps to StoreError::Core, got {error:?}"
    );
    assert!(
        copy_exists(&manager, &shard.db_path, "dup-id").await,
        "the seeded row survives a rejected hash"
    );
}

/// AC7: a pre-v3 shard is migrated on touch before the counted batch —
/// `open_shard` does not migrate and `delete_memories` needs `shard_stats`.
#[tokio::test]
async fn test_delete_memory_in_scope_migrates_a_v2_stamped_shard_before_the_counted_delete() {
    let config = common::test_store_config("scope-delete-v2-stamped", 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let shard = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create the shard");
    insert_into_shard(
        &manager,
        &shard.db_path,
        &seed_record("dup-id", TAG_SHARD_0),
    )
    .await;
    insert_into_shard(
        &manager,
        &shard.db_path,
        &seed_record("neighbour", TAG_SHARD_0),
    )
    .await;

    // The W2 `test_a_v3_shaped_file_stamped_v2_converges` shape: the table is
    // gone but the file still claims v2, so only a migrating touch creates it.
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    db.execute("DROP TABLE shard_stats", Vec::new())
        .await
        .expect("drop the v3 table");
    db.execute(&sql::set_user_version_sql(2), Vec::new())
        .await
        .expect("stamp v2");
    drop(db);

    let copies = manager
        .delete_memory_in_scope(Scope::Project, HASH, "dup-id")
        .await
        .expect("a v2-stamped shard is migrated, not failed");
    assert_eq!(copies, 1, "the pre-v3 shard's copy is deleted");

    assert!(
        !copy_exists(&manager, &shard.db_path, "dup-id").await,
        "the row is gone"
    );
    assert_eq!(
        stored_count(&manager, &shard.db_path).await,
        1,
        "the migrated table carries the absolute count"
    );
    assert_eq!(
        registry_count(&manager, &shard).await,
        1,
        "the registry adopts the committed count"
    );
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    let version = query_one(db.as_ref(), sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version probe")
        .and_then(|row| row.i64("user_version"))
        .unwrap_or(0);
    assert_eq!(version, 3, "the file converged to v3");
}
