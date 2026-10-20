//! The profile-learning service shell (W3.1 step 11) — port of upstream
//! `src/services/user-memory-learning.ts` L59-330 (`performUserProfileLearning`).
//!
//! The round is, in order: **lock → count → select → analyze → merge → update
//! → mark**, with two nested guards:
//!
//! 1. an in-process `AtomicBool` flag raised **before the first await** so a
//!    same-process re-entry bounces instead of queueing a second analysis;
//! 2. the cross-process coordination-DB lock, so two instances sharing one
//!    storage never analyze the same prompts (contention skips the round; the
//!    next idle event retries).
//!
//! Finally semantics mirror upstream's nested `finally`: the lock is released
//! first, THEN the flag resets — the flag stays raised while the release is in
//! flight, so a same-process re-entry cannot slip in and find the cross-process
//! lock already gone.
//!
//! Everything the round touches at its boundaries is an injectable trait
//! (`PromptLedger`, `ProfileWriter`, `AnalysisGateway`); W3.1 tests inject
//! mocks, W3.2 supplies the real AI-provider adapter.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mem_store::db::BoxFuture;

use crate::error::ProfileError;
use crate::learning_lock::LearningLock;
use crate::profile_store::{Profile, ProfileData, ProfileUpdates};
use crate::prompt::UserPrompt;

/// Upstream `MAX_RETRIES` for the optimistic-version update loop.
pub const UPDATE_MAX_RETRIES: usize = 2;

// ---------------------------------------------------------------------------
// Boundaries
// ---------------------------------------------------------------------------

/// The prompt-side boundary (real adapter: [`UserPromptManager`]).
///
/// [`UserPromptManager`]: crate::prompt::UserPromptManager
pub trait PromptLedger: Send + Sync {
    fn count_unanalyzed(&self) -> BoxFuture<'_, Result<i64, ProfileError>>;
    fn get_prompts_for_learning(
        &self,
        limit: i64,
    ) -> BoxFuture<'_, Result<Vec<UserPrompt>, ProfileError>>;
    fn mark_many(&self, ids: Vec<String>) -> BoxFuture<'_, Result<(), ProfileError>>;
}

/// The profile-side boundary (real adapter: [`ProfileStore`]).
///
/// [`ProfileStore`]: crate::profile_store::ProfileStore
pub trait ProfileWriter: Send + Sync {
    fn get_active_profile<'a>(
        &'a self,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Profile>, ProfileError>>;
    fn merge_profile_data<'a>(
        &'a self,
        existing: &'a ProfileData,
        updates: &'a ProfileUpdates,
        profile_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<ProfileData, ProfileError>>;
    fn update_profile<'a>(
        &'a self,
        profile_id: &'a str,
        data: &'a ProfileData,
        prompts_analyzed: i64,
        change_summary: &'a str,
    ) -> BoxFuture<'a, Result<bool, ProfileError>>;
    fn create_profile<'a>(
        &'a self,
        user_id: &'a str,
        display_name: &'a str,
        user_name: &'a str,
        user_email: &'a str,
        data: &'a ProfileData,
        prompts_analyzed: i64,
    ) -> BoxFuture<'a, Result<String, ProfileError>>;
}

/// The analysis boundary: native (in-host provider) and external (remote
/// provider) completions, both returning structured-JSON text.
pub trait AnalysisGateway: Send + Sync {
    fn native_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>>;
    fn external_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>>;

    /// Whether an external provider is configured. Additive default (`true`)
    /// keeps W3.1 mocks source-compatible; the real gateway overrides it with
    /// `external.is_some()`.
    ///
    /// When `false`, a native provider fault is unrecoverable and propagates
    /// verbatim instead of attempting a fallback that cannot exist — the
    /// place upstream would synthesize `External API not configured`.
    fn external_configured(&self) -> bool {
        true
    }
}

/// The analysis input (upstream `buildUserAnalysisContext` inputs plus the
/// pre-built prompts).
///
/// Both system prompts are built by the round (not by the transport) so a
/// gateway — real or spy — cannot silently pick the wrong mode: the native
/// structured-output transport uses `system_prompt_json`, the external
/// tool-call transport uses `system_prompt_tool`.
#[derive(Debug, Clone)]
pub struct AnalysisContext {
    pub prompts: Vec<UserPrompt>,
    pub existing_profile: Option<Profile>,
    /// The resolved output language name (`langName` upstream).
    pub lang_name: String,
    /// The assembled user prompt (`userPrompt` upstream), capped and possibly
    /// closed with the truncation marker.
    pub user_prompt: String,
    /// System prompt for the structured-output transport (no tool mention).
    pub system_prompt_json: String,
    /// System prompt for the external tool-call transport (`update_user_profile`).
    pub system_prompt_tool: String,
}

/// A provider/parse/storage fault on the analysis path.
///
/// Only [`AnalysisError::Provider`] is fallback-eligible: a provider fault
/// means "try the other provider", while a stored-profile/storage fault must
/// propagate (falling back would mask a storage defect and could clobber the
/// profile with a fresh one).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnalysisError {
    #[error("{0}")]
    Provider(String),
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Parse(String),
}

impl From<AnalysisError> for ProfileError {
    fn from(error: AnalysisError) -> Self {
        match error {
            AnalysisError::Provider(message) => ProfileError::Analysis(message),
            AnalysisError::Storage(message) => ProfileError::Save(message),
            AnalysisError::Parse(message) => ProfileError::Save(message),
        }
    }
}

/// Observability sink for the round (upstream `log()` calls; the fixture
/// asserts event ordering through it).
pub trait RoundEventSink: Send + Sync {
    fn emit(&self, event: &str, detail: serde_json::Value);
}

/// Discards every event (production default).
pub struct NoopEventSink;

impl RoundEventSink for NoopEventSink {
    fn emit(&self, _event: &str, _detail: serde_json::Value) {}
}

// ---------------------------------------------------------------------------
// Round
// ---------------------------------------------------------------------------

/// How a round ended (observable, and reported by the fixture children).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearningOutcome {
    /// The profile was created or updated and the batch was marked.
    Completed { prompts: usize },
    /// Another process (or a lost CAS race) holds the lock.
    Contention,
    /// A same-process re-entry bounced off the in-process flag.
    AlreadyRunning,
    /// The provider readiness gate said no.
    NotReady,
    /// Fewer unanalyzed prompts than the configured threshold.
    BelowThreshold { count: i64 },
    /// No prompts were selected at the threshold.
    NoPrompts,
}

/// One learning round (upstream's module-level singleton + function).
pub struct LearningRound {
    flag: Arc<AtomicBool>,
    ledger: Arc<dyn PromptLedger>,
    profiles: Arc<dyn ProfileWriter>,
    gateway: Arc<dyn AnalysisGateway>,
    lock: Arc<LearningLock>,
    readiness: Arc<dyn Fn() -> bool + Send + Sync>,
    events: Arc<dyn RoundEventSink>,
    threshold: i64,
    user_id: String,
    language_config: Option<String>,
    max_context_chars: usize,
    /// The incremental cleanup seam (R6/R13). `None` leaves the round exactly
    /// as W3.2 shipped it — nothing is constructed in production this wave.
    cleanup: Option<Arc<dyn crate::incremental_cleanup::IncrementalCleanup>>,
    cleanup_enabled: bool,
    cleanup_interval: f64,
}

impl LearningRound {
    pub fn new(
        ledger: Arc<dyn PromptLedger>,
        profiles: Arc<dyn ProfileWriter>,
        gateway: Arc<dyn AnalysisGateway>,
        lock: Arc<LearningLock>,
        readiness: Arc<dyn Fn() -> bool + Send + Sync>,
        threshold: i64,
        user_id: impl Into<String>,
    ) -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            ledger,
            profiles,
            gateway,
            lock,
            readiness,
            events: Arc::new(NoopEventSink),
            threshold,
            user_id: user_id.into(),
            language_config: None,
            max_context_chars: crate::analysis_prompt::DEFAULT_MAX_CONTEXT_CHARS,
            cleanup: None,
            cleanup_enabled: false,
            cleanup_interval: 100.0,
        }
    }

    /// Sets the configured output language (`autoCaptureLanguage`: an explicit
    /// ISO 639-1 code, `"auto"`, or absent ⇒ detect). Builder style.
    pub fn with_language_config(mut self, language_config: Option<String>) -> Self {
        self.language_config = language_config;
        self
    }

    /// Sets the whole-context char cap (`userProfileMaxContextBytes`,
    /// default [`crate::analysis_prompt::DEFAULT_MAX_CONTEXT_CHARS`]). Builder
    /// style.
    pub fn with_max_context_chars(mut self, max_context_chars: usize) -> Self {
        self.max_context_chars = max_context_chars;
        self
    }

    /// Upstream `resolveProfileLanguageName` for this round's prompt batch:
    /// explicit config wins, otherwise detection over the joined raw contents.
    pub fn resolve_profile_language_name(&self, prompts: &[UserPrompt]) -> String {
        crate::language_detector::resolve_profile_language_name(
            prompts,
            self.language_config.as_deref(),
        )
        .to_string()
    }

    /// Sets the observability sink (builder style).
    pub fn with_event_sink(mut self, events: Arc<dyn RoundEventSink>) -> Self {
        self.events = events;
        self
    }

    /// Installs the incremental cleanup hook (builder style). `new` leaves it
    /// unset, so a round without this builder behaves exactly like W3.2's.
    pub fn with_incremental_cleanup(
        mut self,
        cleanup: Arc<dyn crate::incremental_cleanup::IncrementalCleanup>,
        enabled: bool,
        interval: f64,
    ) -> Self {
        self.cleanup = Some(cleanup);
        self.cleanup_enabled = enabled;
        self.cleanup_interval = interval;
        self
    }

    /// The in-process re-entry flag (shared with fixtures that prove the
    /// pre-first-await set).
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }

    /// Upstream `performUserProfileLearning` minus the plugin `ctx`.
    pub async fn run(&self, now: i64) -> Result<LearningOutcome, ProfileError> {
        // 1. Readiness BEFORE anything else (upstream logs and returns).
        if !(self.readiness)() {
            self.events.emit(
                "log",
                serde_json::json!({"message": "skipped (provider not ready)"}),
            );
            return Ok(LearningOutcome::NotReady);
        }

        // 2. Set BEFORE the first await so a same-process re-entry bounces
        //    instead of queueing behind this run.
        if self
            .flag
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(LearningOutcome::AlreadyRunning);
        }

        // 3. Nested finally: release() first, THEN reset the flag.
        let mut guard = None;
        let outcome = self.run_locked(&mut guard, now).await;
        if let Some(guard) = guard.take() {
            guard.release().await;
        }
        self.flag.store(false, Ordering::Release);
        outcome
    }

    async fn run_locked(
        &self,
        guard_slot: &mut Option<crate::learning_lock::LearningGuard>,
        now: i64,
    ) -> Result<LearningOutcome, ProfileError> {
        // Lock.
        let Some(guard) = self.lock.try_acquire(None).await else {
            self.events.emit(
                "log",
                serde_json::json!({
                    "message": "skipped (another process holds the learning lock)"
                }),
            );
            return Ok(LearningOutcome::Contention);
        };
        *guard_slot = Some(guard);
        self.events
            .emit("acquired", serde_json::json!({ "at": now }));

        // Count.
        // The ledger boundary owns the `count`/`select`/`mark` events (the
        // fixture mocks emit them); the service owns `acquired` and `log`.
        let count = self.ledger.count_unanalyzed().await?;
        if count < self.threshold {
            return Ok(LearningOutcome::BelowThreshold { count });
        }

        // Select.
        let prompts = self.ledger.get_prompts_for_learning(self.threshold).await?;
        if prompts.is_empty() {
            return Ok(LearningOutcome::NoPrompts);
        }
        let ids: Vec<String> = prompts.iter().map(|prompt| prompt.id.clone()).collect();

        // Analyze (provider errors fall back; parse/storage errors do not).
        //
        // The active profile is fetched AND parsed BEFORE the provider call
        // (upstream `buildUserAnalysisContext` parses there), outside the
        // provider catch: a corrupt stored profile propagates and is never
        // mistaken for a provider fault, so it can neither trigger the
        // fallback nor clobber the profile with a fresh one.
        let existing_profile = self.profiles.get_active_profile(&self.user_id).await?;
        let existing_data = match existing_profile.as_ref() {
            Some(profile) => Some(serde_json::from_str::<ProfileData>(&profile.profile_data)?),
            None => None,
        };

        // Language resolution runs exactly ONCE per round, after select and
        // before any truncation or scaffold: detection sees the raw prompt
        // contents (the English analysis scaffold would bias short non-English
        // batches toward English).
        let lang_name = self.resolve_profile_language_name(&prompts);
        let has_existing = existing_data.is_some();
        let user_prompt = crate::analysis_prompt::build_user_prompt(
            &prompts,
            existing_data.as_ref(),
            self.max_context_chars,
        );
        let system_prompt_json = crate::analysis_prompt::build_system_prompt(
            has_existing,
            &lang_name,
            crate::analysis_prompt::PromptMode::Json,
        );
        let system_prompt_tool = crate::analysis_prompt::build_system_prompt(
            has_existing,
            &lang_name,
            crate::analysis_prompt::PromptMode::Tool,
        );
        let context = AnalysisContext {
            prompts: prompts.clone(),
            existing_profile: existing_profile.clone(),
            lang_name,
            user_prompt,
            system_prompt_json,
            system_prompt_tool,
        };
        let raw = match self.gateway.native_complete(&context).await {
            Ok(raw) => raw,
            Err(AnalysisError::Provider(native_message)) => {
                // Only a provider fault is fallback-eligible, and only when an
                // external provider actually exists. Messages are preserved
                // verbatim in every branch — never prefixed, never synthesized.
                if !self.gateway.external_configured() {
                    return Err(ProfileError::Analysis(native_message));
                }
                match self.gateway.external_complete(&context).await {
                    Ok(raw) => raw,
                    Err(AnalysisError::Provider(external_message)) => {
                        return Err(ProfileError::Analysis(external_message));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        };
        let updates = parse_analysis(&raw)?;

        // Upstream `cleanupPreviousPromptCount = existingProfile.totalPromptsAnalyzed`
        // (`0` on the create path): captured BEFORE the merge/update so the
        // interval gate sees the pre-round count — and before the `match`
        // consumes `existing_data`.
        let previous_prompt_count = existing_profile
            .as_ref()
            .map(|profile| profile.total_prompts_analyzed)
            .unwrap_or(0);

        // Merge → update (or create) → mark.
        match existing_data {
            Some(existing) => {
                // Unreachable by construction (parsed data implies a stored
                // profile); a Save error errs loudly instead of panicking or
                // clobbering the profile with a fresh one.
                let Some(profile) = existing_profile.as_ref() else {
                    return Err(ProfileError::Save(
                        "profile analysis: parsed profile data without a stored profile"
                            .to_string(),
                    ));
                };
                let mut target_id = profile.id.clone();
                let mut working = existing.clone();
                let mut merged = self
                    .profiles
                    .merge_profile_data(&existing, &updates, Some(&target_id))
                    .await?;

                let mut attempts = 0usize;
                let mut updated;
                loop {
                    let summary = generate_change_summary(&working, &merged);
                    updated = self
                        .profiles
                        .update_profile(&target_id, &merged, prompts.len() as i64, &summary)
                        .await?;
                    if updated || attempts >= UPDATE_MAX_RETRIES {
                        break;
                    }
                    attempts += 1;
                    let Some(current) = self.profiles.get_active_profile(&self.user_id).await?
                    else {
                        break;
                    };
                    target_id = current.id.clone();
                    working = serde_json::from_str(&current.profile_data)?;
                    merged = self
                        .profiles
                        .merge_profile_data(&working, &updates, Some(&target_id))
                        .await?;
                }
                if !updated {
                    // Mark anyway so the same prompts are not retried forever.
                    self.events.emit(
                        "log",
                        serde_json::json!({"message": "update conflict: exhausted retries"}),
                    );
                }
            }
            None => {
                let data = updates_to_data(&updates);
                self.profiles
                    .create_profile(
                        &self.user_id,
                        "Unknown",
                        "unknown",
                        &self.user_id,
                        &data,
                        prompts.len() as i64,
                    )
                    .await?;
            }
        }

        self.ledger.mark_many(ids).await?;

        // The incremental cleanup hook (upstream L286-288) runs AFTER the
        // batch is marked and can never fail the round (R11): its error is
        // logged through the round's sink and swallowed.
        if let Some(cleanup) = self.cleanup.as_ref()
            && crate::validation::should_run_automatic_profile_cleanup(
                previous_prompt_count,
                prompts.len() as i64,
                self.cleanup_interval,
                self.cleanup_enabled,
            )
        {
            match cleanup
                .run_incremental(
                    &self.user_id,
                    &updates,
                    previous_prompt_count,
                    prompts.len() as i64,
                    now,
                )
                .await
            {
                Ok(outcome) => self.events.emit(
                    "cleanup",
                    serde_json::json!({
                        "message": "user-profile-learning: automatic cleanup complete",
                        "merged": outcome.merged,
                        "removed": outcome.removed,
                        "updated": outcome.updated,
                    }),
                ),
                Err(error) => self.events.emit(
                    "log",
                    serde_json::json!({
                        "message": "user-profile-learning: automatic cleanup failed",
                        "error": error.to_string(),
                    }),
                ),
            }
        }

        Ok(LearningOutcome::Completed {
            prompts: prompts.len(),
        })
    }
}

/// Parses the gateway's structured JSON into the three category updates.
///
/// The payload is validated first (W3.3/R8): the confidence range and the
/// 3–500 character bounds are enforced on real provider output *before* any
/// merge, so a violating payload yields a parse-class [`ProfileError::Save`]
/// with zero profile writes and zero marks (and is never fallback-eligible).
fn parse_analysis(raw: &str) -> Result<ProfileUpdates, ProfileError> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    crate::validation::validate_analysis_value(&value)
}

fn updates_to_data(updates: &ProfileUpdates) -> ProfileData {
    ProfileData {
        preferences: updates.preferences.clone().unwrap_or_default(),
        patterns: updates.patterns.clone().unwrap_or_default(),
        workflows: updates.workflows.clone().unwrap_or_default(),
        learning_paths: None,
    }
}

/// Upstream `generateChangeSummary`.
pub fn generate_change_summary(old: &ProfileData, new: &ProfileData) -> String {
    let mut changes = Vec::new();
    let preference_diff = new.preferences.len() as i64 - old.preferences.len() as i64;
    if preference_diff > 0 {
        changes.push(format!("+{preference_diff} preferences"));
    }
    let pattern_diff = new.patterns.len() as i64 - old.patterns.len() as i64;
    if pattern_diff > 0 {
        changes.push(format!("+{pattern_diff} patterns"));
    }
    let workflow_diff = new.workflows.len() as i64 - old.workflows.len() as i64;
    if workflow_diff > 0 {
        changes.push(format!("+{workflow_diff} workflows"));
    }
    if changes.is_empty() {
        "Profile refinement".to_string()
    } else {
        changes.join(", ")
    }
}

// ---------------------------------------------------------------------------
// Real adapters
// ---------------------------------------------------------------------------

/// The real [`PromptLedger`] over [`UserPromptManager`].
///
/// [`UserPromptManager`]: crate::prompt::UserPromptManager
pub struct PromptLedgerAdapter {
    manager: Arc<crate::prompt::UserPromptManager>,
}

impl PromptLedgerAdapter {
    pub fn new(manager: Arc<crate::prompt::UserPromptManager>) -> Self {
        Self { manager }
    }
}

impl PromptLedger for PromptLedgerAdapter {
    fn count_unanalyzed(&self) -> BoxFuture<'_, Result<i64, ProfileError>> {
        Box::pin(async move { self.manager.count_unanalyzed_for_user_learning().await })
    }

    fn get_prompts_for_learning(
        &self,
        limit: i64,
    ) -> BoxFuture<'_, Result<Vec<UserPrompt>, ProfileError>> {
        Box::pin(async move { self.manager.get_prompts_for_user_learning(limit).await })
    }

    fn mark_many(&self, ids: Vec<String>) -> BoxFuture<'_, Result<(), ProfileError>> {
        Box::pin(async move {
            self.manager
                .mark_multiple_as_user_learning_captured(&ids)
                .await
        })
    }
}

/// The real [`ProfileWriter`] over [`ProfileStore`].
///
/// [`ProfileStore`]: crate::profile_store::ProfileStore
pub struct ProfileWriterAdapter {
    store: Arc<crate::profile_store::ProfileStore>,
}

impl ProfileWriterAdapter {
    pub fn new(store: Arc<crate::profile_store::ProfileStore>) -> Self {
        Self { store }
    }
}

impl ProfileWriter for ProfileWriterAdapter {
    fn get_active_profile<'a>(
        &'a self,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Profile>, ProfileError>> {
        Box::pin(async move { self.store.get_active_profile(user_id).await })
    }

    fn merge_profile_data<'a>(
        &'a self,
        existing: &'a ProfileData,
        updates: &'a ProfileUpdates,
        profile_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<ProfileData, ProfileError>> {
        Box::pin(async move {
            self.store
                .merge_profile_data(existing, updates, None, profile_id)
                .await
        })
    }

    fn update_profile<'a>(
        &'a self,
        profile_id: &'a str,
        data: &'a ProfileData,
        prompts_analyzed: i64,
        change_summary: &'a str,
    ) -> BoxFuture<'a, Result<bool, ProfileError>> {
        Box::pin(async move {
            self.store
                .update_profile(profile_id, data, prompts_analyzed, change_summary)
                .await
        })
    }

    fn create_profile<'a>(
        &'a self,
        user_id: &'a str,
        display_name: &'a str,
        user_name: &'a str,
        user_email: &'a str,
        data: &'a ProfileData,
        prompts_analyzed: i64,
    ) -> BoxFuture<'a, Result<String, ProfileError>> {
        Box::pin(async move {
            self.store
                .create_profile(
                    user_id,
                    display_name,
                    user_name,
                    user_email,
                    data,
                    prompts_analyzed,
                )
                .await
        })
    }
}
