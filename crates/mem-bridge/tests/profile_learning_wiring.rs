//! Issue 48 / A1 — the wired profile learner, end to end.
//!
//! The real `UserPromptManager`, `ProfileStore` and `LearningLock` sit behind
//! the learner; only the provider boundary, the event sink and (for the race)
//! the scheduling clock are spies. Every assertion is therefore about what the
//! wiring actually writes, retries, arms or emits.
//!
//! Tests:
//! - `test_record_filters_injected_blocks_before_write`
//! - `test_record_is_idempotent_on_message_id_retry`
//! - `test_record_skips_a_capture_without_prompt_id`
//! - `test_two_idle_events_inside_one_window_produce_one_round`
//! - `test_unknown_identity_builds_no_learner`
//! - `test_composition_passes_an_explicit_identity`
//! - `test_manual_only_arm_makes_the_manual_provider_native`
//! - `test_concurrent_round_drives_are_serialized_by_the_round_flag`
//! - `test_learning_canary_never_enters_any_log_channel`
//! - `test_round_failures_are_typed_not_panics`
//! - `test_round_failure_event_carries_the_closed_reason_token`
//! - `test_shutdown_flag_stops_run_due_between_rounds`

mod common;

/// A flag that never flips: the default argument for every drive that is not
/// testing shutdown.
static NEVER_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// The identity the record-path fixtures stamp (the learner's round is built
/// for it, so the record tests prove the ARGUMENT is what lands).
const TEST_IDENTITY: &str = "probe@example.com";

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;

use mem_bridge::capture::CaptureParams;
use mem_bridge::composition::build_composed_runtime;
use mem_bridge::identity::UNKNOWN_USER_ID;
use mem_bridge::profile_learning::{
    ProfileLearning, RecordOutcome, build_learning_gateway, build_profile_learning,
};
use mem_core::config::{Config, ConfigPaths, MemoryProvider};
use mem_store::db::BoxFuture;

use mem_ai::privacy::PrivacyConfig;
use mem_ai::provider::{HttpRequest, HttpResponse, HttpTransport, ProviderError};

use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, ProfileWriter,
    PromptLedger, RoundEventSink,
};
use mem_profile::profile_store::{
    Profile, ProfileConfig, ProfileData, ProfileStore, ProfileUpdates,
};
use mem_profile::prompt::{UserPrompt, UserPromptManager};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The prompt a capture carried, with `promptId` when one was present.
fn params(prompt_id: Option<&str>, transcript: &str) -> CaptureParams {
    CaptureParams {
        session_id: "session-1".to_string(),
        capture_id: "capture-1".to_string(),
        project_id: "/workspace".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: None,
        parent_session_id: None,
        transcript: Some(transcript.to_string()),
        summary: None,
        metadata: prompt_id.map(|id| json!({ "promptId": id })),
        raw_context: None,
        agent_role: None,
        vault: false,
    }
}

/// Records every event the learner and the round emit.
#[derive(Default)]
struct SpySink {
    events: Mutex<Vec<(String, String)>>,
}

impl SpySink {
    fn count(&self, name: &str) -> usize {
        self.events
            .lock()
            .expect("events lock")
            .iter()
            .filter(|(event, _)| event == name)
            .count()
    }

    fn emitted(&self) -> usize {
        self.events.lock().expect("events lock").len()
    }

    fn all_text(&self) -> String {
        self.events
            .lock()
            .expect("events lock")
            .iter()
            .map(|(event, detail)| format!("{event} {detail}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The parsed detail of the FIRST event with this name, if any.
    ///
    /// `emit` stores `detail.to_string()`, so the getter re-parses that string.
    fn detail(&self, event: &str) -> Option<serde_json::Value> {
        let events = self.events.lock().expect("events lock");
        let (_, detail) = events.iter().find(|(name, _)| name == event)?;
        Some(serde_json::from_str(detail).expect("the sink stores JSON"))
    }
}

impl RoundEventSink for SpySink {
    fn emit(&self, event: &str, detail: serde_json::Value) {
        self.events
            .lock()
            .expect("events lock")
            .push((event.to_string(), detail.to_string()));
    }
}

/// A one-prompt ledger, so the round reaches its provider.
struct OnePromptLedger {
    prompt: UserPrompt,
}

impl PromptLedger for OnePromptLedger {
    fn count_unanalyzed(&self, _user_id: &str) -> BoxFuture<'_, Result<i64, ProfileErrorAlias>> {
        Box::pin(async { Ok(1) })
    }

    fn get_prompts_for_learning(
        &self,
        _limit: i64,
        _user_id: &str,
    ) -> BoxFuture<'_, Result<Vec<UserPrompt>, ProfileErrorAlias>> {
        let prompt = self.prompt.clone();
        Box::pin(async move { Ok(vec![prompt]) })
    }

    fn mark_many(&self, _ids: Vec<String>) -> BoxFuture<'_, Result<(), ProfileErrorAlias>> {
        Box::pin(async { Ok(()) })
    }

    fn distinct_user_ids_for_learning(
        &self,
    ) -> BoxFuture<'_, Result<Vec<String>, ProfileErrorAlias>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// `ProfileError` under a short alias (the trait's own return type).
type ProfileErrorAlias = mem_profile::ProfileError;

fn one_prompt_ledger() -> Arc<dyn PromptLedger> {
    Arc::new(OnePromptLedger {
        prompt: UserPrompt {
            id: "prompt-0".to_string(),
            session_id: "session-1".to_string(),
            message_id: "message-0".to_string(),
            project_path: Some("/workspace".to_string()),
            content: "a real request about the parser".to_string(),
            created_at: 0,
            captured: true,
            user_learning_captured: false,
            linked_memory_id: None,
            capture_attempts: 0,
            provider_id: None,
            model_id: None,
        },
    })
}

/// A writer that counts writes instead of touching a profile database.
#[derive(Default)]
struct CountingWriter {
    writes: Arc<AtomicUsize>,
}

impl ProfileWriter for CountingWriter {
    fn get_active_profile<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Profile>, ProfileErrorAlias>> {
        Box::pin(async { Ok(None) })
    }

    fn merge_profile_data<'a>(
        &'a self,
        existing: &'a ProfileData,
        _updates: &'a ProfileUpdates,
        _profile_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<ProfileData, ProfileErrorAlias>> {
        Box::pin(async move { Ok(existing.clone()) })
    }

    fn update_profile<'a>(
        &'a self,
        _profile_id: &'a str,
        _data: &'a ProfileData,
        _prompts_analyzed: i64,
        _change_summary: &'a str,
    ) -> BoxFuture<'a, Result<bool, ProfileErrorAlias>> {
        self.writes.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Ok(true) })
    }

    fn create_profile<'a>(
        &'a self,
        _user_id: &'a str,
        _display_name: &'a str,
        _user_name: &'a str,
        _user_email: &'a str,
        _data: &'a ProfileData,
        _prompts_analyzed: i64,
    ) -> BoxFuture<'a, Result<String, ProfileErrorAlias>> {
        self.writes.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Ok("profile-1".to_string()) })
    }
}

/// How the spy gateway answers a native call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Failure {
    None,
    Provider,
    InvalidJson,
}

/// A gateway that counts native calls and optionally parks inside one.
struct SpyGateway {
    native_calls: AtomicUsize,
    failure: Failure,
    entered: Option<Arc<tokio::sync::Notify>>,
    release: Option<Arc<tokio::sync::Notify>>,
}

impl SpyGateway {
    fn counting() -> Arc<Self> {
        Arc::new(Self {
            native_calls: AtomicUsize::new(0),
            failure: Failure::None,
            entered: None,
            release: None,
        })
    }

    fn failing() -> Arc<Self> {
        Arc::new(Self {
            native_calls: AtomicUsize::new(0),
            failure: Failure::Provider,
            entered: None,
            release: None,
        })
    }

    /// A provider that answers with a payload that is not JSON: the round's
    /// parse turns it into `ProfileError::Json`, a different variant from the
    /// `Analysis` one `failing()` produces.
    fn invalid_json() -> Arc<Self> {
        Arc::new(Self {
            native_calls: AtomicUsize::new(0),
            failure: Failure::InvalidJson,
            entered: None,
            release: None,
        })
    }

    fn parked(entered: Arc<tokio::sync::Notify>, release: Arc<tokio::sync::Notify>) -> Arc<Self> {
        Arc::new(Self {
            native_calls: AtomicUsize::new(0),
            failure: Failure::None,
            entered: Some(entered),
            release: Some(release),
        })
    }

    fn calls(&self) -> usize {
        self.native_calls.load(Ordering::Acquire)
    }
}

impl AnalysisGateway for SpyGateway {
    fn native_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        let call = self.native_calls.fetch_add(1, Ordering::AcqRel);
        Box::pin(async move {
            // Only the FIRST call parks: a mutant that drops the shutdown check
            // then drives a second round that completes instead of hanging.
            if call == 0 {
                if let Some(entered) = self.entered.as_ref() {
                    entered.notify_one();
                }
                if let Some(release) = self.release.as_ref() {
                    release.notified().await;
                }
            }
            match self.failure {
                Failure::Provider => Err(AnalysisError::Provider("provider fault".to_string())),
                Failure::InvalidJson => Ok("not json".to_string()),
                Failure::None => {
                    Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string())
                }
            }
        })
    }

    fn external_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        // `external_configured()` is false, so a native fault propagates and
        // this arm is unreachable by contract; it answers success so a leaked
        // call would show up as a completed round instead of an error.
        Box::pin(async { Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()) })
    }

    fn external_configured(&self) -> bool {
        false
    }
}

/// Builds a learner over a real prompt store and lock, with spy seams.
fn build_learner(
    root: &Path,
    sink: Arc<SpySink>,
    gateway: Arc<dyn AnalysisGateway>,
    ledger: Arc<dyn PromptLedger>,
    threshold: i64,
) -> (
    Arc<ProfileLearning>,
    Arc<UserPromptManager>,
    Arc<AtomicUsize>,
) {
    let store_config = common::store_config(root.to_path_buf());
    let prompts = Arc::new(UserPromptManager::new(
        root.to_path_buf(),
        store_config.clone(),
        3,
    ));
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        root.to_path_buf(),
        store_config,
    )));
    let writes = Arc::new(AtomicUsize::new(0));
    let round = LearningRound::new(
        ledger,
        Arc::new(CountingWriter {
            writes: writes.clone(),
        }),
        gateway,
        lock,
        Arc::new(|| true),
        threshold,
        "probe@example.com",
    )
    .with_event_sink(sink.clone());
    let learning = Arc::new(ProfileLearning::new(
        Arc::new(round),
        prompts.clone(),
        PrivacyConfig::default(),
        sink,
    ));
    (learning, prompts, writes)
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

fn load_config(home: &Path) -> Config {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create the config dir");
    std::fs::write(dir.join("opencode-mem.json"), "{}").expect("write the config");
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.display().to_string())]
        .into_iter()
        .collect();
    Config::load(&ConfigPaths::standard(home), &env).expect("the config loads")
}

/// The documented complete manual block, pointed at loopback (never dialled).
fn with_manual_block(config: &mut Config) {
    config.memory_provider = MemoryProvider::OpenAiChat;
    config.memory_provider_claimed = true;
    config.memory_model = Some("test-model".to_string());
    config.memory_api_url = Some("http://127.0.0.1:9/v1".to_string());
    config.memory_api_key = Some("sk-test-not-a-placeholder".to_string());
}

fn profile_store(root: &Path) -> Arc<ProfileStore> {
    Arc::new(ProfileStore::new(
        root.to_path_buf(),
        common::store_config(root.to_path_buf()),
        ProfileConfig::default(),
    ))
}

fn learning_lock(root: &Path) -> Arc<LearningLock> {
    Arc::new(LearningLock::new(LearningLockOptions::new(
        root.to_path_buf(),
        common::store_config(root.to_path_buf()),
    )))
}

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_record_filters_injected_blocks_before_write() {
    let dir = common::TestDir::new("profile-learning-filter");
    let sink = Arc::new(SpySink::default());
    let (learning, prompts, _writes) = build_learner(
        dir.path(),
        sink.clone(),
        SpyGateway::counting(),
        one_prompt_ledger(),
        1,
    );

    let outcome = learning
        .record(
            &params(
                Some("p-filter"),
                "<!-- OMO_INTERNAL_INITIATOR -->\nplease fix the parser",
            ),
            Some(TEST_IDENTITY),
        )
        .await;
    assert!(
        matches!(outcome, RecordOutcome::Recorded { .. }),
        "recording must succeed: {outcome:?}"
    );

    let rows = prompts
        .get_prompts_for_user_learning(10, TEST_IDENTITY)
        .await
        .expect("the ledger reads");
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0].content.contains("OMO_INTERNAL_INITIATOR"),
        "the injected marker must never reach disk: {:?}",
        rows[0].content
    );
    assert!(rows[0].content.contains("please fix the parser"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_record_is_idempotent_on_message_id_retry() {
    let dir = common::TestDir::new("profile-learning-idempotent");
    let sink = Arc::new(SpySink::default());
    let (learning, prompts, _writes) = build_learner(
        dir.path(),
        sink,
        SpyGateway::counting(),
        one_prompt_ledger(),
        1,
    );

    let first = learning
        .record(
            &params(Some("p-retry"), "the same turn"),
            Some(TEST_IDENTITY),
        )
        .await;
    let second = learning
        .record(
            &params(Some("p-retry"), "the same turn"),
            Some(TEST_IDENTITY),
        )
        .await;

    let RecordOutcome::Recorded {
        prompt_row_id: first_id,
    } = first
    else {
        panic!("the first record must land: {first:?}");
    };
    let RecordOutcome::Recorded {
        prompt_row_id: second_id,
    } = second
    else {
        panic!("the retry must resolve the same row: {second:?}");
    };
    assert_eq!(first_id, second_id, "one promptId is one row");

    let rows = prompts
        .get_prompts_for_user_learning(10, TEST_IDENTITY)
        .await
        .expect("the ledger reads");
    assert_eq!(rows.len(), 1, "a retry must not duplicate the row");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_record_skips_a_capture_without_prompt_id() {
    let dir = common::TestDir::new("profile-learning-no-id");
    let sink = Arc::new(SpySink::default());
    let (learning, prompts, _writes) = build_learner(
        dir.path(),
        sink.clone(),
        SpyGateway::counting(),
        one_prompt_ledger(),
        1,
    );

    let outcome = learning
        .record(&params(None, "a turn without an id"), Some(TEST_IDENTITY))
        .await;
    assert_eq!(outcome, RecordOutcome::SkippedNoPromptId);
    assert_eq!(sink.count("profile.learning_record_skipped"), 1);

    let rows = prompts
        .get_prompts_for_user_learning(10, TEST_IDENTITY)
        .await
        .expect("the ledger reads");
    assert!(rows.is_empty(), "no synthetic id may be invented");
}

/// Issue 174: `record` persists the identity it is GIVEN, never the round's
/// own — the accept-time resolution is the single source of both rows.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_record_persists_the_accept_time_identity() {
    let dir = common::TestDir::new("profile-learning-record-identity");
    let sink = Arc::new(SpySink::default());
    let (learning, prompts, _writes) = build_learner(
        dir.path(),
        sink,
        SpyGateway::counting(),
        one_prompt_ledger(),
        1,
    );
    assert_eq!(
        learning.user_id(),
        "",
        "the learner itself carries no identity in this fixture"
    );

    let outcome = learning
        .record(&params(Some("p-identity"), "a turn"), Some("a@x"))
        .await;
    assert!(
        matches!(outcome, RecordOutcome::Recorded { .. }),
        "{outcome:?}"
    );

    assert_eq!(
        prompts
            .get_prompts_for_user_learning(10, "a@x")
            .await
            .expect("select under the record argument")
            .len(),
        1,
        "the row carries the identity passed to record"
    );
    assert!(
        prompts
            .get_prompts_for_user_learning(10, TEST_IDENTITY)
            .await
            .expect("select under the round's identity")
            .is_empty(),
        "record never substitutes the round's identity"
    );
}

// ---------------------------------------------------------------------------
// Debounce and identity
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_two_idle_events_inside_one_window_produce_one_round() {
    let dir = common::TestDir::new("profile-learning-debounce");
    let sink = Arc::new(SpySink::default());
    let gateway = SpyGateway::counting();
    let (learning, _prompts, _writes) =
        build_learner(dir.path(), sink, gateway.clone(), one_prompt_ledger(), 1);

    assert!(learning.arm("session-1", 0), "the first idle arms");
    assert!(
        learning.arm("session-1", 2_000),
        "a second idle inside the window replaces the deadline"
    );

    let early = learning.run_due(9_999, &NEVER_SHUTDOWN).await;
    assert_eq!(early.due, 0, "the original 10 s deadline is gone");
    assert_eq!(gateway.calls(), 0);

    let first = learning.run_due(12_000, &NEVER_SHUTDOWN).await;
    assert_eq!(first.due, 1, "the replaced deadline is 12 000");
    assert_eq!(first.completed, 1);
    assert_eq!(gateway.calls(), 1);

    let again = learning.run_due(12_001, &NEVER_SHUTDOWN).await;
    assert_eq!(again.due, 0, "poll_due removes the arm it observed");
    assert!(again.outcomes.is_empty());
    assert_eq!(gateway.calls(), 1, "one window, one round");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_unknown_identity_builds_no_learner() {
    let dir = common::TestDir::new("profile-learning-identity");
    let home = common::TestDir::new("profile-learning-identity-home");
    let config = load_config(home.path());

    let sink = Arc::new(SpySink::default());
    let built = build_profile_learning(
        &config,
        UNKNOWN_USER_ID,
        Arc::new(UserPromptManager::new(
            dir.path().to_path_buf(),
            common::store_config(dir.path().to_path_buf()),
            3,
        )),
        profile_store(dir.path()),
        learning_lock(dir.path()),
        None,
        sink.clone(),
    );
    assert!(built.is_none(), "an unknown identity never learns");
    assert_eq!(sink.count("profile.learning_identity_unknown"), 1);

    // The positive arm needs a complete manual block (no host in this binary).
    let home = common::TestDir::new("profile-learning-identity-home-ok");
    let mut config = load_config(home.path());
    with_manual_block(&mut config);
    let built = build_profile_learning(
        &config,
        "probe@example.com",
        Arc::new(UserPromptManager::new(
            dir.path().to_path_buf(),
            common::store_config(dir.path().to_path_buf()),
            3,
        )),
        profile_store(dir.path()),
        learning_lock(dir.path()),
        None,
        Arc::new(SpySink::default()),
    )
    .expect("an explicit identity with a provider arm builds a learner");
    assert_eq!(built.user_id(), "probe@example.com");
}

/// The composition root's embedder seam (never used by this arm).
struct NoopEmbedder;

impl mem_embed::embedder::Embedder for NoopEmbedder {
    fn warmup(
        &self,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<(), mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(
        &self,
        _text: &str,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<Vec<f32>, mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(vec![0.0; 8]) })
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_composition_passes_an_explicit_identity() {
    let root = common::TestDir::new("profile-learning-composition");
    let home = common::TestDir::new("profile-learning-composition-home");
    let mut config = load_config(home.path());
    config.storage_path = root.path().to_path_buf();
    config.embedding_dimensions = 8;
    with_manual_block(&mut config);
    config.user_email_override = Some("probe@example.com".to_string());

    let runtime = build_composed_runtime(root.path(), &config, Arc::new(NoopEmbedder), None)
        .await
        .expect("the runtime composes");
    let learning = runtime
        .learning
        .expect("the composition wires the learner when an identity and arm exist");
    assert_eq!(
        learning.user_id(),
        "probe@example.com",
        "the composition passes the resolved override, never a literal"
    );
}

// ---------------------------------------------------------------------------
// Provider arm and serialization
// ---------------------------------------------------------------------------

/// Records every outbound HTTP request; answers one analysis document.
#[derive(Default)]
struct RecordingTransport {
    calls: Mutex<Vec<HttpRequest>>,
}

impl HttpTransport for RecordingTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'static, Result<HttpResponse, ProviderError>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(std::future::ready(Ok(HttpResponse {
            status: 200,
            body: json!({
                "choices": [{
                    "message": {
                        "content": r#"{"preferences":[],"patterns":[],"workflows":[]}"#
                    }
                }]
            })
            .to_string(),
        })))
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_manual_only_arm_makes_the_manual_provider_native() {
    let home = common::TestDir::new("profile-learning-manual-home");
    let mut config = load_config(home.path());
    with_manual_block(&mut config);

    let transport = Arc::new(RecordingTransport::default());
    let manual = mem_bridge::manual_provider::build_manual_provider(&config, transport.clone())
        .expect("the complete block builds")
        .expect("the block is claimed");
    let gateway = build_learning_gateway(&config, None, Some(manual)).expect("the arm exists");
    assert!(
        !gateway.external_configured(),
        "with no host the manual provider IS the native arm"
    );

    let context = AnalysisContext {
        prompts: Vec::new(),
        existing_profile: None,
        lang_name: "English".to_string(),
        user_prompt: "PROMPT-SENTINEL".to_string(),
        system_prompt_json: "JSON-MODE-SENTINEL".to_string(),
        system_prompt_tool: "TOOL-MODE-SENTINEL".to_string(),
    };
    let _ = gateway.native_complete(&context).await;

    let calls = transport.calls.lock().expect("calls lock").clone();
    assert_eq!(calls.len(), 1, "native drives exactly one request");
    let body = calls[0]
        .body
        .as_ref()
        .map(|body| body.to_string())
        .unwrap_or_default();
    assert!(
        body.contains("JSON-MODE-SENTINEL"),
        "the JSON-mode system prompt is the native one"
    );
    assert!(
        !body.contains("TOOL-MODE-SENTINEL"),
        "the tool-call prompt belongs to the absent external arm"
    );

    // No manual block ⇒ no arm at all.
    let home = common::TestDir::new("profile-learning-manual-home-empty");
    let empty = load_config(home.path());
    assert!(build_learning_gateway(&empty, None, None).is_none());
}

#[mem_store::tokio::test(
    flavor = "multi_thread",
    worker_threads = 2,
    crate = "mem_store::tokio"
)]
async fn test_concurrent_round_drives_are_serialized_by_the_round_flag() {
    let dir = common::TestDir::new("profile-learning-serialize");
    let sink = Arc::new(SpySink::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let gateway = SpyGateway::parked(entered.clone(), release.clone());
    let (learning, _prompts, writes) =
        build_learner(dir.path(), sink, gateway.clone(), one_prompt_ledger(), 1);

    assert!(learning.arm("session-1", 0));
    let first = tokio::spawn({
        let learning = learning.clone();
        async move { learning.run_due(10_000, &NEVER_SHUTDOWN).await }
    });
    entered.notified().await;

    // The re-arm lands WHILE the first drive is inside the provider. It must
    // not block: no scheduler guard is held across the drive.
    assert!(learning.arm("session-1", 10_000));

    let second = learning.run_due(20_000, &NEVER_SHUTDOWN).await;
    assert_eq!(second.due, 1, "the re-armed session is due again");
    assert!(
        second.outcomes.contains(&LearningOutcome::AlreadyRunning),
        "the in-process flag bounces the concurrent drive: {:?}",
        second.outcomes
    );

    release.notify_one();
    let first = first.await.expect("the parked drive returns");
    assert_eq!(first.completed, 1);
    assert_eq!(gateway.calls(), 1, "one gateway call, not two");
    assert_eq!(
        writes.load(Ordering::Acquire),
        1,
        "one profile write, not two"
    );
}

// ---------------------------------------------------------------------------
// Logging discipline and typed failures
// ---------------------------------------------------------------------------

/// The process-global sink, installed exactly once per binary.
fn sink() -> mem_core::logging::LogSink {
    static ONCE: OnceLock<mem_core::logging::LogSink> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = common::process_temp_dir("profile-learning-log");
        let sink = mem_core::logging::LogSink::new(dir.join("opencode-mem.log"));
        mem_core::logging::init_process_logging(sink.clone())
            .expect("the per-binary sink installs");
        sink
    })
    .clone()
}

/// The rendered lines of the per-binary sink file.
fn sink_text() -> String {
    std::fs::read_to_string(sink().path()).unwrap_or_default()
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_learning_canary_never_enters_any_log_channel() {
    // Install the process sink BEFORE the run: a line emitted through the
    // facade while the learner records or drives has a live subscriber.
    let _ = sink();
    let dir = common::TestDir::new("profile-learning-canary");
    let spy = Arc::new(SpySink::default());
    let (learning, _prompts, _writes) = build_learner(
        dir.path(),
        spy.clone(),
        SpyGateway::failing(),
        one_prompt_ledger(),
        1,
    );

    // Both the recording path and the failing round run.
    let outcome = learning
        .record(&params(Some("p-canary"), "CANARY-PROMPT-7f3a"), None)
        .await;
    assert!(
        matches!(outcome, RecordOutcome::Recorded { .. }),
        "{outcome:?}"
    );
    assert!(learning.arm("session-1", 0));
    let tick = learning.run_due(10_000, &NEVER_SHUTDOWN).await;
    assert_eq!(tick.failed, 1, "the provider fault is typed, not a panic");

    // (1) The positive control proves the facade → layer → file route is live
    // and writes synchronously, so the absence below is evidence.
    mem_core::logging::log("canary-control", None);
    let text = sink_text();
    assert!(
        text.contains("canary-control"),
        "the control line must land in the sink file before the absence is read"
    );

    // (2) The prompt body appears on no channel.
    assert!(
        !text.contains("CANARY-PROMPT-7f3a"),
        "the prompt body must never reach the log file"
    );
    assert!(spy.emitted() >= 1, "the learner reports through the sink");
    assert!(
        !spy.all_text().contains("CANARY-PROMPT-7f3a"),
        "no spy event may carry the prompt body"
    );
}

/// Issue 174: the resolved identity is storage, never diagnostics. The
/// existing canary covers the prompt BODY; this one covers the identity on
/// every channel the learner can reach — the dynamic spy sink AND the process
/// `LogSink` file, with a positive control proving the file route is live.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_learning_identity_never_enters_any_log_channel() {
    const CANARY_IDENTITY: &str = "canary-identity@example.invalid";

    let _ = sink();
    let dir = common::TestDir::new("profile-learning-identity-canary");
    let spy = Arc::new(SpySink::default());
    let (learning, prompts, _writes) = build_learner(
        dir.path(),
        spy.clone(),
        SpyGateway::failing(),
        one_prompt_ledger(),
        1,
    );

    let outcome = learning
        .record(
            &params(Some("p-identity-canary"), "a canary turn"),
            Some(CANARY_IDENTITY),
        )
        .await;
    assert!(
        matches!(outcome, RecordOutcome::Recorded { .. }),
        "{outcome:?}"
    );
    // Storage is the ONE place the identity belongs.
    assert_eq!(
        prompts
            .get_prompts_for_user_learning(10, CANARY_IDENTITY)
            .await
            .expect("select by the canary identity")
            .len(),
        1,
        "the identity must reach the row — absence below is not absence everywhere"
    );

    assert!(learning.arm("session-1", 0));
    let tick = learning.run_due(10_000, &NEVER_SHUTDOWN).await;
    assert_eq!(tick.failed, 1, "the provider fault is typed, not a panic");

    mem_core::logging::log("identity-canary-control", None);
    let text = sink_text();
    assert!(
        text.contains("identity-canary-control"),
        "the control line must land in the sink file before the absence is read"
    );
    assert!(
        !text.contains(CANARY_IDENTITY),
        "the identity must never reach the log file"
    );
    assert!(spy.emitted() >= 1, "the learner reports through the sink");
    assert!(
        !spy.all_text().contains(CANARY_IDENTITY),
        "no event detail may carry the identity"
    );
}

// ---------------------------------------------------------------------------
// The static half of the identity canary: a literal-aware source scan
// ---------------------------------------------------------------------------

const SRC: &str = include_str!("../src/profile_learning.rs");
const FORBIDDEN: [&str; 6] = [
    "tracing::",
    "structured_stderr",
    "mem_core::logging",
    "println!",
    "eprintln!",
    "dbg!",
];

/// Length of the char literal starting at `at` (`at` is the opening `'`), or
/// `None` when this `'` is a lifetime tick rather than a literal.
fn char_literal_len(src: &[char], at: usize) -> Option<usize> {
    let mut j = at + 1;
    if src.get(j) == Some(&'\\') {
        j += 1;
        if src.get(j) == Some(&'u') && src.get(j + 1) == Some(&'{') {
            j += 2;
            while j < src.len() && src[j] != '}' {
                j += 1;
            }
            j += 1;
        } else {
            j += 1;
        }
    } else {
        j += 1;
    }
    (src.get(j) == Some(&'\'')).then_some(j + 1 - at)
}

/// The source with comments dropped and literal CONTENTS blanked (quotes
/// kept, byte offsets stable), so a token may appear in prose but never hide
/// behind a string or sit inside one.
fn code_only(text: &str) -> String {
    let src: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < src.len() {
        match src[i] {
            '/' if src.get(i + 1) == Some(&'/') => {
                while i < src.len() && src[i] != '\n' {
                    i += 1;
                }
            }
            '/' if src.get(i + 1) == Some(&'*') => {
                let mut depth = 1usize;
                i += 2;
                while i < src.len() && depth > 0 {
                    if src[i] == '/' && src.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if src[i] == '*' && src.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            '"' => {
                out.push('"');
                i += 1;
                while i < src.len() {
                    if src[i] == '\\' {
                        out.push(' ');
                        out.push(' ');
                        i += 2;
                    } else if src[i] == '"' {
                        out.push('"');
                        i += 1;
                        break;
                    } else {
                        out.push(' ');
                        i += 1;
                    }
                }
            }
            '\'' => match char_literal_len(&src, i) {
                Some(len) => {
                    out.push('\'');
                    for _ in 1..len - 1 {
                        out.push(' ');
                    }
                    out.push('\'');
                    i += len;
                }
                None => {
                    out.push('\'');
                    i += 1;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn forbidden_hits(text: &str) -> Vec<&'static str> {
    let code = code_only(text);
    FORBIDDEN
        .iter()
        .copied()
        .filter(|token| code.contains(token))
        .collect()
}

/// Issue 174 (r4): the scan must be literal- and comment-aware — a line-prefix
/// stripper hides a real call behind a `//` in a URL literal, and a raw
/// `contains` reds on the module doc. All eight cases run in one test.
#[test]
fn test_identity_scan_strips_comments_but_reds_on_code() {
    // (a) the real file is clean.
    assert_eq!(forbidden_hits(SRC), Vec::<&str>::new());
    // (b) the stripping is load-bearing: the module doc names the tokens.
    assert!(SRC.contains("tracing::") && SRC.contains("structured_stderr"));
    // (c) a leading-comment mention stays green.
    assert!(forbidden_hits("//! no tracing:: here").is_empty());
    assert!(forbidden_hits("/// docs: no structured_stderr").is_empty());
    // (d) plain code reds.
    assert_eq!(
        forbidden_hits("fn f() { tracing::info!(\"x\"); }"),
        vec!["tracing::"]
    );
    // (e) THE ADVERSARY: a literal before the call must not hide it.
    assert!(forbidden_hits("let u = \"http://x\"; tracing::info!(\"y\");").contains(&"tracing::"));
    // (f) literal contents are blanked, so a message naming a token stays green.
    assert!(forbidden_hits("let s = \"tracing::\";").is_empty());
    // (g) char literal vs lifetime, and a macro inside code after a char.
    assert!(forbidden_hits("let c = '/'; // dbg!\n").is_empty());
    assert!(forbidden_hits("let r = 'a'; eprintln!(\"x\");").contains(&"eprintln!"));
    // (h) no multi-hash raw string in the scanned file: the scanner's
    // documented gap stays loud the day one appears.
    assert!(
        !SRC.contains("r#\"") && !SRC.contains("br#\""),
        "extend the scanner before adding r#…"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_round_failures_are_typed_not_panics() {
    let dir = common::TestDir::new("profile-learning-typed");
    let sink = Arc::new(SpySink::default());
    let (learning, _prompts, writes) = build_learner(
        dir.path(),
        sink.clone(),
        SpyGateway::failing(),
        one_prompt_ledger(),
        1,
    );

    assert!(learning.arm("session-1", 0));
    let tick = learning.run_due(10_000, &NEVER_SHUTDOWN).await;
    assert_eq!(tick.due, 1);
    assert_eq!(tick.failed, 1, "a provider fault is a typed failure");
    assert_eq!(tick.completed, 0);
    assert_eq!(writes.load(Ordering::Acquire), 0, "nothing is written");
    assert_eq!(sink.count("profile.learning_round_failed"), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_round_failure_event_carries_the_closed_reason_token() {
    // Two faults, two variants, two tokens: a hardcoded literal cannot satisfy
    // both rows, so this is a test of `failure_reason()`, not of the event shape.
    for (gateway, expected) in [
        (SpyGateway::failing(), "analysis"),
        (SpyGateway::invalid_json(), "json"),
    ] {
        let dir = common::TestDir::new("profile-learning-reason");
        let sink = Arc::new(SpySink::default());
        let (learning, _prompts, _writes) =
            build_learner(dir.path(), sink.clone(), gateway, one_prompt_ledger(), 1);
        assert!(learning.arm("session-1", 0));
        let tick = learning.run_due(10_000, &NEVER_SHUTDOWN).await;
        assert_eq!(tick.failed, 1, "{expected}");
        let detail = sink
            .detail("profile.learning_round_failed")
            .expect("the failure event");
        assert_eq!(detail["kind"], "round");
        let reason = detail["reason"]
            .as_str()
            .expect("a closed-set reason token");
        assert_eq!(
            reason, expected,
            "a hardcoded token cannot satisfy both rows"
        );
        assert!(
            mem_profile::error::failure_reason::ALL.contains(&reason),
            "{reason} is not in the closed set"
        );
        assert!(
            !detail.to_string().contains("provider fault"),
            "the provider message never enters the event"
        );
    }
}

#[mem_store::tokio::test(
    flavor = "multi_thread",
    worker_threads = 2,
    crate = "mem_store::tokio"
)]
async fn test_shutdown_flag_stops_run_due_between_rounds() {
    let dir = common::TestDir::new("profile-learning-shutdown");
    let sink = Arc::new(SpySink::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let gateway = SpyGateway::parked(entered.clone(), release.clone());
    let (learning, _prompts, _writes) =
        build_learner(dir.path(), sink, gateway.clone(), one_prompt_ledger(), 1);
    assert!(learning.arm("session-1", 0));
    assert!(learning.arm("session-2", 0));

    let shutdown = Arc::new(AtomicBool::new(false));
    let drive = tokio::spawn({
        let learning = learning.clone();
        let shutdown = shutdown.clone();
        async move { learning.run_due(10_000, &shutdown).await }
    });
    entered.notified().await; // round 1 is inside the provider
    shutdown.store(true, Ordering::SeqCst);
    release.notify_one();
    let tick = drive.await.expect("the drive returns");

    assert_eq!(tick.due, 2, "both arms were due");
    assert_eq!(tick.outcomes.len(), 1, "only the in-flight round ran");
    assert_eq!(gateway.calls(), 1, "the second round never starts");
}
