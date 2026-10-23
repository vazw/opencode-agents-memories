//! W7.2 + W8a — the fail-closed open, key custody, the v1 schema and
//! encryption at rest (ADR-0012 D-3, D-4, D-5, D-10), plus the W8a content
//! write/read path (`put`/`get`, validation, idempotency, cached key).
//!
//! These tests prove the *contract* — the open order, the key's shape, the
//! schema, that the file on disk is not readable without its key, and the
//! content-addressed put/get behaviour.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions, EncryptionOptions, turso_connect_factory};
use mem_store::platform::Platform;
use mem_vault::{VaultEntry, VaultError, VaultStore};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const SCOPE: &str = "0123456789abcdef";
const CIPHER: &str = "aes256gcm";
/// A valid 64-lowercase-hex key that a *caller* might configure. The vault
/// must ignore it and mint from its own material.
const CALLER_KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

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

fn store_config(dir: &Path, enabled: bool, key: Option<&str>) -> StoreConfig {
    StoreConfig {
        storage_path: dir.to_path_buf(),
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: enabled,
        database_encryption_cipher: CIPHER.to_string(),
        database_encryption_key: key.map(str::to_string),
    }
}

/// A minimal config for [`ConnectOptions::for_config`] only (it reads the
/// cipher field, which the options builder ignores).
fn options_config() -> StoreConfig {
    store_config(Path::new("."), false, None)
}

/// Whether `vault_entries` can be read from `path` under `hexkey` (`None` =
/// a plaintext open). Open failure **and** query failure both mean
/// "unreadable": turso may defer a codec error to the first query, and only a
/// successful read is a failure of the encryption claim.
async fn table_is_readable(path: &Path, cipher: &str, hexkey: Option<&str>) -> bool {
    let config = options_config();
    let mut options = ConnectOptions::for_config(&config, Platform::current());
    options.encryption = hexkey.map(|key| EncryptionOptions {
        cipher: cipher.to_string(),
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

fn entries_of(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list the directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// The fail-closed open order (D-3)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_store_refuses_open_when_encryption_is_disabled() {
    // The second fixture is the load-bearing one: `mem-store`'s resolver
    // returns an explicit hex key EVEN WHEN encryption is disabled, and
    // `resolve_database_encryption` turns any resolvable key into encryption
    // options regardless of the flag — so a vault that trusted the resolver
    // would create an encrypted file under a plaintext-off configuration.
    for (label, key) in [("no-key", None), ("poisoned-key", Some(CALLER_KEY))] {
        let dir = scratch_root(&format!("disabled-{label}"));
        let store = VaultStore::new(store_config(&dir, false, key));
        let error = store.ready(SCOPE).await.err().expect("must refuse");
        assert!(
            matches!(&error, VaultError::EncryptionRequired),
            "{label}: expected EncryptionRequired, got {error:?}"
        );
        assert!(
            !dir.join("vault-keys").exists(),
            "{label}: a refused open leaves no key directory"
        );
        assert!(
            !dir.join("vault").exists(),
            "{label}: a refused open leaves no data directory"
        );
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_store_still_refuses_a_second_open_after_a_refusal() {
    let dir = scratch_root("disabled-second-open");
    let store = VaultStore::new(store_config(&dir, false, Some(CALLER_KEY)));
    for _ in 0..2 {
        let error = store.ready(SCOPE).await.err().expect("must refuse");
        assert!(
            matches!(&error, VaultError::EncryptionRequired),
            "{error:?}"
        );
        assert!(!dir.join("vault-keys").exists());
        assert!(!dir.join("vault").exists());
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_ready_rejects_an_invalid_scope_before_touching_disk() {
    let dir = scratch_root("invalid-scope-ready");
    let store = VaultStore::new(store_config(&dir, true, None));
    let before = entries_of(&dir);

    let error = store.ready("../escape").await.err().expect("must refuse");
    assert!(matches!(&error, VaultError::InvalidScope), "{error:?}");
    assert!(!error.to_string().contains("../escape"), "no echo");

    let after = entries_of(&dir);
    assert_eq!(before, after, "a rejected scope touches no disk");
}

// ---------------------------------------------------------------------------
// Key custody (D-4)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_mint_creates_one_key_file_of_64_lowercase_hex() {
    let dir = scratch_root("mint");
    // An explicitly EMPTY caller key: the vault ignores it, mints from its own
    // material, and `build_manager` overwrites the caller's value.
    let store = VaultStore::new(store_config(&dir, true, Some("")));
    store.ready(SCOPE).await.expect("ready");

    let keys_dir = dir.join("vault-keys");
    let entries = entries_of(&keys_dir);
    assert_eq!(
        entries,
        vec![format!("{SCOPE}.key")],
        "exactly one key file"
    );

    let key_path = store.key_path(SCOPE).expect("key path");
    let bytes = std::fs::read(&key_path).expect("read the key file");
    assert_eq!(bytes.len(), 65, "64 hex characters plus one newline");
    assert_eq!(*bytes.last().expect("non-empty"), b'\n');
    assert!(
        bytes[..64]
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "lowercase hex only"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file_mode = std::fs::metadata(&key_path)
            .expect("stat key")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        for dir_path in [keys_dir, dir.join("vault").join(SCOPE)] {
            let dir_mode = std::fs::metadata(&dir_path)
                .expect("stat dir")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "{} is owner-only", dir_path.display());
        }
    }

    // The other half of branch 4: a *valid* caller key is ignored too. If the
    // manager honoured it, the file would be readable with the caller's key —
    // and it must not be.
    let caller_dir = scratch_root("mint-caller-key");
    let caller_store = VaultStore::new(store_config(&caller_dir, true, Some(CALLER_KEY)));
    let handle = caller_store.ready(SCOPE).await.expect("ready");
    let caller_db_path = caller_store.db_path(SCOPE).expect("db path");
    drop(handle);
    drop(caller_store);
    assert!(
        !table_is_readable(&caller_db_path, CIPHER, Some(CALLER_KEY)).await,
        "the vault never uses the caller's database_encryption_key"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_second_open_reuses_the_same_key_bytes() {
    let dir = scratch_root("reuse");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("first open");
    let key_path = store.key_path(SCOPE).expect("key path");
    let first = std::fs::read(&key_path).expect("read");

    store.ready(SCOPE).await.expect("second open, same store");
    assert_eq!(std::fs::read(&key_path).expect("read"), first);

    let fresh = VaultStore::new(store_config(&dir, true, None));
    fresh
        .ready(SCOPE)
        .await
        .expect("open through a fresh store");
    assert_eq!(
        std::fs::read(&key_path).expect("read"),
        first,
        "the key is never regenerated"
    );
    assert_eq!(entries_of(&dir.join("vault-keys")).len(), 1);
}

// ---------------------------------------------------------------------------
// The v1 schema (AC-10) and encryption at rest (AC-11)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_store_creates_v1_schema_when_encrypted() {
    let dir = scratch_root("schema-v1");
    let store = VaultStore::new(store_config(&dir, true, None));
    let db = store.ready(SCOPE).await.expect("ready");
    assert_eq!(store.schema_version(SCOPE).await.expect("version"), 1);

    let rows = db
        .query("PRAGMA table_info(vault_entries)", Vec::new())
        .await
        .expect("table_info");
    let columns: Vec<String> = rows.iter().filter_map(|row| row.text("name")).collect();
    assert_eq!(
        columns,
        vec![
            "content_hash",
            "content",
            "mime_type",
            "source_ref",
            "created_at",
            "created_by"
        ],
        "the six v1 columns, in order"
    );

    let indexes = db
        .query("PRAGMA index_list(vault_entries)", Vec::new())
        .await
        .expect("index_list");
    let index_names: Vec<String> = indexes.iter().filter_map(|row| row.text("name")).collect();
    assert!(
        index_names
            .iter()
            .any(|name| name == "idx_vault_entries_source_ref"),
        "the source_ref erase join is indexed: {index_names:?}"
    );

    // A fresh file yields from_version == 0, so no backup is created.
    let scope_dir = dir.join("vault").join(SCOPE);
    assert!(
        !entries_of(&scope_dir)
            .iter()
            .any(|name| name.contains(".bak")),
        "a fresh v0 -> v1 migration creates no backup file"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_reopen_reports_schema_version_1() {
    let dir = scratch_root("schema-reopen");
    VaultStore::new(store_config(&dir, true, None))
        .ready(SCOPE)
        .await
        .expect("first open");
    let scope_dir = dir.join("vault").join(SCOPE);
    let after_first = entries_of(&scope_dir);

    let fresh = VaultStore::new(store_config(&dir, true, None));
    let version = fresh.schema_version(SCOPE).await.expect("version");
    assert_eq!(version, 1, "the file stays at v1");
    fresh.ready(SCOPE).await.expect("second open");
    assert_eq!(
        entries_of(&scope_dir),
        after_first,
        "a reopen creates no second backup and no new file"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_schema_has_no_key_material_column() {
    let dir = scratch_root("schema-columns");
    let store = VaultStore::new(store_config(&dir, true, None));
    let db = store.ready(SCOPE).await.expect("ready");
    let rows = db
        .query("PRAGMA table_info(vault_entries)", Vec::new())
        .await
        .expect("table_info");
    let columns: Vec<String> = rows.iter().filter_map(|row| row.text("name")).collect();

    assert_eq!(
        columns.len(),
        6,
        "exactly six columns: a seventh cannot be added unnoticed"
    );
    for column in &columns {
        assert!(
            !column.to_ascii_lowercase().contains("key"),
            "no column may invite key material into a row: {column}"
        );
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_db_is_not_readable_as_a_plaintext_database() {
    let dir = scratch_root("encrypted-at-rest");
    let config = store_config(&dir, true, None);
    let store = VaultStore::new(config.clone());
    let handle = store.ready(SCOPE).await.expect("ready");
    let db_path = store.db_path(SCOPE).expect("db path");
    drop(handle);
    drop(store);

    let length = std::fs::metadata(&db_path).expect("stat vault.db").len();
    assert!(length > 0, "the encrypted file is non-empty on disk");
    assert!(
        !table_is_readable(&db_path, &config.database_encryption_cipher, None).await,
        "a plaintext open must not read the vault table — a success here is a \
         finding about the encryption claim, not a test to relax"
    );
}

// ---------------------------------------------------------------------------
// W8a content write path (AC-2..AC-6, AC-16)
// ---------------------------------------------------------------------------

#[test]
fn test_vault_error_invalid_entry_display() {
    let err = VaultError::InvalidEntry {
        field: "created_by",
    };
    assert_eq!(err.to_string(), "invalid vault entry field: created_by");
    let err2 = VaultError::InvalidEntry {
        field: "source_ref",
    };
    assert_eq!(err2.to_string(), "invalid vault entry field: source_ref");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_put_returns_keyed_content_hash() {
    let dir = scratch_root("put-hash");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    let entry = VaultEntry {
        content: b"hello vault".to_vec(),
        mime_type: Some("text/plain".to_string()),
        source_ref: "session:abc".to_string(),
        created_by: "agent".to_string(),
    };
    let hash = store.put(SCOPE, entry.clone()).await.expect("put");
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    let key_bytes = std::fs::read_to_string(store.key_path(SCOPE).expect("kp")).expect("read");
    let expected = mem_vault::hmac::keyed_content_hash(&key_bytes, &entry.content).expect("hash");
    assert_eq!(hash, expected);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_put_is_idempotent() {
    let dir = scratch_root("put-idempotent");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    let entry = VaultEntry {
        content: b"idempotent".to_vec(),
        mime_type: Some("text/plain".to_string()),
        source_ref: "session:x".to_string(),
        created_by: "agent".to_string(),
    };
    let h1 = store.put(SCOPE, entry.clone()).await.expect("put1");
    let entry2 = VaultEntry {
        content: b"idempotent".to_vec(),
        mime_type: Some("application/json".to_string()),
        source_ref: "session:y".to_string(),
        created_by: "other".to_string(),
    };
    let h2 = store.put(SCOPE, entry2).await.expect("put2");
    assert_eq!(h1, h2);
    let rec = store.get(SCOPE, &h1).await.expect("get").expect("some");
    assert_eq!(rec.mime_type.as_deref(), Some("text/plain"));
    assert_eq!(rec.source_ref, "session:x");
    assert_eq!(rec.created_by, "agent");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_put_validates_created_by() {
    let dir = scratch_root("put-cb");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    for cb in ["", "   "] {
        let entry = VaultEntry {
            content: b"x".to_vec(),
            mime_type: None,
            source_ref: "".to_string(),
            created_by: cb.to_string(),
        };
        let err = store.put(SCOPE, entry).await.unwrap_err();
        assert!(
            matches!(
                err,
                VaultError::InvalidEntry {
                    field: "created_by"
                }
            ),
            "{err:?}"
        );
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_put_validates_source_ref() {
    let dir = scratch_root("put-sr");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    let cases: Vec<String> = vec![
        String::new(),
        "   ".to_string(),
        "a\0b".to_string(),
        "a".repeat(257),
    ];
    for sr in cases {
        let entry = VaultEntry {
            content: b"x".to_vec(),
            mime_type: None,
            source_ref: sr,
            created_by: "a".to_string(),
        };
        let err = store.put(SCOPE, entry).await.unwrap_err();
        assert!(
            matches!(
                err,
                VaultError::InvalidEntry {
                    field: "source_ref"
                }
            ),
            "{err:?}"
        );
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_get_returns_stored_record() {
    let dir = scratch_root("get-record");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    let entry = VaultEntry {
        content: b"round trip".to_vec(),
        mime_type: Some("text/plain".to_string()),
        source_ref: "session:abc".to_string(),
        created_by: "agent".to_string(),
    };
    let hash = store.put(SCOPE, entry.clone()).await.expect("put");
    let rec = store.get(SCOPE, &hash).await.expect("get").expect("some");
    assert_eq!(rec.content_hash, hash);
    assert_eq!(rec.content, entry.content);
    assert_eq!(rec.mime_type, Some("text/plain".to_string()));
    assert_eq!(rec.source_ref, "session:abc");
    assert_eq!(rec.created_by, "agent");
    assert!(rec.created_at > 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_get_returns_none_for_missing() {
    let dir = scratch_root("get-missing");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    let missing = "0".repeat(64);
    assert!(store.get(SCOPE, &missing).await.expect("get").is_none());
    let entry = VaultEntry {
        content: Vec::new(),
        mime_type: None,
        source_ref: "manual:test".to_string(),
        created_by: "agent".to_string(),
    };
    let hash = store.put(SCOPE, entry).await.expect("put");
    let rec = store.get(SCOPE, &hash).await.expect("get").expect("some");
    assert!(rec.content.is_empty());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_cached_key_populated_after_ready() {
    let dir = scratch_root("cached-key");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    let entry = VaultEntry {
        content: b"cached".to_vec(),
        mime_type: None,
        source_ref: "manual:test".to_string(),
        created_by: "agent".to_string(),
    };
    assert!(store.put(SCOPE, entry).await.is_ok());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_put_stamps_created_at() {
    let dir = scratch_root("stamps");
    let store = VaultStore::new(store_config(&dir, true, None));
    store.ready(SCOPE).await.expect("ready");
    let entry = VaultEntry {
        content: b"stamp".to_vec(),
        mime_type: None,
        source_ref: "manual:test".to_string(),
        created_by: "agent".to_string(),
    };
    let hash = store.put(SCOPE, entry).await.expect("put");
    let rec = store.get(SCOPE, &hash).await.expect("get").expect("some");
    assert!(rec.created_at > 0);
}
