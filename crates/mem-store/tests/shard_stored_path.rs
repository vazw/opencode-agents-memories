//! Stored `db_path` shape battery. Upstream `shard-manager.ts` L441 documents
//! the invariant the registry relies on: "Stored db_path is always
//! `<scope>s/<basename>`", produced as
//! `join(`${scope}s`, basename(...)).replace(/\\/g, "/")` at `createShard`
//! (L173), `registerExistingShard` (L230) and `reassignShardScope` (L528);
//! `resolveStoredPath` re-anchors a row under `{scope}s/` on every read.
//!
//! Three statements write the column (`sql.rs`: `CREATE_SHARD`,
//! `REGISTER_EXISTING_SHARD`, `REASSIGN_SHARD_SCOPE`); the port has four
//! writers over them, each covered here by the raw column it leaves behind:
//!
//! 1. `create_shard` — upstream `createShard` (L173), via `CREATE_SHARD`;
//! 2. `register_existing_shard` — upstream `registerExistingShard` (L230),
//!    via `REGISTER_EXISTING_SHARD`;
//! 3. `reassign_shard_scope` — upstream `reassignShardScope` (L528), via
//!    `REASSIGN_SHARD_SCOPE`;
//! 4. `rotate_shard` — **port-specific, no upstream counterpart** (ADR-0022
//!    D-22-4: one atomic retire+insert batch in place of upstream's two-step
//!    create/retire). It reuses `CREATE_SHARD` and must keep the same shape.
//!
//! The readers all take the BASENAME, so a singular stored value still
//! resolved to the right file: the defect was invisible from outside the
//! registry. These tests read the RAW `db_path` column (never the resolved
//! `ShardInfo.db_path`, which masks the shape).

mod common;

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{Value, query_one};
use mem_store::operation_lock::METADATA_DB_NAME;
use mem_store::scope::Scope;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::sql;

const HASH: &str = "a1b2c3d4e5f60001";
/// A second well-formed hash: after a reassign the stored basename must be the
/// NEW file's, not the one the row already carried.
const NEW_HASH: &str = "b7e4c1d29f3a0002";

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

/// (a) the row stores `<scope>s/<file_name>`, (b) its resolution is a file
/// that exists under the storage root, (c) `resolve_stored_path` agrees with
/// the row's own resolved path.
async fn assert_stored_path(
    manager: &ShardManager,
    connections: &ConnectionManager,
    config: &StoreConfig,
    shard: &ShardInfo,
    scope: Scope,
    file_name: &str,
) {
    let scope_dir = format!("{}s", scope.as_str());
    let stored = stored_db_path(connections, config, shard.id).await;
    assert_eq!(
        stored,
        format!("{scope_dir}/{file_name}"),
        "upstream stores join(`${{scope}}s`, basename): {stored}"
    );
    assert!(
        stored.starts_with(&format!("{scope_dir}/")),
        "the singular scope dir is not a registry value: {stored}"
    );

    let resolved = manager.resolve_stored_path(&stored, scope.as_str());
    assert_eq!(
        resolved,
        config.storage_path.join(&scope_dir).join(file_name),
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

/// The file name `create_shard`/`rotate_shard` derive for `index`.
fn written_file_name(scope: Scope, scope_hash: &str, index: u32) -> String {
    format!("{}_{scope_hash}_shard_{index}.db", scope.as_str())
}

#[tokio::test]
async fn test_shard_writers_store_the_plural_scope_directory() {
    let config = common::test_store_config("stored-path-write", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;

    let created = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create shard");
    assert_eq!(created.shard_index, 0);
    assert_stored_path(
        &manager,
        &connections,
        &config,
        &created,
        Scope::Project,
        &written_file_name(Scope::Project, HASH, 0),
    )
    .await;

    // The rotation is the shard-SWAP write: the row for the new shard is the
    // one a rollback restores, so it must carry the same plural shape.
    let rotated = manager
        .rotate_shard(Scope::Project, HASH, created.id, 1)
        .await
        .expect("rotate shard");
    assert_eq!(rotated.shard_index, 1);
    assert_stored_path(
        &manager,
        &connections,
        &config,
        &rotated,
        Scope::Project,
        &written_file_name(Scope::Project, HASH, 1),
    )
    .await;
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
    assert_stored_path(
        &manager,
        &connections,
        &config,
        &registered,
        Scope::Project,
        &written_file_name(Scope::Project, HASH, 0),
    )
    .await;
}

/// `reassign_shard_scope` takes the NEW basename but keeps the scope the row
/// already carries (upstream L528: `const scope = String(row.scope)`), so the
/// plural is applied to the row's own scope text — the seam where a wrong
/// prefix is most plausible, since nothing in the argument names the scope.
#[tokio::test]
async fn test_reassigned_shard_row_stores_the_plural_scope_directory() {
    let config = common::test_store_config("stored-path-reassign", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;

    // Upstream's contract (L516): the caller renames the file — under the
    // scope's plural directory — before calling this.
    let created = manager
        .create_shard(Scope::Project, HASH, 0)
        .await
        .expect("create shard");
    let new_file_name = written_file_name(Scope::Project, NEW_HASH, 0);
    let new_path = config.storage_path.join("projects").join(&new_file_name);
    connections
        .close_connection(&created.db_path)
        .await
        .expect("close shard before the rename");
    std::fs::rename(&created.db_path, &new_path).expect("rename shard file");

    let reassigned = manager
        .reassign_shard_scope(created.id, NEW_HASH, &new_path, 0, true)
        .await
        .expect("reassign shard scope");
    assert_eq!(reassigned.id, created.id, "the same registry row");
    assert_eq!(reassigned.scope, Scope::Project, "reassign keeps the scope");
    assert_eq!(reassigned.scope_hash, NEW_HASH);
    assert_stored_path(
        &manager,
        &connections,
        &config,
        &reassigned,
        Scope::Project,
        &new_file_name,
    )
    .await;
}

/// The same writer on a USER row: the derived scope text is `user`, so a site
/// that hardcodes `projects/` (or drops the plural) is caught here even though
/// the project case above passes.
#[tokio::test]
async fn test_reassigned_user_shard_row_stores_the_plural_scope_directory() {
    let config = common::test_store_config("stored-path-reassign-user", 2, 1000);
    let (config, connections, manager) = common::real_components(config).await;

    let created = manager
        .create_shard(Scope::User, HASH, 0)
        .await
        .expect("create user shard");
    let new_file_name = written_file_name(Scope::User, NEW_HASH, 0);
    let new_path = config.storage_path.join("users").join(&new_file_name);
    connections
        .close_connection(&created.db_path)
        .await
        .expect("close shard before the rename");
    std::fs::rename(&created.db_path, &new_path).expect("rename shard file");

    let reassigned = manager
        .reassign_shard_scope(created.id, NEW_HASH, &new_path, 0, true)
        .await
        .expect("reassign shard scope");
    assert_eq!(reassigned.scope, Scope::User, "reassign keeps the scope");
    assert_stored_path(
        &manager,
        &connections,
        &config,
        &reassigned,
        Scope::User,
        &new_file_name,
    )
    .await;
}
