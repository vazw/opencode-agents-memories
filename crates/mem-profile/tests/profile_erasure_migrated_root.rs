//! Issue 48 (48.9) — the migrated-root population, where A2 is not a no-op.
//!
//! The audit left live roots that **already** carry `user-profiles.db` and
//! `user-prompts.db`. Two properties matter for them:
//!
//! 1. a profile written by a previous install is erased by the prompt path;
//! 2. a root created before `user_profile_changelogs` declared its
//!    `ON DELETE CASCADE` (or any root at all — `CREATE TABLE IF NOT EXISTS`
//!    can never add the FK) still loses its changelog snapshots, because the
//!    explicit two-table delete is the contract, not the declared cascade.
//!
//! `delete_old_prompts` (retention pruning) stays untouched in this wave: its
//! deferral to issue 39 is pinned by the third test.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use mem_profile::erasure::{ProfileEraser, PromptDeletion};
use mem_profile::ids::now_millis;
use mem_profile::profile_store::{
    ProfileConfig, ProfileData, ProfileItem, ProfileStore, USER_PROFILES_DB_NAME,
};
use mem_profile::prompt::UserPromptManager;
use mem_store::db::{ConnectFactory as _, ConnectOptions, Value};
use mem_store::platform::Platform;

/// The marker that proves the changelog snapshot's payload existed before the
/// erase (the snapshot is a full copy of the profile text).
const MARKER: &str = "CANARY-48-MIGRATED-5e7c2d41";

const PROFILE_ID: &str = "profile_migrated_48";
const USER_ID: &str = "migrated@example.com";

/// The pre-FK `user_profiles` shape (`mem_store::sql::USER_PROFILES_DDL`).
const PRE_FK_USER_PROFILES_DDL: &str = r#"
          CREATE TABLE IF NOT EXISTS user_profiles (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            user_name TEXT NOT NULL,
            user_email TEXT NOT NULL,
            profile_data TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            last_analyzed_at INTEGER NOT NULL,
            total_prompts_analyzed INTEGER NOT NULL DEFAULT 0,
            is_active BOOLEAN NOT NULL DEFAULT 1
          )
        "#;

/// The pre-FK `user_profile_changelogs` shape: same columns as
/// `mem_store::sql::USER_PROFILE_CHANGELOGS_DDL`, with the
/// `FOREIGN KEY … ON DELETE CASCADE` clause **omitted** (a migrated root).
const PRE_FK_USER_PROFILE_CHANGELOGS_DDL: &str = r#"
          CREATE TABLE IF NOT EXISTS user_profile_changelogs (
            id TEXT PRIMARY KEY,
            profile_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            change_type TEXT NOT NULL,
            change_summary TEXT NOT NULL,
            profile_data_snapshot TEXT NOT NULL,
            created_at INTEGER NOT NULL
          )
        "#;

fn seeded_root() -> PathBuf {
    common::temp_dir("erasure-migrated")
}

fn profile_data() -> ProfileData {
    ProfileData {
        preferences: vec![ProfileItem {
            description: MARKER.to_string(),
            ..ProfileItem::default()
        }],
        ..ProfileData::default()
    }
}

async fn count_rows(store: &ProfileStore, table: &str) -> i64 {
    let db = store.ready().await.expect("profile store ready");
    mem_store::db::query_one(
        db.as_ref(),
        &format!("SELECT COUNT(*) AS count FROM {table}"),
        Vec::new(),
    )
    .await
    .expect("count the rows")
    .and_then(|row| row.i64("count"))
    .unwrap_or(-1)
}

#[tokio::test]
async fn test_migrated_root_without_fk_cascade_erases_changelog_rows() {
    let root = seeded_root();
    let config = common::store_config(root.clone());

    // Phase 1: a peer connection writes the PRE-FK schema and one profile +
    // one changelog row by raw DDL, shaped exactly like `with_coordination_db`
    // (a direct factory open, never the caching ConnectionManager).
    let options = ConnectOptions::for_config(&config, Platform::current());
    let db = mem_store::db::turso_connect_factory()
        .connect(root.join(USER_PROFILES_DB_NAME), options)
        .await
        .expect("open the migrated profile db");
    db.execute("PRAGMA busy_timeout = 5000", Vec::new())
        .await
        .expect("busy timeout");
    db.execute(PRE_FK_USER_PROFILES_DDL, Vec::new())
        .await
        .expect("create the pre-FK user_profiles table");
    db.execute(PRE_FK_USER_PROFILE_CHANGELOGS_DDL, Vec::new())
        .await
        .expect("create the pre-FK changelogs table");

    let now = now_millis();
    db.execute(
        "
      INSERT INTO user_profiles (
        id, user_id, display_name, user_name, user_email,
        profile_data, version, created_at, last_analyzed_at,
        total_prompts_analyzed, is_active
      ) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, 4, 1)
    ",
        vec![
            Value::Text(PROFILE_ID.to_string()),
            Value::Text(USER_ID.to_string()),
            Value::Text("Migrated".to_string()),
            Value::Text("migrated".to_string()),
            Value::Text(USER_ID.to_string()),
            Value::Text(format!(
                r#"{{"preferences":[{{"description":"{MARKER}"}}],"patterns":[],"workflows":[]}}"#
            )),
            Value::Integer(now),
            Value::Integer(now),
        ],
    )
    .await
    .expect("seed the migrated profile row");
    db.execute(
        "
      INSERT INTO user_profile_changelogs (
        id, profile_id, version, change_type, change_summary,
        profile_data_snapshot, created_at
      ) VALUES (?, ?, 1, 'create', 'Initial profile creation', ?, ?)
    ",
        vec![
            Value::Text("changelog_migrated_48".to_string()),
            Value::Text(PROFILE_ID.to_string()),
            Value::Text(format!(
                r#"{{"preferences":[{{"description":"{MARKER}"}}],"patterns":[],"workflows":[]}}"#
            )),
            Value::Integer(now),
        ],
    )
    .await
    .expect("seed the migrated changelog row");
    drop(db);

    // Phase 2: the real store over the already-existing tables. Its
    // `CREATE TABLE IF NOT EXISTS` DDL is a no-op — the migrated condition —
    // and the seeded row must read back through the store's own accessors
    // (otherwise the literal DDL above has drifted from the store's reads).
    let store = ProfileStore::new(root.clone(), config, ProfileConfig::default());
    let seeded = store
        .get_active_profile(USER_ID)
        .await
        .expect("read the seeded profile")
        .expect("the seeded profile row exists");
    assert_eq!(seeded.id, PROFILE_ID);
    assert_eq!(seeded.last_analyzed_at, now);
    assert_eq!(
        store
            .get_profile_changelogs(PROFILE_ID, 10)
            .await
            .expect("read the seeded changelogs")
            .len(),
        1,
        "the seeded changelog snapshot exists before the erase"
    );

    let erased = store
        .delete_profile(PROFILE_ID)
        .await
        .expect("erase the migrated profile");

    assert_eq!(erased.profiles_deleted, 1, "the profile row is deleted");
    assert_eq!(
        erased.changelogs_deleted, 1,
        "the changelog snapshot is deleted explicitly (no FK cascade exists here)"
    );

    let db = store.ready().await.expect("profile store ready");
    let profiles = mem_store::db::query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM user_profiles",
        Vec::new(),
    )
    .await
    .expect("count user_profiles");
    assert_eq!(
        profiles.and_then(|row| row.i64("count")),
        Some(0),
        "no profile row survives"
    );
    let changelogs = mem_store::db::query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM user_profile_changelogs",
        Vec::new(),
    )
    .await
    .expect("count user_profile_changelogs");
    assert_eq!(
        changelogs.and_then(|row| row.i64("count")),
        Some(0),
        "no changelog snapshot (which carries the marker) survives"
    );
}

/// 48.9 — a profile written by a *previous install* over the same root is
/// erased by the (fresh) prompt path. Both phases are same-process peer
/// handles over one root, so they read each other's WAL: no checkpoint is
/// needed here (the canary checkpoints only because it reads raw file bytes).
#[tokio::test]
async fn test_migrated_root_profile_is_erased_by_the_prompt_path() {
    let root = seeded_root();
    let config = common::store_config(root.clone());

    // Phase 1 — the "previous install": prompt first, then the profile, so the
    // profile's watermark (= now) is >= the prompt's created_at.
    let prompt_id = {
        let store = ProfileStore::new(root.clone(), config.clone(), ProfileConfig::default());
        let manager = UserPromptManager::new(root.clone(), config.clone(), 3);
        let prompt_id = manager
            .save_prompt("session-48", "message-48", "/probe", "migrated prompt")
            .await
            .expect("seed the prompt");
        store
            .create_profile(USER_ID, "Migrated", "migrated", USER_ID, &profile_data(), 1)
            .await
            .expect("seed the profile");
        prompt_id
    };

    // Phase 2 — fresh store + manager over the same root; the prompt delete
    // must invalidate the pre-existing profile.
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        config.clone(),
        ProfileConfig::default(),
    ));
    let manager = UserPromptManager::new(root.clone(), config, 3);
    let eraser: Arc<dyn ProfileEraser> = store.clone();
    let outcome = manager
        .with_eraser(eraser)
        .delete_prompt(&prompt_id)
        .await
        .expect("delete the prompt");

    assert!(
        matches!(
            outcome,
            PromptDeletion::Deleted {
                erasure: Some(report)
            } if report.profiles_deleted == 1
        ),
        "the migrated profile is erased by the prompt path: {outcome:?}"
    );
    assert_eq!(count_rows(&store, "user_profiles").await, 0);
    assert_eq!(count_rows(&store, "user_profile_changelogs").await, 0);
}

/// Retention pruning is deliberately deferred to **issue 39**, which must cite
/// 47.1-B2′ and decide whether pruning joins the invalidation. This is the one
/// assertion that stops the deferral reversing silently: `delete_old_prompts`
/// removes prompts and leaves the profile (and its changelogs) intact — even
/// with an eraser wired into the manager.
#[tokio::test]
async fn test_delete_old_prompts_does_not_erase_the_profile() {
    let root = seeded_root();
    let config = common::store_config(root.clone());
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        config.clone(),
        ProfileConfig::default(),
    ));
    let manager = UserPromptManager::new(root.clone(), config, 3);
    let prompt_id = manager
        .save_prompt("session-48", "message-48", "/probe", "old prompt")
        .await
        .expect("seed the prompt");
    let profile_id = store
        .create_profile(USER_ID, "Migrated", "migrated", USER_ID, &profile_data(), 1)
        .await
        .expect("seed the profile");

    let eraser: Arc<dyn ProfileEraser> = store.clone();
    let manager = manager.with_eraser(eraser);
    let removed = manager
        .delete_old_prompts(now_millis() + 1_000)
        .await
        .expect("prune the old prompts");

    assert_eq!(removed, 1, "the pruning path removed the prompt");
    assert!(
        manager
            .get_prompt_by_id(&prompt_id)
            .await
            .expect("read the prompt")
            .is_none(),
        "the prompt is gone"
    );
    assert_eq!(
        count_rows(&store, "user_profiles").await,
        1,
        "pruning does not erase the profile (deferred to issue 39)"
    );
    assert_eq!(
        count_rows(&store, "user_profile_changelogs").await,
        1,
        "pruning does not erase the changelog snapshots"
    );
    assert_eq!(
        store
            .get_active_profile(USER_ID)
            .await
            .expect("read the profile")
            .map(|profile| profile.id),
        Some(profile_id),
        "the seeded profile is still the active one"
    );
}
