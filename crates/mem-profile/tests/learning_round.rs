//! Round-level integration for the W3.2 language and truncation seams.
//!
//! The round, the lock and the detection are real; the provider boundary is a
//! spy gateway, so every assertion is about what the round actually sends.
//!
//! Tests:
//! - `test_round_resolves_language_once_from_joined_raw_contents`
//! - `test_round_truncates_each_prompt_to_4096_chars`
//! - `test_round_caps_analysis_context_at_configured_chars`

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mem_profile::ProfileError;
use mem_profile::analysis_prompt::{CONTEXT_TRUNCATION_MARKER, MAX_PROMPT_CHARS};
use mem_profile::learning_lock::{LearningLock, LearningLockOptions};
use mem_profile::learning_service::{
    AnalysisContext, AnalysisError, AnalysisGateway, LearningOutcome, LearningRound, ProfileWriter,
    PromptLedger,
};
use mem_profile::profile_store::{Profile, ProfileData, ProfileUpdates};
use mem_profile::prompt::UserPrompt;
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

fn prompts(contents: &[String]) -> Vec<UserPrompt> {
    contents
        .iter()
        .enumerate()
        .map(|(index, content)| UserPrompt {
            id: format!("prompt-{index}"),
            session_id: "session-1".to_string(),
            message_id: format!("msg-{index}"),
            project_path: Some("/workspace".to_string()),
            content: content.clone(),
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

struct SpyGateway {
    calls: Arc<AtomicUsize>,
    contexts: Arc<Mutex<Vec<AnalysisContext>>>,
}

impl AnalysisGateway for SpyGateway {
    fn native_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.contexts
            .lock()
            .expect("context lock")
            .push(context.clone());
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

struct Captured {
    calls: usize,
    context: AnalysisContext,
}

async fn run_round(
    dir: &std::path::Path,
    contents: &[String],
    language: Option<&str>,
    max_context_chars: Option<usize>,
) -> Captured {
    let calls = Arc::new(AtomicUsize::new(0));
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    )));
    let mut round = LearningRound::new(
        Arc::new(SpyLedger {
            prompts: prompts(contents),
        }),
        Arc::new(SpyProfiles),
        Arc::new(SpyGateway {
            calls: calls.clone(),
            contexts: contexts.clone(),
        }),
        lock,
        Arc::new(|| true),
        contents.len() as i64,
        "test@example.com",
    )
    .with_language_config(language.map(str::to_string));
    if let Some(max_chars) = max_context_chars {
        round = round.with_max_context_chars(max_chars);
    }

    let outcome = round.run(0).await.expect("round completes");
    assert_eq!(
        outcome,
        LearningOutcome::Completed {
            prompts: contents.len()
        }
    );
    let captured = contexts.lock().expect("context lock");
    assert_eq!(captured.len(), 1, "one analysis per round");
    Captured {
        calls: calls.load(Ordering::Acquire),
        context: captured[0].clone(),
    }
}

#[tokio::test]
async fn test_round_resolves_language_once_from_joined_raw_contents() {
    let dir = TempDir::new("learning-round-language");
    let fragments: Vec<String> = ["el", "la", "de", "que", "y"]
        .iter()
        .map(|fragment| fragment.to_string())
        .collect();
    let captured = run_round(dir.path(), &fragments, Some("auto"), None).await;

    // Each fragment is below the 5-char floor and individually resolves to
    // English; only the joined raw contents detect Spanish.
    assert_eq!(captured.context.lang_name, "Spanish");
    // Detection runs once, on the raw batch, before the scaffold: the context
    // that reaches the provider is still the English-scaffolded user prompt.
    assert_eq!(captured.calls, 1);
    assert!(
        captured
            .context
            .user_prompt
            .contains("# User Profile Analysis")
    );
    for fragment in &fragments {
        assert!(
            captured.context.user_prompt.contains(fragment),
            "every raw fragment reaches the numbered prompt list"
        );
    }
}

#[tokio::test]
async fn test_round_truncates_each_prompt_to_4096_chars() {
    let dir = TempDir::new("learning-round-per-prompt-cap");
    let long = "é".repeat(MAX_PROMPT_CHARS + 500);
    let captured = run_round(dir.path(), &[long], None, None).await;

    assert_eq!(
        captured.context.user_prompt.matches('é').count(),
        MAX_PROMPT_CHARS,
        "the per-prompt cap is 4096 chars, never a byte slice"
    );
    assert!(
        !captured
            .context
            .user_prompt
            .contains(CONTEXT_TRUNCATION_MARKER)
    );
}

#[tokio::test]
async fn test_round_caps_analysis_context_at_configured_chars() {
    let dir = TempDir::new("learning-round-context-cap");
    let max_chars = 120;
    let contents: Vec<String> = (0..5).map(|index| format!("observation {index}")).collect();
    let captured = run_round(dir.path(), &contents, None, Some(max_chars)).await;

    let user_prompt = &captured.context.user_prompt;
    assert!(user_prompt.ends_with(CONTEXT_TRUNCATION_MARKER));
    assert!(!user_prompt.contains("## Analysis Guidelines"));
    let head = user_prompt
        .strip_suffix(CONTEXT_TRUNCATION_MARKER)
        .expect("marker suffix")
        .strip_suffix('\n')
        .expect("newline before the marker");
    assert_eq!(head.chars().count(), max_chars);
}
