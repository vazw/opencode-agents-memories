//! Restore-on-verify-failure regression for the encryption migrator
//! (`dump_and_reload_encrypted`) — W1.5 r1 review fix.
//!
//! This lives in its OWN test binary on purpose: the fault-injection seam
//! [`mem_store::encryption_migrator::set_force_verify_fail`] flips a
//! process-global flag, and integration-test binaries run their tests in
//! parallel threads. Keeping the only flag-using test alone in this binary
//! guarantees it can never race a sibling test that also drives
//! `dump_and_reload_encrypted` (e.g. `turso_encryption_migrator.rs`'s
//! marker-writing test).
//!
//! Regression (Mandate 7 — error handling): when the post-swap encrypted
//! verification fails, the migrator must remove the unverifiable encrypted
//! file and restore the `.pre-encrypt-*.bak` backup to the active path
//! BEFORE rethrowing, so the original plaintext shard is never stranded.

mod common;

use std::path::Path;

use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions};
use mem_store::encryption_migrator::{dump_and_reload_encrypted, set_force_verify_fail};

/// Upstream `buildConnectOptions()`: experimental features on, no key
/// material (a keyless open of a plaintext shard succeeds).
fn plain_options(config: &StoreConfig) -> ConnectOptions {
    ConnectOptions::for_config(config, mem_store::platform::Platform::current())
}

/// A minimal plaintext shard with one generic table carrying one row.
async fn create_plaintext_shard(config: &StoreConfig, path: &Path) {
    let db = mem_store::db::turso_connect_factory()
        .connect(path.to_path_buf(), plain_options(config))
        .await
        .expect("plain open");
    db.execute("CREATE TABLE t (x INTEGER)", Vec::new())
        .await
        .expect("ddl");
    db.execute("INSERT INTO t (x) VALUES (7)", Vec::new())
        .await
        .expect("insert");
    drop(db);
}

/// Names in `storage` containing `needle` (swap-artifact detector).
fn names_with(storage: &Path, needle: &str) -> Vec<String> {
    std::fs::read_dir(storage)
        .expect("storage read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains(needle))
        .collect()
}

#[tokio::test]
async fn test_verify_failure_restores_the_plaintext_backup() {
    let key_hex = common::hex_key_64();
    let config = common::encrypted_store_config("enc-restore", &key_hex);
    let (config, connections, _shards) = common::real_components(config).await;
    // The verify-failure restore renames under the root's proof.
    let _proof = common::exclusive_root(&config.storage_path);
    let storage = config.storage_path.clone();

    let db_path = storage.join("data.db");
    create_plaintext_shard(&config, &db_path).await;

    // Force the encrypted self-check to fail after the swap.
    set_force_verify_fail(true);
    let outcome = dump_and_reload_encrypted(&storage, &db_path, &connections, &config).await;
    set_force_verify_fail(false);

    assert!(
        outcome.is_err(),
        "a failed verification must propagate an error"
    );

    // The active shard is restored to its original plaintext bytes and still
    // opens — a verification failure never strands the shard.
    let reopened = mem_store::db::turso_connect_factory()
        .connect(db_path.clone(), plain_options(&config))
        .await
        .expect("restored plaintext shard reopens");
    let rows = reopened
        .query("SELECT x FROM t", Vec::new())
        .await
        .expect("select");
    assert_eq!(
        rows.first().and_then(|row| row.i64("x")),
        Some(7),
        "original row survives the restore"
    );
    drop(reopened);

    // Neither the staged encrypted file nor the backup is left behind: the
    // encrypted side was removed and the backup was renamed back to active.
    assert!(
        names_with(&storage, ".pre-encrypt-").is_empty(),
        "no `.pre-encrypt-*.bak` left behind (backup restored to active)"
    );
    assert!(
        names_with(&storage, ".encrypt-").is_empty(),
        "no `.encrypt-*.tmp` left behind"
    );

    common::cleanup_temp_dir(&storage).ok();
}
