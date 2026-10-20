//! New coverage for the round-side incremental cleanup wiring (W3.3).
//!
//! Behavioural origin: upstream `user-memory-learning.ts` L286-288 (the hook
//! runs after the batch is marked) and R11 (a cleanup fault never turns a
//! completed round into a failure).
//!
//! Tests:
//! - `test_round_triggers_incremental_cleanup_when_the_interval_boundary_is_crossed`
//! - `test_round_skips_incremental_cleanup_when_the_interval_is_not_crossed`
//! - `test_round_skips_incremental_cleanup_when_disabled`
//! - `test_cleanup_failure_does_not_fail_the_round_and_prompts_stay_marked`

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mem_profile::ProfileError;
use mem_profile::incremental_cleanup::{CleanupOutcome, IncrementalCleanup};
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, ProfileWriter,
    PromptLedger, PromptLedgerAdapter, RoundEventSink,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::{UserPrompt, UserPromptManager};
use mem_store::db::BoxFuture;

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

/// A profile writer whose active profile already analyzed `total` prompts —
/// what the cleanup interval gate is computed from.
struct ExistingProfileWriter {
    total_prompts_analyzed: i64,
    writes: Arc<AtomicUsize>,
}

impl ProfileWriter for ExistingProfileWriter {
    fn get_active_profile<'a>(
        &'a self,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Profile>, ProfileError>> {
        let total = self.total_prompts_analyzed;
        let user_id = user_id.to_string();
        Box::pin(async move {
            Ok(Some(Profile {
                id: "profile-1".to_string(),
                user_id,
                display_name: "Unknown".to_string(),
                user_name: "unknown".to_string(),
                user_email: "test@example.com".to_string(),
                profile_data: r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string(),
                version: 1,
                created_at: 1,
                last_analyzed_at: 1,
                total_prompts_analyzed: total,
                is_active: true,
            }))
        })
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
        Box::pin(async move { Ok("profile-created".to_string()) })
    }
}

struct EmptyAnalysisGateway;

impl AnalysisGateway for EmptyAnalysisGateway {
    fn native_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(
            async move { Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()) },
        )
    }

    fn external_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(
            async move { Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()) },
        )
    }
}

#[derive(Clone, Debug)]
struct CleanupCall {
    previous_prompt_count: i64,
    added_prompt_count: i64,
    now: i64,
}

struct SpyCleanup {
    calls: Arc<Mutex<Vec<CleanupCall>>>,
    result: Result<CleanupOutcome, ProfileError>,
}

impl IncrementalCleanup for SpyCleanup {
    fn run_incremental<'a>(
        &'a self,
        _user_id: &'a str,
        _landed: &'a ProfileUpdates,
        previous_prompt_count: i64,
        added_prompt_count: i64,
        now: i64,
    ) -> BoxFuture<'a, Result<CleanupOutcome, ProfileError>> {
        self.calls.lock().expect("call log").push(CleanupCall {
            previous_prompt_count,
            added_prompt_count,
            now,
        });
        let result = self.result.clone();
        Box::pin(async move { result })
    }
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl RoundEventSink for RecordingSink {
    fn emit(&self, event: &str, detail: serde_json::Value) {
        self.events
            .lock()
            .expect("event log")
            .push((event.to_string(), detail));
    }
}

fn build_round(
    dir: &std::path::Path,
    ledger: Arc<dyn PromptLedger>,
    profiles: Arc<dyn ProfileWriter>,
    cleanup: Arc<dyn IncrementalCleanup>,
    enabled: bool,
    interval: f64,
    events: Arc<dyn RoundEventSink>,
) -> LearningRound {
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    )));
    LearningRound::new(
        ledger,
        profiles,
        Arc::new(EmptyAnalysisGateway),
        lock,
        Arc::new(|| true),
        5,
        "test@example.com",
    )
    .with_event_sink(events)
    .with_incremental_cleanup(cleanup, enabled, interval)
}

fn cleanup_ok() -> Result<CleanupOutcome, ProfileError> {
    Ok(CleanupOutcome {
        merged: 1,
        removed: 0,
        updated: true,
    })
}

#[tokio::test]
async fn test_round_triggers_incremental_cleanup_when_the_interval_boundary_is_crossed() {
    let dir = TempDir::new("round-cleanup-crossed");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(RecordingSink::default());
    let round = build_round(
        dir.path(),
        Arc::new(CountingLedger {
            prompts: prompts(5),
            marks: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(ExistingProfileWriter {
            total_prompts_analyzed: 95,
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(SpyCleanup {
            calls: calls.clone(),
            result: cleanup_ok(),
        }),
        true,
        100.0,
        events.clone(),
    );

    let outcome = round.run(4_242).await.expect("round completes");

    assert_eq!(outcome, LearningOutcome::Completed { prompts: 5 });
    let calls = calls.lock().expect("call log");
    assert_eq!(calls.len(), 1, "95 + 5 crosses the 100-prompt boundary");
    assert_eq!(calls[0].previous_prompt_count, 95);
    assert_eq!(calls[0].added_prompt_count, 5);
    assert_eq!(
        calls[0].now, 4_242,
        "the round's injected clock is passed on"
    );

    let events = events.events.lock().expect("event log");
    assert!(
        events.iter().any(|(event, detail)| {
            event == "cleanup"
                && detail["message"] == "user-profile-learning: automatic cleanup complete"
                && detail["merged"] == 1
                && detail["updated"] == true
        }),
        "the completion event carries the counts: {events:?}"
    );
}

#[tokio::test]
async fn test_round_skips_incremental_cleanup_when_the_interval_is_not_crossed() {
    let dir = TempDir::new("round-cleanup-not-crossed");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let round = build_round(
        dir.path(),
        Arc::new(CountingLedger {
            prompts: prompts(5),
            marks: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(ExistingProfileWriter {
            total_prompts_analyzed: 100,
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(SpyCleanup {
            calls: calls.clone(),
            result: cleanup_ok(),
        }),
        true,
        100.0,
        Arc::new(RecordingSink::default()),
    );

    let outcome = round.run(4_242).await.expect("round completes");

    assert_eq!(outcome, LearningOutcome::Completed { prompts: 5 });
    assert!(
        calls.lock().expect("call log").is_empty(),
        "100 + 5 stays inside the same 100-prompt interval"
    );
}

#[tokio::test]
async fn test_round_skips_incremental_cleanup_when_disabled() {
    let dir = TempDir::new("round-cleanup-disabled");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let round = build_round(
        dir.path(),
        Arc::new(CountingLedger {
            prompts: prompts(5),
            marks: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(ExistingProfileWriter {
            total_prompts_analyzed: 95,
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(SpyCleanup {
            calls: calls.clone(),
            result: cleanup_ok(),
        }),
        false,
        100.0,
        Arc::new(RecordingSink::default()),
    );

    let outcome = round.run(4_242).await.expect("round completes");

    assert_eq!(outcome, LearningOutcome::Completed { prompts: 5 });
    assert!(
        calls.lock().expect("call log").is_empty(),
        "the disabled gate wins even across a boundary"
    );
}

#[tokio::test]
async fn test_cleanup_failure_does_not_fail_the_round_and_prompts_stay_marked() {
    let dir = TempDir::new("round-cleanup-failure");
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

    let events = Arc::new(RecordingSink::default());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let round = build_round(
        dir.path(),
        Arc::new(PromptLedgerAdapter::new(manager.clone())),
        Arc::new(ExistingProfileWriter {
            total_prompts_analyzed: 95,
            writes: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(SpyCleanup {
            calls: calls.clone(),
            result: Err(ProfileError::Analysis("cleanup exploded".to_string())),
        }),
        true,
        100.0,
        events.clone(),
    );

    let outcome = round.run(4_242).await.expect("the round still completes");

    assert_eq!(outcome, LearningOutcome::Completed { prompts: 5 });
    assert_eq!(calls.lock().expect("call log").len(), 1);

    assert_eq!(
        manager
            .count_unanalyzed_for_user_learning()
            .await
            .expect("count unanalyzed"),
        0,
        "the prompts were marked before the hook ran and stay marked"
    );

    let events = events.events.lock().expect("event log");
    assert!(
        events.iter().any(|(event, detail)| {
            event == "log"
                && detail["message"] == "user-profile-learning: automatic cleanup failed"
                && detail["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("cleanup exploded"))
        }),
        "the cleanup fault is logged through the round's sink: {events:?}"
    );
}
