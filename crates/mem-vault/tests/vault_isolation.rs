//! W7.2 — one manager and one cell per scope (a consequence of ADR-0012
//! D-1 + D-4: one key per file means one manager per scope).
//!
//! A shared `ConnectionManager` carries one `StoreConfig`, hence one key. Used
//! for a second scope it does not refuse — it encrypts a *fresh* file with the
//! wrong scope's key, which is silent and unrecoverable. Only these two tests
//! can see it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions, EncryptionOptions, turso_connect_factory};
use mem_store::platform::Platform;
use mem_vault::VaultStore;

static COUNTER: AtomicU64 = AtomicU64::new(0);

const SCOPE_A: &str = "0123456789abcdef";
const SCOPE_B: &str = "fedcba9876543210";
const CIPHER: &str = "aes256gcm";

fn scratch_root(label: &str) -> PathBuf {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "mem-vault-{label}-{}-{counter}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the scratch root");
    dir
}

fn store_config(dir: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: dir.to_path_buf(),
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: true,
        database_encryption_cipher: CIPHER.to_string(),
        database_encryption_key: None,
    }
}

/// Whether `vault_entries` can be read under `hexkey`.
async fn table_is_readable(path: &Path, hexkey: Option<&str>) -> bool {
    let mut config = store_config(Path::new("."));
    config.database_encryption_enabled = false;
    let mut options = ConnectOptions::for_config(&config, Platform::current());
    options.encryption = hexkey.map(|key| EncryptionOptions {
        cipher: CIPHER.to_string(),
        hexkey: key.to_string(),
    });
    match turso_connect_factory()
        .connect(path.to_path_buf(), options)
        .await
    {
        Ok(db) => db
            .query("SELECT content_hash FROM vault_entries LIMIT 1", Vec::new())
            .await
            .is_ok(),
        Err(_) => false,
    }
}

fn key_text(path: &Path) -> String {
    std::fs::read_to_string(path)
        .expect("read the key file")
        .trim_end_matches('\n')
        .to_string()
}

fn entries_of(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list the directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_concurrent_opens_create_one_key_and_one_database() {
    let dir = scratch_root("isolation-concurrent");
    let store = VaultStore::new(store_config(&dir));

    let (first, second) = mem_store::tokio::join!(store.ready(SCOPE_A), store.ready(SCOPE_A));
    first.expect("the first concurrent open");
    second.expect("the second concurrent open");

    let key_path = store.key_path(SCOPE_A).expect("key path");
    let db_path = store.db_path(SCOPE_A).expect("db path");
    assert_eq!(
        entries_of(&dir.join("vault-keys")),
        vec![format!("{SCOPE_A}.key")],
        "two concurrent opens mint exactly one key"
    );
    assert!(db_path.exists(), "one database");
    let key_bytes = std::fs::read(&key_path).expect("read the key");

    store.ready(SCOPE_A).await.expect("a third open");
    assert_eq!(
        std::fs::read(&key_path).expect("read the key"),
        key_bytes,
        "a later open reuses the identical key"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_two_scopes_get_distinct_keys_and_each_db_opens_only_with_its_own_key() {
    let dir = scratch_root("isolation-two-scopes");
    let store = VaultStore::new(store_config(&dir));

    store.ready(SCOPE_A).await.expect("scope A");
    store.ready(SCOPE_B).await.expect("scope B");

    let key_a_path = store.key_path(SCOPE_A).expect("key path A");
    let key_b_path = store.key_path(SCOPE_B).expect("key path B");
    assert!(
        key_b_path.exists(),
        "every scope mints its own key file (a shared manager would mint only the first)"
    );
    let db_a_path = store.db_path(SCOPE_A).expect("db path A");
    let db_b_path = store.db_path(SCOPE_B).expect("db path B");
    let key_a = key_text(&key_a_path);
    let key_b = key_text(&key_b_path);
    assert_ne!(key_a, key_b, "two scopes, two keys");
    drop(store);

    assert!(
        table_is_readable(&db_a_path, Some(&key_a)).await,
        "A's database opens with A's key"
    );
    assert!(
        !table_is_readable(&db_a_path, Some(&key_b)).await,
        "A's database must NOT open with B's key: a shared manager would have \
         encrypted A's file with B's key and made this assertion pass"
    );
    assert!(
        table_is_readable(&db_b_path, Some(&key_b)).await,
        "B's database opens with B's key"
    );
    assert!(
        !table_is_readable(&db_b_path, Some(&key_a)).await,
        "B's database must NOT open with A's key"
    );
}
