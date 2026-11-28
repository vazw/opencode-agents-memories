//! Profile-learning wiring: prompt recording, the idle debounce, and the
//! round driver (issue 48, A1; blueprint r4).
//!
//! Four links land here:
//!
//! 1. [`ProfileLearning::record`] persists the turn's user prompt from the
//!    capture payload that already crosses the boundary — stripped and
//!    injection-filtered **before** the write (D-1…D-4, D-52-3 order), keyed by
//!    `metadata.promptId` so a retry is one row (D-3).
//! 2. [`ProfileLearning::arm`] arms the pure [`IdleScheduler`] per accepted
//!    capture; a re-arm REPLACES the deadline and bumps the generation (D-6).
//! 3. [`ProfileLearning::run_due`] is the only `mem-bridge` caller of
//!    `IdleScheduler::poll_due` (D-15): it polls, drops the scheduler mutex,
//!    then drives the round — a guard held across the drive would deadlock the
//!    R9 fixture's re-arm.
//! 4. [`build_profile_learning`] constructs the one [`LearningRound`] the tree
//!    owns, choosing the provider arm per D-7/D-13.
//!
//! **Logging discipline (D-14).** Every diagnostic in this module goes through
//! the injected [`RoundEventSink`]; this file opens NO other channel — no
//! `tracing::` macro, no `mem_core::logging::log`, no `structured_stderr`. The
//! production sink lives in `composition.rs`. A static scan enforces this.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mem_ai::privacy::PrivacyConfig;
use mem_ai::provider::{AiProviderStructured, ChatMessage, CompletionRequest};
use mem_core::config::Config;
use mem_core::sync::lock_or_recover;
use mem_store::db::BoxFuture;

use mem_profile::error::ProfileError;
use mem_profile::idle_scheduler::{IdleGates, IdleJob, IdleScheduler};
use mem_profile::learning_lock::LearningLock;
use mem_profile::learning_service::{
    AnalysisError, LearningOutcome, LearningRound, ProfileWriterAdapter, PromptLedgerAdapter,
    RoundEventSink,
};
use mem_profile::profile_store::ProfileStore;
use mem_profile::prompt::UserPromptManager;
use mem_profile::provider_gateway::{
    AiProviderCompletion, ProviderAnalysisGateway, StructuredCompletion,
};

use crate::capture::CaptureParams;
use crate::host_provider::LazyHostProvider;

/// How one `record` call ended. Content never appears here or in any event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// One row written (or the existing row returned on an identical retry).
    Recorded { prompt_row_id: String },
    /// No `metadata.promptId`: no row, never a synthetic id (D-3).
    SkippedNoPromptId,
    /// The payload carried no user prompt at all.
    SkippedNoContent,
    /// The prompt was blank after strip + filter.
    SkippedBlank,
    /// The double gate rejected fully-private content.
    SkippedPrivate,
    /// The store refused the write; typed, never a panic.
    Failed,
}

/// One tick's aggregate, for the engine's counts-only log line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LearningTick {
    /// Due arms the scheduler fired.
    pub due: usize,
    /// Rounds that reached `Completed`.
    pub completed: usize,
    /// Rounds that returned a typed error.
    pub failed: usize,
    /// Every round's outcome, in drive order (tests inspect this).
    pub outcomes: Vec<LearningOutcome>,
}

/// The native structured handle for profile analysis: the lazily-resolved
/// OpenCode host, with the same invalidate-on-ANY-error rule as
/// `HostStructured` (a stranger on a recycled port must not be cached).
pub struct LazyHostCompletion {
    lazy: Arc<LazyHostProvider>,
}

impl LazyHostCompletion {
    /// Build the adapter. No I/O: the host resolves on the first call.
    pub fn new(lazy: Arc<LazyHostProvider>) -> Self {
        Self { lazy }
    }
}

impl StructuredCompletion for LazyHostCompletion {
    fn complete<'a>(
        &'a self,
        system_prompt: &'a str,
        user_prompt: &'a str,
        schema: serde_json::Value,
    ) -> BoxFuture<'a, Result<serde_json::Value, AnalysisError>> {
        Box::pin(async move {
            // The host provider names its own wiring model; the request model is
            // ignored (`OpenCodeHostProvider`), so an empty one is correct here.
            let request = CompletionRequest::new(String::new())
                .with_system(system_prompt)
                .with_message(ChatMessage::user(user_prompt))
                .with_json_schema(schema.clone());
            let inner = match self.lazy.resolved() {
                Ok(inner) => inner,
                Err(error) => {
                    self.lazy.invalidate();
                    return Err(AnalysisError::Provider(error.to_string()));
                }
            };
            match inner.complete_structured(request, schema).await {
                Ok(value) => Ok(value),
                Err(error) => {
                    self.lazy.invalidate();
                    Err(AnalysisError::Provider(error.to_string()))
                }
            }
        })
    }
}

/// The wired learner: one round, the prompt ledger it records into, the privacy
/// policy applied at the record seam, and the pure debounce scheduler.
pub struct ProfileLearning {
    round: Arc<LearningRound>,
    prompts: Arc<UserPromptManager>,
    privacy: PrivacyConfig,
    events: Arc<dyn RoundEventSink>,
    scheduler: Mutex<IdleScheduler>,
    user_id: String,
}

impl ProfileLearning {
    /// The testing seam (D-16): inject a spy round and a spy sink.
    pub fn new(
        round: Arc<LearningRound>,
        prompts: Arc<UserPromptManager>,
        privacy: PrivacyConfig,
        events: Arc<dyn RoundEventSink>,
    ) -> Self {
        Self {
            round,
            prompts,
            privacy,
            events,
            scheduler: Mutex::new(IdleScheduler::new()),
            user_id: String::new(),
        }
    }

    /// Records the resolved identity for `user_id()` (builder style).
    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = user_id.into();
        self
    }

    /// The identity this learner analyzes for.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Persists the turn's user prompt from an accepted capture.
    ///
    /// Content is `strip_then_filter_text`ed BEFORE the write (D-4), so
    /// boilerplate never reaches disk and private spans are stripped; the
    /// store's own gate is the second gate. Absent prompt id or blank result ⇒
    /// no row, a typed skip (D-3).
    ///
    /// Issue 174: `user_id` is the accept-time identity the engine resolved
    /// ONCE for this capture. This method performs no resolution and never
    /// puts it in an event detail or log (the identity canary pins that).
    pub async fn record(&self, params: &CaptureParams, user_id: Option<&str>) -> RecordOutcome {
        let Some(prompt_id) = params.prompt_id() else {
            self.events.emit(
                "profile.learning_record_skipped",
                serde_json::json!({ "kind": "no_prompt_id" }),
            );
            return RecordOutcome::SkippedNoPromptId;
        };
        let Some(raw) = params.user_prompt() else {
            self.events.emit(
                "profile.learning_record_skipped",
                serde_json::json!({ "kind": "no_content" }),
            );
            return RecordOutcome::SkippedNoContent;
        };
        let filtered = self.privacy.strip_then_filter_text(raw);
        if filtered.trim().is_empty() {
            self.events.emit(
                "profile.learning_record_skipped",
                serde_json::json!({ "kind": "blank" }),
            );
            return RecordOutcome::SkippedBlank;
        }
        match self
            .prompts
            .save_prompt(
                &params.session_id,
                &prompt_id,
                &params.project_id,
                &filtered,
                user_id,
            )
            .await
        {
            Ok(prompt_row_id) => {
                self.events.emit(
                    "profile.learning_prompt_recorded",
                    serde_json::json!({ "kind": "recorded" }),
                );
                RecordOutcome::Recorded { prompt_row_id }
            }
            Err(ProfileError::PrivateContentBlocked) => {
                self.events.emit(
                    "profile.learning_record_skipped",
                    serde_json::json!({ "kind": "private" }),
                );
                RecordOutcome::SkippedPrivate
            }
            Err(_) => {
                self.events.emit(
                    "profile.learning_record_failed",
                    serde_json::json!({ "kind": "store" }),
                );
                RecordOutcome::Failed
            }
        }
    }

    /// Arms the debounce for `session_id` at `now` (the engine's `now_millis()`).
    ///
    /// Returns `true` when an arm is pending. A re-arm REPLACES the deadline
    /// (D-6): arm@2000 on a 10 s debounce ⇒ deadline 12 000, not 10 000.
    pub fn arm(&self, session_id: &str, now: i64) -> bool {
        let mut scheduler = lock_or_recover(&self.scheduler);
        scheduler.on_session_idle(session_id, IdleGates::default(), now)
    }

    /// Fires every due arm, drives at most one round per due session.
    ///
    /// The scheduler mutex is held ONLY across `poll_due` and dropped BEFORE
    /// the round is driven (D-15), so the R9 fixture's re-`arm` can proceed
    /// while a round is in flight. `poll_due` is the sole removal edge.
    ///
    /// `shutdown` is re-read before EVERY round, not once per tick: a teardown
    /// that flips the flag while a slow provider is in flight stops the next
    /// arm instead of driving it. An arm polled and then skipped stays
    /// unanalyzed; the next boot's select reselects it, so nothing is lost.
    pub async fn run_due(&self, now: i64, shutdown: &AtomicBool) -> LearningTick {
        let due = {
            let mut scheduler = lock_or_recover(&self.scheduler);
            scheduler.poll_due(now)
        };
        let mut tick = LearningTick {
            due: due.len(),
            ..LearningTick::default()
        };
        for (_session_id, jobs) in due {
            if !jobs.contains(&IdleJob::Learn) {
                continue;
            }
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            match self.round.run(now).await {
                Ok(outcome) => {
                    if matches!(outcome, LearningOutcome::Completed { .. }) {
                        tick.completed += 1;
                    }
                    tick.outcomes.push(outcome);
                }
                Err(error) => {
                    tick.failed += 1;
                    self.events.emit(
                        "profile.learning_round_failed",
                        serde_json::json!({
                            "kind": "round",
                            "reason": error.failure_reason(),
                        }),
                    );
                }
            }
        }
        tick
    }
}

/// Selects the learner's analysis gateway per D-7/D-13, or `None` when no
/// provider arm exists.
///
/// - Arm 1 — `host` is `Some`: native = the live host ([`LazyHostCompletion`]),
///   external = the manual provider when one is complete.
/// - Arm 2 — `host` is `None` but a manual completion exists: the manual
///   completion IS the native arm, external `None`, so `external_configured()`
///   is `false` and a provider fault propagates instead of an impossible
///   fallback.
/// - Arm 3 — neither: `None` (the caller emits one text-free event).
///
/// `manual_override` is the test seam: production passes `None` and the manual
/// provider is built from `config`.
pub fn build_learning_gateway(
    config: &Config,
    host: Option<Arc<LazyHostProvider>>,
    manual_override: Option<Arc<dyn AiProviderStructured>>,
) -> Option<Arc<dyn mem_profile::learning_service::AnalysisGateway>> {
    let manual = match manual_override {
        Some(provider) => Some(provider),
        None => crate::manual_provider::manual_capture_provider(config)
            .ok()
            .flatten(),
    };
    let model = config.memory_model.clone().unwrap_or_default();
    let manual_completion: Option<Arc<dyn StructuredCompletion>> = manual.map(|provider| {
        Arc::new(AiProviderCompletion::new(provider, model.clone()))
            as Arc<dyn StructuredCompletion>
    });
    match host {
        Some(host) => {
            let native = Arc::new(LazyHostCompletion::new(host)) as Arc<dyn StructuredCompletion>;
            Some(Arc::new(ProviderAnalysisGateway::new(
                native,
                manual_completion,
            )))
        }
        // Arm 2: the manual completion is the native arm; never synthesize an
        // external fallback that cannot exist.
        None => manual_completion.map(|native| {
            Arc::new(ProviderAnalysisGateway::new(native, None))
                as Arc<dyn mem_profile::learning_service::AnalysisGateway>
        }),
    }
}

/// Builds the one learner the composition root owns, or `None` when the
/// identity is unknown or no provider arm exists (one text-free event each).
///
/// `resolved_user_id` is an injected input (D-5): the caller resolves it
/// through `mem_bridge::identity`, never inside this constructor, so the
/// negative path is hermetic.
pub fn build_profile_learning(
    config: &Config,
    resolved_user_id: &str,
    prompts: Arc<UserPromptManager>,
    profiles: Arc<ProfileStore>,
    lock: Arc<LearningLock>,
    host: Option<Arc<LazyHostProvider>>,
    events: Arc<dyn RoundEventSink>,
) -> Option<ProfileLearning> {
    let user_id = resolved_user_id.trim();
    if user_id.is_empty() || user_id == crate::identity::UNKNOWN_USER_ID {
        events.emit("profile.learning_identity_unknown", serde_json::json!({}));
        return None;
    }
    // Cloned so the same event sink is both the round's sink and the learner's.
    let gateway = build_learning_gateway(config, host, None);
    let Some(gateway) = gateway else {
        events.emit("profile.learning_no_provider", serde_json::json!({}));
        return None;
    };
    let ledger = Arc::new(PromptLedgerAdapter::new(prompts.clone()));
    let writer = Arc::new(ProfileWriterAdapter::new(profiles));
    let threshold = (config.user_profile_analysis_interval as i64).max(1);
    // D-8: a learner is built only when an arm exists, so readiness is the
    // construction-time fact; a later provider fault surfaces as the round's
    // typed error and retries on the next arm.
    let readiness: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| true);
    let round = LearningRound::new(
        ledger,
        writer,
        gateway,
        lock,
        readiness,
        threshold,
        user_id.to_string(),
    )
    .with_event_sink(events.clone())
    .with_language_config(config.auto_capture_language.clone())
    .with_max_context_chars(config.user_profile_max_context_bytes as usize);
    Some(
        ProfileLearning::new(
            Arc::new(round),
            prompts,
            PrivacyConfig::from_core(config),
            events,
        )
        .with_user_id(user_id),
    )
}
