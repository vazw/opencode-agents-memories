//! Port of upstream `tests/user-profile-learning-error.test.ts` (W3.2).
//!
//! Upstream runs a mocked subprocess and asserts the propagated message; the
//! extra cases here pin the W3.2 provider-failure contract (one outcome,
//! verbatim messages, zero writes/marks, retryable state, on-disk prompt
//! state) that the upstream error test only implies.
//!
//! Tests:
//! - `test_provider_error_message_is_preserved_without_external_config`
//! - `test_provider_error_does_not_write_profile_or_mark_prompts`
//! - `test_external_failure_propagates_external_provider_message`
//! - `test_provider_error_releases_lock_and_flag_for_retry`
//! - `test_provider_error_keeps_real_prompts_unmarked_in_db`

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use mem_profile::ProfileError;
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, ProfileWriter,
    PromptLedger, PromptLedgerAdapter,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::{UserPrompt, UserPromptManager};
use mem_store::db::BoxFuture;

const NATIVE_MESSAGE: &str =
    "opencode-mem: opencode reported APIError: Thinking mode does not support this tool_choice";
const EXTERNAL_MESSAGE: &str = "external provider transport exploded";

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

fn prompts(count: usize) -> Vec<UserPrompt> {
    (0..count)
        .map(|index| UserPrompt {
            id: format!("prompt-{index}"),
            session_id: "session-1".to_string(),
            message_id: format!("msg-{index}"),
            project_path: Some("/workspace".to_string()),
            content: format!("Implement feature {index}"),
            created_at: index as i64 + 1,
            captured: false,
            user_learning_captured: false,
            linked_memory_id: None,
            capture_attempts: 0,
            provider_id: None,
            model_id: None,
        })
        .collect()
}

/// Ledger mock with a mark-call counter.
struct CountingLedger {
    prompts: Vec<UserPrompt>,
    marks: Arc<AtomicUsize>,
}

impl PromptLedger for CountingLedger {
    fn count_unanalyzed(&self) -> BoxFuture<'_, Result<i64, ProfileError>> {
        let count = self.prompts.len() as i64;
        Box::pin(async move { Ok(count) })
    }

    fn get_prompts_for_learning(
        &self,
        _limit: i64,
    ) -> BoxFuture<'_, Result<Vec<UserPrompt>, ProfileError>> {
        let prompts = self.prompts.clone();
        Box::pin(async move { Ok(prompts) })
    }

    fn mark_many(&self, _ids: Vec<String>) -> BoxFuture<'_, Result<(), ProfileError>> {
        self.marks.fetch_add(1, Ordering::AcqRel);
        Box::pin(async move { Ok(()) })
    }
}

/// Profile-writer mock with write counters.
struct CountingProfiles {
    writes: Arc<AtomicUsize>,
}

impl ProfileWriter for CountingProfiles {
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
        self.writes.fetch_add(1, Ordering::AcqRel);
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
        self.writes.fetch_add(1, Ordering::AcqRel);
        Box::pin(async move { Ok("profile_created".to_string()) })
    }
}

struct FailingGateway {
    fail: Arc<AtomicBool>,
    external_configured: bool,
    external_calls: Arc<AtomicUsize>,
}

impl AnalysisGateway for FailingGateway {
    fn native_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        let fails = self.fail.load(Ordering::Acquire);
        Box::pin(async move {
            if fails {
                Err(AnalysisError::Provider(NATIVE_MESSAGE.to_string()))
            } else {
                Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string())
            }
        })
    }

    fn external_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        self.external_calls.fetch_add(1, Ordering::AcqRel);
        let fails = self.fail.load(Ordering::Acquire);
        Box::pin(async move {
            if fails {
                Err(AnalysisError::Provider(EXTERNAL_MESSAGE.to_string()))
            } else {
                Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string())
            }
        })
    }

    fn external_configured(&self) -> bool {
        self.external_configured
    }
}

fn build_round(
    dir: &std::path::Path,
    ledger: Arc<dyn PromptLedger>,
    profiles: Arc<dyn ProfileWriter>,
    gateway: Arc<FailingGateway>,
) -> (Arc<LearningLock>, LearningRound) {
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    )));
    let round = LearningRound::new(
        ledger,
        profiles,
        gateway,
        lock.clone(),
        Arc::new(|| true),
        5,
        "test@example.com",
    );
    (lock, round)
}

#[tokio::test]
async fn test_provider_error_message_is_preserved_without_external_config() {
    let dir = TempDir::new("learning-error-native");
    let fail = Arc::new(AtomicBool::new(true));
    let (_lock, round) = build_round(
        dir.path(),
        Arc::new(CountingLedger {
            prompts: prompts(5),
            marks: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(CountingProfiles {
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(FailingGateway {
            fail: fail.clone(),
            external_configured: false,
            external_calls: Arc::new(AtomicUsize::new(0)),
        }),
    );

    let error = round.run(0).await.expect_err("provider failure propagates");
    let message = error.to_string();
    assert!(
        message.contains("Thinking mode does not support"),
        "{message}"
    );
    assert!(
        !message.contains("External API not configured"),
        "{message}"
    );
}

#[tokio::test]
async fn test_provider_error_does_not_write_profile_or_mark_prompts() {
    let dir = TempDir::new("learning-error-no-writes");
    let marks = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let external_calls = Arc::new(AtomicUsize::new(0));
    let (_lock, round) = build_round(
        dir.path(),
        Arc::new(CountingLedger {
            prompts: prompts(5),
            marks: marks.clone(),
        }),
        Arc::new(CountingProfiles {
            writes: writes.clone(),
        }),
        Arc::new(FailingGateway {
            fail: Arc::new(AtomicBool::new(true)),
            external_configured: false,
            external_calls: external_calls.clone(),
        }),
    );

    round.run(0).await.expect_err("provider failure propagates");
    assert_eq!(writes.load(Ordering::Acquire), 0, "no profile write");
    assert_eq!(marks.load(Ordering::Acquire), 0, "no mark_many call");
    assert_eq!(
        external_calls.load(Ordering::Acquire),
        0,
        "an unconfigured external provider is never called"
    );
}

#[tokio::test]
async fn test_external_failure_propagates_external_provider_message() {
    let dir = TempDir::new("learning-error-external");
    let external_calls = Arc::new(AtomicUsize::new(0));
    let (_lock, round) = build_round(
        dir.path(),
        Arc::new(CountingLedger {
            prompts: prompts(5),
            marks: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(CountingProfiles {
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(FailingGateway {
            fail: Arc::new(AtomicBool::new(true)),
            external_configured: true,
            external_calls: external_calls.clone(),
        }),
    );

    let error = round.run(0).await.expect_err("double failure propagates");
    // The provider actually attempted last owns the message; no prefix, no
    // synthesis.
    assert_eq!(error.to_string(), EXTERNAL_MESSAGE);
    assert_eq!(external_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn test_provider_error_releases_lock_and_flag_for_retry() {
    let dir = TempDir::new("learning-error-retry");
    let fail = Arc::new(AtomicBool::new(true));
    let marks = Arc::new(AtomicUsize::new(0));
    let (lock, round) = build_round(
        dir.path(),
        Arc::new(CountingLedger {
            prompts: prompts(5),
            marks: marks.clone(),
        }),
        Arc::new(CountingProfiles {
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(FailingGateway {
            fail: fail.clone(),
            external_configured: false,
            external_calls: Arc::new(AtomicUsize::new(0)),
        }),
    );

    round.run(0).await.expect_err("provider failure propagates");
    assert!(
        !lock.is_held().await,
        "the cross-process lock must be released on the error path"
    );

    // A healed provider completes on the very same round instance: the
    // in-process flag was reset too.
    fail.store(false, Ordering::Release);
    let outcome = round.run(0).await.expect("the retry completes");
    assert_eq!(outcome, LearningOutcome::Completed { prompts: 5 });
    assert_eq!(marks.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn test_provider_error_keeps_real_prompts_unmarked_in_db() {
    let dir = TempDir::new("learning-error-db");
    let storage = dir.path().to_path_buf();
    let manager = Arc::new(UserPromptManager::new(
        storage.clone(),
        common::store_config(storage.clone()),
        3,
    ));
    for index in 0..5 {
        manager
            .save_prompt(
                "session-1",
                &format!("msg-{index}"),
                "/workspace",
                &format!("Implement feature {index}"),
            )
            .await
            .expect("save prompt");
    }
    assert_eq!(
        manager
            .count_unanalyzed_for_user_learning()
            .await
            .expect("count unanalyzed"),
        5
    );

    let fail = Arc::new(AtomicBool::new(true));
    let (lock, round) = build_round(
        dir.path(),
        Arc::new(PromptLedgerAdapter::new(manager.clone())),
        Arc::new(CountingProfiles {
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(FailingGateway {
            fail: fail.clone(),
            external_configured: false,
            external_calls: Arc::new(AtomicUsize::new(0)),
        }),
    );

    round.run(0).await.expect_err("provider failure propagates");
    assert_eq!(
        manager
            .count_unanalyzed_for_user_learning()
            .await
            .expect("count unanalyzed"),
        5,
        "a failed round never marks: the prompts stay retryable"
    );
    assert!(!lock.is_held().await);

    fail.store(false, Ordering::Release);
    let outcome = round.run(0).await.expect("the retry completes");
    assert_eq!(outcome, LearningOutcome::Completed { prompts: 5 });
    assert_eq!(
        manager
            .count_unanalyzed_for_user_learning()
            .await
            .expect("count unanalyzed"),
        0,
        "a successful retry marks the batch"
    );
}
