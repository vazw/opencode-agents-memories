//! Issue 176 — the v4→v5 prompt-identity backfill.
//!
//! Every fixture is a GENUINE v4 `user-prompts.db` (built through
//! `apply_user_prompts_schema` with `max_known = 4`), then seeded with rows
//! whose `user_id` cannot exist yet. The resolver answers per PATH, so a run
//! that stamps the launch identity for everything cannot pass.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mem_bridge::project_scope::{ProjectScopeError, ResolvedProjectScope};
use mem_bridge::prompt_identity_backfill::{
    PromptIdentityBackfillReport, backfill_event_detail, maybe_backfill_prompt_identities,
};
use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_core::config::{Config, ConfigPaths};
use mem_profile::profile_store::{ProfileConfig, ProfileData, ProfileStore};
use mem_profile::prompt::{USER_PROMPTS_DB_NAME, UserPromptManager};
use mem_store::db::{
    ConnectFactory as _, ConnectOptions, Db, Value, query_one, turso_connect_factory,
};
use mem_store::platform::Platform;
use mem_store::schema_migrations::{MigrationOptions, apply_user_prompts_schema};

const TAG: &str = "opencode_project_0123456789abcdef";
const HASH: &str = "0123456789abcdef";
const MAX_RETRIES: u32 = 3;
const A: &str = "a@x";
const B: &str = "b@y";

/// Per-path identities: `/a` → `a@x`, `/b` → `b@y`, anything else unresolvable.
struct PathResolver;

impl ProjectScopeResolver for PathResolver {
    fn resolve(&self, project_id: &str) -> Result<ResolvedProjectScope, ProjectScopeError> {
        let email = match project_id {
            "/a" => Some(A),
            "/b" => Some(B),
            _ => {
                return Err(ProjectScopeError::Core(
                    "no identity for this path".to_string(),
                ));
            }
        };
        let mut scope = mem_bridge::project_scope::apply_resolved_scope(project_id, TAG, HASH)?;
        scope.user_email = email.map(str::to_string);
        Ok(scope)
    }
}

fn load_config(home: &Path) -> Config {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create the config dir");
    std::fs::write(dir.join("opencode-mem.json"), "{}").expect("write the config");
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.display().to_string())]
        .into_iter()
        .collect();
    Config::load(&ConfigPaths::standard(home), &env).expect("the config loads")
}

fn manager(root: &Path) -> UserPromptManager {
    UserPromptManager::new(
        root.to_path_buf(),
        common::store_config(root.to_path_buf()),
        MAX_RETRIES,
    )
}

/// Builds a genuine v4 file (v1..v4 through the real applier) and seeds one
/// unanalyzed row per `(id, project_path)`.
async fn seed_v4_prompts(root: &Path, rows: &[(&str, &str)]) {
    let config = common::store_config(root.to_path_buf());
    let db_path = root.join(USER_PROMPTS_DB_NAME);
    let opts = ConnectOptions::for_config(&config, Platform::current());
    let db = turso_connect_factory()
        .connect(db_path.clone(), opts)
        .await
        .expect("open the prompts file");
    let version = apply_user_prompts_schema(
        db.as_ref(),
        &MigrationOptions {
            db_path: Some(db_path),
            label: USER_PROMPTS_DB_NAME,
            max_known: 4,
        },
        None,
    )
    .await
    .expect("a fresh file converges to v4");
    assert_eq!(version, 4, "the fixture is a v4 file");
    for (id, project_path) in rows {
        db.execute(
            "INSERT INTO user_prompts
                (id, session_id, message_id, project_path, content, created_at, captured, user_learning_captured)
             VALUES (?, ?, ?, ?, ?, 1, 0, 0)",
            vec![
                Value::Text((*id).to_string()),
                Value::Text("session-backfill".to_string()),
                Value::Text((*id).to_string()),
                Value::Text((*project_path).to_string()),
                Value::Text(format!("prompt {id}")),
            ],
        )
        .await
        .expect("seed the v4 row");
    }
    drop(db);
}

async fn row_user_id(db: &dyn Db, id: &str) -> Option<String> {
    query_one(
        db,
        "SELECT user_id FROM user_prompts WHERE id = ?",
        vec![Value::Text(id.to_string())],
    )
    .await
    .expect("read user_id")
    .expect("the row exists")
    .text("user_id")
}

fn profile_store(root: &Path) -> ProfileStore {
    ProfileStore::new(
        root.to_path_buf(),
        common::store_config(root.to_path_buf()),
        ProfileConfig::default(),
    )
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_backfill_skips_a_fresh_root_without_creating_the_db() {
    let dir = common::TestDir::new("backfill-fresh-root");
    let home = common::TestDir::new("backfill-fresh-root-home");
    let config = load_config(home.path());
    let prompts = manager(dir.path());

    let report = maybe_backfill_prompt_identities(dir.path(), &prompts, &config, &PathResolver)
        .await
        .expect("a fresh root is not an error");

    assert_eq!(report, None, "nothing ran, nothing was reported");
    assert!(
        !dir.path().join(USER_PROMPTS_DB_NAME).exists(),
        "the gate must never create user-prompts.db"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_backfill_stamps_each_paths_own_identity() {
    let dir = common::TestDir::new("backfill-per-path");
    let home = common::TestDir::new("backfill-per-path-home");
    let config = load_config(home.path());
    assert!(
        config.user_email_override.is_none(),
        "each stamp must come from the per-path resolver, not an override"
    );
    seed_v4_prompts(
        dir.path(),
        &[
            ("prompt-a", "/a"),
            ("prompt-b", "/b"),
            ("prompt-dead", "/dead"),
        ],
    )
    .await;
    let prompts = manager(dir.path());

    let report = maybe_backfill_prompt_identities(dir.path(), &prompts, &config, &PathResolver)
        .await
        .expect("backfill runs")
        .expect("the file existed, so a report comes back");

    assert_eq!(report.stamped, 2, "one row per resolvable path");
    assert_eq!(report.remaining_null, 1, "the dead path's row stays NULL");
    // Checked in this order on purpose: stamping the FIRST path's identity for
    // every row makes this assertion (0) the one that reds.
    assert_eq!(
        prompts
            .get_prompts_for_user_learning(10, B)
            .await
            .expect("select b@y")
            .len(),
        1,
        "the second identity's OWN row is stamped with it"
    );
    assert_eq!(
        prompts
            .get_prompts_for_user_learning(10, A)
            .await
            .expect("select a@x")
            .len(),
        1,
        "the first identity's row is not double-stamped"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_backfill_leaves_unresolvable_rows_null_and_reports_the_count() {
    let dir = common::TestDir::new("backfill-dead-path");
    let home = common::TestDir::new("backfill-dead-path-home");
    let config = load_config(home.path());
    seed_v4_prompts(dir.path(), &[("prompt-a", "/a"), ("prompt-dead", "/dead")]).await;
    let prompts = manager(dir.path());

    let report = maybe_backfill_prompt_identities(dir.path(), &prompts, &config, &PathResolver)
        .await
        .expect("backfill runs")
        .expect("report");

    assert_eq!(report.remaining_null, 1);
    let db = prompts.ready().await.expect("prompts ready");
    assert_eq!(
        row_user_id(db.as_ref(), "prompt-dead").await,
        None,
        "an unresolvable path is never defaulted to the launch identity"
    );
    assert!(
        prompts
            .get_prompt_by_id("prompt-dead")
            .await
            .expect("read the dead-path row")
            .is_some(),
        "the row stays: backfill never deletes"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_backfill_does_not_touch_profiles() {
    let dir = common::TestDir::new("backfill-profiles");
    let home = common::TestDir::new("backfill-profiles-home");
    let config = load_config(home.path());
    seed_v4_prompts(dir.path(), &[("prompt-a", "/a"), ("prompt-b", "/b")]).await;

    let store = profile_store(dir.path());
    store
        .create_profile(A, "A", "a", A, &ProfileData::default(), 1)
        .await
        .expect("seed a profile");
    let before = snapshot_profiles(&store).await;

    let prompts = manager(dir.path());
    maybe_backfill_prompt_identities(dir.path(), &prompts, &config, &PathResolver)
        .await
        .expect("backfill runs")
        .expect("report");

    assert_eq!(
        snapshot_profiles(&store).await,
        before,
        "the backfill writes no profile row and reads none"
    );
}

/// `(id, profile_data)` for every profile, ordered — the byte-level snapshot.
async fn snapshot_profiles(store: &ProfileStore) -> Vec<(String, String)> {
    let db = store.ready().await.expect("profile store ready");
    db.query(
        "SELECT id, profile_data FROM user_profiles ORDER BY id",
        Vec::new(),
    )
    .await
    .expect("snapshot profiles")
    .iter()
    .map(|row| {
        (
            row.text("id").unwrap_or_default(),
            row.text("profile_data").unwrap_or_default(),
        )
    })
    .collect()
}

#[test]
fn test_backfill_event_detail_is_counts_only() {
    let detail = backfill_event_detail(&PromptIdentityBackfillReport {
        stamped: 2,
        remaining_null: 1,
    });
    let keys: BTreeSet<&str> = detail
        .as_object()
        .expect("a JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["stamped", "remainingNull"]),
        "counts only: no path, no email, no prompt text"
    );
    assert_eq!(detail["stamped"], 2);
    assert_eq!(detail["remainingNull"], 1);
}
