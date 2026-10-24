//! Turso connection manager battery (blueprint W1.2 step 4): containment,
//! open dedup, close serialization, encryption resolution, experiment flags.

mod common;

use std::sync::Arc;

use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, ExperimentalFeature, TursoConnectFactory};
use mem_store::error::StoreError;

// ---------------------------------------------------------------------------
// 1/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_deduplicates_concurrent_get_connection_calls_for_same_path() {
    let config = common::store_config("cm-dedup");
    let factory = Arc::new(common::FakeConnectFactory::default());
    // Gate THREE opens; only the first must ever fire.
    for _ in 0..3 {
        factory.arm_open().await;
    }
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        factory.clone() as Arc<dyn ConnectFactory>,
    );
    let path = config.storage_path.join("shared.db");

    let mut getters = Vec::new();
    for _ in 0..3 {
        let manager = manager.clone();
        let path = path.clone();
        getters.push(tokio::spawn(
            async move { manager.get_connection(&path).await },
        ));
    }
    // While the shared open waits behind its gate, release all armed opens.
    factory.release_all();
    let mut joined = Vec::new();
    for getter in getters {
        joined.push(getter.await.expect("getter join").expect("open"));
    }
    let handles = joined;

    // All three getters observe the SAME handle.
    assert!(
        Arc::ptr_eq(&handles[0], &handles[1]) && Arc::ptr_eq(&handles[0], &handles[2]),
        "one handle shared by all three getters"
    );
}

// ---------------------------------------------------------------------------
// 2/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_waits_for_in_flight_open_before_closing_and_serializes_reopen() {
    let config = common::store_config("cm-order");
    let factory = Arc::new(common::FakeConnectFactory::default());
    factory.arm_open().await; // gate open0 only
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        factory.clone() as Arc<dyn ConnectFactory>,
    );
    let path = config.storage_path.join("cycle.db");

    let open_task = tokio::spawn({
        let manager = manager.clone();
        let path = path.clone();
        async move { manager.get_connection(&path).await }
    });
    // Event-driven readiness: wait until open0 actually started (no sleeps).
    while !factory
        .event_log()
        .iter()
        .any(|event| event == "open0:start")
    {
        tokio::task::yield_now().await;
    }
    let close_task = tokio::spawn({
        let manager = manager.clone();
        let path = path.clone();
        async move {
            manager
                .close_connection(&path)
                .await
                .expect("connection closes")
        }
    });
    // Drive open0 to completion; the close task must wait for it.
    factory.release_next();
    let _ = open_task.await.expect("open0 join").expect("open0");
    close_task.await.expect("close join");

    // Reopen must be a SECOND open, strictly after close finished.
    let handle = manager.get_connection(&path).await.expect("open1");
    drop(handle);

    let events = factory.event_log();
    let needle = |name: &str| {
        events
            .iter()
            .position(|event| event == name)
            .unwrap_or_else(|| panic!("missing event {name}: {events:?}"))
    };
    let sequence = [
        needle("open0:start"),
        needle("open0:done"),
        needle("open1:start"),
        needle("open1:done"),
    ];
    assert!(
        sequence.windows(2).all(|pair| pair[0] <= pair[1]),
        "open0 completes before open1 starts: {sequence:?}"
    );
    assert_eq!(factory.opens.load(std::sync::atomic::Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// 3/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_enables_foreign_keys_on_new_connections() {
    let config = common::store_config("cm-fk");
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        Arc::new(TursoConnectFactory) as Arc<dyn ConnectFactory>,
    );
    let path = config.storage_path.join("fk.db");
    let handle = manager.get_connection(&path).await.expect("open");
    let rows = handle
        .query("PRAGMA foreign_keys", Vec::new())
        .await
        .expect("pragma");
    assert_eq!(rows[0].i64("foreign_keys"), Some(1));
}

// ---------------------------------------------------------------------------
// 4/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_opens_databases_with_experimental_encryption_flags_but_no_encryption_when_disabled() {
    let config = common::store_config("cm-flags");
    let factory = Arc::new(common::FakeConnectFactory::default());
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        factory.clone() as Arc<dyn ConnectFactory>,
    );
    let path = config.storage_path.join("flags.db");
    let _ = manager.get_connection(&path).await.expect("open");

    let captured = factory.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    let opts = &captured[0];
    if cfg!(target_os = "windows") {
        assert_eq!(opts.experimental, vec![ExperimentalFeature::Encryption]);
    } else {
        assert_eq!(
            opts.experimental,
            vec![
                ExperimentalFeature::Encryption,
                ExperimentalFeature::MultiprocessWal,
            ],
            "unix always adds multiprocess_wal"
        );
    }
    assert!(opts.encryption.is_none(), "encryption disabled");
}

// ---------------------------------------------------------------------------
// 5/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_passes_encryption_options_when_configured_and_fails_clearly_when_key_unresolvable() {
    let mut config = common::store_config("cm-enc");
    config.database_encryption_enabled = true;
    config.database_encryption_key = Some("00112233445566778899aabbccddeeff".to_string());
    let factory = Arc::new(common::FakeConnectFactory::default());
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        factory.clone() as Arc<dyn ConnectFactory>,
    );
    let path = config.storage_path.join("encrypted.db");
    let _ = manager.get_connection(&path).await.expect("open");
    let captured = factory.captured.lock().unwrap().clone();
    let encryption = captured[0].encryption.clone().expect("encryption options");
    assert_eq!(encryption.cipher, "aes256gcm");
    assert_eq!(encryption.hexkey, "00112233445566778899aabbccddeeff");

    // Unresolvable key: connection refused BEFORE the factory runs.
    //
    // W1.5 supersession: upstream `resolveOrCreateDatabaseEncryptionKey`
    // auto-provisions the default key file when encryption is enabled without
    // an explicit key, so `enabled && key == None` is no longer an error (the
    // pre-W1.5 expectation here asserted `EncryptionKeyMissing`, which the
    // auto-provision path now makes unreachable). The reachable "key missing"
    // failure is an explicit key source that cannot be resolved — here an
    // `env://` reference to an unset variable.
    let mut missing = config.clone();
    missing.database_encryption_key = Some("env://OPENCODE_MEM_W15_UNSET_KEY_7f3a9c".to_string());
    let factory2 = Arc::new(common::FakeConnectFactory::default());
    let manager2 = ConnectionManager::new(
        missing.storage_path.clone(),
        missing.clone(),
        mem_store::platform::Platform::current(),
        factory2.clone() as Arc<dyn ConnectFactory>,
    );
    let error = manager2
        .get_connection(&missing.storage_path.join("x.db"))
        .await
        .err()
        .unwrap_or_else(|| panic!("expected error"));
    assert!(
        matches!(
            &error,
            StoreError::Internal { message }
                if message.contains("must resolve to a 32- or 64-character hex string")
        ),
        "unresolvable env:// key must fail clearly: {error}"
    );
    assert_eq!(factory2.opens.load(std::sync::atomic::Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// 6/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_refuses_paths_outside_storage_path() {
    let config = common::store_config("cm-outside");
    let factory = Arc::new(common::FakeConnectFactory::default());
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        factory.clone() as Arc<dyn ConnectFactory>,
    );
    let elsewhere = common::temp_dir("cm-elsewhere").join("outside.db");
    let error = manager
        .get_connection(&elsewhere)
        .await
        .err()
        .unwrap_or_else(|| panic!("expected error"));
    assert!(
        error.to_string().contains("outside storagePath"),
        "upstream message: {error}"
    );
    assert_eq!(factory.opens.load(std::sync::atomic::Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// 7/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_filename_containing_dotdot_is_allowed_but_segment_traversal_is_not() {
    let config = common::store_config("cm-dotdot");
    let factory = Arc::new(common::FakeConnectFactory::default());
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        factory.clone() as Arc<dyn ConnectFactory>,
    );

    // A filename MERELY containing ".." is fine (upstream comment).
    let ok_path = config.storage_path.join("..x.db");
    let handle = manager.get_connection(&ok_path).await.expect("allowed");
    drop(handle);
    assert_eq!(factory.opens.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Segment traversal (".." as a path component) is refused.
    let escape = config.storage_path.join("../escape.db");
    let error = manager
        .get_connection(&escape)
        .await
        .err()
        .unwrap_or_else(|| panic!("expected error"));
    assert!(matches!(error, StoreError::PathOutsideStorage { .. }));
    assert_eq!(factory.opens.load(std::sync::atomic::Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// Rework: mid-path `..` traversal (Node `path.relative` resolves first)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_refuses_deep_traversal_paths() {
    let config = common::store_config("cm-deep");
    let factory = Arc::new(common::FakeConnectFactory::default());
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        mem_store::platform::Platform::current(),
        factory.clone() as Arc<dyn ConnectFactory>,
    );

    // Mid-path `..` chains resolve OUTSIDE the storage root and must be
    // refused before the factory ever runs — even though the intermediate
    // directory does not exist (lexical resolution, no `canonicalize`).
    for relative in ["sub/../../outside.db", "a/../../../etc"] {
        let path = config.storage_path.join(relative);
        let error = manager
            .get_connection(&path)
            .await
            .err()
            .unwrap_or_else(|| panic!("expected error for {path:?}"));
        assert!(
            matches!(error, StoreError::PathOutsideStorage { .. }),
            "typed PathOutsideStorage for {path:?}: {error}"
        );
    }
    assert_eq!(
        factory.opens.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no open for a deep traversal path"
    );
}
