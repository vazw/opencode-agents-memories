//! W4 — exactly one active shard per `(scope, scope_hash)`.
//!
//! Eleven tests, net-new battery (no upstream counterpart): the metadata v2
//! partial unique index + repair, the typed conflicts, the one-transaction
//! rotation and the validated reactivation.

mod common;

use mem_store::db::{Value, query_one};
use mem_store::error::StoreError;
use mem_store::scope::Scope;
use mem_store::sql;
use mem_store::vector::{MemoryInsert, insert_vector};

const HASH: &str = "a1b2c3d4e5f60001";

fn project() -> Value {
    Value::Text("project".to_string())
}

fn hash() -> Value {
    Value::Text(HASH.to_string())
}

// ---------------------------------------------------------------------------
// Step 1 — v2: at most one active row, many inactive rows allowed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_metadata_v2_allows_many_inactive_rows_but_only_one_active() {
    let config = common::test_store_config("w4-idx", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;
    let first = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("first active shard");

    let metadata = connections
        .get_connection(&config.storage_path.join("metadata.db"))
        .await
        .expect("metadata connection");
    let duplicate = metadata
        .execute(
            sql::CREATE_SHARD,
            vec![
                project(),
                hash(),
                Value::Integer(1),
                Value::Text(format!("project/project_{HASH}_shard_1.db")),
                Value::Integer(0),
            ],
        )
        .await;
    match duplicate {
        Err(StoreError::Turso {
            is_constraint: true,
            ..
        }) => {}
        Err(other) => panic!("expected a unique-constraint refusal, got {other:?}"),
        Ok(_) => panic!("a second active row was accepted"),
    }

    let path_two = manager
        .get_shard_path(Scope::Project, HASH, 2)
        .expect("path 2");
    let path_three = manager
        .get_shard_path(Scope::Project, HASH, 3)
        .expect("path 3");
    manager
        .register_existing_shard(Scope::Project, HASH, 2, &path_two, 0, false)
        .await
        .expect("inactive row 2 is allowed");
    manager
        .register_existing_shard(Scope::Project, HASH, 3, &path_three, 0, false)
        .await
        .expect("inactive row 3 is allowed");

    let active = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("active read")
        .expect("active row present");
    assert_eq!(active.id, first.id);
    assert_eq!(active.shard_index, 0);
}

#[tokio::test]
async fn test_metadata_db_reopens_at_v2_after_the_migration() {
    let config = common::test_store_config("w4-reopen", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;
    let _ = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("metadata initialized");

    let metadata = connections
        .get_connection(&config.storage_path.join("metadata.db"))
        .await
        .expect("metadata connection");
    let version = query_one(metadata.as_ref(), sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version read")
        .and_then(|row| row.i64("user_version"));
    assert_eq!(version, Some(2), "metadata.db is stamped v2");

    manager.reset().expect("reset");
    let reopened = manager.get_active_shard(Scope::Project, HASH).await;
    assert!(
        reopened.is_ok(),
        "a v2 file must reopen under the bumped ceiling, got {reopened:?}"
    );
}

#[tokio::test]
async fn test_metadata_v2_repairs_extra_active_shards_to_the_highest_index() {
    let config = common::test_store_config("w4-repair", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;
    let _ = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("metadata initialized at v2");
    let metadata = connections
        .get_connection(&config.storage_path.join("metadata.db"))
        .await
        .expect("metadata connection");

    // Reachable v1 state: two actives, one inactive, no partial index.
    metadata
        .execute("DROP INDEX IF EXISTS idx_shards_one_active", Vec::new())
        .await
        .expect("drop the v2 index");
    metadata
        .execute(&sql::set_user_version_sql(1), Vec::new())
        .await
        .expect("downgrade to v1");
    for (index, is_active) in [(0_i64, 1_i64), (1, 0), (2, 1)] {
        metadata
            .execute(
                "INSERT INTO shards \
                 (scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at) \
                 VALUES (?, ?, ?, ?, 0, ?, 0)",
                vec![
                    project(),
                    hash(),
                    Value::Integer(index),
                    Value::Text(format!("project/project_{HASH}_shard_{index}.db")),
                    Value::Integer(is_active),
                ],
            )
            .await
            .expect("seed v1 row");
    }

    let before = query_one(
        metadata.as_ref(),
        sql::GET_ACTIVE_SHARD,
        vec![project(), hash()],
    )
    .await
    .expect("pre-migration read")
    .expect("pre-migration active row");
    assert_eq!(before.i64("shard_index"), Some(2));
    let survivor_id = before.i64("id").expect("survivor id");

    manager.reset().expect("reset");
    let active = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("post-migration read")
        .expect("active row present");
    assert_eq!(active.shard_index, 2, "highest index survives the repair");
    assert_eq!(active.id, survivor_id, "the row the code already read wins");

    let rows = metadata
        .query("SELECT id, shard_index, is_active FROM shards", Vec::new())
        .await
        .expect("registry rows");
    let active_rows: Vec<_> = rows
        .iter()
        .filter(|row| row.i64("is_active") == Some(1))
        .collect();
    assert_eq!(active_rows.len(), 1, "exactly one active row remains");
    assert_eq!(active_rows[0].i64("shard_index"), Some(2));

    let version = query_one(metadata.as_ref(), sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version read")
        .and_then(|row| row.i64("user_version"));
    assert_eq!(version, Some(2), "the repair committed with v2");
}

// ---------------------------------------------------------------------------
// Step 2 — typed conflict from `create_shard`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_shard_refuses_a_second_active_shard_with_a_typed_conflict() {
    let config = common::test_store_config("w4-create-conflict", 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let first = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("first active shard");

    let error = manager
        .create_shard(Scope::Project, HASH, 1)
        .await
        .expect_err("a second active shard is refused");
    match error {
        StoreError::ActiveShardConflict {
            scope,
            scope_hash,
            shard_index,
            active_shard_index,
        } => {
            assert_eq!(scope, "project");
            assert_eq!(scope_hash, HASH);
            assert_eq!(shard_index, 1);
            assert_eq!(active_shard_index, 0);
        }
        other => panic!("expected ActiveShardConflict, got {other:?}"),
    }

    let active = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("active read")
        .expect("active row present");
    assert_eq!(active.id, first.id);
    assert_eq!(active.shard_index, 0);
}

#[tokio::test]
async fn test_registering_a_second_active_shard_is_a_typed_conflict() {
    let config = common::test_store_config("w4-register-conflict", 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let first = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("first active shard");
    let path = manager
        .get_shard_path(Scope::Project, HASH, 1)
        .expect("path 1");

    let error = manager
        .register_existing_shard(Scope::Project, HASH, 1, &path, 0, true)
        .await
        .expect_err("a second active shard is refused");
    match error {
        StoreError::ActiveShardConflict {
            shard_index,
            active_shard_index,
            ..
        } => {
            assert_eq!(shard_index, 1);
            assert_eq!(active_shard_index, 0);
        }
        other => panic!("expected ActiveShardConflict, got {other:?}"),
    }

    let active = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("active read")
        .expect("active row present");
    assert_eq!(active.id, first.id);
}

#[tokio::test]
async fn test_registering_an_inactive_row_as_active_when_another_is_active_is_a_typed_conflict() {
    let config = common::test_store_config("w4-register-existing", 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let first = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("first active shard");
    let path = manager
        .get_shard_path(Scope::Project, HASH, 1)
        .expect("path 1");
    let inactive = manager
        .register_existing_shard(Scope::Project, HASH, 1, &path, 0, false)
        .await
        .expect("inactive row 1");
    assert!(!inactive.is_active);

    let error = manager
        .register_existing_shard(Scope::Project, HASH, 1, &path, 0, true)
        .await
        .expect_err("activating row 1 is refused");
    match error {
        StoreError::ActiveShardConflict {
            shard_index,
            active_shard_index,
            ..
        } => {
            assert_eq!(shard_index, 1);
            assert_eq!(active_shard_index, 0);
        }
        other => panic!("expected ActiveShardConflict, got {other:?}"),
    }

    let rows = manager
        .get_all_shards(Scope::Project, Some(HASH))
        .await
        .expect("registry read");
    let row_one = rows
        .iter()
        .find(|row| row.shard_index == 1)
        .expect("row 1 present");
    assert!(
        !row_one.is_active,
        "the activation was not dropped into row 1"
    );
    let active = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("active read")
        .expect("active row present");
    assert_eq!(active.id, first.id);
}

fn record(id: &str, created: i64) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: "rotation".to_string(),
        vector: vec![1.0f32, 0.0],
        tags_vector: None,
        container_tag: format!("opencode_project_{HASH}"),
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
    }
}

// ---------------------------------------------------------------------------
// Step 4 — one-transaction rotation
// ---------------------------------------------------------------------------

/// A failed rotation must leave the previous shard active: the retire and the
/// insert are ONE batch, so the key conflict rolls the retire back.
#[tokio::test]
async fn test_a_failed_rotation_leaves_the_previous_shard_active() {
    let config = common::test_store_config("w4-rotate-fail", 2, 1);
    let (_config, _connections, manager) = common::real_components(config).await;
    let first = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("shard 0");
    let db = manager
        .open_shard(&first.db_path)
        .await
        .expect("open shard 0");
    insert_vector(db.as_ref(), &record("m0", 0))
        .await
        .expect("fill shard 0");
    drop(db);
    let path_one = manager
        .get_shard_path(Scope::Project, HASH, 1)
        .expect("path 1");
    manager
        .register_existing_shard(Scope::Project, HASH, 1, &path_one, 0, false)
        .await
        .expect("inactive row 1");

    let error = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect_err("the occupied index refuses the rotation");
    match error {
        StoreError::ShardIndexOccupied { shard_index } => assert_eq!(shard_index, 1),
        other => panic!("expected ShardIndexOccupied, got {other:?}"),
    }

    let active = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("active read")
        .expect("the previous shard stays active");
    assert_eq!(active.id, first.id);
    assert_eq!(active.shard_index, 0);
}

#[tokio::test]
async fn test_two_rotations_from_one_observation_leave_exactly_one_active_shard() {
    let config = common::test_store_config("w4-rotate-stale", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;
    let a = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("shard 0");
    let rotated = manager
        .rotate_shard(Scope::Project, HASH, a.id, 1)
        .await
        .expect("first rotation");
    assert_eq!(rotated.shard_index, 1);
    assert!(rotated.is_active);

    let error = manager
        .rotate_shard(Scope::Project, HASH, a.id, 2)
        .await
        .expect_err("the stale retire id cannot rotate again");
    match error {
        StoreError::ActiveShardConflict {
            shard_index,
            active_shard_index,
            ..
        } => {
            assert_eq!(shard_index, 2);
            assert_eq!(active_shard_index, 1);
        }
        other => panic!("expected ActiveShardConflict, got {other:?}"),
    }

    let metadata = connections
        .get_connection(&config.storage_path.join("metadata.db"))
        .await
        .expect("metadata connection");
    let active_rows = metadata
        .query(
            "SELECT id FROM shards WHERE scope = ? AND scope_hash = ? AND is_active = 1",
            vec![project(), hash()],
        )
        .await
        .expect("active rows");
    assert_eq!(active_rows.len(), 1, "exactly one active row");
}

#[tokio::test]
async fn test_rotation_does_not_retire_a_shard_outside_its_scope() {
    const OTHER_HASH: &str = "a1b2c3d4e5f60002";
    let config = common::test_store_config("w4-rotate-scope", 2, 1000);
    let (_config, _connections, manager) = common::real_components(config).await;
    let b = manager
        .create_shard(Scope::Project, OTHER_HASH, 0)
        .await
        .expect("B active shard");
    let a = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("A active shard");

    let error = manager
        .rotate_shard(Scope::Project, HASH, b.id, 1)
        .await
        .expect_err("a foreign retire id cannot rotate A");
    match error {
        StoreError::ActiveShardConflict {
            shard_index,
            active_shard_index,
            ..
        } => {
            assert_eq!(shard_index, 1);
            assert_eq!(active_shard_index, 0);
        }
        other => panic!("expected ActiveShardConflict, got {other:?}"),
    }

    let b_active = manager
        .get_active_shard(Scope::Project, OTHER_HASH)
        .await
        .expect("B active read")
        .expect("B active row present");
    assert_eq!(b_active.id, b.id, "B's active row is untouched");
    let a_active = manager
        .get_active_shard(Scope::Project, HASH)
        .await
        .expect("A active read")
        .expect("A active row present");
    assert_eq!(a_active.id, a.id);

    // The id-only mutant is only observable on a SUCCESSFUL batch (a failed
    // one rolls its deactivate back): A has no active row, so the scoped
    // retire of a foreign id is a no-op and the rotation succeeds. An id-only
    // retire would deactivate B while A rotates.
    manager
        .mark_shard_read_only(a.id)
        .await
        .expect("retire A before the success arm");
    let rotated = manager
        .rotate_shard(Scope::Project, HASH, b.id, 2)
        .await
        .expect("a scoped retire of a foreign id is a no-op");
    assert_eq!(rotated.shard_index, 2);
    assert!(rotated.is_active);
    let b_active = manager
        .get_active_shard(Scope::Project, OTHER_HASH)
        .await
        .expect("B active read")
        .expect("B active row present after A's successful rotation");
    assert_eq!(
        b_active.id, b.id,
        "B's active row is untouched by A's rotation"
    );
}
