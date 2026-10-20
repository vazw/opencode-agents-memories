//! Port of `tests/ai-cleanup.test.ts`.
//!
//! 8 upstream cases: the four `extractTextFromPromptResult` shape cases and
//! the four provider-path cases, re-expressed on W3.2's `StructuredCompletion`
//! seam (upstream's host `session.create`/`session.delete` lifecycle is N-A —
//! the sealed `AiProviderStructured` gives this port no session object, and the
//! external *body* shapes are asserted in `mem-ai`'s own tests, never here).
//!
//! 3 justified additions: the missing-provider sentence, the prompt/schema pin,
//! and the malformed-mapping no-op.
//!
//! Tests:
//! - `test_extract_text_reads_text_parts_under_the_data_wrapper`
//! - `test_extract_text_reads_top_level_parts_without_the_data_wrapper`
//! - `test_extract_text_never_uses_the_info_text_field`
//! - `test_extract_text_surfaces_assistant_errors_from_info`
//! - `test_cleanup_via_the_native_completion_returns_the_indexed_profile_and_diff`
//! - `test_cleanup_native_fault_surfaces_verbatim_and_is_not_masked_as_missing_provider`
//! - `test_cleanup_does_not_fall_back_when_the_native_completion_fails`
//! - `test_cleanup_falls_back_to_the_external_completion_when_native_is_absent`
//! - `test_cleanup_without_any_completion_reports_no_provider_configured`
//! - `test_cleanup_prompt_pins_the_rules_indexed_ids_and_mapping_schema`
//! - `test_cleanup_malformed_mapping_degrades_to_a_noop`

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use mem_profile::ai_cleanup::{
    CLEANUP_SYSTEM_PROMPT_EXTERNAL, CLEANUP_SYSTEM_PROMPT_OPENCODE, CleanupMapping,
    NO_PROVIDER_MESSAGE, ProfileCleanupGateway, ai_cleanup_profile,
    ai_cleanup_profile_from_indexed, build_cleanup_prompt, cleanup_schema,
    extract_text_from_prompt_result, index_profile, normalize_ai_mapping,
};
use mem_profile::learning_service::AnalysisError;
use mem_profile::profile_store::{ProfileData, ProfileItem};
use mem_profile::provider_gateway::StructuredCompletion;
use mem_store::db::BoxFuture;

#[derive(Clone, Debug)]
struct RecordedCall {
    system_prompt: String,
    user_prompt: String,
    schema: Value,
}

/// A recording `StructuredCompletion`: no network, no provider, no session.
struct FakeCompletion {
    response: Result<Value, AnalysisError>,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
}

impl FakeCompletion {
    fn returning(response: Value) -> Self {
        Self {
            response: Ok(response),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn failing(message: &str) -> Self {
        Self {
            response: Err(AnalysisError::Provider(message.to_string())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl StructuredCompletion for FakeCompletion {
    fn complete<'a>(
        &'a self,
        system_prompt: &'a str,
        user_prompt: &'a str,
        schema: Value,
    ) -> BoxFuture<'a, Result<Value, AnalysisError>> {
        self.calls.lock().expect("call log").push(RecordedCall {
            system_prompt: system_prompt.to_string(),
            user_prompt: user_prompt.to_string(),
            schema,
        });
        let response = self.response.clone();
        Box::pin(async move { response })
    }
}

fn pref(description: &str) -> ProfileItem {
    ProfileItem {
        description: description.to_string(),
        category: Some("style".to_string()),
        confidence: Some(0.9),
        frequency: Some(2),
        ..ProfileItem::default()
    }
}

fn data(items: Vec<ProfileItem>) -> ProfileData {
    ProfileData {
        preferences: items,
        patterns: Vec::new(),
        workflows: Vec::new(),
        learning_paths: None,
    }
}

fn two_preferences() -> ProfileData {
    data(vec![
        pref("Prefer concise answers"),
        ProfileItem {
            description: "likes short replies".to_string(),
            category: Some("style".to_string()),
            confidence: Some(0.5),
            frequency: Some(1),
            ..ProfileItem::default()
        },
    ])
}

fn cleanup_response() -> Value {
    // Upstream's `cleanupJson`, with the two response entries deliberately in
    // reverse id order: the port must read each id from the raw object, so a
    // positionally re-indexed parse would attach "hallucinated entry" to
    // `pref_0` and fail the assertions below.
    json!({
        "preferences": [
            {"id": "pref_31", "category": "style", "description": "hallucinated entry"},
            {"id": "pref_0", "category": "style", "description": "Prefer concise answers"}
        ],
        "patterns": [],
        "workflows": [],
        "mapping": {"kept": ["pref_0"], "merged": [], "removed": ["pref_1"]}
    })
}

#[test]
fn test_extract_text_reads_text_parts_under_the_data_wrapper() {
    let (info, raw_text) = extract_text_from_prompt_result(&json!({
        "data": {
            "info": {},
            "parts": [
                {"type": "text", "text": "{\"mapping\":{\"kept\":[\"pref_0\"],\"merged\":[],\"removed\":[]}}"},
                {"type": "step-finish"}
            ]
        }
    }));

    assert_eq!(info, json!({}));
    assert!(raw_text.contains("\"kept\":[\"pref_0\"]"));
}

#[test]
fn test_extract_text_reads_top_level_parts_without_the_data_wrapper() {
    let (info, raw_text) = extract_text_from_prompt_result(&json!({
        "info": {},
        "parts": [{"type": "text", "text": "{\"ok\":true}"}]
    }));

    assert_eq!(info, json!({}));
    assert_eq!(raw_text, "{\"ok\":true}");
}

#[test]
fn test_extract_text_never_uses_the_info_text_field() {
    // #177: `AssistantMessage` has no `text` field; content lives in `parts`.
    let (_, raw_text) = extract_text_from_prompt_result(&json!({
        "data": {
            "info": {"text": "{\"fromInfo\":true}"},
            "parts": []
        }
    }));

    assert_eq!(raw_text, "");
}

#[test]
fn test_extract_text_surfaces_assistant_errors_from_info() {
    let (info, raw_text) = extract_text_from_prompt_result(&json!({
        "data": {
            "info": {"error": {"name": "ApiError", "data": {"message": "boom"}}},
            "parts": []
        }
    }));

    assert_eq!(info["error"]["name"], json!("ApiError"));
    assert_eq!(info["error"]["data"]["message"], json!("boom"));
    assert_eq!(raw_text, "");
}

#[tokio::test]
async fn test_cleanup_via_the_native_completion_returns_the_indexed_profile_and_diff() {
    let native = Arc::new(FakeCompletion::returning(cleanup_response()));
    let gateway = ProfileCleanupGateway::new(Some(native.clone()), None);

    let result = ai_cleanup_profile(&two_preferences(), &gateway)
        .await
        .expect("cleanup succeeds");

    assert_eq!(result.diff.kept, vec!["Prefer concise answers"]);
    assert_eq!(
        result
            .diff
            .removed
            .iter()
            .map(|removed| removed.id.as_str())
            .collect::<Vec<_>>(),
        vec!["pref_1"]
    );
    assert!(
        result.diff.merged.is_empty(),
        "no merge group was requested"
    );

    assert_eq!(result.cleaned.preferences.len(), 1);
    let kept = &result.cleaned.preferences[0];
    assert_eq!(
        kept.description, "Prefer concise answers",
        "the raw object's id selects the entry, never its array position"
    );
    assert!(
        !kept.extra.contains_key("id"),
        "the synthetic id is stripped from the rebuilt item"
    );
    assert_eq!(
        kept.confidence,
        Some(0.9),
        "the original's metadata is preserved when the cleaned item omits it"
    );
    assert_eq!(kept.frequency, Some(2));

    let calls = native.calls.lock().expect("call log");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].system_prompt, CLEANUP_SYSTEM_PROMPT_OPENCODE);
}

#[tokio::test]
async fn test_cleanup_native_fault_surfaces_verbatim_and_is_not_masked_as_missing_provider() {
    let native = Arc::new(FakeCompletion::failing("prompt failed for test"));
    let gateway = ProfileCleanupGateway::new(Some(native), None);

    let error = ai_cleanup_profile(&two_preferences(), &gateway)
        .await
        .expect_err("the native fault propagates");

    let message = error.to_string();
    assert!(message.contains("prompt failed for test"), "got {message}");
    assert!(
        !message.contains(NO_PROVIDER_MESSAGE),
        "a native fault is never rewritten into the missing-provider sentence: {message}"
    );
}

#[tokio::test]
async fn test_cleanup_does_not_fall_back_when_the_native_completion_fails() {
    let native = Arc::new(FakeCompletion::failing("prompt failed for test"));
    let external = Arc::new(FakeCompletion::returning(cleanup_response()));
    let gateway = ProfileCleanupGateway::new(Some(native), Some(external.clone()));

    let error = ai_cleanup_profile(&two_preferences(), &gateway)
        .await
        .expect_err("no external fallback may run when a native completion exists");

    assert!(error.to_string().contains("prompt failed for test"));
    assert!(
        external.calls.lock().expect("call log").is_empty(),
        "the external completion must never be reached"
    );
}

#[tokio::test]
async fn test_cleanup_falls_back_to_the_external_completion_when_native_is_absent() {
    let external = Arc::new(FakeCompletion::returning(cleanup_response()));
    let gateway = ProfileCleanupGateway::new(None, Some(external.clone()));

    let result = ai_cleanup_profile(&two_preferences(), &gateway)
        .await
        .expect("the external path serves the cleanup");

    assert_eq!(result.diff.kept, vec!["Prefer concise answers"]);
    let calls = external.calls.lock().expect("call log");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].system_prompt, CLEANUP_SYSTEM_PROMPT_EXTERNAL,
        "the external transport carries its own system prompt"
    );
    assert_ne!(
        CLEANUP_SYSTEM_PROMPT_EXTERNAL, CLEANUP_SYSTEM_PROMPT_OPENCODE,
        "the two transports are distinct sentences"
    );
}

#[tokio::test]
async fn test_cleanup_without_any_completion_reports_no_provider_configured() {
    let gateway = ProfileCleanupGateway::new(None, None);

    let error = ai_cleanup_profile(&two_preferences(), &gateway)
        .await
        .expect_err("no provider");

    assert_eq!(error.to_string(), NO_PROVIDER_MESSAGE);
    assert_eq!(
        error.to_string(),
        "No AI provider configured for profile cleanup"
    );
}

#[tokio::test]
async fn test_cleanup_prompt_pins_the_rules_indexed_ids_and_mapping_schema() {
    let indexed = index_profile(&two_preferences());
    let prompt = build_cleanup_prompt(&indexed);

    for rule in [
        "1. Merge semantically identical entries ONLY within the same section (pref_ with pref_, pat_ with pat_, wf_ with wf_)",
        "2. Do NOT merge across sections — preferences and patterns are different things",
        "3. When merging, keep the most specific description. Do not artificially shorten or inflate — the natural length of the original is fine.",
        "4. Do not add new entries; only merge and remove",
        "5. Prefer merging over removing. If entries share the same topic or describe similar behavior, merge them. Only remove truly irrelevant/generic items that add no value (e.g. \"uses tools\", \"checks things\").",
        "6. You MUST return a mapping showing the disposition of each id",
    ] {
        assert!(prompt.contains(rule), "missing rule: {rule}");
    }
    assert!(prompt.contains("Current profile:"));
    assert!(prompt.contains("\"id\": \"pref_0\""));
    assert!(prompt.contains("\"id\": \"pref_1\""));
    assert!(prompt.contains("\"description\": \"Prefer concise answers\""));
    assert!(prompt.contains("\"frequency\": 2"));
    assert!(prompt.ends_with(
        "The first id in each merged group is the kept entry; the rest are merged into it."
    ));

    let schema = cleanup_schema();
    for section in ["preferences", "patterns", "workflows"] {
        assert_eq!(schema["properties"][section]["type"], json!("array"));
        assert_eq!(
            schema["properties"][section]["items"]["required"],
            json!(["id", "category", "description"])
        );
    }
    assert_eq!(
        schema["properties"]["mapping"]["required"],
        json!(["kept", "merged", "removed"])
    );
    assert_eq!(
        schema["properties"]["mapping"]["properties"]["merged"]["items"]["items"]["type"],
        json!("string")
    );

    // The same prompt and schema reach the completion verbatim.
    let native = Arc::new(FakeCompletion::returning(cleanup_response()));
    let gateway = ProfileCleanupGateway::new(Some(native.clone()), None);
    ai_cleanup_profile_from_indexed(&indexed, &gateway)
        .await
        .expect("cleanup succeeds");
    let calls = native.calls.lock().expect("call log");
    assert_eq!(calls[0].user_prompt, prompt);
    assert_eq!(calls[0].schema, schema);
}

#[tokio::test]
async fn test_cleanup_malformed_mapping_degrades_to_a_noop() {
    // (1) A mapping that is not an object at all: the whole cleanup degrades
    // to a no-op and every original survives as "unmentioned".
    let native = Arc::new(FakeCompletion::returning(json!({
        "preferences": [{"id": "pref_0", "category": "style", "description": "Prefer concise answers"}],
        "patterns": [],
        "workflows": [],
        "mapping": "not-an-object"
    })));
    let gateway = ProfileCleanupGateway::new(Some(native), None);

    let result = ai_cleanup_profile(&two_preferences(), &gateway)
        .await
        .expect("a malformed mapping is not an error");

    assert!(result.diff.kept.is_empty());
    assert!(result.diff.merged.is_empty());
    assert!(result.diff.removed.is_empty());
    assert_eq!(
        result.cleaned.preferences.len(),
        2,
        "an empty mapping preserves every original as unmentioned"
    );
    let mut descriptions: Vec<&str> = result
        .cleaned
        .preferences
        .iter()
        .map(|item| item.description.as_str())
        .collect();
    descriptions.sort_unstable();
    assert_eq!(
        descriptions,
        vec!["Prefer concise answers", "likes short replies"]
    );

    // (2) A response with no mapping object degrades the same way.
    let native = Arc::new(FakeCompletion::returning(json!({
        "preferences": [],
        "patterns": [],
        "workflows": []
    })));
    let gateway = ProfileCleanupGateway::new(Some(native), None);
    let result = ai_cleanup_profile(&two_preferences(), &gateway)
        .await
        .expect("a missing mapping is not an error");
    assert!(
        result.diff.kept.is_empty()
            && result.diff.merged.is_empty()
            && result.diff.removed.is_empty()
    );
    assert_eq!(result.cleaned.preferences.len(), 2);

    // (3) The coercion itself: non-strings are dropped from `kept`/`removed`,
    // and merge groups that are not arrays or become empty are dropped —
    // never an index-out-of-range.
    let coerced = normalize_ai_mapping(&json!({
        "kept": ["pref_0", 7, null],
        "merged": [null, 7, [], "nope", [1, 2], ["pref_0", 3]],
        "removed": ["pref_1", 5]
    }));
    assert_eq!(coerced.kept, vec!["pref_0"]);
    assert_eq!(coerced.merged, vec![vec!["pref_0"]]);
    assert_eq!(coerced.removed, vec!["pref_1"]);
    assert_eq!(
        normalize_ai_mapping(&json!("nope")),
        CleanupMapping::default()
    );
    assert_eq!(
        normalize_ai_mapping(&Value::Null),
        CleanupMapping::default()
    );
}
