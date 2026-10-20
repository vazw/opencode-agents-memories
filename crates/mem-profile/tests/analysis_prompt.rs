//! Unit contract for the profile-analysis prompt builder (W3.2).
//!
//! Upstream sources: `src/services/user-memory-learning.ts` L392-450
//! (`buildUserAnalysisContext`) and L704-719 (`buildProfileSystemPrompt`),
//! asserted upstream only through mocked subprocesses in
//! `tests/user-profile-learning-prompt-mode.test.ts`. This file pins the
//! builder's observable strings directly:
//! - `test_json_system_prompt_requests_json_object_without_tool_instruction`
//! - `test_tool_system_prompt_requires_update_user_profile_tool`
//! - `test_prompt_content_is_truncated_to_4096_chars_at_char_boundary`
//! - `test_context_truncation_is_head_only_with_marker`
//! - `test_analysis_schema_requires_arrays_and_closes_objects`

use mem_profile::analysis_prompt::{
    CONTEXT_TRUNCATION_MARKER, MAX_PROMPT_CHARS, PromptMode, analysis_schema, build_system_prompt,
    build_user_prompt,
};
use mem_profile::prompt::UserPrompt;
use serde_json::Value;

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

// ---------------------------------------------------------------------------
// System prompt
// ---------------------------------------------------------------------------

#[test]
fn test_json_system_prompt_requests_json_object_without_tool_instruction() {
    let prompt = build_system_prompt(false, "English", PromptMode::Json);

    // No tool mention: on the v2 Generate API no tools exist, and the model
    // obeying the old sentence stalled with a preamble and no JSON.
    assert!(!prompt.contains("update_user_profile"));
    assert!(!prompt.contains("tool"));
    // The model must be told to answer with the profile JSON itself.
    assert!(prompt.contains("JSON object"));
    assert!(prompt.contains(
        "Return the new profile as the JSON object described in the request. Reply with JSON only."
    ));
    assert!(
        prompt.contains(
            "CRITICAL: You MUST write all descriptions, categories, and text in English."
        )
    );
    assert!(prompt.contains("create a comprehensive user profile"));

    let updated = build_system_prompt(true, "Spanish", PromptMode::Json);
    assert!(updated.contains("update a comprehensive user profile"));
    assert!(updated.contains("Return the updated profile as the JSON object described in the request. Reply with JSON only."));
    assert!(updated.contains("text in Spanish."));
}

#[test]
fn test_tool_system_prompt_requires_update_user_profile_tool() {
    let prompt = build_system_prompt(false, "English", PromptMode::Tool);

    assert!(prompt.contains("Use the update_user_profile tool to save the new profile."));
    assert!(
        prompt.contains(
            "CRITICAL: You MUST write all descriptions, categories, and text in English."
        )
    );

    let updated = build_system_prompt(true, "Chinese", PromptMode::Tool);
    assert!(updated.contains("Use the update_user_profile tool to save the updated profile."));
    assert!(
        updated.contains(
            "CRITICAL: You MUST write all descriptions, categories, and text in Chinese."
        )
    );
}

// ---------------------------------------------------------------------------
// User prompt + truncation
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_content_is_truncated_to_4096_chars_at_char_boundary() {
    let content = "é".repeat(MAX_PROMPT_CHARS + 500);
    let user_prompt = build_user_prompt(&[prompt(&content)], None, 32768);

    assert!(user_prompt.contains("# User Profile Analysis"));
    assert!(user_prompt.contains("## Recent Prompts"));
    assert!(user_prompt.contains("## Analysis Guidelines"));
    assert!(user_prompt.contains("1. "));
    // Every retained char is a multi-byte one, so a byte-slice implementation
    // would panic (or split the char) long before this count is reached.
    assert_eq!(user_prompt.matches('é').count(), MAX_PROMPT_CHARS);
    assert!(!user_prompt.contains(CONTEXT_TRUNCATION_MARKER));
}

#[test]
fn test_context_truncation_is_head_only_with_marker() {
    let content = format!("{}TAIL_SENTINEL", "á".repeat(400));
    let max_chars = 100;
    let user_prompt = build_user_prompt(&[prompt(&content)], None, max_chars);

    assert!(user_prompt.starts_with("# User Profile Analysis"));
    assert!(user_prompt.ends_with(CONTEXT_TRUNCATION_MARKER));
    // Head-only: the tail (and the guidance block) never survives the cap.
    assert!(!user_prompt.contains("TAIL_SENTINEL"));
    assert!(!user_prompt.contains("## Analysis Guidelines"));

    let body = user_prompt
        .strip_suffix(CONTEXT_TRUNCATION_MARKER)
        .expect("marker suffix");
    let head = body.strip_suffix('\n').expect("newline before the marker");
    assert_eq!(head.chars().count(), max_chars);
}

// ---------------------------------------------------------------------------
// Structured-output schema
// ---------------------------------------------------------------------------

#[test]
fn test_analysis_schema_requires_arrays_and_closes_objects() {
    let schema = analysis_schema();
    let object = schema.as_object().expect("schema is an object");

    assert_eq!(object["type"], Value::from("object"));
    assert_eq!(object["additionalProperties"], Value::from(false));
    assert_eq!(
        object["required"],
        Value::from(vec!["preferences", "patterns", "workflows"])
    );

    let properties = object["properties"].as_object().expect("properties");
    assert_eq!(properties["preferences"]["type"], Value::from("array"));
    assert_eq!(properties["patterns"]["type"], Value::from("array"));
    assert_eq!(properties["workflows"]["type"], Value::from("array"));

    let preference = &properties["preferences"]["items"];
    assert_eq!(preference["type"], Value::from("object"));
    assert_eq!(preference["additionalProperties"], Value::from(false));
    assert_eq!(
        preference["properties"]["category"]["type"],
        Value::from("string")
    );
    assert_eq!(
        preference["properties"]["description"]["type"],
        Value::from("string")
    );
    assert_eq!(
        preference["properties"]["confidence"]["type"],
        Value::from("number")
    );
    assert_eq!(
        preference["properties"]["confidence"]["minimum"],
        Value::from(0)
    );
    assert_eq!(
        preference["properties"]["confidence"]["maximum"],
        Value::from(1)
    );
    assert_eq!(
        preference["properties"]["evidence"]["items"]["type"],
        Value::from("string")
    );

    let pattern = &properties["patterns"]["items"];
    assert_eq!(pattern["additionalProperties"], Value::from(false));
    assert_eq!(
        pattern["properties"]["category"]["type"],
        Value::from("string")
    );
    assert_eq!(
        pattern["properties"]["description"]["type"],
        Value::from("string")
    );

    let workflow = &properties["workflows"]["items"];
    assert_eq!(workflow["additionalProperties"], Value::from(false));
    assert_eq!(
        workflow["properties"]["description"]["type"],
        Value::from("string")
    );
    assert_eq!(
        workflow["properties"]["steps"]["type"],
        Value::from("array")
    );
    assert_eq!(
        workflow["properties"]["steps"]["items"]["type"],
        Value::from("string")
    );
}
