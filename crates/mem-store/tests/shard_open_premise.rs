//! Step-4 layer pins for issue 130: the create-on-open premise the mem-web
//! READ-lane guard decision stands on.
//!
//! turso create-on-open is the ONLY file creator in the engine (issues 130's
//! read lane never creates — the guard stats and refuses instead). These pins
//! freeze the two mem-store seams that create:
//!
//! - `ConnectionManager::get_connection` (→ `open_database`: `create_dir_all`
//!   + turso-connect) — blueprint pin 4b;
//! - `ShardManager::open_shard` (delegates to `get_connection`) — blueprint
//!   pin 4c, the seam any future producer-side refusal (the ADD/MOVE hoist
//!   pairings) must not break for a file that does not exist yet.
//!
//! A missing path MUST answer `Ok` and the file MUST exist afterwards.

mod common;

use std::sync::Arc;

use common::test_store_config;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::Platform;
use mem_store::shard_manager::ShardManager;

/// Premise 4b (layer-2 pin): `get_connection` on a not-yet-existing path
/// answers `Ok` and the file exists afterwards. This is the documented create
/// contract the read guard deliberately does NOT touch; a refusal inserted
/// here (Step-5 kill 3a's ADD) reds this test.
#[tokio::test]
async fn get_connection_creates_a_missing_file() {
    let config = test_store_config("connection-premise", 768, 1000);
    let storage = config.storage_path.clone();
    let connections = Arc::new(ConnectionManager::new(
        storage.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let path = storage.join("project_a1b2c3d4e5f67890_shard_0.db");
    assert!(
        !path.exists(),
        "premise: the file does not exist before the call"
    );
    let handle = connections
        .get_connection(&path)
        .await
        .expect("create-on-open must answer Ok for a new path");
    assert!(
        path.exists(),
        "the open must CREATE the missing database file"
    );
    drop(handle);
}

/// Premise 4c (layer-2 pin one level up): `ShardManager::open_shard` on a
/// not-yet-existing path answers `Ok` and the file exists afterwards. The
/// refused-open mutant of kill 3b (the MOVE hoist) reds this test.
#[tokio::test]
async fn open_shard_creates_a_missing_file() {
    let config = test_store_config("open-shard-premise", 768, 1000);
    let storage = config.storage_path.clone();
    let connections = Arc::new(ConnectionManager::new(
        storage.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        storage.clone(),
        config.clone(),
        connections,
        Arc::new(mem_store::platform::SystemProcessProbe),
        None,
    );
    let path = storage.join("user_a1b2c3d4e5f67890_shard_0.db");
    assert!(
        !path.exists(),
        "premise: the file does not exist before the call"
    );
    let handle = manager
        .open_shard(&path)
        .await
        .expect("create-on-open must answer Ok for a new path");
    assert!(
        path.exists(),
        "the open must CREATE the missing database file"
    );
    drop(handle);
}
