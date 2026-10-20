//! Port of upstream `tests/user-profile-learning-language.test.ts` (W3.2).
//!
//! Upstream runs a mocked subprocess and asserts the system prompt the
//! structured-output call would have been sent; the same observation is made
//! here through the round's own seam — a spy [`AnalysisGateway`] that records
//! the [`AnalysisContext`] the round built. The round, the lock and the
//! detection are real; only the provider/prompt/profile boundaries are mocks.
//!
//! Upstream cases (traced 1:1 by name, 4 total):
//! - "auto: detects short non-English prompts despite the English analysis scaffold"
//!   → `test_auto_spanish_prompts_instruct_text_in_spanish`
//! - "unset language behaves like auto" → `test_unset_language_behaves_like_auto`
//! - "auto: English prompts resolve to English" → `test_english_prompts_instruct_text_in_english`
//! - "configured language overrides the language of the prompts"
//!   → `test_configured_en_overrides_spanish_prompts`

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
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

const SHORT_SPANISH_PROMPTS: [&str; 5] = [
    "Corrige el error en la función de login",
    "Añade pruebas para el servicio de usuarios",
    "Explica por qué falla la compilación",
    "Refactoriza el módulo de pagos",
    "Revisa los cambios de esta rama",
];

const SHORT_ENGLISH_PROMPTS: [&str; 5] = [
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

fn prompts(contents: &[&str]) -> Vec<UserPrompt> {
    contents
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

struct SpyProfiles {
    updates: Arc<AtomicUsize>,
}

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

/// Records every context the native path received (one analysis per round).
struct SpyGateway {
    contexts: Arc<Mutex<Vec<AnalysisContext>>>,
}

impl AnalysisGateway for SpyGateway {
    fn native_complete<'a>(
        &'a self,
        context: &'a AnalysisContext,
    ) -> BoxFuture<'a, Result<String, AnalysisError>> {
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

async fn run_round(
    dir: &std::path::Path,
    language: Option<&str>,
    prompt_texts: &[&str],
) -> AnalysisContext {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let lock = Arc::new(LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    )));
    let round = LearningRound::new(
        Arc::new(SpyLedger {
            prompts: prompts(prompt_texts),
        }),
        Arc::new(SpyProfiles {
            updates: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(SpyGateway {
            contexts: contexts.clone(),
        }),
        lock,
        Arc::new(|| true),
        prompt_texts.len() as i64,
        "test@example.com",
    )
    .with_language_config(language.map(str::to_string));

    let outcome = round.run(0).await.expect("round completes");
    assert_eq!(
        outcome,
        LearningOutcome::Completed {
            prompts: prompt_texts.len()
        }
    );
    let captured = contexts.lock().expect("context lock");
    assert_eq!(captured.len(), 1, "exactly one analysis per round");
    captured[0].clone()
}

#[tokio::test]
async fn test_auto_spanish_prompts_instruct_text_in_spanish() {
    let dir = TempDir::new("learning-language-auto-es");
    let context = run_round(dir.path(), Some("auto"), &SHORT_SPANISH_PROMPTS).await;

    assert_eq!(context.lang_name, "Spanish");
    assert!(context.system_prompt_json.contains("text in Spanish."));
    // Sanity check: the context sent to the LLM really is wrapped in the
    // English scaffold.
    assert!(context.user_prompt.contains("# User Profile Analysis"));
}

#[tokio::test]
async fn test_unset_language_behaves_like_auto() {
    let dir = TempDir::new("learning-language-unset");
    let context = run_round(dir.path(), None, &SHORT_SPANISH_PROMPTS).await;

    assert_eq!(context.lang_name, "Spanish");
    assert!(context.system_prompt_json.contains("text in Spanish."));
}

#[tokio::test]
async fn test_english_prompts_instruct_text_in_english() {
    let dir = TempDir::new("learning-language-auto-en");
    let context = run_round(dir.path(), Some("auto"), &SHORT_ENGLISH_PROMPTS).await;

    assert_eq!(context.lang_name, "English");
    assert!(context.system_prompt_json.contains("text in English."));
}

#[tokio::test]
async fn test_configured_en_overrides_spanish_prompts() {
    let dir = TempDir::new("learning-language-configured");
    let context = run_round(dir.path(), Some("en"), &SHORT_SPANISH_PROMPTS).await;

    assert_eq!(context.lang_name, "English");
    assert!(context.system_prompt_json.contains("text in English."));
}
