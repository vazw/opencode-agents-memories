//! W7.2 — the vault's failure and crash matrix (ADR-0012 D-4, D-5, D-10).
//!
//! Every row here asserts the same shape: a refusal leaves the disk exactly as
//! it found it — no regeneration, no repair, no deletion.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mem_store::config::StoreConfig;
use mem_vault::{VaultError, VaultStore};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const SCOPE: &str = "0123456789abcdef";
const CIPHER: &str = "aes256gcm";
/// A valid 64-lowercase-hex key.
const VALID_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
        allow_unguarded_open: false,
    }
}

// ---------------------------------------------------------------------------
// c2 — an invalid key file is refused, never regenerated, never opened (D-4/D-10)
// ---------------------------------------------------------------------------

/// The RED that drives `KeyHex::parse`: written before the validation exists.
///
/// It cannot reach `mem-store`'s default-key branch in any state: before the
/// implementation the resolver is never called, and after it an empty or
/// malformed key is refused by `KeyHex::parse` **before** `build_manager` runs.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_rejects_an_invalid_key_file_without_regenerating_or_opening() {
    let uppercase = VALID_KEY.to_uppercase();
    let fixtures: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("63-hex", VALID_KEY[..63].to_string()),
        ("65-hex", format!("{VALID_KEY}0")),
        ("uppercase", uppercase.clone()),
        // 32 hex is a valid AES-128 key for `mem-store`'s resolver and invalid
        // here: the vault's rule is deliberately stricter (D-4).
        ("aes128", VALID_KEY[..32].to_string()),
        ("junk-suffix", format!("{VALID_KEY}\nJUNK")),
    ];

    for (label, content) in fixtures {
        let dir = scratch_root(&format!("custody-invalid-{label}"));
        let store = VaultStore::new(store_config(&dir));
        let key_path = store.key_path(SCOPE).expect("key path");
        std::fs::create_dir_all(key_path.parent().expect("keys dir")).expect("keys dir");
        std::fs::write(&key_path, content.as_bytes()).expect("write the key file");
        let bytes_before = std::fs::read(&key_path).expect("read the key file");

        let error = store
            .ready(SCOPE)
            .await
            .err()
            .unwrap_or_else(|| panic!("{label}: must refuse"));
        match &error {
            VaultError::InvalidKey { path } => {
                assert_eq!(path, &key_path, "{label}: the error names the key file")
            }
            other => panic!("{label}: expected InvalidKey, got {other:?}"),
        }

        let display = error.to_string();
        assert!(
            display.contains(&key_path.display().to_string()),
            "{label}: the error names the file"
        );
        if !content.is_empty() {
            // Substring semantics: the message legitimately contains the path,
            // and the path carries the 16-char scope hash, so per-character
            // disjointness between message and a hex fixture is unassertable.
            assert!(
                !display.contains(&content),
                "{label}: the error never echoes the file's content"
            );
        }
        // The empty fixture carries no `!contains` assertion above: `!s.contains("")`
        // is false for every string, so the assertion would be vacuous.
        if label == "junk-suffix" {
            assert!(
                !display.contains("JUNK"),
                "a canary that cannot occur in the fixed message"
            );
        }
        if label == "uppercase" {
            assert!(
                !display.contains(&uppercase),
                "the fixed message is lowercase-only"
            );
        }

        assert_eq!(
            std::fs::read(&key_path).expect("read back"),
            bytes_before,
            "{label}: the key file is never rewritten or deleted"
        );
        assert!(
            !store.db_path(SCOPE).expect("db path").exists(),
            "{label}: the database is never opened"
        );
    }
}

// ---------------------------------------------------------------------------
// Shared helper: is the table readable under this key?
// ---------------------------------------------------------------------------

/// A minimal config for [`ConnectOptions::for_config`] only.
fn options_config() -> StoreConfig {
    let mut config = store_config(Path::new("."));
    config.database_encryption_enabled = false;
    config
}

/// Whether `vault_entries` can be read under `hexkey`. Open failure **and**
/// query failure both mean "unreadable" (turso may defer a codec error to the
/// first query); only a successful read is a failure of the encryption claim.
async fn table_is_readable(
    path: &Path,
    hexkey: Option<&str>,
) -> Result<bool, mem_store::error::StoreError> {
    use mem_store::db::{ConnectFactory, ConnectOptions, EncryptionOptions, turso_connect_factory};
    use mem_store::platform::Platform;

    let config = options_config();
    let mut options = ConnectOptions::for_config(&config, Platform::current());
    // `options_config()`'s root is the placeholder `"."`: this helper inspects a
    // database in the vault's own root and owns no storage root, so it must not
    // write a `.wal-mode` marker into the package directory.
    options.storage_root = None;
    options.encryption = hexkey.map(|key| EncryptionOptions {
        cipher: CIPHER.to_string(),
        hexkey: key.to_string(),
    });
    match turso_connect_factory()
        .connect(path.to_path_buf(), options)
        .await
    {
        // A guard refusal is a diagnosis, not "unreadable": propagate it, or an
        // `assert!(!table_is_readable(..))` passes vacuously on a refused open.
        Err(error) if error.is_open_guard_refusal() => Err(error),
        Err(_) => Ok(false),
        Ok(db) => Ok(db
            .query("SELECT content_hash FROM vault_entries LIMIT 1", Vec::new())
            .await
            .is_ok()),
    }
}

fn read_or_empty(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// c1 — never mint over existing encrypted data (D-10)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_refuses_to_generate_a_key_over_existing_encrypted_data() {
    // Phase (a): a database exists and the key file is gone.
    let dir = scratch_root("custody-missing-key");
    let store = VaultStore::new(store_config(&dir));
    store.ready(SCOPE).await.expect("ready");
    let key_path = store.key_path(SCOPE).expect("key path");
    let db_path = store.db_path(SCOPE).expect("db path");
    assert!(db_path.exists(), "the fixture really has ciphertext");
    drop(store);
    std::fs::remove_file(&key_path).expect("delete the key file");

    let fresh = VaultStore::new(store_config(&dir));
    let error = fresh.ready(SCOPE).await.err().expect("must refuse");
    match &error {
        VaultError::KeyMissingForExistingData { path } => {
            assert_eq!(path, &key_path, "the error names the missing key file")
        }
        other => panic!("expected KeyMissingForExistingData, got {other:?}"),
    }
    assert!(
        !key_path.exists(),
        "a lost key is never regenerated over live ciphertext"
    );

    // Phase (b): only the marker exists, no database yet.
    let dir = scratch_root("custody-missing-key-marker-only");
    let store = VaultStore::new(store_config(&dir));
    let key_path = store.key_path(SCOPE).expect("key path");
    let marker_path = store.marker_path(SCOPE).expect("marker path");
    std::fs::create_dir_all(marker_path.parent().expect("scope dir")).expect("scope dir");
    std::fs::write(&marker_path, format!("cipher={CIPHER}\n")).expect("write the marker");

    let fresh = VaultStore::new(store_config(&dir));
    let error = fresh.ready(SCOPE).await.err().expect("must refuse");
    assert!(
        matches!(&error, VaultError::KeyMissingForExistingData { .. }),
        "a marker alone means the scope was encrypted: {error:?}"
    );
    assert!(!key_path.exists(), "still no regeneration");
    assert!(
        !store.db_path(SCOPE).expect("db path").exists(),
        "and no database is created"
    );
}

// ---------------------------------------------------------------------------
// c3 / c4 — the marker is a named error, before the open (D-5)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_refuses_a_malformed_marker() {
    let dir = scratch_root("custody-marker-malformed");
    VaultStore::new(store_config(&dir))
        .ready(SCOPE)
        .await
        .expect("first open");
    let store = VaultStore::new(store_config(&dir));
    let marker_path = store.marker_path(SCOPE).expect("marker path");
    let db_path = store.db_path(SCOPE).expect("db path");
    let db_bytes = read_or_empty(&db_path);
    assert!(!db_bytes.is_empty());

    for content in ["garbage", "not-a-cipher=x\n", ""] {
        std::fs::write(&marker_path, content).expect("overwrite the marker");
        let fresh = VaultStore::new(store_config(&dir));
        let error = fresh.ready(SCOPE).await.err().expect("must refuse");
        match &error {
            VaultError::MarkerInvalid { path } => {
                assert_eq!(path, &marker_path, "the error names the marker")
            }
            other => panic!("marker {content:?}: expected MarkerInvalid, got {other:?}"),
        }
        assert_eq!(
            read_or_empty(&db_path),
            db_bytes,
            "marker {content:?}: the database is untouched"
        );
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_refuses_a_cipher_change_before_opening() {
    let dir = scratch_root("custody-cipher-change");
    let created = store_config(&dir);
    VaultStore::new(created.clone())
        .ready(SCOPE)
        .await
        .expect("first open");
    let db_path = VaultStore::new(created.clone())
        .db_path(SCOPE)
        .expect("db path");
    let db_bytes = read_or_empty(&db_path);

    let mut reconfigured = created;
    reconfigured.database_encryption_cipher = "aegis256".to_string();
    let fresh = VaultStore::new(reconfigured);
    let error = fresh.ready(SCOPE).await.err().expect("must refuse");
    match &error {
        VaultError::CipherMismatch {
            recorded,
            configured,
        } => {
            // The vault's own named error, not an opaque codec failure.
            assert_eq!(recorded, CIPHER);
            assert_eq!(configured, "aegis256");
        }
        other => panic!("expected CipherMismatch, got {other:?}"),
    }
    assert!(
        error.to_string().contains(CIPHER),
        "the operator is told which cipher the vault was written with"
    );
    assert_eq!(
        read_or_empty(&db_path),
        db_bytes,
        "the mismatch is detected before the file is opened"
    );
}

// ---------------------------------------------------------------------------
// c5 — a wrong (but valid) key fails closed and destroys nothing (D-10)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_wrong_key_fails_closed_and_leaves_the_file_untouched() {
    let dir = scratch_root("custody-wrong-key");
    let config = store_config(&dir);
    let store = VaultStore::new(config.clone());
    store.ready(SCOPE).await.expect("first open");
    let key_path = store.key_path(SCOPE).expect("key path");
    let db_path = store.db_path(SCOPE).expect("db path");
    let db_bytes = read_or_empty(&db_path);
    let original_bytes = std::fs::read(&key_path).expect("read the key file");
    let original_key = String::from_utf8(original_bytes.clone()).expect("the key file is text");
    drop(store);

    // A different, equally valid 64-lowercase-hex key.
    let other_key = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    assert_ne!(original_key.trim_end(), other_key);
    std::fs::write(&key_path, format!("{other_key}\n")).expect("replace the key file");
    let replaced_bytes = std::fs::read(&key_path).expect("read the replaced key file");

    let fresh = VaultStore::new(config);
    let error = fresh.ready(SCOPE).await.err().expect("must fail closed");
    let message = error.to_string();
    assert!(
        !message.contains(original_key.trim_end()),
        "the error never carries the real key"
    );
    assert!(
        !message.contains(other_key),
        "the error never carries the wrong key either"
    );

    assert_eq!(
        read_or_empty(&db_path),
        db_bytes,
        "ciphertext is untouched by a failed open"
    );
    assert_eq!(
        std::fs::read(&key_path).expect("read back"),
        replaced_bytes,
        "the key file is untouched by a failed open"
    );
    assert!(!db_bytes.is_empty(), "the fixture really has ciphertext");
    assert!(
        !table_is_readable(&db_path, Some(other_key))
            .await
            .expect("the probe answers (no guard refusal)"),
        "the wrong key cannot read the vault table"
    );
}

// ---------------------------------------------------------------------------
// c6 / c7 — an interrupted initialization converges (D-10)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_recovers_from_a_key_without_marker_or_database() {
    let dir = scratch_root("custody-key-only");
    let config = store_config(&dir);
    let store = VaultStore::new(config.clone());
    store.ready(SCOPE).await.expect("first open");
    let key_path = store.key_path(SCOPE).expect("key path");
    let db_path = store.db_path(SCOPE).expect("db path");
    let marker_path = store.marker_path(SCOPE).expect("marker path");
    let key_bytes = std::fs::read(&key_path).expect("read the key");
    drop(store);

    for name in ["vault.db", "vault.db-wal", "vault.db-shm"] {
        let _ = std::fs::remove_file(dir.join("vault").join(SCOPE).join(name));
    }
    std::fs::remove_file(&marker_path).expect("delete the marker");
    assert!(!db_path.exists());

    let fresh = VaultStore::new(config);
    fresh
        .ready(SCOPE)
        .await
        .expect("a key with no marker or database re-opens");
    assert_eq!(
        std::fs::read(&key_path).expect("read the key"),
        key_bytes,
        "the identical key is reused"
    );
    assert!(marker_path.exists(), "the marker is recreated");
    assert!(db_path.exists(), "the database is recreated");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_vault_scope_with_marker_but_no_database_opens() {
    let dir = scratch_root("custody-marker-no-db");
    let config = store_config(&dir);
    let store = VaultStore::new(config.clone());
    store.ready(SCOPE).await.expect("first open");
    let key_path = store.key_path(SCOPE).expect("key path");
    let db_path = store.db_path(SCOPE).expect("db path");
    let key_bytes = std::fs::read(&key_path).expect("read the key");
    drop(store);

    for name in ["vault.db", "vault.db-wal", "vault.db-shm"] {
        let _ = std::fs::remove_file(dir.join("vault").join(SCOPE).join(name));
    }
    assert!(!db_path.exists());

    let fresh = VaultStore::new(config);
    fresh
        .ready(SCOPE)
        .await
        .expect("key + marker with no database opens");
    assert_eq!(
        fresh.schema_version(SCOPE).await.expect("version"),
        1,
        "the recreated file is v1"
    );
    assert_eq!(
        std::fs::read(&key_path).expect("read the key"),
        key_bytes,
        "the identical key is reused"
    );
}
