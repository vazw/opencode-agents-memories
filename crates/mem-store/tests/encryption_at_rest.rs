//! Encryption at rest — port of upstream `tests/encryption-at-rest.test.ts`
//! (blueprint W1.5 step 3, lane sE).
//!
//! Scaffolded empty by the captain (step 2); the lane fills it RED-first.

mod common;

use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions, EncryptionOptions};
use mem_store::encryption_key::{
    ensure_database_encryption_key_file, generate_database_encryption_key_file,
    is_valid_database_encryption_hex_key, resolve_or_create_database_encryption_key,
};

/// Upstream `buildConnectOptions(encryption)`: the always-on experimental
/// features plus the pinned aes256gcm key.
fn encrypted_options(config: &StoreConfig, hexkey: &str) -> ConnectOptions {
    let mut options = ConnectOptions::for_config(config, mem_store::platform::Platform::current());
    options.encryption = Some(EncryptionOptions {
        cipher: "aes256gcm".to_string(),
        hexkey: hexkey.to_string(),
    });
    options
}

/// Upstream `buildConnectOptions()`: experimental features on, no key
/// material (a keyless open of an encrypted shard must fail).
fn plain_options(config: &StoreConfig) -> ConnectOptions {
    ConnectOptions::for_config(config, mem_store::platform::Platform::current())
}

// ---------------------------------------------------------------------------
// 1/4 — encrypted open with the correct key; reject a missing key
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_opens_encrypted_dbs_with_correct_key_and_rejects_missing_keys() {
    let config = common::store_config("enc-at-rest-open");
    let dir = config.storage_path.clone();
    let db_path = dir.join("enc.db");
    let hexkey = common::hex_key_64();
    let factory = mem_store::db::turso_connect_factory();

    let created = factory
        .connect(db_path.clone(), encrypted_options(&config, &hexkey))
        .await
        .expect("encrypted open");
    created
        .execute("CREATE TABLE t (x INTEGER)", Vec::new())
        .await
        .expect("create table");
    created
        .execute("INSERT INTO t VALUES (42)", Vec::new())
        .await
        .expect("insert");
    drop(created);

    let missing_key = factory
        .connect(db_path.clone(), plain_options(&config))
        .await;
    assert!(
        missing_key.is_err(),
        "an encrypted shard must not open without the key"
    );

    let reopened = factory
        .connect(db_path.clone(), encrypted_options(&config, &hexkey))
        .await
        .expect("reopen with the key");
    let rows = reopened
        .query("SELECT x FROM t", Vec::new())
        .await
        .expect("select");
    assert_eq!(rows.first().and_then(|row| row.i64("x")), Some(42));
    drop(reopened);

    common::cleanup_temp_dir(&dir).ok();
}

// ---------------------------------------------------------------------------
// 2/4 — 64-char hex key file, owner-only permissions, reuse
// ---------------------------------------------------------------------------

#[test]
fn test_auto_generates_64_char_hex_key_file_with_owner_only_permissions() {
    let dir = common::temp_dir("enc-at-rest-key");
    let storage = dir.clone();
    let key_path = dir.join("keys").join("opencode-mem-db.key");

    let hex = generate_database_encryption_key_file(&key_path, &storage).expect("generate");
    assert!(is_valid_database_encryption_hex_key(&hex), "hex validates");
    assert_eq!(hex.len(), 64, "aes-256 = 64 hex chars");
    assert!(key_path.exists(), "key file written");
    let content = std::fs::read_to_string(&key_path)
        .expect("read key file")
        .trim()
        .to_string();
    assert_eq!(content, hex, "file holds the generated hex");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path)
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "owner-only key file mode");
    }

    assert_eq!(
        ensure_database_encryption_key_file(&key_path, &storage).expect("ensure"),
        hex,
        "second ensure reuses the same key"
    );

    common::cleanup_temp_dir(&dir).ok();
}

// ---------------------------------------------------------------------------
// 3/4 — refuse to generate a new key when encrypted shards already exist
// ---------------------------------------------------------------------------

#[test]
fn test_refuses_to_generate_new_key_when_encrypted_dbs_already_exist() {
    let dir = common::temp_dir("enc-at-rest-refuse");
    let storage = dir.clone();
    std::fs::write(storage.join(".tursodb-encrypted-v1"), "{}").expect("marker write");
    let key_path = dir.join("missing.key");

    let error = generate_database_encryption_key_file(&key_path, &storage).expect_err("refuses");
    assert!(
        error.message().contains("already exist"),
        "fail-closed message: {}",
        error.message()
    );
    assert!(!key_path.exists(), "no key file when the marker exists");

    common::cleanup_temp_dir(&dir).ok();
}

// ---------------------------------------------------------------------------
// 4/4 — resolve_or_create: null when off, auto-create when on
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_or_create_is_null_when_off_and_auto_creates_when_on() {
    let off = common::store_config("enc-at-rest-off");
    assert!(
        resolve_or_create_database_encryption_key(&off)
            .expect("off resolution")
            .is_none(),
        "disabled without a key resolves to null"
    );
    assert!(
        mem_store::db::resolve_database_encryption(&off)
            .expect("off seam resolution")
            .is_none(),
        "the connect seam stays plain when encryption is off"
    );

    let mut on = common::store_config("enc-at-rest-on");
    let dir = on.storage_path.clone();
    // Never touch the real `~/.config` default path: pin `file://` to a
    // hermetic temp location.
    let key_path = dir.join("config").join("opencode-mem-db.key");
    on.database_encryption_enabled = true;
    on.database_encryption_key = Some(format!("file://{}", key_path.display()));

    let hex = resolve_or_create_database_encryption_key(&on)
        .expect("on resolution")
        .expect("a resolved key");
    assert!(is_valid_database_encryption_hex_key(&hex));
    assert!(key_path.exists(), "file:// key auto-created on demand");

    let options = mem_store::db::resolve_database_encryption(&on)
        .expect("on seam resolution")
        .expect("encryption options");
    assert_eq!(options.cipher, "aes256gcm");
    assert_eq!(
        options.hexkey, hex,
        "the seam hands the resolved key to turso"
    );

    common::cleanup_temp_dir(&dir).ok();
}
