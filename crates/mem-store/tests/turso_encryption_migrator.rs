//! Encryption migrator — port of upstream
//! `src/services/turso/encryption-migrator.ts` (blueprint W1.5 step 4,
//! lane sE).
//!
//! Scaffolded empty by the captain (step 2); the lane fills it RED-first.

mod common;

use std::path::Path;
use std::sync::Arc;

use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions, EncryptionOptions, Value};
use mem_store::encryption_migrator::{
    ENCRYPT_MARKER, dump_and_reload_encrypted, is_already_encrypted, marker_path,
    run_database_encryption_migration,
};
use mem_store::operation_lock::OPERATION_LOCK_FILE;
use mem_store::scope::Scope;

/// Upstream `buildConnectOptions(encryption)`.
fn encrypted_options(config: &StoreConfig, hexkey: &str) -> ConnectOptions {
    let mut options = ConnectOptions::for_config(config, mem_store::platform::Platform::current());
    options.encryption = Some(EncryptionOptions {
        cipher: "aes256gcm".to_string(),
        hexkey: hexkey.to_string(),
    });
    options
}

/// Upstream `buildConnectOptions()`.
fn plain_options(config: &StoreConfig) -> ConnectOptions {
    ConnectOptions::for_config(config, mem_store::platform::Platform::current())
}

/// The 16-column upstream-local memory insert (note: no `session_id` — the
/// ready-gate schema migration backfills it later, exactly like upstream).
const MEMORIES_INSERT: &str = "INSERT INTO memories \
    (id, content, vector, container_tag, tags, type, created_at, updated_at, metadata, \
     display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned) \
    VALUES (?,?,vector32(?),?,?,?,?,?,?,?,?,?,?,?,?,?)";

/// A plaintext shard carrying a generic table (`notes`) AND a `memories`
/// table (upstream `createPlaintextShard`).
async fn create_plaintext_shard(config: &StoreConfig, path: &Path) {
    let db = mem_store::db::turso_connect_factory()
        .connect(path.to_path_buf(), plain_options(config))
        .await
        .expect("plain open");
    db.execute(
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)",
        Vec::new(),
    )
    .await
    .expect("notes ddl");
    db.execute(
        "INSERT INTO notes (id, body) VALUES (1, 'hello')",
        Vec::new(),
    )
    .await
    .expect("notes insert");
    db.execute(&mem_store::sql::memories_ddl(2), Vec::new())
        .await
        .expect("memories ddl");
    db.execute(mem_store::sql::MEMORY_SHARD_V2_ALTER, Vec::new())
        .await
        .expect("v2 alter");
    db.execute(
        MEMORIES_INSERT,
        vec![
            Value::Text("mem_1".to_string()),
            Value::Text("encrypt me".to_string()),
            Value::Text("[1.0,0.25]".to_string()),
            Value::Text("opencode_project_a1b2c3d4e5f67890".to_string()),
            Value::Null,
            Value::Null,
            Value::Integer(1),
            Value::Integer(2),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Integer(0),
        ],
    )
    .await
    .expect("memories insert");
    drop(db);
}

fn names_with(storage: &Path, needle: &str) -> Vec<String> {
    std::fs::read_dir(storage)
        .expect("storage read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains(needle))
        .collect()
}

// ---------------------------------------------------------------------------
// 1/8 — no-op when the encryption marker already exists
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_is_a_no_op_when_the_encryption_marker_already_exists() {
    let key_hex = common::hex_key_64();
    let config = common::encrypted_store_config("enc-mig-marker", &key_hex);
    let (config, connections, shards) = common::real_components(config).await;
    // Converting shards copies, removes and renames under the root's proof.
    let _proof = common::exclusive_root(&config.storage_path);
    let storage = config.storage_path.clone();
    let shards = Arc::new(shards);

    let marker = storage.join(ENCRYPT_MARKER);
    let original = r#"{"cipher":"aes256gcm","migratedAt":"2026-01-01T00:00:00.000Z"}"#;
    std::fs::write(&marker, original).expect("marker write");

    // A plaintext db that must NOT be converted (marker short-circuits first).
    let keep = storage.join("keep.db");
    create_plaintext_shard(&config, &keep).await;

    run_database_encryption_migration(&storage, &connections, &shards, &config)
        .await
        .expect("no-op");

    assert_eq!(
        std::fs::read_to_string(&marker).expect("marker read"),
        original,
        "marker bytes untouched"
    );
    let reopened = mem_store::db::turso_connect_factory()
        .connect(keep.clone(), plain_options(&config))
        .await
        .expect("keep.db still opens plaintext");
    drop(reopened);
    assert!(
        !storage.join(OPERATION_LOCK_FILE).exists(),
        "no operation lock left behind"
    );

    common::cleanup_temp_dir(&storage).ok();
}

// ---------------------------------------------------------------------------
// 2/8 — writes the marker after converting plaintext shards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_writes_marker_after_converting_plaintext_shards() {
    let key_hex = common::hex_key_64();
    let config = common::encrypted_store_config("enc-mig-convert", &key_hex);
    let (config, connections, shards) = common::real_components(config).await;
    // Converting shards copies, removes and renames under the root's proof.
    let _proof = common::exclusive_root(&config.storage_path);
    let storage = config.storage_path.clone();
    let shards = Arc::new(shards);

    let data = storage.join("data.db");
    create_plaintext_shard(&config, &data).await;

    run_database_encryption_migration(&storage, &connections, &shards, &config)
        .await
        .expect("migrates");

    let marker = marker_path(&storage);
    assert!(marker.exists(), "marker written");
    let marker_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&marker).expect("marker read"))
            .expect("marker json");
    assert_eq!(marker_json["cipher"], "aes256gcm");

    let factory = mem_store::db::turso_connect_factory();
    assert!(
        factory
            .connect(data.clone(), plain_options(&config))
            .await
            .is_err(),
        "converted shard no longer opens plaintext"
    );

    let encrypted = factory
        .connect(data.clone(), encrypted_options(&config, &key_hex))
        .await
        .expect("encrypted open");
    let notes = encrypted
        .query("SELECT body FROM notes WHERE id = 1", Vec::new())
        .await
        .expect("notes select");
    assert_eq!(
        notes.first().and_then(|row| row.text("body")).as_deref(),
        Some("hello")
    );
    let memories = encrypted
        .query(
            "SELECT content, vector_extract(vector) AS v FROM memories WHERE id = 'mem_1'",
            Vec::new(),
        )
        .await
        .expect("memories select");
    let memory = memories.first().expect("memory row");
    assert_eq!(memory.text("content").as_deref(), Some("encrypt me"));
    let extracted = memory.text("v").expect("vector_extract text");
    let parsed: Vec<f64> = serde_json::from_str(&extracted).expect("vector json");
    assert_eq!(parsed.len(), 2, "2-element vector round-trips");
    drop(encrypted);

    assert!(
        !names_with(&storage, ".pre-encrypt-").is_empty(),
        "pre-encrypt backup kept"
    );
    assert!(
        !storage.join(OPERATION_LOCK_FILE).exists(),
        "no operation lock left behind"
    );

    common::cleanup_temp_dir(&storage).ok();
}

// ---------------------------------------------------------------------------
// 3/8 — refuse an already-encrypted shard with a different key
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_refuses_already_encrypted_with_different_key() {
    // The configured key differs from the one the shard was created with.
    let config = common::encrypted_store_config("enc-mig-refuse", &"ab".repeat(32));
    let (config, connections, shards) = common::real_components(config).await;
    // Converting shards copies, removes and renames under the root's proof.
    let _proof = common::exclusive_root(&config.storage_path);
    let storage = config.storage_path.clone();
    let shards = Arc::new(shards);

    let secret = storage.join("secret.db");
    let created = mem_store::db::turso_connect_factory()
        .connect(
            secret.clone(),
            encrypted_options(&config, &common::hex_key_64()),
        )
        .await
        .expect("encrypted create");
    created
        .execute("CREATE TABLE t (x INTEGER)", Vec::new())
        .await
        .expect("ddl");
    drop(created);

    let error = run_database_encryption_migration(&storage, &connections, &shards, &config)
        .await
        .expect_err("refuses a foreign-key shard");
    assert!(
        error.message().contains("Cannot open"),
        "message: {}",
        error.message()
    );
    assert!(
        !marker_path(&storage).exists(),
        "no marker after a refused migration"
    );

    common::cleanup_temp_dir(&storage).ok();
}

// ---------------------------------------------------------------------------
// 4/8 — a copy failure removes the staged `.encrypt-*.tmp`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_copy_failure_removes_the_staged_encrypt_tmp_file() {
    let key_hex = common::hex_key_64();
    let config = common::encrypted_store_config("enc-mig-copyfail", &key_hex);
    let (config, connections, _shards) = common::real_components(config).await;
    // Converting shards copies, removes and renames under the root's proof.
    let _proof = common::exclusive_root(&config.storage_path);
    let storage = config.storage_path.clone();

    // A `memories` table missing every column the fixed memories-copy SQL
    // references. Its DDL replays into the staged db verbatim, then the copy
    // SELECT fails deterministically (no such column: content) — reproducing
    // the copy-failure path without a synthetic fault seam.
    let db_path = storage.join("broken.db");
    let db = mem_store::db::turso_connect_factory()
        .connect(db_path.clone(), plain_options(&config))
        .await
        .expect("plain open");
    db.execute("CREATE TABLE memories (id TEXT PRIMARY KEY)", Vec::new())
        .await
        .expect("ddl");
    db.execute("INSERT INTO memories (id) VALUES ('mem_1')", Vec::new())
        .await
        .expect("insert");
    drop(db);

    dump_and_reload_encrypted(&storage, &db_path, &connections, &config)
        .await
        .expect_err("the broken copy fails");

    assert!(
        names_with(&storage, ".encrypt-").is_empty(),
        "staged `.encrypt-*.tmp` removed on copy failure"
    );
    // The original plaintext shard is untouched and still opens.
    let reopened = mem_store::db::turso_connect_factory()
        .connect(db_path.clone(), plain_options(&config))
        .await
        .expect("plaintext shard still opens");
    drop(reopened);

    common::cleanup_temp_dir(&storage).ok();
}

// ---------------------------------------------------------------------------
// 5/8 — identifier quoting round-trips a name containing a double quote
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_migrates_a_table_name_that_requires_identifier_quoting() {
    let key_hex = common::hex_key_64();
    let config = common::encrypted_store_config("enc-mig-quote", &key_hex);
    let (config, connections, shards) = common::real_components(config).await;
    // Converting shards copies, removes and renames under the root's proof.
    let _proof = common::exclusive_root(&config.storage_path);
    let storage = config.storage_path.clone();
    let shards = Arc::new(shards);

    let db_path = storage.join("quoted.db");
    let db = mem_store::db::turso_connect_factory()
        .connect(db_path.clone(), plain_options(&config))
        .await
        .expect("plain open");
    // A double quote in a table AND column name: only correct identifier
    // quoting round-trips it (`SELECT * FROM "foo""bar"`). Unquoted
    // interpolation (`SELECT * FROM foo"bar`) is a syntax error.
    db.execute(
        "CREATE TABLE \"foo\"\"bar\" (x INTEGER, \"we\"\"ird\" TEXT)",
        Vec::new(),
    )
    .await
    .expect("ddl");
    db.execute(
        "INSERT INTO \"foo\"\"bar\" (x, \"we\"\"ird\") VALUES (41, 'ok')",
        Vec::new(),
    )
    .await
    .expect("insert");
    drop(db);

    run_database_encryption_migration(&storage, &connections, &shards, &config)
        .await
        .expect("migrates a quoted-name table");

    let encrypted = mem_store::db::turso_connect_factory()
        .connect(db_path.clone(), encrypted_options(&config, &key_hex))
        .await
        .expect("encrypted open");
    let rows = encrypted
        .query("SELECT x, \"we\"\"ird\" FROM \"foo\"\"bar\"", Vec::new())
        .await
        .expect("select");
    let row = rows.first().expect("row");
    assert_eq!(row.i64("x"), Some(41));
    assert_eq!(row.text("we\"ird").as_deref(), Some("ok"));
    drop(encrypted);

    common::cleanup_temp_dir(&storage).ok();
}

// ---------------------------------------------------------------------------
// 6/8 — the `is_already_encrypted` probe never mints a key file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_is_already_encrypted_probe_does_not_mint_a_key_file() {
    let mut config = common::store_config("enc-mig-nomint");
    let storage = config.storage_path.clone();
    let key_path = storage.join("keys").join("missing.key");
    config.database_encryption_enabled = true;
    // A `file://` key pointing at a file that does NOT exist yet: the
    // provisioning resolver would create it, the read-only probe must not.
    config.database_encryption_key = Some(format!("file://{}", key_path.display()));

    let db_path = storage.join("probe.db");
    create_plaintext_shard(&config, &db_path).await;

    assert!(
        !is_already_encrypted(&db_path, &config)
            .await
            .expect("the probe answers (no guard refusal here)"),
        "a plaintext shard is not already encrypted"
    );
    assert!(
        !key_path.exists(),
        "probing must not mint the file:// key as a side effect"
    );

    common::cleanup_temp_dir(&storage).ok();
}

// 7/8 - encryption disabled is a total no-op (upstream
// "is a no-op when encryption is disabled", line 124).
#[tokio::test]
async fn test_is_a_no_op_when_encryption_is_disabled() {
    let config = common::store_config("enc-mig-disabled"); // enabled=false, key=None
    let (config, connections, shards) = common::real_components(config).await;
    let storage = config.storage_path.clone();
    let shards = Arc::new(shards);

    let data = storage.join("data.db");
    create_plaintext_shard(&config, &data).await;

    run_database_encryption_migration(&storage, &connections, &shards, &config)
        .await
        .expect("no-op when encryption is disabled");

    assert!(
        !marker_path(&storage).exists(),
        "no marker when encryption is disabled"
    );
    assert!(
        names_with(&storage, ".pre-encrypt-").is_empty(),
        "no pre-encrypt backup when encryption is disabled"
    );
    assert!(
        names_with(&storage, ".encrypt-").is_empty(),
        "no staged .encrypt-*.tmp when encryption is disabled"
    );
    assert!(
        !storage.join(OPERATION_LOCK_FILE).exists(),
        "the disabled path never takes the operation lock"
    );
    let plain = mem_store::db::turso_connect_factory()
        .connect(data.clone(), plain_options(&config))
        .await
        .expect("the shard still opens plaintext");
    let rows = plain
        .query(
            "SELECT content FROM memories WHERE id = 'mem_1'",
            Vec::new(),
        )
        .await
        .expect("plaintext select");
    assert_eq!(
        rows.first().and_then(|row| row.text("content")).as_deref(),
        Some("encrypt me"),
        "the plaintext row is untouched"
    );
    drop(plain);

    common::cleanup_temp_dir(&storage).ok();
}

// 8/8 - the shard manager is usable after metadata.db is encrypted (upstream
// "keeps shard manager usable after encrypting metadata.db", line 210).
#[tokio::test]
async fn test_shard_manager_lists_shards_after_metadata_db_encryption() {
    const HASH: &str = "a1b2c3d4e5f67890";
    let key_hex = common::hex_key_64();
    let mut config = common::store_config("enc-mig-shardmgr");
    let storage = config.storage_path.clone();

    // Previous run: encryption off, plaintext metadata.db + one shard row.
    {
        let (_, connections, shards) = common::real_components(config.clone()).await;
        shards
            .create_shard(Scope::Project, HASH, 0)
            .await
            .expect("seed a plaintext shard");
        assert_eq!(
            shards
                .get_all_shards(Scope::Project, Some(HASH))
                .await
                .expect("plaintext read")
                .len(),
            1,
            "the seed is visible before the migration"
        );
        drop(shards);
        drop(connections);
    }

    // Next run: encryption on over the same root.
    config.database_encryption_enabled = true;
    config.database_encryption_key = Some(key_hex.clone());
    let (config, connections, shards) = common::real_components(config).await;
    let _proof = common::exclusive_root(&storage);
    let shards = Arc::new(shards);

    run_database_encryption_migration(&storage, &connections, &shards, &config)
        .await
        .expect("migrates the plaintext root");

    // The active metadata.db really is the encrypted file.
    let metadata_path = storage.join(mem_store::operation_lock::METADATA_DB_NAME);
    let factory = mem_store::db::turso_connect_factory();
    // The existing `test_writes_marker_after_converting_plaintext_shards`
    // pins connect-time failure for a converted file; if a platform defers the
    // refusal to the first read, fall back to asserting the query errors and
    // record in the ledger which arm fired.
    assert!(
        factory
            .connect(metadata_path.clone(), plain_options(&config))
            .await
            .is_err(),
        "metadata.db no longer opens plaintext"
    );
    let encrypted = factory
        .connect(metadata_path, encrypted_options(&config, &key_hex))
        .await
        .expect("encrypted metadata open");
    assert_eq!(
        encrypted
            .query("SELECT scope_hash FROM shards", Vec::new())
            .await
            .expect("encrypted select")
            .len(),
        1,
        "the registry row survived the conversion"
    );
    drop(encrypted);

    let listed = shards
        .get_all_shards(Scope::Project, Some(HASH))
        .await
        .expect("the shard manager lists shards after metadata.db encryption");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].scope_hash, HASH);
    assert_eq!(listed[0].shard_index, 0);

    common::cleanup_temp_dir(&storage).ok();
}
