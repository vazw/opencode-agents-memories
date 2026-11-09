//! Issue 43 W6c — the sweep stamp's schema bumps: `user_prompts` v3,
//! `user_profiles` v2, episodic v8.
//!
//! Each bump adds exactly ONE thing: the `sweep_state` table
//! (`tier TEXT PRIMARY KEY, last_swept_unix_ms INTEGER NOT NULL`) where the
//! sweep writes its truthful stamp. They are data-migration-free: no column
//! changes, no row rewrite — only `CREATE TABLE IF NOT EXISTS`.
//!
//! Both arms of the ADR-0021 rule are pinned for every set: a file stamped by
//! the new build is applied once (`Ok(new_version)`), and reopening it with the
//! OLD `max_known` refuses with `SchemaNewerThanBuild` (no downgrade).

mod common;

use common::{exclusive_root, open_real_db, store_config};
use mem_store::config::StoreConfig;
use mem_store::db::{Db, Value, query_one};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    EPISODIC_MAX_KNOWN, MigrationOptions, USER_PROFILES_MAX_KNOWN, USER_PROMPTS_MAX_KNOWN,
    apply_episodic_schema, apply_schema_migrations, apply_user_prompts_schema,
    user_profiles_migrations,
};

const DIMS: u32 = 2;

async fn user_version(db: &dyn Db) -> i64 {
    query_one(db, "PRAGMA user_version", Vec::new())
        .await
        .expect("pragma")
        .expect("user_version row")
        .i64("user_version")
        .expect("integer user_version")
}

async fn table_exists(db: &dyn Db, table: &str) -> bool {
    query_one(
        db,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
        vec![Value::Text(table.to_string())],
    )
    .await
    .expect("sqlite_master")
    .is_some()
}

async fn columns(db: &dyn Db, table: &str) -> Vec<String> {
    db.query(&format!("PRAGMA table_info({table})"), Vec::new())
        .await
        .expect("table_info")
        .iter()
        .filter_map(|row| row.text("name"))
        .collect()
}

/// The one table every bump adds, with exactly two columns.
async fn assert_sweep_state_table(db: &dyn Db) {
    assert!(
        table_exists(db, "sweep_state").await,
        "the bump creates sweep_state"
    );
    assert_eq!(
        columns(db, "sweep_state").await,
        ["tier", "last_swept_unix_ms"],
        "the stamp table's shape"
    );
    let info = db
        .query("PRAGMA table_info(sweep_state)", Vec::new())
        .await
        .expect("table_info(sweep_state)");
    let last = info
        .iter()
        .find(|row| row.text("name").as_deref() == Some("last_swept_unix_ms"))
        .expect("the timestamp column");
    assert_eq!(
        last.i64("notnull"),
        Some(1),
        "the timestamp is NOT NULL (a stamp always names a time)"
    );
}

fn prompts_options(config: &StoreConfig, max_known: i64) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("user-prompts.db")),
        label: "user-prompts.db",
        max_known,
    }
}

fn profiles_options(config: &StoreConfig, max_known: i64) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("user-profiles.db")),
        label: "user-profiles.db",
        max_known,
    }
}

fn episodic_options(config: &StoreConfig, max_known: i64) -> MigrationOptions {
    MigrationOptions {
        db_path: Some(config.storage_path.join("episodic.db")),
        label: "episodic.db",
        max_known,
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_user_prompts_v3_creates_the_sweep_state_table() {
    let config = store_config("sweep-stamp-prompts");
    let db = open_real_db(&config, "user-prompts.db")
        .await
        .expect("open");

    let version = apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, USER_PROMPTS_MAX_KNOWN),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("the prompts fold reaches its maximum");

    assert_eq!(USER_PROMPTS_MAX_KNOWN, 3, "the constant moved WITH the leg");
    assert_eq!(version, 3);
    assert_eq!(user_version(db.as_ref()).await, 3);
    assert_sweep_state_table(db.as_ref()).await;

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_user_prompts_v3_is_a_no_op_for_the_prompt_rows() {
    let config = store_config("sweep-stamp-prompts-noop");
    let db = open_real_db(&config, "user-prompts.db")
        .await
        .expect("open");

    // Build a genuine v2 file (a build that knows only 2 runs no v3 leg).
    apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, 2),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("build a v2 file");
    db.execute(
        "INSERT INTO user_prompts (id, session_id, message_id, content, created_at) \
         VALUES ('prompt_v2', 'session_v2', 'message_v2', 'survive the stamp bump', 7)",
        Vec::new(),
    )
    .await
    .expect("seed one v2 row");
    let before = columns(db.as_ref(), "user_prompts").await;

    let version = apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, USER_PROMPTS_MAX_KNOWN),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("v2 -> v3");

    assert_eq!(version, 3);
    assert_eq!(
        columns(db.as_ref(), "user_prompts").await,
        before,
        "the v3 leg changes no user_prompts column"
    );
    let row = query_one(
        db.as_ref(),
        "SELECT content FROM user_prompts WHERE id = 'prompt_v2'",
        Vec::new(),
    )
    .await
    .expect("select")
    .expect("the pre-v3 row survives");
    assert_eq!(
        row.text("content").as_deref(),
        Some("survive the stamp bump")
    );

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_user_prompts_v3_stamp_is_refused_by_a_two_only_build() {
    let config = store_config("sweep-stamp-prompts-old");
    let db = open_real_db(&config, "user-prompts.db")
        .await
        .expect("open");
    apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, USER_PROMPTS_MAX_KNOWN),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect("stamp v3");

    let refused = apply_user_prompts_schema(
        db.as_ref(),
        &prompts_options(&config, 2),
        Some(exclusive_root(&config.storage_path).as_ref()),
    )
    .await
    .expect_err("a two-only build refuses a v3 file");
    match refused {
        StoreError::SchemaNewerThanBuild {
            found, supported, ..
        } => {
            assert_eq!((found, supported), (3, 2));
        }
        other => panic!("expected SchemaNewerThanBuild, got {other:?}"),
    }

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_user_profiles_v2_creates_the_sweep_state_table() {
    let config = store_config("sweep-stamp-profiles");
    let db = open_real_db(&config, "user-profiles.db")
        .await
        .expect("open");
    let proof = exclusive_root(&config.storage_path);

    let version = apply_schema_migrations(
        db.as_ref(),
        &user_profiles_migrations(),
        &profiles_options(&config, USER_PROFILES_MAX_KNOWN),
        Some(proof.as_ref()),
    )
    .await
    .expect("the profiles set reaches its maximum");

    assert_eq!(
        USER_PROFILES_MAX_KNOWN, 2,
        "the constant moved WITH the row"
    );
    assert_eq!(version, 2);
    assert_eq!(user_version(db.as_ref()).await, 2);
    assert_sweep_state_table(db.as_ref()).await;

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_user_profiles_v2_stamp_is_refused_by_a_one_only_build() {
    let config = store_config("sweep-stamp-profiles-old");
    let db = open_real_db(&config, "user-profiles.db")
        .await
        .expect("open");
    let proof = exclusive_root(&config.storage_path);
    apply_schema_migrations(
        db.as_ref(),
        &user_profiles_migrations(),
        &profiles_options(&config, USER_PROFILES_MAX_KNOWN),
        Some(proof.as_ref()),
    )
    .await
    .expect("stamp v2");

    let refused = apply_schema_migrations(
        db.as_ref(),
        &user_profiles_migrations(),
        &profiles_options(&config, 1),
        Some(proof.as_ref()),
    )
    .await
    .expect_err("a one-only build refuses a v2 file");
    match refused {
        StoreError::SchemaNewerThanBuild {
            found, supported, ..
        } => {
            assert_eq!((found, supported), (2, 1));
        }
        other => panic!("expected SchemaNewerThanBuild, got {other:?}"),
    }

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_episodic_v8_creates_the_sweep_state_table() {
    let config = store_config("sweep-stamp-episodic");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    let proof = exclusive_root(&config.storage_path);

    let outcome = apply_episodic_schema(
        db.as_ref(),
        DIMS,
        &episodic_options(&config, EPISODIC_MAX_KNOWN),
        Some(proof.as_ref()),
    )
    .await
    .expect("the episodic fold reaches its maximum");

    assert_eq!(EPISODIC_MAX_KNOWN, 8, "the constant moved WITH the leg");
    assert_eq!(outcome.version, 8);
    assert_eq!(user_version(db.as_ref()).await, 8);
    assert_sweep_state_table(db.as_ref()).await;

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_episodic_v8_is_a_no_op_data_migration() {
    let config = store_config("sweep-stamp-episodic-noop");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    let proof = exclusive_root(&config.storage_path);

    // Build a genuine v7 file: a build that knows only 7 runs no v8 leg.
    let v7 = apply_episodic_schema(
        db.as_ref(),
        DIMS,
        &episodic_options(&config, 7),
        Some(proof.as_ref()),
    )
    .await
    .expect("build a v7 file");
    assert_eq!(v7.version, 7);
    let before = columns(db.as_ref(), "agent_sessions").await;

    let outcome = apply_episodic_schema(
        db.as_ref(),
        DIMS,
        &episodic_options(&config, EPISODIC_MAX_KNOWN),
        Some(proof.as_ref()),
    )
    .await
    .expect("v7 -> v8");

    assert_eq!(outcome.version, 8);
    assert_eq!(
        columns(db.as_ref(), "agent_sessions").await,
        before,
        "the v8 leg changes no agent_sessions column"
    );
    assert_sweep_state_table(db.as_ref()).await;

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_episodic_v8_stamp_is_refused_by_a_seven_only_build() {
    let config = store_config("sweep-stamp-episodic-old");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    let proof = exclusive_root(&config.storage_path);
    apply_episodic_schema(
        db.as_ref(),
        DIMS,
        &episodic_options(&config, EPISODIC_MAX_KNOWN),
        Some(proof.as_ref()),
    )
    .await
    .expect("stamp v8");

    let refused = apply_episodic_schema(
        db.as_ref(),
        DIMS,
        &episodic_options(&config, 7),
        Some(proof.as_ref()),
    )
    .await
    .expect_err("a seven-only build refuses a v8 file");
    match refused {
        StoreError::SchemaNewerThanBuild {
            found, supported, ..
        } => {
            assert_eq!((found, supported), (8, 7));
        }
        other => panic!("expected SchemaNewerThanBuild, got {other:?}"),
    }

    drop(db);
    common::cleanup_temp_dir(&config.storage_path).expect("cleanup");
}
