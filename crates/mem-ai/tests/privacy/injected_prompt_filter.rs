//! Port of upstream `tests/injected-prompt-filter.test.ts` (AC3, D6).
//!
//! Seam: `mem_ai::privacy::injection::*` — the marker predicate, the part
//! classification, the order-preserving filter, and the re-exported
//! `mem_core::config::normalize_injection_markers` seam.
//!
//! The allowlist / internal-prompt cases live in Step 5 of the blueprint and
//! are deliberately absent here.

#[path = "common/mod.rs"]
mod common;

use mem_ai::privacy::injection::{
    DEFAULT_INJECTION_MARKERS, InjectionFilter, MessagePart, contains_default_injection_marker,
    contains_injection_marker, filter_injected_parts, is_injected_part, is_internal_prompt,
    normalize_injection_markers,
};
use serde_json::Value;

// Verbatim samples taken from a real OpenCode session store, where 490 of 801
// recorded user messages consisted entirely of injected content.
const BACKGROUND_TASK_NOTIFICATION: &str = "<system-reminder>\n[BACKGROUND TASK RESULT READY]\n**ID:** `bg_5f29c4a2`\n**Description:** Explore codebase patterns\n**Duration:** 2m 41s\n\n**1 task still in progress.** You WILL be notified when ALL complete.\nDo NOT poll - continue productive work.\n</system-reminder>";

const INTERNAL_CONTINUATION: &str = "[restore checkpointed session agent configuration after compaction]\n<!-- OMO_INTERNAL_INITIATOR -->\n<!-- OMO_INTERNAL_NOREPLY -->";

const REAL_USER_PROMPT: &str =
    "Refactor the auth middleware so token refresh happens before validation.";

fn text_part(text: &str) -> MessagePart {
    MessagePart {
        kind: Some("text".to_owned()),
        text: Some(text.to_owned()),
        synthetic: false,
    }
}

// ---------------------------------------------------------------------------
// contains_injection_marker
// ---------------------------------------------------------------------------

#[test]
fn test_marker_detects_host_reminder_wrapper() {
    assert!(contains_default_injection_marker(
        BACKGROUND_TASK_NOTIFICATION
    ));
}

#[test]
fn test_marker_detects_plugin_internal_markers() {
    assert!(contains_default_injection_marker(INTERNAL_CONTINUATION));
}

#[test]
fn test_marker_leaves_user_text_untouched() {
    assert!(!contains_default_injection_marker(REAL_USER_PROMPT));
}

#[test]
fn test_marker_ignores_empty_input() {
    assert!(!contains_default_injection_marker(""));
}

#[test]
fn test_marker_matches_regardless_of_casing() {
    assert!(contains_default_injection_marker(
        "<System-Reminder>\nstatus\n</System-Reminder>"
    ));
}

#[test]
fn test_marker_honors_caller_supplied_list() {
    let custom = vec!["<my-banner>".to_owned()];
    assert!(contains_injection_marker("<my-banner>hi", &custom));
    assert!(!contains_injection_marker(
        BACKGROUND_TASK_NOTIFICATION,
        &custom
    ));
}

#[test]
fn test_marker_does_not_flag_prose_mention() {
    assert!(!contains_default_injection_marker(
        "why does the system reminder keep firing?"
    ));
}

// ---------------------------------------------------------------------------
// is_injected_part
// ---------------------------------------------------------------------------

#[test]
fn test_part_synthetic_flag_is_injected() {
    let part = MessagePart {
        kind: Some("text".to_owned()),
        text: Some("anything".to_owned()),
        synthetic: true,
    };
    assert!(is_injected_part(&part, &default_markers_owned()));
}

#[test]
fn test_part_marker_without_synthetic_flag_is_injected() {
    // Regression guard: the observed injections set no synthetic flag at all,
    // so structural detection alone would miss the overwhelming majority.
    assert!(is_injected_part(
        &text_part(INTERNAL_CONTINUATION),
        &default_markers_owned(),
    ));
    assert!(is_injected_part(
        &text_part(BACKGROUND_TASK_NOTIFICATION),
        &default_markers_owned(),
    ));
}

#[test]
fn test_part_keeps_authored_text() {
    assert!(!is_injected_part(
        &text_part(REAL_USER_PROMPT),
        &default_markers_owned(),
    ));
}

#[test]
fn test_part_without_text_is_authored() {
    let part = MessagePart {
        kind: Some("text".to_owned()),
        text: None,
        synthetic: false,
    };
    assert!(!is_injected_part(&part, &default_markers_owned()));
}

// ---------------------------------------------------------------------------
// filter_injected_parts
// ---------------------------------------------------------------------------

#[test]
fn test_filter_returns_empty_when_all_injected() {
    let parts = vec![
        text_part(BACKGROUND_TASK_NOTIFICATION),
        text_part(INTERNAL_CONTINUATION),
    ];
    assert_eq!(
        filter_injected_parts(&parts, &default_markers_owned()),
        Vec::<MessagePart>::new()
    );
}

#[test]
fn test_filter_strips_injected_block_from_mixed_message() {
    let parts = vec![
        text_part(BACKGROUND_TASK_NOTIFICATION),
        text_part(REAL_USER_PROMPT),
    ];
    assert_eq!(
        filter_injected_parts(&parts, &default_markers_owned()),
        vec![text_part(REAL_USER_PROMPT)]
    );
}

#[test]
fn test_filter_preserves_authored_message() {
    let parts = vec![text_part(REAL_USER_PROMPT)];
    assert_eq!(
        filter_injected_parts(&parts, &default_markers_owned()),
        parts
    );
}

#[test]
fn test_filter_preserves_part_order() {
    let parts = vec![
        text_part("first"),
        text_part(BACKGROUND_TASK_NOTIFICATION),
        text_part("second"),
    ];
    let kept: Vec<String> = filter_injected_parts(&parts, &default_markers_owned())
        .into_iter()
        .map(|part| part.text.unwrap_or_default())
        .collect();
    assert_eq!(kept, vec!["first".to_owned(), "second".to_owned()]);
}

#[test]
fn test_filter_handles_empty_input() {
    assert_eq!(
        filter_injected_parts::<MessagePart>(&[], &default_markers_owned()),
        Vec::<MessagePart>::new()
    );
}

// ---------------------------------------------------------------------------
// Shared-normalizer seam (upstream's `normalizeInjectionMarkers` block ports
// 1:1 against the re-exported `mem_core::config` implementation).
// ---------------------------------------------------------------------------

fn default_markers_owned() -> Vec<String> {
    DEFAULT_INJECTION_MARKERS
        .iter()
        .map(|marker| (*marker).to_owned())
        .collect()
}

#[test]
fn test_default_markers_match_config_defaults() {
    // Re-export identity guard: W2.3 must reuse the W0.2 kernel list, not
    // re-port it.
    assert_eq!(
        DEFAULT_INJECTION_MARKERS,
        mem_core::config::DEFAULT_INJECTION_MARKERS
    );
}

#[test]
fn test_normalize_returns_default_markers_when_unset() {
    assert_eq!(
        normalize_injection_markers(None).unwrap(),
        default_markers_owned()
    );
}

#[test]
fn test_normalize_appends_custom_markers_to_defaults() {
    let value = serde_json::json!(["<my-banner>"]);
    let result = normalize_injection_markers(Some(&value)).unwrap();

    for builtin in default_markers_owned() {
        assert!(result.contains(&builtin));
    }
    assert!(result.contains(&"<my-banner>".to_owned()));
}

#[test]
fn test_normalize_ignores_blank_entries_and_case_insensitive_duplicates() {
    let value = serde_json::json!(["", "   ", "<SYSTEM-REMINDER>"]);
    assert_eq!(
        normalize_injection_markers(Some(&value)).unwrap(),
        default_markers_owned()
    );
}

#[test]
fn test_normalize_trims_custom_marker_whitespace() {
    let value = serde_json::json!(["  <my-banner>  "]);
    let result = normalize_injection_markers(Some(&value)).unwrap();
    assert!(result.contains(&"<my-banner>".to_owned()));
}

#[test]
fn test_normalize_rejects_non_array_value() {
    let value = Value::String("<my-banner>".to_owned());
    let error = normalize_injection_markers(Some(&value)).unwrap_err();
    assert!(error.to_string().contains("expected an array"), "{error}");
}

#[test]
fn test_normalize_rejects_non_string_entry() {
    let value = serde_json::json!([42]);
    let error = normalize_injection_markers(Some(&value)).unwrap_err();
    assert!(error.to_string().contains("expected a string"), "{error}");
}

#[test]
fn test_normalize_does_not_mutate_default_marker_list_across_calls() {
    let custom = serde_json::json!(["<my-banner>"]);
    let _ = normalize_injection_markers(Some(&custom)).unwrap();
    assert!(!default_markers_owned().contains(&"<my-banner>".to_owned()));
    assert!(
        !normalize_injection_markers(None)
            .unwrap()
            .contains(&"<my-banner>".to_owned())
    );
}

// ---------------------------------------------------------------------------
// Allowlist / internal prompts (D6 — upstream `isStructuredSummaryPromptMessage`).
// The plugin's own summary prompts are exempt from the injection blocklist.
// ---------------------------------------------------------------------------

#[test]
fn test_internal_prompt_matches_profile_analysis_marker() {
    let prompt = "# User Profile Analysis\n\nAnalyze the following session excerpts.";
    assert!(is_internal_prompt(prompt));
}

#[test]
fn test_internal_prompt_matches_structured_summary_pair() {
    // Both signals are required: the summary instruction *and* `type="skip"`.
    let prompt = "Analyze this conversation.\n<conversation type=\"skip\">…</conversation>";
    assert!(is_internal_prompt(prompt));
}

#[test]
fn test_internal_prompt_rejects_partial_signals() {
    assert!(!is_internal_prompt("Analyze this conversation."));
    assert!(!is_internal_prompt("please type=\"skip\" this"));
    assert!(!is_internal_prompt(REAL_USER_PROMPT));
}

#[test]
fn test_allowlisted_internal_prompt_is_not_injected() {
    // An internal prompt that also carries an injection marker stays, because
    // the allowlist is consulted before the blocklist.
    let prompt =
        format!("<!-- OMO_INTERNAL_INITIATOR -->\n# User Profile Analysis\n{REAL_USER_PROMPT}");
    let filter = InjectionFilter::with_defaults();

    assert!(filter.is_allowed(&prompt));
    assert!(!filter.is_injected(&text_part(&prompt)));
    assert_eq!(filter.filter(&[text_part(&prompt)]).len(), 1);
    // The same marker in ordinary text is still blocked.
    assert!(filter.is_injected(&text_part(INTERNAL_CONTINUATION)));
}
