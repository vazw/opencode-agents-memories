//! Issue 48 — the B2′ watermark matrix and `delete_prompt`'s failure arms.
//!
//! 47.1-B2′: a prompt delete must invalidate every profile that could have
//! been derived from the deleted prompt. The rule is `created_at <=
//! last_analyzed_at` (and an unset/zero watermark always contributes), so the
//! erase direction is conservative: it may over-erase, never under-erase.
//!
//! Every case drives the public seam (`UserPromptManager::delete_prompt`)
//! against a real `ProfileStore`/`UserPromptManager` pair over one temp root;
//! only the eraser is swapped where the failure arms need one.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use mem_profile::erasure::{EraseReport, ProfileEraser, PromptDeletion, could_have_contributed};
use mem_profile::error::ProfileError;
use mem_profile::profile_store::{ProfileConfig, ProfileData, ProfileItem, ProfileStore};
use mem_profile::prompt::{USER_PROMPTS_DB_NAME, UserPromptManager};
use mem_store::db::{BoxFuture, ConnectFactory as _, ConnectOptions, Value};
use mem_store::platform::Platform;

/// The user identity every seeded profile belongs to.
const USER_ID: &str = "watermark@example.com";

#[test]
fn test_could_have_contributed_treats_zero_and_negative_watermarks_as_contributing() {
    // Watermark boundary: <= contributes, > does not.
    assert!(
        could_have_contributed(10, 10),
        "equal timestamps contribute"
    );
    assert!(could_have_contributed(10, 9), "older prompts contribute");
    assert!(
        !could_have_contributed(10, 11),
        "a strictly newer prompt cannot have contributed"
    );

    // An unset (0) or negative watermark fails toward erasure.
    assert!(could_have_contributed(0, 1), "a zero watermark contributes");
    assert!(
        could_have_contributed(-1, 1),
        "a negative watermark contributes"
    );
    assert!(
        could_have_contributed(0, i64::MAX),
        "an unset watermark contributes under every timestamp"
    );
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn fixture() -> (PathBuf, Arc<ProfileStore>, UserPromptManager) {
    let root = common::temp_dir("erasure-watermark");
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        common::store_config(root.clone()),
        ProfileConfig::default(),
    ));
    let manager = UserPromptManager::new(root.clone(), common::store_config(root.clone()), 3);
    (root, store, manager)
}

fn profile_data() -> ProfileData {
    ProfileData {
        preferences: vec![ProfileItem {
            description: "probe preference".to_string(),
            ..ProfileItem::default()
        }],
        ..ProfileData::default()
    }
}

async fn seed_profile(store: &ProfileStore) -> String {
    store
        .create_profile(USER_ID, "Probe", "probe", USER_ID, &profile_data(), 1)
        .await
        .expect("seed the profile")
}

async fn seed_prompt(manager: &UserPromptManager) -> String {
    manager
        .save_prompt("session-48", "message-48", "/probe", "probe prompt")
        .await
        .expect("seed the prompt")
}

/// Sets the watermark directly (the fixture clock is not slept on).
async fn set_watermark(store: &ProfileStore, watermark: i64) {
    let db = store.ready().await.expect("profile store ready");
    db.execute(
        "UPDATE user_profiles SET last_analyzed_at = ?",
        vec![Value::Integer(watermark)],
    )
    .await
    .expect("set the watermark");
}

async fn set_inactive(store: &ProfileStore) {
    let db = store.ready().await.expect("profile store ready");
    db.execute("UPDATE user_profiles SET is_active = 0", Vec::new())
        .await
        .expect("deactivate the profile");
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

// ---------------------------------------------------------------------------
// Step 4 — the watermark matrix's first three arms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_prompt_erases_profile_when_prompt_predates_watermark() {
    let (_root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    let profile_id = seed_profile(&store).await;
    let created_at = manager
        .get_prompt_by_id(&prompt_id)
        .await
        .expect("read the prompt")
        .expect("the prompt exists")
        .created_at;
    set_watermark(&store, created_at + 1_000).await;

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
            } if report.profiles_deleted == 1 && report.changelogs_deleted == 1
        ),
        "the contributing profile and its changelog are erased: {outcome:?}"
    );
    assert_eq!(count_rows(&store, "user_profiles").await, 0);
    assert_eq!(count_rows(&store, "user_profile_changelogs").await, 0);
    assert!(
        store
            .get_profile_changelogs(&profile_id, 10)
            .await
            .expect("read the changelogs")
            .is_empty()
    );
}

#[tokio::test]
async fn test_delete_prompt_keeps_profile_when_prompt_postdates_watermark() {
    let (_root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    seed_profile(&store).await;
    let created_at = manager
        .get_prompt_by_id(&prompt_id)
        .await
        .expect("read the prompt")
        .expect("the prompt exists")
        .created_at;
    // The prompt is strictly newer than the watermark => it cannot have
    // contributed, so the profile is retained.
    set_watermark(&store, created_at - 1).await;

    let eraser: Arc<dyn ProfileEraser> = store.clone();
    let manager = manager.with_eraser(eraser);
    let outcome = manager
        .delete_prompt(&prompt_id)
        .await
        .expect("delete the prompt");

    assert!(
        matches!(
            outcome,
            PromptDeletion::Deleted {
                erasure: Some(report)
            } if report.profiles_deleted == 0
        ),
        "nothing is erased for a non-contributing prompt: {outcome:?}"
    );
    assert_eq!(
        count_rows(&store, "user_profiles").await,
        1,
        "the retained profile row survives"
    );
    assert_eq!(
        count_rows(&store, "user_profile_changelogs").await,
        1,
        "the retained profile's changelog survives"
    );
    assert!(
        manager
            .get_prompt_by_id(&prompt_id)
            .await
            .expect("read the prompt")
            .is_none(),
        "the prompt itself is still deleted"
    );
}

#[tokio::test]
async fn test_delete_prompt_reports_absent_for_unknown_prompt() {
    let (_root, store, manager) = fixture();
    seed_profile(&store).await;

    let eraser: Arc<dyn ProfileEraser> = store.clone();
    let outcome = manager
        .with_eraser(eraser)
        .delete_prompt("prompt_does_not_exist")
        .await
        .expect("an absent prompt is not an error");

    assert_eq!(outcome, PromptDeletion::Absent);
    assert_eq!(
        count_rows(&store, "user_profiles").await,
        1,
        "an absent prompt has no event to invalidate, so nothing is erased"
    );
}

// ---------------------------------------------------------------------------
// Step 9 — the remaining matrix arms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_prompt_erases_profile_at_equal_watermark_boundary() {
    let (_root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    seed_profile(&store).await;
    let created_at = manager
        .get_prompt_by_id(&prompt_id)
        .await
        .expect("read the prompt")
        .expect("the prompt exists")
        .created_at;
    set_watermark(&store, created_at).await;

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
        "the rule is <=, so an equal watermark is erased: {outcome:?}"
    );
    assert_eq!(count_rows(&store, "user_profiles").await, 0);
}

#[tokio::test]
async fn test_delete_prompt_erases_profile_with_zero_watermark() {
    let (_root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    seed_profile(&store).await;
    set_watermark(&store, 0).await;

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
        "an unset watermark fails toward erasure: {outcome:?}"
    );
    assert_eq!(count_rows(&store, "user_profiles").await, 0);
}

#[tokio::test]
async fn test_delete_prompt_erases_inactive_profile_that_could_have_contributed() {
    let (_root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    seed_profile(&store).await;
    set_inactive(&store).await;

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
        "an inactive profile still holds derived text, so it is erased: {outcome:?}"
    );
    assert_eq!(count_rows(&store, "user_profiles").await, 0);
}

#[tokio::test]
async fn test_delete_prompt_returns_deleted_without_erasure_when_no_eraser_is_configured() {
    let (_root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    seed_profile(&store).await;

    let outcome = manager
        .delete_prompt(&prompt_id)
        .await
        .expect("delete the prompt without an eraser");

    assert_eq!(
        outcome,
        PromptDeletion::Deleted { erasure: None },
        "a manager built without an eraser reports the absence, never silence"
    );
    assert!(
        manager
            .get_prompt_by_id(&prompt_id)
            .await
            .expect("read the prompt")
            .is_none(),
        "the prompt is deleted"
    );
    assert_eq!(
        count_rows(&store, "user_profiles").await,
        1,
        "no eraser means no erasure (test/embedded construction only)"
    );
}

// ---------------------------------------------------------------------------
// Step 5 — the failure arms (E2: erase first, delete second)
// ---------------------------------------------------------------------------

/// Fails every erase: the prompt must survive so a retry re-fires the event.
struct FailingEraser;

impl ProfileEraser for FailingEraser {
    fn erase_derived_from<'a>(
        &'a self,
        _prompt_created_at: i64,
    ) -> BoxFuture<'a, Result<EraseReport, ProfileError>> {
        Box::pin(async { Err(ProfileError::Save("boom".to_string())) })
    }
}

#[tokio::test]
async fn test_profile_erase_failure_keeps_the_prompt() {
    let (_root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    seed_profile(&store).await;

    let manager = manager.with_eraser(Arc::new(FailingEraser));
    let outcome = manager.delete_prompt(&prompt_id).await;

    match outcome {
        Err(ProfileError::ErasureFailed {
            prompt_id: failed_id,
            ..
        }) => assert_eq!(failed_id, prompt_id, "the error names the prompt"),
        other => panic!("expected ErasureFailed, got {other:?}"),
    }
    assert!(
        manager
            .get_prompt_by_id(&prompt_id)
            .await
            .expect("read the prompt")
            .is_some(),
        "E2: the source row must survive a failed erase so a retry can fire"
    );
}

/// Performs the **real** erase, then sabotages the manager's own prompt DELETE
/// by dropping `user_prompts` from a second connection to the live database.
struct SabotagingEraser {
    profiles: Arc<ProfileStore>,
    prompts_db: PathBuf,
}

impl ProfileEraser for SabotagingEraser {
    fn erase_derived_from<'a>(
        &'a self,
        prompt_created_at: i64,
    ) -> BoxFuture<'a, Result<EraseReport, ProfileError>> {
        Box::pin(async move {
            // 1. the REAL erase runs first, so the reported count is measured.
            let report = self
                .profiles
                .erase_profiles_derived_from(prompt_created_at)
                .await?;
            // 2. then the peer sabotage: a second connection to the live
            //    user-prompts.db drops the table, so the manager's own
            //    DELETE (step 4 of delete_prompt) must fail.
            let Some(root) = self.prompts_db.parent() else {
                return Err(ProfileError::Save("prompts db has no parent".into()));
            };
            let mut config = common::store_config(root.to_path_buf());
            config.database_encryption_enabled = false;
            let options = ConnectOptions::for_config(&config, Platform::current());
            let db = mem_store::db::turso_connect_factory()
                .connect(self.prompts_db.clone(), options)
                .await?;
            db.execute("PRAGMA busy_timeout = 5000", Vec::new()).await?;
            db.execute("DROP TABLE user_prompts", Vec::new()).await?;
            drop(db);
            // 3. the store's own report, not a literal.
            Ok(report)
        })
    }
}

#[tokio::test]
async fn test_prompt_delete_failure_after_erase_reports_a_partial_outcome() {
    let (root, store, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;
    seed_profile(&store).await;

    let manager = manager.with_eraser(Arc::new(SabotagingEraser {
        profiles: Arc::clone(&store),
        prompts_db: root.join(USER_PROMPTS_DB_NAME),
    }));
    let outcome = manager.delete_prompt(&prompt_id).await;

    // (1) exactly one acceptable outcome: the typed partial result.
    match outcome {
        Err(ProfileError::PromptDeleteAfterErasure {
            prompt_id: failed_id,
            profiles_erased,
            ..
        }) => {
            assert_eq!(failed_id, prompt_id, "the error names the prompt");
            assert_eq!(
                profiles_erased, 1,
                "the count is the real erase's report, not a literal"
            );
        }
        other => panic!("expected PromptDeleteAfterErasure, got {other:?}"),
    }

    // (2) the real profile store is empty — non-vacuous because the profile IS
    // seeded and the eraser DOES delete it.
    for table in ["user_profiles", "user_profile_changelogs"] {
        assert_eq!(
            count_rows(&store, table).await,
            0,
            "{table} must be empty after the real erase"
        );
    }
}
