//! Shard v3 battery (issue 43 W2): the `shard_stats` schema, the
//! convergence and refusal behaviour of `shard_facts::ensure_v3`, the count
//! sync, and — from step 4 on — the lazy upgrade on the write path plus the
//! fingerprint claim precedence.

mod common;

use mem_core::config::EmbeddingPooling;
use mem_core::embedding::EmbeddingIdentity;
use mem_store::db::{DbHandle, Value, query_one};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    MEMORY_SHARD_MAX_KNOWN, MigrationOptions, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::scope::Scope;
use mem_store::shard_facts;
use mem_store::sql;

const DIMS: u32 = 2;

/// Opens a file and applies only v1..v2 (the pre-W2 on-disk shape).
async fn open_v2(config: &mem_store::config::StoreConfig, file: &str) -> DbHandle {
    let db = common::open_real_db(config, file).await.expect("open");
    let through_v2: Vec<_> = memory_shard_migrations(DIMS)
        .into_iter()
        .filter(|migration| migration.version <= 2)
        .collect();
    apply_schema_migrations(
        db.as_ref(),
        &through_v2,
        &MigrationOptions {
            db_path: None,
            label: "v2-fixture",
            max_known: 2,
        },
        None,
    )
    .await
    .expect("apply v1..v2");
    db
}

/// Writes the two legacy identity keys a v2 shard carries.
async fn stamp_metadata(db: &dyn mem_store::db::Db, dims: u32, model: &str) {
    db.execute(
        "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_dimensions', ?1)",
        vec![Value::Text(dims.to_string())],
    )
    .await
    .expect("dimensions metadata");
    db.execute(
        "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_model', ?1)",
        vec![Value::Text(model.to_string())],
    )
    .await
    .expect("model metadata");
}

async fn seed_vectors(db: &dyn mem_store::db::Db, ids: &[&str]) {
    for (index, id) in ids.iter().enumerate() {
        let vector = mem_store::vector::vector_to_text(&[1.0f32, index as f32]).expect("vector");
        db.execute(
            "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at)
             VALUES (?1, ?2, vector32(?3), 'opencode_project_test', ?4, ?4)",
            vec![
                Value::Text((*id).to_string()),
                Value::Text(format!("content-{index}")),
                Value::Text(vector),
                Value::Integer(100 + index as i64),
            ],
        )
        .await
        .expect("insert memory");
    }
}

async fn user_version(db: &dyn mem_store::db::Db) -> i64 {
    query_one(db, sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version probe")
        .and_then(|row| row.i64("user_version"))
        .unwrap_or(0)
}

async fn table_exists(db: &dyn mem_store::db::Db, table: &str) -> bool {
    query_one(
        db,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
        vec![Value::Text(table.to_string())],
    )
    .await
    .expect("table probe")
    .is_some()
}

async fn stats_rows(db: &dyn mem_store::db::Db) -> i64 {
    query_one(db, "SELECT COUNT(*) AS count FROM shard_stats", Vec::new())
        .await
        .expect("stats probe")
        .and_then(|row| row.i64("count"))
        .unwrap_or(-1)
}

async fn stats_count(db: &dyn mem_store::db::Db) -> Option<i64> {
    query_one(
        db,
        "SELECT vector_count FROM shard_stats WHERE id = 1",
        Vec::new(),
    )
    .await
    .expect("stats row probe")
    .and_then(|row| row.i64("vector_count"))
}

// ---------------------------------------------------------------------------
// AC3
// ---------------------------------------------------------------------------

/// A fresh shard is v3 and carries **no** `shard_stats` row: a placeholder
/// `0` before the count is known would be a W7 reaping trigger.
#[tokio::test]
async fn test_a_fresh_shard_is_v3_and_has_no_stats_row() {
    let (config, _connections, manager) =
        common::real_components(common::store_config("v3-fresh")).await;
    let db = common::open_real_db(&config, "fresh.db")
        .await
        .expect("open");

    manager
        .init_shard_db(db.as_ref(), DIMS, "test-model")
        .await
        .expect("initialize a fresh shard");

    assert_eq!(user_version(db.as_ref()).await, 3, "the fresh shard is v3");
    assert!(
        table_exists(db.as_ref(), "shard_stats").await,
        "the v3 migration must create shard_stats"
    );
    assert_eq!(
        stats_rows(db.as_ref()).await,
        0,
        "the migration must not write a placeholder count"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

// ---------------------------------------------------------------------------
// AC4
// ---------------------------------------------------------------------------

/// v2→v3 preserves every row, vector and `shard_metadata` entry; the count is
/// the truth measured after the upgrade.
#[tokio::test]
async fn test_upgrading_a_v2_shard_preserves_rows_vectors_and_metadata() {
    let config = common::store_config("v3-upgrade");
    let db = open_v2(&config, "shard.db").await;
    stamp_metadata(db.as_ref(), DIMS, "test-model").await;
    seed_vectors(db.as_ref(), &["m1", "m2"]).await;

    let before: Vec<(Option<String>, Option<String>)> = db
        .query(
            "SELECT id, vector_extract(vector) AS vector_json FROM memories ORDER BY id",
            Vec::new(),
        )
        .await
        .expect("read before")
        .into_iter()
        .map(|row| (row.text("id"), row.text("vector_json")))
        .collect();
    assert_eq!(before.len(), 2);

    let version = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("upgrade to v3");
    assert_eq!(version, 3);
    assert_eq!(user_version(db.as_ref()).await, 3);

    let count = shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync the observed count");
    assert_eq!(count, 2, "the count is the measured truth");
    assert_eq!(stats_count(db.as_ref()).await, Some(2));

    let after: Vec<(Option<String>, Option<String>)> = db
        .query(
            "SELECT id, vector_extract(vector) AS vector_json FROM memories ORDER BY id",
            Vec::new(),
        )
        .await
        .expect("read after")
        .into_iter()
        .map(|row| (row.text("id"), row.text("vector_json")))
        .collect();
    assert_eq!(before, after, "every row and vector must survive");

    for (key, expected) in [
        ("embedding_dimensions", "2"),
        ("embedding_model", "test-model"),
    ] {
        let row = query_one(
            db.as_ref(),
            "SELECT value FROM shard_metadata WHERE key = ?1",
            vec![Value::Text(key.to_string())],
        )
        .await
        .expect("metadata read")
        .expect("metadata row");
        assert_eq!(row.text("value").as_deref(), Some(expected), "{key}");
    }

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

// ---------------------------------------------------------------------------
// AC5
// ---------------------------------------------------------------------------

/// A file whose v3 DDL committed but whose pragma did not is re-applied
/// convergently (`CREATE TABLE IF NOT EXISTS`).
#[tokio::test]
async fn test_a_v3_shaped_file_stamped_v2_converges() {
    let config = common::store_config("v3-converge");
    let db = open_v2(&config, "shard.db").await;
    db.execute(shard_facts::SHARD_STATS_DDL, Vec::new())
        .await
        .expect("v3 DDL committed out of band");
    db.execute(&sql::set_user_version_sql(2), Vec::new())
        .await
        .expect("stamp v2");

    let version = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect("converge instead of failing on the existing table");
    assert_eq!(version, 3, "the pragma catches up with the committed DDL");
    assert_eq!(user_version(db.as_ref()).await, 3);

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

// ---------------------------------------------------------------------------
// AC6
// ---------------------------------------------------------------------------

/// A v4 shard is refused by this build (it knows v3), with the typed error.
#[tokio::test]
async fn test_a_v4_shard_is_refused_by_this_build() {
    let config = common::store_config("v3-refuse-v4");
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open");
    db.execute(&sql::set_user_version_sql(4), Vec::new())
        .await
        .expect("stamp v4");

    let error = shard_facts::ensure_v3(db.as_ref(), DIMS)
        .await
        .expect_err("a v4 file must be refused by a v3 build");
    assert!(
        matches!(
            error,
            StoreError::SchemaNewerThanBuild {
                found: 4,
                supported: 3,
                ..
            }
        ),
        "got {error:?}"
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

// ---------------------------------------------------------------------------
// AC17
// ---------------------------------------------------------------------------

/// The shard set's declared maximum is its highest migration.
#[test]
fn test_memory_shard_max_known_matches_the_highest_migration() {
    let declared = MEMORY_SHARD_MAX_KNOWN;
    let highest = memory_shard_migrations(768)
        .last()
        .map(|migration| migration.version);
    assert_eq!(
        Some(declared),
        highest,
        "MEMORY_SHARD_MAX_KNOWN must track the set's highest version"
    );
    assert_eq!(declared, 3, "W2's v3 is the highest step");
}

// ---------------------------------------------------------------------------
// AC7 — the write path
// ---------------------------------------------------------------------------

const HASH: &str = "a1b2c3d4e5f60001";

/// A registered v2 shard whose registry row is deliberately stale at 0: the
/// next write must observe the truth, not trust the cache.
async fn registered_v2_shard(
    prefix: &str,
    ids: &[&str],
    identity: Option<EmbeddingIdentity>,
) -> (
    mem_store::config::StoreConfig,
    std::sync::Arc<mem_store::connection::ConnectionManager>,
    mem_store::shard_manager::ShardManager,
    mem_store::shard_manager::ShardInfo,
    std::path::PathBuf,
) {
    let (_config, connections, manager) =
        common::real_components_with_identity(common::store_config(prefix), identity).await;
    let path = manager
        .get_shard_path(Scope::Project, HASH, 0)
        .expect("shard path");
    let db = manager
        .open_shard(&path)
        .await
        .expect("open the shard file");
    let through_v2: Vec<_> = memory_shard_migrations(DIMS)
        .into_iter()
        .filter(|migration| migration.version <= 2)
        .collect();
    apply_schema_migrations(
        db.as_ref(),
        &through_v2,
        &MigrationOptions {
            db_path: None,
            label: "v2-fixture",
            max_known: 2,
        },
        None,
    )
    .await
    .expect("apply v1..v2");
    stamp_metadata(db.as_ref(), DIMS, "test-model").await;
    seed_vectors(db.as_ref(), ids).await;
    let shard = manager
        .register_existing_shard(Scope::Project, HASH, 0, &path, 0, true)
        .await
        .expect("register the shard");
    drop(db);
    let config = manager.config().clone();
    (config, connections, manager, shard, path)
}

/// AC7a: the writer upgrades a v2 shard lazily and records the observed
/// count in both authorities (the shard's `shard_stats` and the registry).
#[tokio::test]
async fn test_a_v2_shard_is_upgraded_by_the_next_write() {
    let (config, _connections, manager, shard, path) =
        registered_v2_shard("v3-write-upgrade", &["m1", "m2"], None).await;

    let written = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("resolve the write shard");
    assert_eq!(written.id, shard.id);
    assert_eq!(
        written.vector_count, 2,
        "a populated lost-registry file reports its observed count, not a false 0"
    );
    let registry = manager
        .get_shard_by_id(shard.id)
        .await
        .expect("registry read")
        .expect("registry row");
    assert_eq!(registry.vector_count, 2, "the registry is synced once");

    let db = manager.open_shard(&path).await.expect("reopen");
    assert_eq!(
        user_version(db.as_ref()).await,
        3,
        "the next write upgrades the file lazily"
    );
    assert_eq!(stats_count(db.as_ref()).await, Some(2));
    assert_eq!(stats_rows(db.as_ref()).await, 1);
    drop(db);

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC7b: a second write to an already-v3 shard converges (no error, no
/// version change) and re-syncs the observed count absolutely.
#[tokio::test]
async fn test_a_second_write_does_not_re_run_the_upgrade() {
    let (config, connections, manager, shard, path) =
        registered_v2_shard("v3-write-second", &["m1", "m2"], None).await;
    manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("first write upgrades");

    // New truth arrives between the two writes (a direct insert bypasses the
    // manager's count paths, like the delete paths of R-5).
    let db = connections
        .get_connection(&path)
        .await
        .expect("direct connection");
    seed_vectors(db.as_ref(), &["m3"]).await;
    drop(db);

    let second = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("the second write converges on the v3 file");
    assert_eq!(
        second.vector_count, 3,
        "the second write re-syncs the truth"
    );
    assert_eq!(
        manager
            .get_shard_by_id(shard.id)
            .await
            .expect("registry read")
            .expect("registry row")
            .vector_count,
        3
    );

    let db = manager.open_shard(&path).await.expect("reopen");
    assert_eq!(user_version(db.as_ref()).await, 3, "still v3");
    assert_eq!(stats_count(db.as_ref()).await, Some(3));
    assert_eq!(
        stats_rows(db.as_ref()).await,
        1,
        "the sync upserts, never appends"
    );
    drop(db);

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

// ---------------------------------------------------------------------------
// AC10 — the claim precedence
// ---------------------------------------------------------------------------

/// The identity the manager's config describes.
fn identity_for(config: &mem_store::config::StoreConfig) -> EmbeddingIdentity {
    EmbeddingIdentity {
        model: config.embedding_model.clone(),
        dimensions: config.embedding_dimensions,
        pooling: EmbeddingPooling::Mean,
        dtype: None,
        use_task_prefixes: false,
        query_prefix: None,
        document_prefix: None,
        max_tokens: None,
    }
}

async fn fingerprint_of(db: &dyn mem_store::db::Db) -> Option<String> {
    shard_facts::read_fingerprint(db)
        .await
        .expect("fingerprint read")
}

/// AC10a: an empty shard is claimed; the next write finds it already present.
#[tokio::test]
async fn test_an_empty_v2_shard_is_claimed_by_the_next_write() {
    let config_probe = common::store_config("v3-claim-empty");
    let identity = identity_for(&config_probe);
    let (config, _connections, manager, _shard, path) =
        registered_v2_shard("v3-claim-empty", &[], Some(identity.clone())).await;
    let expected = identity.fingerprint().expect("identity serializes");

    let db = manager.open_shard(&path).await.expect("open the shard");
    let outcome =
        shard_facts::prepare_active_shard(db.as_ref(), DIMS, Some(&identity), "test-model")
            .await
            .expect("prepare an empty shard");
    assert_eq!(outcome.step, shard_facts::FingerprintStep::Claimed);
    assert_eq!(outcome.vector_count, 0);
    assert_eq!(
        fingerprint_of(db.as_ref()).await.as_deref(),
        Some(expected.as_str()),
        "the claim writes the configured fingerprint"
    );
    drop(db);

    // The write path sees the claim and records the (still empty) count.
    let written = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("write shard");
    assert_eq!(written.vector_count, 0);
    let db = manager.open_shard(&path).await.expect("reopen");
    let again = shard_facts::prepare_active_shard(db.as_ref(), DIMS, Some(&identity), "test-model")
        .await
        .expect("second prepare");
    assert_eq!(again.step, shard_facts::FingerprintStep::AlreadyPresent);
    assert_eq!(stats_count(db.as_ref()).await, Some(0));
    drop(db);

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC10b: a non-empty un-fingerprinted shard is withheld, twice over.
#[tokio::test]
async fn test_a_non_empty_unfingerprinted_shard_stays_unclaimed() {
    let config_probe = common::store_config("v3-claim-nonempty");
    let identity = identity_for(&config_probe);
    let (config, _connections, manager, _shard, path) =
        registered_v2_shard("v3-claim-nonempty", &["m1", "m2"], Some(identity.clone())).await;

    manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("write shard");
    let db = manager.open_shard(&path).await.expect("open the shard");
    let outcome =
        shard_facts::prepare_active_shard(db.as_ref(), DIMS, Some(&identity), "test-model")
            .await
            .expect("prepare a populated shard");
    assert_eq!(
        outcome.step,
        shard_facts::FingerprintStep::WithheldNonEmpty { rows: 2 }
    );
    assert_eq!(outcome.vector_count, 2, "the count is still recorded");
    assert_eq!(stats_count(db.as_ref()).await, Some(2));
    assert_eq!(fingerprint_of(db.as_ref()).await, None);
    drop(db);

    manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("second write shard");
    let db = manager.open_shard(&path).await.expect("reopen");
    assert_eq!(
        fingerprint_of(db.as_ref()).await,
        None,
        "two writes must not conjure a fingerprint"
    );
    drop(db);

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC10c: an identity that disagrees with the configured model/dims never
/// writes a fingerprint.
#[tokio::test]
async fn test_an_inconsistent_identity_never_writes_a_fingerprint() {
    let config_probe = common::store_config("v3-claim-inconsistent");
    let mut identity = identity_for(&config_probe);
    identity.model = "other-model".to_string();
    let (config, _connections, manager, _shard, path) =
        registered_v2_shard("v3-claim-inconsistent", &[], Some(identity.clone())).await;

    manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect("write shard");
    let db = manager.open_shard(&path).await.expect("open the shard");
    let outcome =
        shard_facts::prepare_active_shard(db.as_ref(), DIMS, Some(&identity), "test-model")
            .await
            .expect("prepare");
    assert_eq!(
        outcome.step,
        shard_facts::FingerprintStep::WithheldInconsistent
    );
    assert_eq!(fingerprint_of(db.as_ref()).await, None);
    drop(db);

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

/// AC10d: a present fingerprint on a non-empty shard is `AlreadyPresent`,
/// never replaced and never re-classified as `WithheldNonEmpty`.
#[tokio::test]
async fn test_an_existing_fingerprint_is_never_overwritten() {
    let config_probe = common::store_config("v3-claim-present");
    let identity = identity_for(&config_probe);
    let (config, _connections, manager, _shard, path) =
        registered_v2_shard("v3-claim-present", &["m1", "m2"], Some(identity.clone())).await;
    {
        let db = manager.open_shard(&path).await.expect("open the shard");
        db.execute(
            "INSERT INTO shard_metadata (key, value) VALUES ('embedding_fingerprint', 'other')",
            Vec::new(),
        )
        .await
        .expect("seed a foreign fingerprint");
    }

    // W3: the write path REFUSES this shard — the seeded fingerprint is
    // foreign to the configured identity. The test's subject (claim
    // precedence) is asserted through the W2 primitive below, which keeps its
    // behaviour; `get_write_shard` now answers the typed refusal.
    let error = manager
        .get_write_shard(Scope::Project, HASH)
        .await
        .expect_err("a foreign fingerprint refuses the write path");
    assert!(
        matches!(error, StoreError::ShardModelMismatch(_)),
        "got {error:?}"
    );
    let db = manager.open_shard(&path).await.expect("open the shard");
    let outcome =
        shard_facts::prepare_active_shard(db.as_ref(), DIMS, Some(&identity), "test-model")
            .await
            .expect("prepare");
    assert_eq!(
        outcome.step,
        shard_facts::FingerprintStep::AlreadyPresent,
        "a present key wins over the non-empty withholding"
    );
    assert_eq!(fingerprint_of(db.as_ref()).await.as_deref(), Some("other"));
    drop(db);

    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}
