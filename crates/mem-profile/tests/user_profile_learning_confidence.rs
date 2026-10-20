//! Port of `tests/user-profile-learning-confidence.test.ts` (6 upstream cases)
//! plus the round-side rejection case (a justified addition).
//!
//! Tests 1-6 exercise `validation` / `analysis_prompt`; test 7 proves the round
//! rejects an out-of-range provider payload *before* any write or mark.
//!
//! Tests:
//! - `test_analysis_validation_accepts_confidence_above_half`
//! - `test_analysis_validation_accepts_confidence_at_the_upper_bound`
//! - `test_analysis_validation_rejects_confidence_above_one`
//! - `test_confidence_max_is_exposed_and_matches_the_analysis_tool_schema`
//! - `test_validator_rejects_out_of_range_confidence_during_provider_response_validation`
//! - `test_cleanup_interval_predicate_fires_only_when_a_prompt_interval_is_crossed`
//! - `test_round_rejects_provider_payload_with_out_of_range_confidence_without_marking_prompts`

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use mem_profile::ProfileError;
use mem_profile::analysis_prompt::analysis_schema;
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningRound, ProfileWriter, PromptLedger,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::UserPrompt;
use mem_profile::validation::{
    USER_PROFILE_LLM_CONFIDENCE_MAX, UserProfileValidator, should_run_automatic_profile_cleanup,
    validate_analysis_value,
};
use mem_store::db::BoxFuture;

fn base_profile(confidence: f64) -> Value {
    json!({
        "preferences": [{
            "category": "style",
            "description": "Prefers concise answers",
            "confidence": confidence,
            "evidence": ["please keep it short"]
        }],
        "patterns": [],
        "workflows": []
    })
}

#[test]
fn test_analysis_validation_accepts_confidence_above_half() {
    let updates = validate_analysis_value(&base_profile(0.8)).expect("0.8 is in range");
    let preferences = updates.preferences.expect("preferences present");
    assert_eq!(preferences.len(), 1);
    assert_eq!(preferences[0].confidence, Some(0.8));
}

#[test]
fn test_analysis_validation_accepts_confidence_at_the_upper_bound() {
    let updates = validate_analysis_value(&base_profile(1.0)).expect("1.0 is in range");
    let preferences = updates.preferences.expect("preferences present");
    assert_eq!(preferences[0].confidence, Some(1.0));
}

#[test]
fn test_analysis_validation_rejects_confidence_above_one() {
    let error = validate_analysis_value(&base_profile(1.1)).expect_err("1.1 is out of range");
    match error {
        ProfileError::Save(message) => assert_eq!(
            message,
            "preferences[0].confidence must be a finite number between 0 and 1"
        ),
        other => panic!("a payload fault is a Save (parse-class) error, got {other:?}"),
    }
}

#[test]
fn test_confidence_max_is_exposed_and_matches_the_analysis_tool_schema() {
    assert_eq!(USER_PROFILE_LLM_CONFIDENCE_MAX, 1.0);
    let schema = analysis_schema();
    assert_eq!(
        schema["properties"]["preferences"]["items"]["properties"]["confidence"]["minimum"]
            .as_f64(),
        Some(0.0)
    );
    assert_eq!(
        schema["properties"]["preferences"]["items"]["properties"]["confidence"]["maximum"]
            .as_f64(),
        Some(USER_PROFILE_LLM_CONFIDENCE_MAX)
    );
}

#[test]
fn test_validator_rejects_out_of_range_confidence_during_provider_response_validation() {
    assert!(
        !UserProfileValidator::validate(&base_profile(1.1)).valid,
        "the provider-response validator rejects 1.1"
    );
    let valid = UserProfileValidator::validate(&base_profile(0.8));
    assert!(valid.valid, "errors: {:?}", valid.errors);
    assert!(valid.data.is_some());
}

#[test]
fn test_cleanup_interval_predicate_fires_only_when_a_prompt_interval_is_crossed() {
    assert!(should_run_automatic_profile_cleanup(90, 10, 100.0, true));
    assert!(!should_run_automatic_profile_cleanup(100, 10, 100.0, true));

    // Disabled always wins.
    assert!(!should_run_automatic_profile_cleanup(90, 10, 100.0, false));
    // Upstream's `Number.isInteger(interval)` guard: a fractional interval is
    // never due.
    assert!(!should_run_automatic_profile_cleanup(90, 10, 100.5, true));
    // Non-positive and non-finite intervals are never due.
    assert!(!should_run_automatic_profile_cleanup(90, 10, 0.0, true));
    assert!(!should_run_automatic_profile_cleanup(90, 10, -100.0, true));
    assert!(!should_run_automatic_profile_cleanup(
        90,
        10,
        f64::INFINITY,
        true
    ));
}

// ---------------------------------------------------------------------------
// The round-side rejection (justified addition)
// ---------------------------------------------------------------------------

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

struct CountingProfiles {
    reads: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
}

impl ProfileWriter for CountingProfiles {
    fn get_active_profile<'a>(
        &'a self,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Profile>, ProfileError>> {
        self.reads.fetch_add(1, Ordering::AcqRel);
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
        Box::pin(async move { Ok("created".to_string()) })
    }
}

struct OutOfRangeGateway;

impl AnalysisGateway for OutOfRangeGateway {
    fn native_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(async move { Ok(base_profile(1.1).to_string()) })
    }

    fn external_complete<'a>(
        &'a self,
        _context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        Box::pin(async move { Ok(base_profile(1.1).to_string()) })
    }
}

#[tokio::test]
async fn test_round_rejects_provider_payload_with_out_of_range_confidence_without_marking_prompts()
{
    let dir = TempDir::new("confidence-round-reject");
    let marks = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let prompts: Vec<UserPrompt> = (0..5)
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
        .collect();

    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.path().to_path_buf(),
        common::store_config(dir.path().to_path_buf()),
    )));
    let round = LearningRound::new(
        Arc::new(CountingLedger {
            prompts,
            marks: marks.clone(),
        }),
        Arc::new(CountingProfiles {
            reads: reads.clone(),
            writes: writes.clone(),
        }),
        Arc::new(OutOfRangeGateway),
        lock,
        Arc::new(|| true),
        5,
        "test@example.com",
    );

    let error = round
        .run(0)
        .await
        .expect_err("an out-of-range confidence is rejected");

    match error {
        ProfileError::Save(message) => assert!(
            message.contains("confidence must be a finite number between 0 and 1"),
            "got {message}"
        ),
        other => panic!("expected a Save error, got {other:?}"),
    }
    assert_eq!(writes.load(Ordering::Acquire), 0, "zero profile writes");
    assert_eq!(marks.load(Ordering::Acquire), 0, "zero mark_many calls");
    assert_eq!(
        reads.load(Ordering::Acquire),
        1,
        "the run stops right after reading the stored profile"
    );
}
