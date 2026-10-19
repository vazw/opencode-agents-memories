//! Shard write-scope lock battery (blueprint W1.3 step 2): the W1.2-deferred
//! `ShardManager::with_scope_write_lock` primitive (first caller: the
//! re-embed migration) + the `init_shard_db` dims/model override seam the
//! legacy migrator consumes.

mod common;

use std::sync::{Arc, Mutex};

use mem_store::scope::Scope;

// ---------------------------------------------------------------------------
// 1/2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_with_scope_write_lock_serializes_same_scope_and_allows_distinct_scopes() {
    let config = common::store_config("scope-lock");
    let (_config, _connections, shards) = common::real_components(config).await;

    // Ordering probe: append-only event log (synchronous critical sections,
    // never held across `.await`).
    let probe: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let record = |probe: &Arc<Mutex<Vec<String>>>, event: &str| {
        probe.lock().unwrap().push(event.to_string());
    };

    // Barriers: A signals it holds the lock, then parks until released;
    // C signals full completion (distinct-scope proof).
    let (a_entered_tx, a_entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_a_tx, release_a_rx) = tokio::sync::oneshot::channel::<()>();
    let (b_started_tx, b_started_rx) = tokio::sync::oneshot::channel::<()>();
    let (c_done_tx, c_done_rx) = tokio::sync::oneshot::channel::<()>();

    let scope_a = shards.clone();
    let probe_a = probe.clone();
    let task_a = tokio::spawn(async move {
        let result: Result<String, mem_store::error::StoreError> = scope_a
            .with_scope_write_lock(Scope::User, "a1b2c3d4e5f60001", || {
                let probe = probe_a.clone();
                Box::pin(async move {
                    record(&probe, "A:enter");
                    let _ = a_entered_tx.send(());
                    let _ = release_a_rx.await;
                    record(&probe, "A:exit");
                    Ok::<String, mem_store::error::StoreError>("a".to_string())
                })
            })
            .await;
        result.expect("task A op")
    });

    // Wait until A demonstrably holds the (User, hash_1) lock.
    a_entered_rx.await.expect("A entered the lock");

    let scope_b = shards.clone();
    let probe_b = probe.clone();
    let task_b = tokio::spawn(async move {
        record(&probe_b, "B:attempt");
        let _ = b_started_tx.send(());
        let result: Result<String, mem_store::error::StoreError> = scope_b
            .with_scope_write_lock(Scope::User, "a1b2c3d4e5f60001", || {
                let probe = probe_b.clone();
                Box::pin(async move {
                    record(&probe, "B:enter");
                    record(&probe, "B:exit");
                    Ok::<String, mem_store::error::StoreError>("b".to_string())
                })
            })
            .await;
        result.expect("task B op")
    });

    // C takes a DISTINCT scope hash: it must be able to run while A holds
    // the (User, hash_1) lock.
    let scope_c = shards.clone();
    let probe_c = probe.clone();
    let task_c = tokio::spawn(async move {
        let c_done_tx = c_done_tx;
        let result: Result<String, mem_store::error::StoreError> = scope_c
            .with_scope_write_lock(Scope::User, "b2c3d4e5f6000009", move || {
                let probe = probe_c.clone();
                let c_done_tx = c_done_tx;
                Box::pin(async move {
                    record(&probe, "C:enter");
                    record(&probe, "C:exit");
                    let _ = c_done_tx.send(());
                    Ok::<String, mem_store::error::StoreError>("c".to_string())
                })
            })
            .await;
        result.expect("task C op")
    });

    // B attempted the same-scope lock (and is now parked behind A).
    b_started_rx.await.expect("B attempted");
    // C — distinct scope — completes while A is still parked.
    c_done_rx.await.expect("C completed");

    assert_eq!(
        probe.lock().unwrap().clone(),
        vec![
            "A:enter".to_string(),
            "B:attempt".to_string(),
            "C:enter".to_string(),
            "C:exit".to_string(),
        ],
        "same-scope B blocked while distinct-scope C ran"
    );

    // Release A: A exits, then B acquires the same-scope lock.
    let _ = release_a_tx.send(());
    let (ra, rb) = tokio::join!(task_a, task_b);
    assert_eq!(ra.expect("task A").as_str(), "a");
    assert_eq!(rb.expect("task B").as_str(), "b");
    assert_eq!(task_c.await.expect("task C").as_str(), "c");

    assert_eq!(
        probe.lock().unwrap().clone(),
        vec![
            "A:enter".to_string(),
            "B:attempt".to_string(),
            "C:enter".to_string(),
            "C:exit".to_string(),
            "A:exit".to_string(),
            "B:enter".to_string(),
            "B:exit".to_string(),
        ],
        "strict same-scope serialization: B entered only after A exited"
    );
}

// ---------------------------------------------------------------------------
// 2/2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_init_shard_db_writes_given_dimensions_and_model() {
    let config = common::store_config("scope-init");
    let (config, _connections, shards) = common::real_components(config).await;

    // The legacy-dims override seam: init with dimensions/model that differ
    // from the manager config (768 / "legacy-model" vs config 2 / test-model).
    let db = common::open_real_db(&config, "init.db")
        .await
        .expect("open");
    shards
        .init_shard_db(db.as_ref(), 768, "legacy-model")
        .await
        .expect("init_shard_db");

    let dims = db
        .query(
            "SELECT value FROM shard_metadata WHERE key = 'embedding_dimensions'",
            vec![],
        )
        .await
        .expect("dims probe");
    assert_eq!(dims[0].text("value").as_deref(), Some("768"));
    let model = db
        .query(
            "SELECT value FROM shard_metadata WHERE key = 'embedding_model'",
            vec![],
        )
        .await
        .expect("model probe");
    assert_eq!(model[0].text("value").as_deref(), Some("legacy-model"));
    // The memories table exists with the requested F32_BLOB width (turso
    // normalises the PRAGMA type to "F32_BLOB"; the width survives in the
    // stored DDL, verified against sqlite_master).
    let ddl_rows = db
        .query(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'memories'",
            vec![],
        )
        .await
        .expect("sqlite_master");
    let ddl = ddl_rows[0].text("sql").unwrap_or_default();
    assert!(
        ddl.contains("F32_BLOB(768)"),
        "init writes the GIVEN dimensions into the DDL: {ddl}"
    );
}
