//! Lifecycle battery (blueprint W1.2 step 10): close_turso_and_invalidate_caches
//! = close_all + shard-manager reset + ready-gate reset (upstream lifecycle.ts).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::FakeConnectFactory;
use mem_store::connection::ConnectionManager;
use mem_store::db::ConnectFactory;
use mem_store::lifecycle::close_turso_and_invalidate_caches;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::ready::{ReadyGate, ReadyStep};
use mem_store::scope::Scope;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{MemoryInsert, count_all_vectors, insert_vector};

fn ready_step_counter() -> (Arc<AtomicUsize>, ReadyStep) {
    let counter = Arc::new(AtomicUsize::new(0));
    let probe = counter.clone();
    let step = ReadyStep {
        name: "probe",
        run: Arc::new(move || {
            let probe = probe.clone();
            Box::pin(async move {
                probe.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }),
    };
    (counter, step)
}

fn seed(id: &str, created: i64) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: "lifecycle".to_string(),
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
    }
}

// ---------------------------------------------------------------------------
// 1/3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reopens_databases_after_close_turso_and_invalidate_caches() {
    let config = common::store_config("lc-reopen");
    let storage = config.storage_path.clone();
    let connections = Arc::new(ConnectionManager::new(
        storage.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(mem_store::db::TursoConnectFactory),
    ));
    let shards = ShardManager::new(
        storage.clone(),
        config.clone(),
        connections.clone(),
        Arc::new(SystemProcessProbe),
    );
    let shard = shards
        .get_write_shard(Scope::Project, "a1b2c3d4e5f60001")
        .await
        .expect("shard");
    {
        let db = shards.open_shard(&shard.db_path).await.expect("open");
        insert_vector(db.as_ref(), &seed("keep-me", 1))
            .await
            .expect("insert");
    }

    close_turso_and_invalidate_caches(&connections, &shards, &ReadyGate::new(Vec::new())).await;

    // Everything reopens fresh; the data survived the close.
    let db = shards.open_shard(&shard.db_path).await.expect("reopen");
    assert_eq!(count_all_vectors(db.as_ref()).await, 1, "row persisted");
    let rows = shards
        .get_all_shards(Scope::Project, Some("a1b2c3d4e5f60001"))
        .await
        .expect("registry");
    assert_eq!(
        rows.len(),
        1,
        "registry row survived via reopened metadata.db"
    );
    assert_eq!(rows[0].id, shard.id);
}

// ---------------------------------------------------------------------------
// 2/3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_does_not_leak_in_flight_open_across_close_all() {
    let config = common::store_config("lc-leak");
    let storage = config.storage_path.clone();
    let fake = Arc::new(FakeConnectFactory::default());
    let connections = Arc::new(ConnectionManager::new(
        storage.clone(),
        config.clone(),
        Platform::current(),
        fake.clone() as Arc<dyn ConnectFactory>,
    ));
    let shards = ShardManager::new(
        storage.clone(),
        config.clone(),
        connections.clone(),
        Arc::new(SystemProcessProbe),
    );

    // Arm one gated open on the metadata db.
    fake.arm_open().await;
    let opener = {
        let connections = connections.clone();
        let storage = storage.join("metadata.db");
        tokio::spawn(async move { connections.get_connection(&storage).await })
    };
    // The in-flight open must keep the connection logged, not lost.
    while fake.opens.load(Ordering::SeqCst) < 1 {
        tokio::task::yield_now().await;
    }
    assert!(!connections.has_open_connection(&storage.join("metadata.db")));

    // Lifecycle while the open is parked: close_all awaits the shared
    // close-signal, then closes everything.
    let gate = ReadyGate::new(Vec::new());
    let closer = {
        let (connections, shards) = (connections.clone(), shards.clone());
        tokio::spawn(async move {
            close_turso_and_invalidate_caches(&connections, &shards, &gate).await;
        })
    };
    fake.release_next();
    let (opened, _) = tokio::join!(opener, closer);
    assert!(opened.expect("task").is_ok(), "open finished through close");
    assert!(
        !connections.has_open_connection(&storage.join("metadata.db")),
        "handle released after close-all"
    );

    // Next get opens FRESH (fake counts opens: 2 after ungate).
    fake.ungate().await;
    let _ = connections
        .get_connection(&storage.join("metadata.db"))
        .await;
    assert_eq!(fake.opens.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// 3/3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_close_invalidates_ready_gate_and_shard_manager() {
    let config = common::store_config("lc-gates");
    let connections = Arc::new(ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(mem_store::db::TursoConnectFactory),
    ));
    let shards = Arc::new(ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        Arc::new(SystemProcessProbe),
    ));

    let (counter, step) = ready_step_counter();
    let gate = Arc::new(ReadyGate::new(vec![step]));
    gate.ensure_ready().await.expect("first ready");
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    close_turso_and_invalidate_caches(&connections, &shards, &gate).await;

    gate.ensure_ready().await.expect("re-runs after reset");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "gate reset forced rerun");

    let shard = shards
        .get_write_shard(Scope::Project, "a1b2c3d4e5f60002")
        .await
        .expect("write-shard after reset");
    assert_eq!(shard.shard_index, 0);
}
