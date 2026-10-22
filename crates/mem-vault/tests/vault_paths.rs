//! W7.2 — vault path math and the single-root constructor (ADR-0012 D-1/D-2).
//!
//! A vault keeps its keys **outside** its data tree, so deleting
//! `{storage_root}/vault` destroys ciphertext and leaves the keys (the
//! recoverable direction, ADR-0012 D-1). Every derived path is validated
//! (scope) and contained (storage root) before it is handed out.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mem_store::config::StoreConfig;
use mem_vault::{VaultError, VaultStore};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const SCOPE_A: &str = "0123456789abcdef";
const SCOPE_B: &str = "fedcba9876543210";

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
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

#[test]
fn test_vault_paths_refuse_an_invalid_scope_hash() {
    let store = VaultStore::new(store_config(&scratch_root("paths-invalid")));
    // Not 16 lowercase hex: traversal, empty, uppercase, too short, too long,
    // a non-hex letter, an embedded NUL, and a trailing newline.
    let invalid = [
        "../escape",
        "",
        "0123456789ABCDEF",
        "0123456789abcde",
        "0123456789abcdef0",
        "gggggggggggggggg",
        "0123456789abcde\0",
        "0123456789abcdef\n",
    ];

    let mut messages = Vec::new();
    for scope in invalid {
        let attempts: [Result<PathBuf, VaultError>; 3] = [
            store.key_path(scope),
            store.db_path(scope),
            store.marker_path(scope),
        ];
        for attempt in attempts {
            match attempt {
                Err(VaultError::InvalidScope) => {}
                other => panic!("scope {scope:?} must be refused, got {other:?}"),
            }
        }
        // One fixed message for every builder, and the rejected value is never
        // echoed (it may be arbitrarily long and is attacker-shaped).
        let message = store
            .key_path(scope)
            .expect_err("still refused")
            .to_string();
        messages.push(message);
    }
    for message in &messages[1..] {
        assert_eq!(message, &messages[0], "InvalidScope is one fixed message");
    }
    assert!(
        !messages[0].contains("../escape"),
        "the rejected scope is never echoed"
    );
}

#[test]
fn test_vault_paths_stay_under_the_storage_root() {
    let dir = scratch_root("paths-contained");
    let store = VaultStore::new(store_config(&dir));

    let key_a = store.key_path(SCOPE_A).expect("key path A");
    let db_a = store.db_path(SCOPE_A).expect("db path A");
    let marker_a = store.marker_path(SCOPE_A).expect("marker path A");
    let key_b = store.key_path(SCOPE_B).expect("key path B");
    let db_b = store.db_path(SCOPE_B).expect("db path B");

    for path in [&key_a, &db_a, &marker_a, &key_b, &db_b] {
        assert!(
            path.starts_with(&dir),
            "{} must stay under {}",
            path.display(),
            dir.display()
        );
        assert!(
            !path.to_string_lossy().contains(".."),
            "{} must not contain a parent component",
            path.display()
        );
    }

    assert!(
        key_a.starts_with(dir.join("vault-keys")),
        "keys live outside vault/"
    );
    assert!(db_a.starts_with(dir.join("vault").join(SCOPE_A)));
    assert_eq!(
        marker_a.parent(),
        db_a.parent(),
        "the marker describes the directory whose file it protects"
    );
    assert!(db_a.ends_with("vault.db"));
    assert!(marker_a.ends_with(".vault-encrypted-v1"));
    assert_ne!(key_a, key_b, "two scopes, two keys");
    assert_ne!(db_a, db_b, "two scopes, two databases");
}

#[test]
fn test_vault_store_takes_one_config_and_reports_its_root() {
    let dir = scratch_root("paths-one-config");
    let config = store_config(&dir);
    let expected_root = config.storage_path.clone();
    let store = VaultStore::new(config);

    assert_eq!(store.storage_path(), expected_root.as_path());
    let derived = [
        store.key_path(SCOPE_A).expect("key"),
        store.db_path(SCOPE_A).expect("db"),
        store.marker_path(SCOPE_A).expect("marker"),
    ];
    for path in derived {
        assert!(
            path.starts_with(store.storage_path()),
            "{} descends from the one root {}",
            path.display(),
            store.storage_path().display()
        );
    }
}
