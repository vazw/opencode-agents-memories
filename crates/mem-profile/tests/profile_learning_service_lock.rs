//! Port of upstream `tests/profile-learning-service-lock.test.ts` (W3.1).
//!
//! Both children run the REAL learning service and the REAL cross-process
//! coordination lock; only the provider / prompt / profile boundaries are
//! mocked (the `PromptLedger` / `ProfileWriter` / `AnalysisGateway` traits).
//! The winner blocks inside the LLM call on a parent-created handshake file,
//! so the lock is provably held across a real await — no fixed sleep keeps it
//! alive.
//!
//! Upstream cases (traced 1:1 by name, 8 total):
//! - "serializes two real learning runs: loser touches no SELECT/LLM/write/mark"
//! - "releases the lock on the below-threshold early return"
//! - "releases the lock when the prompt batch comes back empty"
//! - "releases the lock when the provider is not ready"
//! - "propagates a provider LLM error and the lock is acquirable again"
//! - "runs once when the same process calls concurrently (flag set before first await)"
//! - "recovers the in-process flag after a failed run and succeeds on retry"
//! - "ordinary memory writes complete while profile learning holds its lock"
//!
//! Positive-invariant mandate (mem_1792345954123): while the winner is parked
//! inside `llm-entered`, the PARENT opens the coordination DB and asserts the
//! owner row EXISTS; after the winner exits the row is GONE. The loser-side
//! negatives only appear in the same test as those positive proofs.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mem_profile::ProfileError;
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, ProfileWriter,
    PromptLedger, RoundEventSink,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::UserPrompt;
use mem_store::db::BoxFuture;

use common::{
    current_owner_token, read_child_report, spawn_fixture_child, wait_bounded, write_child_report,
};

const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);
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

struct Learner {
    child: std::process::Child,
    out: PathBuf,
    events_file: PathBuf,
}

impl Learner {
    fn spawn(dir: &Path, name: &str, config: serde_json::Value) -> Self {
        let events_file = dir.join(format!("events-{name}.ndjson"));
        let config_path = dir.join(format!("worker-{name}.json"));
        let mut config = config;
        config["storagePath"] = serde_json::json!(dir.display().to_string());
        config["eventsFile"] = serde_json::json!(events_file.display().to_string());
        std::fs::write(
            &config_path,
            serde_json::to_vec(&config).expect("serialize worker config"),
        )
        .expect("write worker config");

        let out = dir.join(format!("learner-{name}.out"));
        let mut env: Vec<(String, String)> = common::isolated_env(dir);
        env.push((
            "MEM_FIXTURE_CFG".to_string(),
            config_path.display().to_string(),
        ));
        let refs: Vec<(&str, &str)> = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let scenario = config["scenario"].as_str().unwrap_or("full");
        let child = spawn_fixture_child(&format!("learn:{scenario}"), &out, &refs)
            .expect("spawn learner child");
        Self {
            child,
            out,
            events_file,
        }
    }

    fn events(&self) -> Vec<serde_json::Value> {
        common::read_events(&self.events_file)
    }

    fn count(&self, ev: &str) -> usize {
        self.events()
            .iter()
            .filter(|event| event["ev"] == serde_json::json!(ev))
            .count()
    }

    fn event_names(&self) -> Vec<String> {
        self.events()
            .iter()
            .filter_map(|event| event["ev"].as_str().map(str::to_string))
            .collect()
    }

    fn wait_for_event(&self, ev: &str) -> bool {
        let started = Instant::now();
        while started.elapsed() < BARRIER_TIMEOUT {
            if self.count(ev) > 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn report(&mut self) -> serde_json::Value {
        let status = wait_bounded(&mut self.child, CHILD_TIMEOUT)
            .unwrap_or_else(|error| panic!("learner child {error:?}"));
        assert!(
            status.success(),
            "learner exited with {status:?}; stderr: {}",
            std::fs::read_to_string(self.out.with_extension("stderr")).unwrap_or_default()
        );
        read_child_report(&self.out)
    }
}

fn count_events(events: &[serde_json::Value], ev: &str) -> usize {
    events
        .iter()
        .filter(|event| event["ev"] == serde_json::json!(ev))
        .count()
}

fn has_log(events: &[serde_json::Value], needle: &str) -> bool {
    events.iter().any(|event| {
        event["ev"] == serde_json::json!("log")
            && event["data"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(needle))
    })
}

fn release_winner(dir: &Path) {
    std::fs::write(dir.join("release"), b"go").expect("write release file");
}

// ---------------------------------------------------------------------------
// 1/8 — two real processes serialize; loser touches nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_two_processes_serialize_loser_touches_no_select_llm_write_mark() {
    let dir = TempDir::new("service-lock-2p");
    let mut winner = Learner::spawn(
        dir.path(),
        "winner",
        serde_json::json!({
            "scenario": "full",
            "blockOnLlm": true,
            "releaseFile": dir.path().join("release").display().to_string(),
            "promptCount": 3,
            "threshold": 2,
        }),
    );
    assert!(
        winner.wait_for_event("llm-entered"),
        "the winner must reach the LLM (which only happens under the lock)"
    );

    // POSITIVE INVARIANT: the mutation window really holds the lock. The
    // parent opens the coordination DB and sees the owner row while the
    // winner is parked inside the LLM.
    let held_token = current_owner_token(dir.path()).await;
    assert!(
        held_token.is_some(),
        "the coordination row must exist while the winner is parked in the LLM"
    );

    let mut loser = Learner::spawn(
        dir.path(),
        "loser",
        serde_json::json!({
            "scenario": "full",
            "promptCount": 3,
            "threshold": 2,
        }),
    );
    let loser_report = loser.report();
    let loser_events = loser.events();

    release_winner(dir.path());
    let winner_report = winner.report();
    let winner_events = winner.events();

    assert_eq!(
        loser_report["outcome"],
        serde_json::json!("contention"),
        "{loser_report}"
    );
    assert_eq!(winner_report["outcome"], serde_json::json!("completed"));

    // The loser bounced off the lock — never into the pipeline.
    assert!(
        has_log(&loser_events, "another process holds"),
        "loser events: {loser_events:?}"
    );
    assert_eq!(count_events(&loser_events, "count"), 0);
    assert_eq!(count_events(&loser_events, "select"), 0);
    assert_eq!(count_events(&loser_events, "llm-entered"), 0);
    assert_eq!(count_events(&loser_events, "profile-write"), 0);
    assert_eq!(count_events(&loser_events, "mark"), 0);

    // Winner event ordering: acquired precedes count precedes llm-entered
    // precedes mark (the claim path really acquires before doing work).
    let names = winner.event_names();
    let position = |event: &str| {
        names
            .iter()
            .position(|name| name == event)
            .unwrap_or_else(|| panic!("missing {event} in {names:?}"))
    };
    assert!(position("acquired") < position("count"), "{names:?}");
    assert!(position("count") < position("llm-entered"), "{names:?}");
    assert!(position("llm-entered") < position("mark"), "{names:?}");

    // Across both processes the pipeline ran exactly once end-to-end.
    let all: Vec<serde_json::Value> = winner_events
        .iter()
        .chain(loser_events.iter())
        .cloned()
        .collect();
    assert_eq!(count_events(&all, "llm-entered"), 1);
    assert_eq!(count_events(&all, "llm-exit"), 1);
    assert_eq!(count_events(&all, "profile-write"), 1);
    assert_eq!(count_events(&all, "mark"), 1);

    // POSITIVE INVARIANT: released after exit.
    assert_eq!(
        current_owner_token(dir.path()).await,
        None,
        "the winner must release the row on exit"
    );
}

// ---------------------------------------------------------------------------
// 2/8 — below-threshold early return releases the lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lock_released_on_below_threshold_early_return() {
    let dir = TempDir::new("service-lock-threshold");
    let mut first = Learner::spawn(
        dir.path(),
        "solo",
        serde_json::json!({"scenario": "early", "threshold": 100, "promptCount": 10}),
    );
    let report = first.report();
    let events = first.events();

    assert_eq!(report["outcome"], serde_json::json!("below-threshold"));
    assert_eq!(count_events(&events, "count"), 1);
    assert_eq!(count_events(&events, "llm-entered"), 0);
    assert_eq!(current_owner_token(dir.path()).await, None);

    // Same storage, fresh process: the lock is acquirable (released, not leaked).
    let mut second = Learner::spawn(
        dir.path(),
        "second",
        serde_json::json!({"scenario": "full", "promptCount": 3, "threshold": 2}),
    );
    assert!(second.wait_for_event("llm-entered"));
    release_winner(dir.path());
    let report = second.report();
    let events = second.events();
    assert_eq!(report["outcome"], serde_json::json!("completed"));
    assert!(!has_log(&events, "another process holds"));
    assert_eq!(count_events(&events, "count"), 1);
    assert_eq!(count_events(&events, "llm-entered"), 1);
}

// ---------------------------------------------------------------------------
// 3/8 — empty batch releases the lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lock_released_when_prompt_batch_empty() {
    let dir = TempDir::new("service-lock-empty");
    let mut first = Learner::spawn(
        dir.path(),
        "solo",
        serde_json::json!({
            "scenario": "empty",
            "selectEmpty": true,
            "promptCount": 5,
            "threshold": 2,
        }),
    );
    let report = first.report();
    let events = first.events();

    assert_eq!(report["outcome"], serde_json::json!("no-prompts"));
    assert_eq!(count_events(&events, "count"), 1);
    assert_eq!(count_events(&events, "select"), 1);
    assert_eq!(count_events(&events, "llm-entered"), 0);
    assert_eq!(count_events(&events, "mark"), 0);
    assert_eq!(current_owner_token(dir.path()).await, None);

    let mut second = Learner::spawn(
        dir.path(),
        "second",
        serde_json::json!({"scenario": "full", "promptCount": 3, "threshold": 2}),
    );
    assert!(second.wait_for_event("llm-entered"));
    release_winner(dir.path());
    let _ = second.report();
    assert_eq!(second.count("llm-entered"), 1);
}

// ---------------------------------------------------------------------------
// 4/8 — provider-not-ready short-circuit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lock_released_when_provider_not_ready() {
    let dir = TempDir::new("service-lock-notready");
    let mut first = Learner::spawn(
        dir.path(),
        "solo",
        serde_json::json!({"scenario": "not-ready", "providerReady": false}),
    );
    let report = first.report();
    let events = first.events();

    assert_eq!(report["outcome"], serde_json::json!("not-ready"));
    assert!(has_log(&events, "provider not ready"), "{events:?}");
    assert_eq!(count_events(&events, "count"), 0);
    assert_eq!(current_owner_token(dir.path()).await, None);

    let mut second = Learner::spawn(
        dir.path(),
        "second",
        serde_json::json!({"scenario": "full", "promptCount": 3, "threshold": 2}),
    );
    assert!(second.wait_for_event("llm-entered"));
    release_winner(dir.path());
    let _ = second.report();
    assert_eq!(second.count("llm-entered"), 1);
}

// ---------------------------------------------------------------------------
// 5/8 — provider error propagates, lock reacquirable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_provider_error_propagates_and_lock_reacquirable() {
    let dir = TempDir::new("service-lock-provider-error");
    let mut first = Learner::spawn(
        dir.path(),
        "solo",
        serde_json::json!({
            "scenario": "llm-error",
            "llmError": true,
            "promptCount": 3,
            "threshold": 2,
        }),
    );
    let report = first.report();
    let events = first.events();

    assert_eq!(report["propagated"], serde_json::Value::Bool(true));
    assert!(
        report["error"]
            .as_str()
            .unwrap_or_default()
            .contains("provider rejected"),
        "{report}"
    );
    assert_eq!(count_events(&events, "llm-entered"), 1);
    assert_eq!(count_events(&events, "mark"), 0);
    assert_eq!(
        current_owner_token(dir.path()).await,
        None,
        "the error path must release the lock"
    );

    let mut second = Learner::spawn(
        dir.path(),
        "second",
        serde_json::json!({"scenario": "full", "promptCount": 3, "threshold": 2}),
    );
    assert!(second.wait_for_event("llm-entered"));
    release_winner(dir.path());
    let second_report = second.report();
    let second_events = second.events();
    assert_eq!(second_report["outcome"], serde_json::json!("completed"));
    assert!(!has_log(&second_events, "another process holds"));
    assert_eq!(count_events(&second_events, "mark"), 1);
}

// ---------------------------------------------------------------------------
// 6/8 — in-process concurrency bounces off the flag
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_concurrent_calls_same_process_run_once_via_pre_first_await_flag() {
    let dir = TempDir::new("service-lock-inproc");
    let mut child = Learner::spawn(
        dir.path(),
        "solo",
        serde_json::json!({"scenario": "inproc-race", "promptCount": 3, "threshold": 2}),
    );
    let report = child.report();
    let events = child.events();

    assert_eq!(
        report["statuses"],
        serde_json::json!(["completed", "already-running"]),
        "{report}"
    );
    assert_eq!(count_events(&events, "count"), 1);
    assert_eq!(count_events(&events, "llm-entered"), 1);
    assert_eq!(count_events(&events, "mark"), 1);
}

// ---------------------------------------------------------------------------
// 7/8 — the flag recovers after a failed run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_in_process_flag_recovers_after_failed_run() {
    let dir = TempDir::new("service-lock-retry");
    let mut child = Learner::spawn(
        dir.path(),
        "solo",
        serde_json::json!({"scenario": "error-then-retry", "promptCount": 3, "threshold": 2}),
    );
    let report = child.report();
    let events = child.events();

    assert!(
        report["firstError"]
            .as_str()
            .unwrap_or_default()
            .contains("provider rejected"),
        "{report}"
    );
    assert_eq!(report["secondOutcome"], serde_json::json!("completed"));
    assert_eq!(count_events(&events, "llm-entered"), 2);
    assert_eq!(count_events(&events, "llm-error"), 1);
    assert_eq!(count_events(&events, "mark"), 1);
    assert_eq!(count_events(&events, "profile-write"), 1);
}

// ---------------------------------------------------------------------------
// 8/8 — memory writes complete while the learning lock is held
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_memory_writes_complete_while_learning_lock_held() {
    let dir = TempDir::new("service-lock-memory");
    let holding_file = dir.path().join("holding");

    let mut winner = Learner::spawn(
        dir.path(),
        "winner",
        serde_json::json!({
            "scenario": "full",
            "blockOnLlm": true,
            "releaseFile": dir.path().join("release").display().to_string(),
            "promptCount": 3,
            "threshold": 2,
        }),
    );
    assert!(winner.wait_for_event("llm-entered"));
    // POSITIVE INVARIANT: the row is held while the winner is inside the LLM.
    assert!(
        current_owner_token(dir.path()).await.is_some(),
        "the learning lock must be held during the mutation window"
    );
    std::fs::write(&holding_file, b"held").expect("signal holding");

    // Independent child drives REAL mem_store machinery (real shard DB, real
    // operation lock, real SQL) against the same storage.
    let mut writer = Learner::spawn(
        dir.path(),
        "memory-writer",
        serde_json::json!({
            "scenario": "memory-writer",
            "holdingFile": holding_file.display().to_string(),
            "marker": "during-learning-lock",
        }),
    );
    let writer_report = writer.report();

    release_winner(dir.path());
    let winner_report = winner.report();

    assert_eq!(winner_report["outcome"], serde_json::json!("completed"));
    assert_eq!(
        writer_report["holdingObserved"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        writer_report["counts"],
        serde_json::json!({"add": 1, "readback": 1, "search": 1, "list": 1}),
        "{writer_report}"
    );
    assert!(writer_report["addedId"].is_string(), "{writer_report}");
    assert_eq!(writer_report["searchTotal"], serde_json::json!(1));

    // And the learning run itself still finished exactly once.
    assert_eq!(winner.count("llm-entered"), 1);
    assert_eq!(winner.count("mark"), 1);
    assert_eq!(current_owner_token(dir.path()).await, None);
}

// ---------------------------------------------------------------------------
// Child entry: mocks at the three boundaries, real service + real lock
// ---------------------------------------------------------------------------

const ANALYSIS_JSON: &str = r#"{"preferences":[{"category":"style","description":"Uses numbered lists","confidence":0.7,"frequency":1,"evidence":["observed"],"lastSeen":1}],"patterns":[],"workflows":[]}"#;

/// Appends one NDJSON event line (`{"ev", "data"}`) per emitted event.
struct EventWriter {
    path: PathBuf,
}

impl EventWriter {
    fn write(&self, event: &str, data: serde_json::Value) {
        common::append_event(&self.path, &serde_json::json!({"ev": event, "data": data}));
    }
}

impl RoundEventSink for EventWriter {
    fn emit(&self, event: &str, detail: serde_json::Value) {
        self.write(event, detail);
    }
}

struct MockLedger {
    events: Arc<EventWriter>,
    prompt_count: i64,
    select_empty: bool,
}

impl MockLedger {
    fn prompts(&self) -> Vec<UserPrompt> {
        (0..self.prompt_count.min(5))
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
            .collect()
    }
}

impl PromptLedger for MockLedger {
    fn count_unanalyzed(&self) -> BoxFuture<'_, Result<i64, ProfileError>> {
        let events = self.events.clone();
        let count = self.prompt_count;
        Box::pin(async move {
            events.write("count", serde_json::json!({"count": count}));
            Ok(count)
        })
    }

    fn get_prompts_for_learning(
        &self,
        _limit: i64,
    ) -> BoxFuture<'_, Result<Vec<UserPrompt>, ProfileError>> {
        let events = self.events.clone();
        let prompts = if self.select_empty {
            Vec::new()
        } else {
            self.prompts()
        };
        Box::pin(async move {
            events.write("select", serde_json::json!({"count": prompts.len()}));
            Ok(prompts)
        })
    }

    fn mark_many(&self, ids: Vec<String>) -> BoxFuture<'_, Result<(), ProfileError>> {
        let events = self.events.clone();
        Box::pin(async move {
            events.write("mark", serde_json::json!({"count": ids.len()}));
            Ok(())
        })
    }
}

struct MockProfiles {
    events: Arc<EventWriter>,
}

impl ProfileWriter for MockProfiles {
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
        let events = self.events.clone();
        Box::pin(async move {
            events.write("profile-write", serde_json::json!({"kind": "update"}));
            Ok(true)
        })
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
        let events = self.events.clone();
        Box::pin(async move {
            events.write("profile-write", serde_json::json!({"kind": "create"}));
            Ok("profile_created".to_string())
        })
    }
}

struct MockGateway {
    events: Arc<EventWriter>,
    fail: Arc<std::sync::atomic::AtomicBool>,
    block_on_llm: bool,
    release_file: Option<PathBuf>,
}

impl MockGateway {
    /// One analysis = one `llm-entered` … `llm-exit` (or `llm-error`) pair,
    /// exactly like upstream's single call site; a fallback provider attempt is
    /// not a second analysis and emits nothing of its own.
    fn complete(&self, provider: &'static str) -> Result<String, AnalysisError> {
        if self.block_on_llm
            && let Some(release_file) = &self.release_file
        {
            let started = Instant::now();
            while !release_file.exists() {
                if started.elapsed() > BARRIER_TIMEOUT {
                    return Err(AnalysisError::Provider(
                        "release barrier timed out".to_string(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if self.fail.load(std::sync::atomic::Ordering::Acquire) {
            if provider == "native" {
                self.events.write(
                    "llm-error",
                    serde_json::json!({"provider": provider, "error": "provider rejected by test"}),
                );
            }
            return Err(AnalysisError::Provider(
                "provider rejected by test".to_string(),
            ));
        }
        self.events
            .write("llm-exit", serde_json::json!({"provider": provider}));
        Ok(ANALYSIS_JSON.to_string())
    }
}

impl AnalysisGateway for MockGateway {
    fn native_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(async move {
            self.events
                .write("llm-entered", serde_json::json!({"provider": "native"}));
            // A real completion always suspends; the yield makes the
            // concurrent-call fixture observe a genuinely in-flight run.
            tokio::task::yield_now().await;
            self.complete("native")
        })
    }

    fn external_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(async move { self.complete("external") })
    }
}

fn outcome_name(outcome: &LearningOutcome) -> &'static str {
    match outcome {
        LearningOutcome::Completed { .. } => "completed",
        LearningOutcome::Contention => "contention",
        LearningOutcome::AlreadyRunning => "already-running",
        LearningOutcome::NotReady => "not-ready",
        LearningOutcome::BelowThreshold { .. } => "below-threshold",
        LearningOutcome::NoPrompts => "no-prompts",
    }
}

fn build_round(
    dir: &Path,
    events: Arc<EventWriter>,
    ledger: MockLedger,
    gateway: MockGateway,
    threshold: i64,
    ready: bool,
) -> LearningRound {
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    )));
    LearningRound::new(
        Arc::new(ledger),
        Arc::new(MockProfiles {
            events: events.clone(),
        }),
        Arc::new(gateway),
        lock,
        Arc::new(move || ready),
        threshold,
        "test@example.com",
    )
    .with_event_sink(events)
}

/// Unreachable-by-default child entry; harmless no-op when the mode env is
/// unset. Executed through self re-exec
/// (`--exact fixture_child_entry --ignored`).
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
    let Some(scenario) = mode.strip_prefix("learn:") else {
        return;
    };
    let config_path = std::env::var("MEM_FIXTURE_CFG").expect("MEM_FIXTURE_CFG");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read worker config"))
            .expect("worker config JSON");
    let dir = PathBuf::from(
        config["storagePath"]
            .as_str()
            .expect("storagePath in config"),
    );
    let events_path = PathBuf::from(config["eventsFile"].as_str().expect("eventsFile in config"));
    let events = Arc::new(EventWriter { path: events_path });
    let prompt_count = config["promptCount"].as_i64().unwrap_or(3);
    let threshold = config["threshold"].as_i64().unwrap_or(2);
    let ready = config["providerReady"].as_bool().unwrap_or(true);
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(
        config["llmError"].as_bool().unwrap_or(false),
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");

    if scenario == "memory-writer" {
        let report = memory_writer_report(&runtime, &dir, &config);
        write_child_report(&out, &report);
        return;
    }

    let report = runtime.block_on(async {
        let gateway = MockGateway {
            events: events.clone(),
            fail: fail.clone(),
            block_on_llm: config["blockOnLlm"].as_bool().unwrap_or(false),
            release_file: config["releaseFile"].as_str().map(PathBuf::from),
        };
        match scenario {
            "inproc-race" => {
                let round = build_round(
                    &dir,
                    events.clone(),
                    MockLedger {
                        events: events.clone(),
                        prompt_count,
                        select_empty: false,
                    },
                    gateway,
                    threshold,
                    ready,
                );
                let (first, second) = tokio::join!(round.run(0), round.run(0));
                let statuses = vec![
                    outcome_name(&first.expect("first run")),
                    outcome_name(&second.expect("second run")),
                ];
                serde_json::json!({"outcome": "inproc-race", "statuses": statuses})
            }
            "error-then-retry" => {
                // The scenario's first run always fails; the second succeeds.
                fail.store(true, std::sync::atomic::Ordering::Release);
                let round = build_round(
                    &dir,
                    events.clone(),
                    MockLedger {
                        events: events.clone(),
                        prompt_count,
                        select_empty: false,
                    },
                    gateway,
                    threshold,
                    ready,
                );
                let first = round.run(0).await;
                fail.store(false, std::sync::atomic::Ordering::Release);
                let second = round.run(0).await;
                let second_outcome = second.expect("second run");
                serde_json::json!({
                    "outcome": outcome_name(&second_outcome),
                    "firstError": first.err().map(|error| error.to_string()),
                    "secondOutcome": outcome_name(&second_outcome),
                })
            }
            _ => {
                let round = build_round(
                    &dir,
                    events.clone(),
                    MockLedger {
                        events: events.clone(),
                        prompt_count,
                        select_empty: config["selectEmpty"].as_bool().unwrap_or(false),
                    },
                    gateway,
                    threshold,
                    ready,
                );
                match round.run(0).await {
                    Ok(outcome) => serde_json::json!({
                        "outcome": outcome_name(&outcome),
                        "propagated": false,
                    }),
                    Err(error) => serde_json::json!({
                        "outcome": "error",
                        "propagated": true,
                        "error": error.to_string(),
                    }),
                }
            }
        }
    });
    write_child_report(&out, &report);
}

/// Drives the REAL mem_store machinery (factory open, operation lock, insert,
/// readback, indexed search, list) against the shared storage.
fn memory_writer_report(
    runtime: &tokio::runtime::Runtime,
    dir: &Path,
    config: &serde_json::Value,
) -> serde_json::Value {
    use mem_store::db::{ConnectFactory as _, Value};
    use mem_store::platform::SystemProcessProbe;

    let holding_file = config["holdingFile"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_default();
    let marker = config["marker"].as_str().unwrap_or("marker").to_string();
    let holding_observed = holding_file.exists();

    let store_config = common::store_config(dir.to_path_buf());
    let operation_lock = mem_store::operation_lock::acquire_turso_operation_lock(
        dir,
        Arc::new(SystemProcessProbe),
        "test-write",
    )
    .expect("acquire the store operation lock");

    let shard_path = dir.join("shard-memtest.db");
    let mut counts = serde_json::json!({"add": 0, "readback": 0, "search": 1 > 2});
    let added_id = format!("mem_{}", mem_profile::ids::now_millis());
    let search_total = runtime.block_on(async {
        let mut connect_config = store_config.clone();
        connect_config.database_encryption_enabled = false;
        let options = mem_store::db::ConnectOptions::for_config(
            &connect_config,
            mem_store::platform::Platform::current(),
        );
        let db = mem_store::db::turso_connect_factory()
            .connect(shard_path.clone(), options)
            .await
            .expect("open shard");
        db.execute(mem_store::sql::memories_ddl(8).as_str(), Vec::new())
            .await
            .expect("create memories table");
        for statement in mem_store::sql::memory_shard_v2_statements() {
            db.execute(statement, Vec::new()).await.expect("v2 ddl");
        }
        for statement in mem_store::sql::memories_indexes() {
            db.execute(statement, Vec::new()).await.expect("indexes");
        }

        let vector = serde_json::to_string(&vec![0.5_f32; 8]).expect("vector json");
        db.execute(
            "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at)
             VALUES (?, ?, vector32(?), ?, ?, ?)",
            vec![
                Value::Text(added_id.clone()),
                Value::Text(format!("{marker} memory")),
                Value::Text(vector),
                Value::Text("container".to_string()),
                Value::Integer(mem_profile::ids::now_millis()),
                Value::Integer(mem_profile::ids::now_millis()),
            ],
        )
        .await
        .expect("insert memory");

        let readback = mem_store::db::query_one(
            db.as_ref(),
            "SELECT id FROM memories WHERE id = ?",
            vec![Value::Text(added_id.clone())],
        )
        .await
        .expect("readback")
        .is_some();

        let search = db
            .query(
                "SELECT id FROM memories WHERE container_tag = ? AND content LIKE ?",
                vec![
                    Value::Text("container".to_string()),
                    Value::Text(format!("%{marker}%")),
                ],
            )
            .await
            .expect("indexed search");

        let list = db
            .query(
                "SELECT id FROM memories ORDER BY created_at DESC LIMIT 10",
                Vec::new(),
            )
            .await
            .expect("list");

        counts = serde_json::json!({
            "add": 1,
            "readback": i64::from(readback),
            "search": i64::from(!search.is_empty()),
            "list": i64::from(!list.is_empty()),
        });
        search.len()
    });

    drop(operation_lock);
    serde_json::json!({
        "holdingObserved": holding_observed,
        "counts": counts,
        "addedId": added_id,
        "searchTotal": search_total,
    })
}
