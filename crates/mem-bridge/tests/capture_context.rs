//! W5.3 Step 3 — RED oracle port of upstream
//! `tests/auto-capture-context.test.ts` (six cases; the behavioral
//! contract for the capture-time budget builders).

use mem_bridge::capture_context::{
    SCHEMA_ANSWER_INSTRUCTION, SUMMARY_OUTPUT_RESERVE_BYTES, SUMMARY_REQUEST_OVERHEAD_BYTES,
    ToolCallInfo, build_bounded_summary_prompt, build_markdown_context,
};

/// Upstream: `leaves small contexts unchanged`.
#[test]
fn test_auto_capture_context_leaves_small_context_unchanged() {
    let context = build_markdown_context(
        "Fix the bug",
        &["Done. Updated the handler.".to_string()],
        &[ToolCallInfo {
            name: "edit".to_string(),
            input: "file.ts".to_string(),
        }],
        Some("Prior memory"),
        131_072,
    );
    assert!(context.contains("## User Request"));
    assert!(context.contains("Fix the bug"));
    assert!(context.contains("Done. Updated the handler."));
    assert!(context.contains("## Tools Used"));
    assert!(!context.contains("truncated to autoCaptureMaxContextBytes"));
}

/// Upstream: `truncates oversized AI responses within the total budget`.
#[test]
fn test_auto_capture_context_truncates_oversized_ai_responses_within_total_budget() {
    let huge_ai = "A".repeat(50_000);
    let max_bytes = 8_192;
    let context = build_markdown_context("short request", &[huge_ai], &[], None, max_bytes);
    assert!(
        context.len() <= max_bytes,
        "the total context must stay within {} bytes (got {})",
        max_bytes,
        context.len()
    );
    assert!(context.contains("short request"));
    assert!(context.contains("truncated to autoCaptureMaxContextBytes"));
}

/// Upstream: `prefers newer AI responses when multiple turns exceed the
/// budget`.
#[test]
fn test_auto_capture_context_prefers_newer_ai_responses() {
    let older = format!("OLD-{}", "x".repeat(4_000));
    let newer = format!("NEW-{}", "y".repeat(4_000));
    let max_bytes = 3_000;
    let context = build_markdown_context("q", &[older, newer], &[], None, max_bytes);
    assert!(context.len() <= max_bytes);
    assert!(context.contains("NEW-"));
    assert!(
        !(context.contains("OLD-") && !context.contains("NEW-")),
        "older content must never be kept while the newest turn is dropped"
    );
}

/// Upstream: `preserves section order: memory, user, AI, tools`.
#[test]
fn test_auto_capture_context_preserves_memory_user_ai_tools_order() {
    let context = build_markdown_context(
        "request",
        &["response".to_string()],
        &[ToolCallInfo {
            name: "bash".to_string(),
            input: "ls".to_string(),
        }],
        Some("memory"),
        131_072,
    );
    let memory_index = context.find("## Previous Memory Context");
    let user_index = context.find("## User Request");
    let ai_index = context.find("## AI Response");
    let tools_index = context.find("## Tools Used");
    let memory_index = memory_index.expect("memory section present");
    let user_index = user_index.expect("user section present");
    let ai_index = ai_index.expect("ai section present");
    let tools_index = tools_index.expect("tools section present");
    assert!(user_index > memory_index);
    assert!(ai_index > user_index);
    assert!(tools_index > ai_index);
}

/// Upstream: `guarantees the total UTF-8 size never exceeds the configured
/// limit` (multi-byte inputs included).
#[test]
fn test_auto_capture_context_caps_total_utf8_bytes() {
    let context = build_markdown_context(
        &"U".repeat(20_000),
        &["A".repeat(40_000), "B".repeat(40_000)],
        &[ToolCallInfo {
            name: "read".to_string(),
            input: "x".repeat(100),
        }],
        Some(&*"M".repeat(500)),
        10_000,
    );
    assert!(context.len() <= 10_000);
}

/// Upstream: `reserves request space for prompts, schemas, and model
/// output`.
///
/// Re-derived: this row used to compose its context through
/// `build_markdown_context`, whose context is capped at 75 % of `total`, so the
/// prompt clamp never fired — the assertion could not fail. It now calls the
/// `pub` function DIRECTLY with a 200 000-byte context, so the clamp always
/// fires, and asserts the invariant the charge exists to hold:
///
/// ```text
/// system.len() + prompt.len() + OUTPUT_RESERVE + REQUEST_OVERHEAD <= total
/// ```
///
/// Every numeral is evaluated at run time from `len()`, never counted by hand,
/// so the identity holds for any `system_prompt`.
#[test]
fn test_auto_capture_context_reserves_prompt_schema_and_output_space() {
    let total_budget = 131_072;
    let system_prompt = "system instructions";
    let schema = r#"{"type":"object","properties":{"summary":{"type":"string"}}}"#;
    let user_prompt =
        build_bounded_summary_prompt(&"A".repeat(200_000), system_prompt, schema, total_budget);

    // `<=`, where the old row asserted `<`: this is a RELAXATION of the row, not
    // a tightening — the charged path lands on equality by construction.
    let charged = system_prompt.len()
        + user_prompt.len()
        + SUMMARY_OUTPUT_RESERVE_BYTES
        + SUMMARY_REQUEST_OVERHEAD_BYTES;
    assert!(
        charged <= total_budget,
        "the prompt + reserves fit the budget: {charged} > {total_budget}"
    );
    assert!(
        user_prompt.contains("truncated to autoCaptureMaxContextBytes"),
        "the 200 000-byte context was clamped"
    );
}

/// The schema must reach the model even though the route that carries it has no
/// `format` field: it is appended to the prompt TEXT, after truncation, so only
/// the conversation is ever cut.
#[test]
fn test_bounded_summary_prompt_carries_the_schema_and_the_json_instruction() {
    let schema = r#"{"type":"object","properties":{"summary":{"type":"string"},"type":{"type":"string"},"tags":{"type":"array"}},"required": ["summary", "type", "tags"]}"#;
    let prompt =
        build_bounded_summary_prompt("user: capture this", "system instructions", schema, 131_072);
    assert!(
        prompt.contains(r#""required": ["summary", "type", "tags"]"#),
        "the schema's required list is in the prompt text"
    );
    assert!(
        prompt.contains(SCHEMA_ANSWER_INSTRUCTION),
        "the schema is a CONTRACT, not a trailing blob"
    );

    // A conversation large enough to be truncated still carries an intact
    // schema block: the schema is appended AFTER the clamp.
    let cut =
        build_bounded_summary_prompt(&"A".repeat(200_000), "system instructions", schema, 131_072);
    assert!(cut.contains("truncated to autoCaptureMaxContextBytes"));
    assert!(cut.ends_with(schema), "the schema block is intact and last");
    assert!(cut.contains(SCHEMA_ANSWER_INSTRUCTION));
}
