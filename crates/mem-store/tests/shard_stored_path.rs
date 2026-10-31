//! Stored `db_path` shape battery. Upstream `shard-manager.ts` L441 documents
//! the invariant the registry relies on: "Stored db_path is always
//! `<scope>s/<basename>`" — written that way at `createShard` (L173),
//! `registerExistingShard` (L230) and `reassignShardScope` (L528) — and
//! `resolveStoredPath` re-anchors a row under `{scope}s/` on every read.
//!
//! The readers all take the BASENAME, so a singular stored value still
//! resolved to the right file: the defect was invisible from outside the
//! registry. These tests read the RAW `db_path` column (never the resolved
//! `ShardInfo.db_path`, which masks the shape) and pin the plural form on
//! every writer.

mod common;

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{Value, query_one};
use mem_store::operation_lock::METADATA_DB_NAME;
use mem_store::scope::Scope;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::sql;

const HASH: &str = "a1b2c3d4e5f60001";

/// The raw `db_path` text of one registry row.
async fn stored_db_path(
    connections: &ConnectionManager,
    config: &StoreConfig,
    shard_id: i64,
) -> String {
    let metadata = connections
        .get_connection(&config.storage_path.join(METADATA_DB_NAME))
        .await
        .expect("metadata connection");
    let row = query_one(
        metadata.as_ref(),
        sql::GET_SHARD_BY_ID,
        vec![Value::Integer(shard_id)],
    )
    .await
    .expect("registry read")
    .expect("registry row present");
    row.text("db_path").expect("db_path column")
}

/// (a) the row stores `projects/<basename>`, (b) its resolution is a file that
/// exists under the storage root, (c) `resolve_stored_path` agrees with the
/// row's own resolved path.
async fn assert_plural_stored_path(
    manager: &ShardManager,
    connections: &ConnectionManager,
    config: &StoreConfig,
    shard: &ShardInfo,
) {
    let stored = stored_db_path(connections, config, shard.id).await;
    let file_name = format!("project_{HASH}_shard_{}.db", shard.shard_index);
    assert_eq!(
        stored,
        format!("projects/{file_name}"),
        "upstream stores join(`${{scope}}s`, basename): {stored}"
    );
    assert!(
        stored.starts_with("projects/"),
        "the singular scope dir is not a registry value: {stored}"
    );

    let resolved = manager.resolve_stored_path(&stored, "project");
    assert_eq!(
        resolved,
        config.storage_path.join("projects").join(&file_name),
        "the stored value resolves under the plural scope dir"
    );
    assert_eq!(
        resolved, shard.db_path,
        "resolve_stored_path(row) equals the row's own path"
    );
    assert!(
        resolved.exists(),
        "the row resolves to a live shard file: {}",
        resolved.display()
    );
    assert!(
        resolved.starts_with(&config.storage_path),
        "the resolved file is inside the storage root: {}",
        resolved.display()
    );
}

#[tokio::test]
async fn test_shard_writers_store_the_plural_scope_directory() {
    let config = common::test_store_config("stored-path-write", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;

    let created = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create shard");
    assert_plural_stored_path(&manager, &connections, &config, &created).await;

    // The rotation is the shard-SWAP write: the row for the new shard is the
    // one a rollback restores, so it must carry the same plural shape.
    let rotated = manager
        .rotate_shard(Scope::Project, HASH, created.id, 1)
        .await
        .expect("rotate shard");
    assert_eq!(rotated.shard_index, 1);
    assert_plural_stored_path(&manager, &connections, &config, &rotated).await;
}

#[tokio::test]
async fn test_registered_shard_row_stores_the_plural_scope_directory() {
    let config = common::test_store_config("stored-path-register", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;

    // Registration does not create the file, and (b) is about a LIVE file, so
    // the shard exists first and is then re-registered by its own path (the
    // upsert halves of `registerExistingShard`).
    let created = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create shard");
    let existing = manager
        .get_shard_path(Scope::Project, HASH, 0)
        .expect("shard path");
    assert!(existing.exists(), "the fixture file exists");

    let registered = manager
        .register_existing_shard(Scope::Project, HASH, 0, &existing, 0, true)
        .await
        .expect("register existing shard");
    assert_eq!(registered.id, created.id, "the upsert keeps the same row");
    assert_plural_stored_path(&manager, &connections, &config, &registered).await;
}
