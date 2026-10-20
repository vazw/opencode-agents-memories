//! Port of upstream `tests/language-detector.test.ts` (W3.2) plus the
//! resolver contract the learning service depends on.
//!
//! Upstream cases (traced 1:1 by name, 9 total):
//! - "should detect Chinese as zh" → `test_detect_language_chinese_returns_zh`
//! - "should detect short Chinese as zh (5+ chars)" → `test_detect_language_short_chinese_returns_zh`
//! - "should detect English as en" → `test_detect_language_english_returns_en`
//! - "should fallback to en for empty string" → `test_detect_language_empty_returns_en`
//! - "should fallback to en for whitespace only" → `test_detect_language_whitespace_returns_en`
//! - "should fallback to en for very short undetectable input" → `test_detect_language_very_short_input_returns_en`
//! - "should return Chinese name for zh" → `test_get_language_name_chinese_contains_chinese`
//! - "should return English name for en" → `test_get_language_name_english_contains_english`
//! - "should fallback to English for unknown code" → `test_get_language_name_unknown_falls_back_to_english`
//!
//! Resolver cases (W3.2 acceptance criteria; upstream asserts them through a
//! mocked subprocess, here through the pure free function):
//! - `test_resolve_language_name_auto_detects_spanish`
//! - `test_resolve_language_name_unset_behaves_like_auto`
//! - `test_resolve_language_name_explicit_config_overrides_detection`
//! - `test_resolve_language_name_joins_all_prompt_contents`

use mem_profile::language_detector::{
    Language, detect_language, get_language_name, resolve_profile_language_name,
};
use mem_profile::prompt::UserPrompt;

fn prompt(content: &str) -> UserPrompt {
    UserPrompt {
        id: "prompt-1".to_string(),
        session_id: "session-1".to_string(),
        message_id: "msg-1".to_string(),
        project_path: Some("/workspace".to_string()),
        content: content.to_string(),
        created_at: 1,
        captured: false,
        user_learning_captured: false,
        linked_memory_id: None,
        capture_attempts: 0,
        provider_id: None,
        model_id: None,
    }
}

fn prompts(contents: &[&str]) -> Vec<UserPrompt> {
    contents
        .iter()
        .enumerate()
        .map(|(index, content)| UserPrompt {
            id: format!("prompt-{index}"),
            message_id: format!("msg-{index}"),
            created_at: index as i64 + 1,
            ..prompt(content)
        })
        .collect()
}

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

// ---------------------------------------------------------------------------
// detectLanguage
// ---------------------------------------------------------------------------

#[test]
fn test_detect_language_chinese_returns_zh() {
    assert_eq!(detect_language("这是一个中文测试").code(), "zh");
}

#[test]
fn test_detect_language_short_chinese_returns_zh() {
    assert_eq!(detect_language("这是一个中文测试").code(), "zh");
}

#[test]
fn test_detect_language_english_returns_en() {
    assert_eq!(detect_language("This is an English test").code(), "en");
}

#[test]
fn test_detect_language_empty_returns_en() {
    assert_eq!(detect_language("").code(), "en");
}

#[test]
fn test_detect_language_whitespace_returns_en() {
    assert_eq!(detect_language("   ").code(), "en");
}

#[test]
fn test_detect_language_very_short_input_returns_en() {
    // Upstream `franc(..., { minLength: 5 })`; whatlang would otherwise call
    // this Estonian (measured confidence 0.005).
    assert_eq!(detect_language("ab").code(), "en");
}

// ---------------------------------------------------------------------------
// getLanguageName
// ---------------------------------------------------------------------------

#[test]
fn test_get_language_name_chinese_contains_chinese() {
    assert!(get_language_name("zh").to_lowercase().contains("chinese"));
}

#[test]
fn test_get_language_name_english_contains_english() {
    assert!(get_language_name("en").to_lowercase().contains("english"));
}

#[test]
fn test_get_language_name_unknown_falls_back_to_english() {
    assert_eq!(get_language_name("xyz"), "English");
}

// ---------------------------------------------------------------------------
// resolveProfileLanguageName
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_language_name_auto_detects_spanish() {
    let resolved = resolve_profile_language_name(&prompts(&SHORT_SPANISH_PROMPTS), Some("auto"));
    assert_eq!(resolved, "Spanish");
}

#[test]
fn test_resolve_language_name_unset_behaves_like_auto() {
    let detected = resolve_profile_language_name(&prompts(&SHORT_SPANISH_PROMPTS), None);
    assert_eq!(detected, "Spanish");
}

#[test]
fn test_resolve_language_name_explicit_config_overrides_detection() {
    let resolved = resolve_profile_language_name(&prompts(&SHORT_SPANISH_PROMPTS), Some("en"));
    assert_eq!(resolved, "English");
}

#[test]
fn test_resolve_language_name_joins_all_prompt_contents() {
    // Every fragment is shorter than the 5-char detection floor and succeeds
    // individually as "English"; only the joined raw contents detect Spanish,
    // which proves the resolver aggregates instead of detecting per prompt.
    let fragments = ["el", "la", "de", "que", "y"];
    for fragment in fragments {
        assert_eq!(detect_language(fragment), Language::ENGLISH);
    }
    let resolved = resolve_profile_language_name(&prompts(&fragments), Some("auto"));
    assert_eq!(resolved, "Spanish");

    let english = resolve_profile_language_name(&prompts(&SHORT_ENGLISH_PROMPTS), Some("auto"));
    assert_eq!(english, "English");
}
