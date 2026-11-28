//! Issue 174 — the round's select and mark are scoped to ONE identity.
//!
//! The real `UserPromptManager`, `PromptLedgerAdapter`, `LearningLock` and
//! `LearningRound` run; only the provider and profile-writer boundaries are
//! spies. The fixture puts B's row FIRST in `created_at`, so an unfiltered
//! `ORDER BY created_at ASC LIMIT 1` selects B and the round built for A would
//! mark B — exactly the defect the `user_id = ?` predicate prevents.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mem_profile::ProfileError;
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, ProfileWriter,
    PromptLedgerAdapter,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::UserPromptManager;
use mem_store::db::BoxFuture;
use mem_store::db::{Value, query_one};

const MAX_RETRIES: u32 = 3;
const ROUND_IDENTITY: &str = "a@x";

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        Self(common::temp_dir(prefix))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.0);
    }
}

struct NoopProfiles;

impl ProfileWriter for NoopProfiles {
    fn get_active_profile<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Profile>, ProfileError>> {
        Box::pin(async move { Ok(None) })
    }

    fn merge_profile_data<'a>(
        &'a self,
        existing: &'a ProfileData,
        _updates: &'a ProfileUpdates,
        _profile_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<ProfileData, ProfileError>> {
        Box::pin(async move { Ok(existing.clone()) })
    }

    fn update_profile<'a>(
        &'a self,
        _profile_id: &'a str,
        _data: &'a ProfileData,
        _prompts_analyzed: i64,
        _change_summary: &'a str,
    ) -> BoxFuture<'a, Result<bool, ProfileError>> {
        Box::pin(async move { Ok(true) })
    }

    fn create_profile<'a>(
        &'a self,
        _user_id: &'a str,
        _display_name: &'a str,
        _user_name: &'a str,
        _user_email: &'a str,
        _data: &'a ProfileData,
        _prompts_analyzed: i64,
    ) -> BoxFuture<'a, Result<String, ProfileError>> {
        Box::pin(async move { Ok("profile-created".to_string()) })
    }
}

struct EmptyGateway {
    calls: Arc<AtomicUsize>,
}

impl AnalysisGateway for EmptyGateway {
    fn native_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()) })
    }

    fn external_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(async { Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()) })
    }
}

/// Pins a row's `created_at` through a direct handle on the SAME manager, so
/// the select's ordering is deterministic instead of clock-dependent.
async fn pin_created_at(manager: &UserPromptManager, prompt_id: &str, created_at: i64) {
    let db = manager.ready().await.expect("ready applies v5");
    db.execute(
        "UPDATE user_prompts SET created_at = ? WHERE id = ?",
        vec![
            Value::Integer(created_at),
            Value::Text(prompt_id.to_string()),
        ],
    )
    .await
    .expect("pin created_at");
}

async fn user_learning_captured(manager: &UserPromptManager, prompt_id: &str) -> i64 {
    let db = manager.ready().await.expect("ready");
    query_one(
        db.as_ref(),
        "SELECT user_learning_captured AS captured FROM user_prompts WHERE id = ?",
        vec![Value::Text(prompt_id.to_string())],
    )
    .await
    .expect("read captured")
    .expect("the row exists")
    .i64("captured")
    .unwrap_or_default()
}

fn build_round(dir: &Path, manager: Arc<UserPromptManager>) -> LearningRound {
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    )));
    LearningRound::new(
        Arc::new(PromptLedgerAdapter::new(manager)),
        Arc::new(NoopProfiles),
        Arc::new(EmptyGateway {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        lock,
        Arc::new(|| true),
        1,
        ROUND_IDENTITY,
    )
}

#[tokio::test]
async fn test_select_and_mark_are_scoped_to_one_identity() {
    let dir = TempDir::new("prompt-identity-round");
    let manager = Arc::new(UserPromptManager::new(
        dir.path().to_path_buf(),
        common::store_config(dir.path().to_path_buf()),
        MAX_RETRIES,
    ));

    // B is OLDER (created_at = 1) and belongs to another identity; A is the
    // round's own row (created_at = 2). Both are unanalyzed.
    let b = manager
        .save_prompt(
            "session-b",
            "message-b",
            "/workspace",
            "B turn",
            Some("b@y"),
        )
        .await
        .expect("save B");
    let a = manager
        .save_prompt(
            "session-a",
            "message-a",
            "/workspace",
            "A turn",
            Some(ROUND_IDENTITY),
        )
        .await
        .expect("save A");
    assert_ne!(a, b);
    pin_created_at(&manager, &b, 1).await;
    pin_created_at(&manager, &a, 2).await;

    let round = build_round(dir.path(), manager.clone());
    let outcome = round.run(0).await.expect("the round runs");

    assert_eq!(
        outcome,
        LearningOutcome::Completed { prompts: 1 },
        "the round analyzes exactly one prompt: its own identity's"
    );
    assert_eq!(
        user_learning_captured(&manager, &a).await,
        1,
        "the round's own row is marked"
    );
    assert_eq!(
        user_learning_captured(&manager, &b).await,
        0,
        "the older other-identity row is neither selected nor marked"
    );
}
