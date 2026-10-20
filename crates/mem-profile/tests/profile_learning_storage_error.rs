//! Port of upstream `tests/profile-learning-storage-error.test.ts` (W3.1).
//!
//! The child runs the REAL learning service and the REAL coordination lock;
//! only the profile-store / prompt-ledger / provider boundaries are mocked,
//! with fault injection (`profile-learning-storage-error-worker.mjs` parity).
//!
//! Upstream cases (traced 1:1 by name, 4 total):
//! - "a storage/merge fault does not trigger the external fallback, does not
//!   update, does not mark, and propagates"
//! - "corrupt stored profile_data propagates without fallback"
//! - "a native provider fault falls back to the external provider and completes"
//! - "the lock is reacquirable in the same process after a propagated storage
//!   error and the second run completes"
//!
//! Event counters: `{ nativeCalls, externalCalls, updates, marks, error,
//! reacquired, secondRunError, secondRunMarks }`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use mem_profile::ProfileError;
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, NoopEventSink,
    ProfileWriter, PromptLedger,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::UserPrompt;
use mem_store::db::BoxFuture;

use common::{read_child_report, spawn_fixture_child, wait_bounded, write_child_report};

const CHILD_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        Self(common::temp_dir(prefix))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.0);
    }
}

/// Spawns the child for one scenario and returns its report.
fn run_storage_error_child(dir: &Path, scenario: &str) -> serde_json::Value {
    let config_path = dir.join(format!("worker-{scenario}.json"));
    let config = serde_json::json!({
        "storagePath": dir.display().to_string(),
        "scenario": scenario,
        "promptCount": 3,
        "threshold": 2,
    });
    std::fs::write(
        &config_path,
        serde_json::to_vec(&config).expect("serialize config"),
    )
    .expect("write config");

    let out = dir.join(format!("storage-error-{scenario}.out"));
    let mut env: Vec<(String, String)> = common::isolated_env(dir);
    env.push((
        "MEM_FIXTURE_CFG".to_string(),
        config_path.display().to_string(),
    ));
    let refs: Vec<(&str, &str)> = env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let mut child = spawn_fixture_child(&format!("storage-error:{scenario}"), &out, &refs)
        .expect("spawn storage-error child");
    let status = wait_bounded(&mut child, CHILD_TIMEOUT).expect("child within the deadline");
    assert!(
        status.success(),
        "child exited with {status:?}; stderr: {}",
        std::fs::read_to_string(out.with_extension("stderr")).unwrap_or_default()
    );
    read_child_report(&out)
}

// ---------------------------------------------------------------------------
// 1/4 — merge fault: no fallback, no update, no mark, error propagates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_storage_merge_error_no_external_fallback_no_update_no_mark() {
    let dir = TempDir::new("storage-error-merge");
    let report = run_storage_error_child(dir.path(), "merge-error");

    assert_eq!(
        report["error"],
        serde_json::Value::Bool(true),
        "the storage error must propagate: {report}"
    );
    assert!(
        report["errorMessage"]
            .as_str()
            .unwrap_or_default()
            .contains("storage merge failed"),
        "the original storage sentence must survive: {report}"
    );
    assert_eq!(
        report["externalCalls"],
        serde_json::json!(0),
        "a storage fault must NEVER trigger the external provider fallback: {report}"
    );
    assert_eq!(report["updates"], serde_json::json!(0));
    assert_eq!(report["marks"], serde_json::json!(0));

    // Positive proof the lock was actually taken before the fault.
    assert!(
        report["firstHadLock"].as_bool().unwrap_or(false),
        "the round must have acquired the lock before failing: {report}"
    );
}

// ---------------------------------------------------------------------------
// 2/4 — corrupt stored profile_data propagates without fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_corrupt_stored_profile_data_propagates_without_fallback() {
    let dir = TempDir::new("storage-error-corrupt");
    let report = run_storage_error_child(dir.path(), "corrupt-profile");

    assert_eq!(report["error"], serde_json::Value::Bool(true));
    assert_eq!(
        report["externalCalls"],
        serde_json::json!(0),
        "a corrupt stored profile must never trigger the external fallback: {report}"
    );
    assert_eq!(report["updates"], serde_json::json!(0));
    assert_eq!(report["marks"], serde_json::json!(0));
}

// ---------------------------------------------------------------------------
// 3/4 — native provider fault falls back to external and completes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_native_provider_failure_falls_back_to_external_and_completes() {
    let dir = TempDir::new("storage-error-fallback");
    let report = run_storage_error_child(dir.path(), "native-error");

    assert_eq!(report["error"], serde_json::Value::Bool(false), "{report}");
    assert_eq!(report["nativeCalls"], serde_json::json!(1));
    assert_eq!(report["externalCalls"], serde_json::json!(1));
    assert_eq!(report["updates"], serde_json::json!(1));
    assert_eq!(report["marks"], serde_json::json!(1));
}

// ---------------------------------------------------------------------------
// 4/4 — lock reacquirable after a propagated storage error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lock_reacquirable_after_propagated_storage_error_same_process_rerun() {
    let dir = TempDir::new("storage-error-rerun");
    let report = run_storage_error_child(dir.path(), "merge-error-rerun");

    // First run failed on the merge...
    assert_eq!(report["error"], serde_json::Value::Bool(true), "{report}");
    // ...and the SAME round instance then completed, which proves both the
    // in-process flag and the cross-process lock were released on the error
    // path (a leaked row would make the second run report "contention").
    assert_eq!(
        report["reacquired"],
        serde_json::Value::Bool(true),
        "{report}"
    );
    assert_eq!(report["secondRunError"], serde_json::Value::Bool(false));
    assert_eq!(report["secondRunMarks"], serde_json::json!(1));
    assert_eq!(report["updates"], serde_json::json!(1));
}

// ---------------------------------------------------------------------------
// Child entry
// ---------------------------------------------------------------------------

struct FaultLedger {
    prompt_count: i64,
    marks: Arc<AtomicUsize>,
}

impl PromptLedger for FaultLedger {
    fn count_unanalyzed(&self) -> BoxFuture<'_, Result<i64, ProfileError>> {
        let count = self.prompt_count;
        Box::pin(async move { Ok(count) })
    }

    fn get_prompts_for_learning(
        &self,
        _limit: i64,
    ) -> BoxFuture<'_, Result<Vec<UserPrompt>, ProfileError>> {
        let count = self.prompt_count.min(5);
        Box::pin(async move {
            Ok((0..count)
                .map(|index| UserPrompt {
                    id: format!("prompt_{index}"),
                    session_id: "session".to_string(),
                    message_id: format!("message_{index}"),
                    project_path: Some("/project".to_string()),
                    content: format!("observation {index}"),
                    created_at: index,
                    captured: true,
                    user_learning_captured: false,
                    linked_memory_id: None,
                    capture_attempts: 0,
                    provider_id: None,
                    model_id: None,
                })
                .collect())
        })
    }

    fn mark_many(&self, _ids: Vec<String>) -> BoxFuture<'_, Result<(), ProfileError>> {
        // Counter = mark CALLS (upstream `countEv("mark")`), not id count.
        self.marks.fetch_add(1, Ordering::AcqRel);
        Box::pin(async move { Ok(()) })
    }
}

struct FaultProfileWriter {
    merge_error: Arc<AtomicBool>,
    corrupt_profile: Arc<AtomicBool>,
    updates: Arc<AtomicUsize>,
    lock_probe: Arc<LearningLock>,
    held_during_merge: Arc<AtomicBool>,
}

impl ProfileWriter for FaultProfileWriter {
    fn get_active_profile<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Profile>, ProfileError>> {
        let corrupt = self.corrupt_profile.load(Ordering::Acquire);
        Box::pin(async move {
            Ok(Some(Profile {
                id: "profile_existing".to_string(),
                user_id: "test@example.com".to_string(),
                display_name: "Unknown".to_string(),
                user_name: "unknown".to_string(),
                user_email: "test@example.com".to_string(),
                profile_data: if corrupt {
                    "{not valid json".to_string()
                } else {
                    r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()
                },
                version: 1,
                created_at: 0,
                last_analyzed_at: 0,
                total_prompts_analyzed: 0,
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
        let failing = self.merge_error.load(Ordering::Acquire);
        let probe = self.lock_probe.clone();
        let held_during_merge = self.held_during_merge.clone();
        Box::pin(async move {
            if failing {
                // Positive-invariant proof: the coordination row is observed
                // HELD from outside the guard, in the exact mutation window
                // that is about to fail.
                held_during_merge.store(probe.is_held().await, Ordering::Release);
                Err(ProfileError::Save("storage merge failed".to_string()))
            } else {
                Ok(existing.clone())
            }
        })
    }

    fn update_profile<'a>(
        &'a self,
        _profile_id: &'a str,
        _data: &'a ProfileData,
        _prompts_analyzed: i64,
        _change_summary: &'a str,
    ) -> BoxFuture<'a, Result<bool, ProfileError>> {
        self.updates.fetch_add(1, Ordering::AcqRel);
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
        self.updates.fetch_add(1, Ordering::AcqRel);
        Box::pin(async move { Ok("profile_created".to_string()) })
    }
}

struct FaultGateway {
    native_fails: bool,
    native_calls: Arc<AtomicUsize>,
    external_calls: Arc<AtomicUsize>,
}

impl AnalysisGateway for FaultGateway {
    fn native_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        self.native_calls.fetch_add(1, Ordering::AcqRel);
        let fails = self.native_fails;
        Box::pin(async move {
            if fails {
                Err(AnalysisError::Provider("provider rejected".to_string()))
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
        Box::pin(
            async move { Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()) },
        )
    }
}

/// Unreachable-by-default child entry; harmless no-op when the mode env is
/// unset.
#[test]
#[ignore = "fixture subprocess entry — driven only via self re-exec"]
fn fixture_child_entry() {
    let Some(mode) = std::env::var_os(common::FIXTURE_CHILD_ENV) else {
        return;
    };
    let mode = mode.to_string_lossy().to_string();
    let Some(out) = std::env::var_os(common::FIXTURE_OUT_ENV) else {
        return;
    };
    let out = PathBuf::from(out);
    let Some(scenario) = mode.strip_prefix("storage-error:") else {
        return;
    };
    let config_path = std::env::var("MEM_FIXTURE_CFG").expect("MEM_FIXTURE_CFG");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
            .expect("config JSON");
    let dir = PathBuf::from(config["storagePath"].as_str().expect("storagePath"));
    let prompt_count = config["promptCount"].as_i64().unwrap_or(3);
    let threshold = config["threshold"].as_i64().unwrap_or(2);

    let native_calls = Arc::new(AtomicUsize::new(0));
    let external_calls = Arc::new(AtomicUsize::new(0));
    let updates = Arc::new(AtomicUsize::new(0));
    let marks = Arc::new(AtomicUsize::new(0));
    let merge_error = Arc::new(AtomicBool::new(matches!(
        scenario,
        "merge-error" | "merge-error-rerun"
    )));
    let corrupt_profile = Arc::new(AtomicBool::new(scenario == "corrupt-profile"));
    let native_fails = scenario == "native-error";
    let held_during_merge = Arc::new(AtomicBool::new(false));

    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.clone(),
        common::store_config(dir.clone()),
    )));
    let round = LearningRound::new(
        Arc::new(FaultLedger {
            prompt_count,
            marks: marks.clone(),
        }),
        Arc::new(FaultProfileWriter {
            merge_error: merge_error.clone(),
            corrupt_profile: corrupt_profile.clone(),
            updates: updates.clone(),
            lock_probe: lock.clone(),
            held_during_merge: held_during_merge.clone(),
        }),
        Arc::new(FaultGateway {
            native_fails,
            native_calls: native_calls.clone(),
            external_calls: external_calls.clone(),
        }),
        lock,
        Arc::new(|| true),
        threshold,
        "test@example.com",
    )
    .with_event_sink(Arc::new(NoopEventSink));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    let report = runtime.block_on(async move {
        let first = round.run(0).await;
        let first_had_lock = held_during_merge.load(Ordering::Acquire);
        let first_error = first.is_err();
        let first_message = first
            .as_ref()
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();

        let mut reacquired = false;
        let mut second_run_error = false;
        let mut second_run_marks = 0usize;
        if scenario == "merge-error-rerun" {
            // Heal the store, then rerun the SAME round instance: a leaked
            // lock row or a stuck in-process flag would surface as contention
            // or as "already running".
            merge_error.store(false, Ordering::Release);
            let marks_before = marks.load(Ordering::Acquire);
            match round.run(0).await {
                Ok(LearningOutcome::Completed { .. }) => {
                    reacquired = true;
                    second_run_marks = marks.load(Ordering::Acquire) - marks_before;
                }
                Ok(_) => reacquired = false,
                Err(_) => second_run_error = true,
            }
        }

        serde_json::json!({
            "scenario": scenario,
            "firstHadLock": first_had_lock,
            "error": first_error,
            "errorMessage": first_message,
            "nativeCalls": native_calls.load(Ordering::Acquire),
            "externalCalls": external_calls.load(Ordering::Acquire),
            "updates": updates.load(Ordering::Acquire),
            "marks": marks.load(Ordering::Acquire),
            "reacquired": reacquired,
            "secondRunError": second_run_error,
            "secondRunMarks": second_run_marks,
        })
    });

    write_child_report(&out, &report);
}
