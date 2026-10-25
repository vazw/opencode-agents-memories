//! W1 (issue 43.1) — the scope-wide memory-id seam
//! [`ShardManager::find_memory_in_scope`](mem_store::shard_manager::ShardManager::find_memory_in_scope).
//!
//! `memories.id` is `TEXT PRIMARY KEY`, so uniqueness is enforced per SHARD
//! FILE while a scope owns many shard files. The seam scans every shard of the
//! scope (active and retired), answers with the copy in the highest
//! `shard_index`, and fails closed when any registered shard cannot be read.
//!
//! Fixtures: dims 2 / cap 1 (`common::test_store_config(prefix, 2, 1)`) so one
//! inserted row forces a rotation, and `common::real_components` for the real
//! turso connections.

mod common;

use std::path::Path;

use mem_store::error::StoreError;
use mem_store::scope::Scope;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{MemoryInsert, insert_vector};

/// The container tag of the ordinary fixture rows.
const CONTAINER_TAG: &str = "opencode_project_a1b2c3d4e5f60101";
/// The retired copy's tag in the duplicate fixture.
const TAG_SHARD_0: &str = "opencode_project_a1b2c3d4e5f60102";
/// The active copy's tag in the duplicate fixture.
const TAG_SHARD_1: &str = "opencode_project_a1b2c3d4e5f60103";

fn scope_hash() -> String {
    "a1b2c3d4e5f60101".to_string()
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

/// AC1: the seam finds a row that lives in a RETIRED shard, and the same call
/// answers `Ok(None)` for an id no shard holds (`None` is reachable, not an
/// error blanket).
#[tokio::test]
async fn test_find_memory_in_scope_finds_a_row_in_a_retired_shard() {
    let config = common::test_store_config("scope-ids-retired", 2, 1);
    let (_config, _connections, manager) = common::real_components(config).await;
    let hash = scope_hash();

    // Shard 0 takes one row; the cap is 1, so the next resolution retires it.
    let first = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 0");
    assert_eq!(first.shard_index, 0);
    insert_into_shard(
        &manager,
        &first.db_path,
        &seed_record("old-id", CONTAINER_TAG),
    )
    .await;

    let second = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 1");
    assert_eq!(second.shard_index, 1, "the rotation really happened");
    insert_into_shard(
        &manager,
        &second.db_path,
        &seed_record("new-id", CONTAINER_TAG),
    )
    .await;

    let found = manager
        .find_memory_in_scope(Scope::Project, &hash, "old-id")
        .await
        .expect("a healthy retired shard is not an error")
        .expect("the retired row is found");
    assert_eq!(
        found.shard.shard_index, 0,
        "the row lives in the retired shard, not the active one"
    );
    assert_eq!(found.container_tag.as_deref(), Some(CONTAINER_TAG));
    assert_eq!(found.session_id.as_deref(), Some("session-1"));

    // Positive control for `Ok(None)`: the id is in no shard of the scope.
    assert!(
        manager
            .find_memory_in_scope(Scope::Project, &hash, "absent-id")
            .await
            .expect("an absent id is not an error")
            .is_none(),
        "an id no shard holds is `Ok(None)`"
    );

    let registry = manager
        .get_all_shards(Scope::Project, Some(&hash))
        .await
        .expect("registry rows");
    assert_eq!(registry.len(), 2, "one active + one retired shard");
}

/// AC2: resolution is deterministic — when an id exists in two shards the
/// HIGHEST `shard_index` wins (the latest truth), never the first hit of an
/// ascending scan.
#[tokio::test]
async fn test_find_memory_in_scope_prefers_the_highest_shard_index_when_an_id_exists_twice() {
    let config = common::test_store_config("scope-ids-duplicate", 2, 1);
    let (_config, _connections, manager) = common::real_components(config).await;
    let hash = scope_hash();

    // Shard 0: one filler row, then rotate (cap 1).
    let first = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 0");
    assert_eq!(first.shard_index, 0);
    insert_into_shard(
        &manager,
        &first.db_path,
        &seed_record("filler", CONTAINER_TAG),
    )
    .await;
    let second = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 1");
    assert_eq!(second.shard_index, 1, "the rotation really happened");

    // The pre-W1 damage state, built directly (the per-file PK allows it):
    // the same id in the retired shard 0 and in the active shard 1, each copy
    // carrying its own container tag.
    insert_into_shard(
        &manager,
        &first.db_path,
        &seed_record("dup-id", TAG_SHARD_0),
    )
    .await;
    insert_into_shard(
        &manager,
        &second.db_path,
        &seed_record("dup-id", TAG_SHARD_1),
    )
    .await;

    let found = manager
        .find_memory_in_scope(Scope::Project, &hash, "dup-id")
        .await
        .expect("lookup")
        .expect("the id exists twice");
    assert_eq!(
        found.shard.shard_index, 1,
        "the newest copy wins (DESC scan, not the first ascending hit)"
    );
    assert_eq!(
        found.container_tag.as_deref(),
        Some(TAG_SHARD_1),
        "the returned identity is the shard-1 copy's, not the shard-0 copy's"
    );
}

/// AC3: fail closed when a registered shard file is missing — and never
/// resurrect it.
///
/// The ONE mechanism able to produce the `Err` in this fixture: shard 0 is
/// registered in the metadata registry and its `.db` was deliberately removed.
/// No other step of this call can fail (every remaining file exists and opens
/// cleanly).
#[tokio::test]
async fn test_find_memory_in_scope_fails_closed_when_a_registered_shard_file_is_missing() {
    let config = common::test_store_config("scope-ids-missing", 2, 1);
    let (config, connections, manager) = common::real_components(config).await;
    let hash = scope_hash();

    let first = manager
        .get_write_shard(Scope::Project, &hash)
        .await
        .expect("shard 0");
    assert_eq!(first.shard_index, 0);
    let shard_zero = manager
        .get_shard_path(Scope::Project, &hash, 0)
        .expect("shard path");
    assert_eq!(shard_zero, first.db_path);
    assert!(shard_zero.exists(), "shard 0's file was created");

    // Positive control: with the file present the very same call is `Ok(None)`.
    assert!(
        manager
            .find_memory_in_scope(Scope::Project, &hash, "absent-id")
            .await
            .expect("a healthy scan")
            .is_none(),
        "the control pass reads every shard without error"
    );

    // Drop every handle — a cached connection would still read the UNLINKED
    // file and mask the absence — then delete shard 0's database and build a
    // FRESH manager (fresh `ConnectionManager` cache) over the same root.
    drop(manager);
    drop(connections);
    std::fs::remove_file(&shard_zero).expect("remove shard 0's database");
    assert!(!shard_zero.exists());

    let (_config, _connections, fresh) = common::real_components(config).await;
    let error = fresh
        .find_memory_in_scope(Scope::Project, &hash, "absent-id")
        .await
        .expect_err("a registered shard whose file is gone is fail-closed");
    match &error {
        StoreError::ScopeShardUnreadable {
            shard_index,
            db_path,
            ..
        } => {
            assert_eq!(*shard_index, 0, "the missing shard is named");
            assert_eq!(db_path, &shard_zero, "the path is the one that is gone");
        }
        other => panic!("expected ScopeShardUnreadable, got {other:?}"),
    }
    assert!(
        !shard_zero.exists(),
        "the scan must never re-create a missing shard file"
    );
}
