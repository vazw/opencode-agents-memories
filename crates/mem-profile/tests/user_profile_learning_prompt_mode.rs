//! Port of upstream `tests/user-profile-learning-prompt-mode.test.ts` (W3.2).
//!
//! Upstream captures the system prompt each transport sends; here the round's
//! own context carries both pre-built prompts, so the spy gateway records
//! which one the transport it served used.
//!
//! Upstream cases (traced 1:1 by name, 2 total):
//! - "opencode structured-output path: no tool instruction, JSON-object
//!   instruction instead" → `test_structured_path_prompt_requests_json_object_without_tool_mention`
//! - "external tool-call path: keeps the update_user_profile tool instruction"
//!   → `test_external_path_prompt_requires_update_user_profile_tool`

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mem_profile::ProfileError;
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, ProfileWriter,
    PromptLedger,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::UserPrompt;
use mem_store::db::BoxFuture;

const PROMPTS: [&str; 5] = [
    "Fix the bug in the login function",
    "Add tests for the user service",
    "Explain why the build is failing",
    "Refactor the payments module",
    "Review the changes on this branch",
];

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

struct SpyLedger {
    prompts: Vec<UserPrompt>,
}

impl PromptLedger for SpyLedger {
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
        Box::pin(async move { Ok(()) })
    }
}

struct SpyProfiles;

impl ProfileWriter for SpyProfiles {
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
        Box::pin(async move { Ok("profile_created".to_string()) })
    }
}

/// Records the context each transport received.
struct SpyGateway {
    native_fails: bool,
    native_context: Arc<Mutex<Option<AnalysisContext>>>,
    external_context: Arc<Mutex<Option<AnalysisContext>>>,
}

impl AnalysisGateway for SpyGateway {
    fn native_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        *self.native_context.lock().expect("context lock") = Some(context.clone());
        let fails = self.native_fails;
        Box::pin(async move {
            if fails {
                Err(AnalysisError::Provider(
                    "native provider rejected".to_string(),
                ))
            } else {
                Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string())
            }
        })
    }

    fn external_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        *self.external_context.lock().expect("context lock") = Some(context.clone());
        Box::pin(
            async move { Ok(r#"{"preferences":[],"patterns":[],"workflows":[]}"#.to_string()) },
        )
    }
}

async fn run_round(
    dir: &std::path::Path,
    native_fails: bool,
) -> (AnalysisContext, AnalysisContext) {
    let native_context = Arc::new(Mutex::new(None));
    let external_context = Arc::new(Mutex::new(None));
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    )));
    let prompts: Vec<UserPrompt> = PROMPTS
        .iter()
        .enumerate()
        .map(|(index, content)| UserPrompt {
            id: format!("prompt-{index}"),
            session_id: "session-1".to_string(),
            message_id: format!("msg-{index}"),
            project_path: Some("/workspace".to_string()),
            content: content.to_string(),
            created_at: index as i64 + 1,
            captured: false,
            user_learning_captured: false,
            linked_memory_id: None,
            capture_attempts: 0,
            provider_id: None,
            model_id: None,
        })
        .collect();
    let round = LearningRound::new(
        Arc::new(SpyLedger { prompts }),
        Arc::new(SpyProfiles),
        Arc::new(SpyGateway {
            native_fails,
            native_context: native_context.clone(),
            external_context: external_context.clone(),
        }),
        lock,
        Arc::new(|| true),
        PROMPTS.len() as i64,
        "test@example.com",
    );

    let outcome = round.run(0).await.expect("round completes");
    assert_eq!(
        outcome,
        LearningOutcome::Completed {
            prompts: PROMPTS.len()
        }
    );
    let native = native_context
        .lock()
        .expect("context lock")
        .clone()
        .expect("native transport used");
    let external = external_context
        .lock()
        .expect("context lock")
        .clone()
        .unwrap_or_else(|| native.clone());
    (native, external)
}

#[tokio::test]
async fn test_structured_path_prompt_requests_json_object_without_tool_mention() {
    let dir = TempDir::new("learning-prompt-mode-structured");
    let (native, _external) = run_round(dir.path(), false).await;

    // The native path serves the structured-output transport: no tool mention,
    // a JSON-object instruction instead.
    assert!(!native.system_prompt_json.contains("update_user_profile"));
    assert!(!native.system_prompt_json.contains("tool"));
    assert!(native.system_prompt_json.contains("JSON object"));
}

#[tokio::test]
async fn test_external_path_prompt_requires_update_user_profile_tool() {
    let dir = TempDir::new("learning-prompt-mode-external");
    let (_native, external) = run_round(dir.path(), true).await;

    // Only the external transport is reached after the native failure, and it
    // carries the tool-mode prompt.
    assert!(
        external
            .system_prompt_tool
            .contains("Use the update_user_profile tool to save the new profile.")
    );
}
