//! W5.3 Step 3 — upstream capture-time context budgeting, ported from
//! `example/opencode-mem/src/services/auto-capture.ts:339-514` (M3
//! correction: the generic `mem_core::utils::context_limit` primitives do
//! NOT implement this family; this module composes them).
//!
//! The marker is DISTINCT from the generic default marker — the custoM
//! upstream literal names the budget knob. Byte budgets are UTF-8 bytes
//! (not char counts); multi-byte characters are never split because every
//! slice goes through `slice_utf8_bytes`.

use mem_core::utils::context_limit::truncate_to_max_bytes_with_marker;

/// The custom capture marker (upstream `CONTEXT_TRUNCATION_MARKER`).
pub const CONTEXT_TRUNCATION_MARKER: &str = "\n[... truncated to autoCaptureMaxContextBytes ...]\n";

/// The default total request budget (upstream
/// `DEFAULT_AUTO_CAPTURE_MAX_CONTEXT_BYTES`, mirrored by mem-core's
/// resolved config default).
pub const DEFAULT_AUTO_CAPTURE_MAX_CONTEXT_BYTES: usize = 131_072;

/// Upstream `SUMMARY_REQUEST_OVERHEAD_BYTES`.
pub const SUMMARY_REQUEST_OVERHEAD_BYTES: usize = 1024;

/// Upstream `SUMMARY_OUTPUT_RESERVE_BYTES`.
pub const SUMMARY_OUTPUT_RESERVE_BYTES: usize = 16_384;

/// The analysis suffix appended to the bounded summary prompt (upstream
/// `SUMMARY_ANALYSIS_SUFFIX`).
pub const SUMMARY_ANALYSIS_SUFFIX: &str = "Analyze this conversation. If it contains technical work (code, bugs, features, decisions), create a concise summary and relevant tags. If it's non-technical (greetings, casual chat, incomplete requests), return type=\"skip\" with empty summary.";

/// One tool call's renderable shape (upstream `ToolCallInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallInfo {
    pub name: String,
    pub input: String,
}

fn join_sections(sections: &[String]) -> String {
    sections.join("\n")
}

/// Upstream `fitTextResponses` (`auto-capture.ts:339-370`).
///
/// Input responses are chronological; the output is unchanged when it fits
/// within `max_bytes`, otherwise it is at most `max_bytes` bytes and keeps
/// the newest suffix in original order. The boundary (oldest-kept)
/// response is truncated ONLY when the remaining budget still fits the
/// marker; otherwise the boundary response is dropped entirely.
///
/// **Loop invariant:** at each reverse-scan iteration, `kept` is the
/// original-order suffix of the visited newest responses and `used_bytes`
/// is exactly its UTF-8 byte length including separators, with
/// `used_bytes <= max_bytes`. The index strictly decreases to zero, so the
/// loop terminates.
fn fit_text_responses(text_responses: &[String], max_bytes: usize) -> String {
    if text_responses.is_empty() || max_bytes == 0 {
        return String::new();
    }

    let separator = "\n\n";
    let separator_bytes = separator.len();
    let joined = text_responses.join(separator);
    if joined.len() <= max_bytes {
        return joined;
    }

    let mut kept: Vec<String> = Vec::new();
    let mut used_bytes = 0usize;
    for response in text_responses.iter().rev() {
        let response_bytes = response.len();
        let extra_separator = if kept.is_empty() { 0 } else { separator_bytes };
        let needed = match response_bytes.checked_add(extra_separator) {
            Some(needed) => needed,
            None => break, // an overflow can never fit the budget
        };
        if used_bytes + needed <= max_bytes {
            kept.insert(0, response.clone());
            used_bytes += needed;
            continue;
        }

        let remaining = match max_bytes
            .checked_sub(used_bytes)
            .and_then(|rest| rest.checked_sub(extra_separator))
        {
            Some(remaining) => remaining,
            None => break,
        };
        if remaining > CONTEXT_TRUNCATION_MARKER.len() {
            kept.insert(
                0,
                truncate_to_max_bytes_with_marker(response, remaining, CONTEXT_TRUNCATION_MARKER),
            );
        }
        break;
    }
    kept.join(separator)
}

/// Upstream `getAutoCaptureMarkdownBudget` (`:377-383`): reserve
/// `min(24,576, floor(total × 0.25))` and return `max(4,096, total −
/// reserve)`.
pub fn get_auto_capture_markdown_budget(total_request_bytes: usize) -> usize {
    let request_reserve = usize::min(24_576, total_request_bytes / 4); // floor(total × 0.25)
    usize::max(4_096, total_request_bytes.saturating_sub(request_reserve))
}

/// Upstream `buildBoundedSummaryPrompt` (`:385-410`): reserve
/// `min(16,384, floor(total × 0.125))` for model output and subtract the
/// system-prompt bytes, the (compact JSON) schema bytes, that reserve and
/// the 1,024 overhead before truncating `context` + suffix.
pub fn build_bounded_summary_prompt(
    context: &str,
    system_prompt: &str,
    schema_json: &str,
    total_request_bytes: usize,
) -> String {
    let schema_bytes = schema_json.len();
    let output_reserve = usize::min(
        SUMMARY_OUTPUT_RESERVE_BYTES,
        total_request_bytes / 8, // floor(total × 0.125)
    );
    let user_budget = total_request_bytes
        .saturating_sub(system_prompt.len())
        .saturating_sub(schema_bytes)
        .saturating_sub(output_reserve)
        .saturating_sub(SUMMARY_REQUEST_OVERHEAD_BYTES);
    truncate_to_max_bytes_with_marker(
        &format!("{context}\n\n{SUMMARY_ANALYSIS_SUFFIX}"),
        user_budget,
        CONTEXT_TRUNCATION_MARKER,
    )
}

/// Upstream `buildMarkdownContext` (`:412-514`).
///
/// The section order is the historical contract: Previous Memory Context →
/// User Request → AI Response → Tools Used. When AI responses exist and
/// the user budget exceeds 1,024 bytes after skeleton accounting, `min(
/// 4,096, floor(maxContextBytes × 0.25))` byte space is reserved for them.
/// The composed result is capped at `max_context_bytes` total UTF-8 bytes
/// (multi-byte characters are never split; the cap only needs the generic
/// head+tail primitive as a last resort).
pub fn build_markdown_context(
    user_prompt: &str,
    text_responses: &[String],
    tool_calls: &[ToolCallInfo],
    latest_memory: Option<&str>,
    max_context_bytes: usize,
) -> String {
    let memory_sections: Vec<&str> = match latest_memory {
        Some(memory) if !memory.is_empty() => {
            vec!["## Previous Memory Context", "---", memory, "---\n"]
        }
        _ => Vec::new(),
    };

    let mut tools_sections: Vec<String> = Vec::new();
    if !tool_calls.is_empty() {
        tools_sections.push("## Tools Used".to_string());
        tools_sections.push("---".to_string());
        for tool in tool_calls {
            if tool.input.is_empty() {
                tools_sections.push(format!("- {}", tool.name));
            } else {
                tools_sections.push(format!("- {}({})", tool.name, tool.input));
            }
        }
        tools_sections.push("---\n".to_string());
    }

    let user_wrapper = join_sections(&[
        "## User Request".to_string(),
        "---".to_string(),
        String::new(),
        "---\n".to_string(),
    ]);
    let ai_wrapper = if text_responses.is_empty() {
        String::new()
    } else {
        join_sections(&[
            "## AI Response".to_string(),
            "---".to_string(),
            String::new(),
            "---\n".to_string(),
        ])
    };

    let mut skeleton_parts: Vec<String> = memory_sections.iter().map(|s| s.to_string()).collect();
    skeleton_parts.push(user_wrapper.clone());
    if !text_responses.is_empty() {
        skeleton_parts.push(ai_wrapper.clone());
    }
    skeleton_parts.extend(tools_sections.iter().cloned());
    let skeleton_bytes = join_sections(&skeleton_parts).len();

    let mut user_budget = max_context_bytes.saturating_sub(skeleton_bytes);
    if !text_responses.is_empty() && user_budget > 1024 {
        let preferred_ai_floor = usize::min(4_096, max_context_bytes / 4);
        user_budget = usize::max(256, user_budget.saturating_sub(preferred_ai_floor));
    }

    let bounded_user = if user_prompt.len() <= user_budget {
        user_prompt.to_string()
    } else {
        truncate_to_max_bytes_with_marker(user_prompt, user_budget, CONTEXT_TRUNCATION_MARKER)
    };

    let user_lines: Vec<String> = vec![
        "## User Request".to_string(),
        "---".to_string(),
        bounded_user.clone(),
        "---\n".to_string(),
    ];

    if text_responses.is_empty() {
        let mut prefix_parts: Vec<String> = memory_sections.iter().map(|s| s.to_string()).collect();
        prefix_parts.extend(user_lines.iter().cloned());
        prefix_parts.extend(tools_sections.iter().cloned());
        let prefix = join_sections(&prefix_parts);
        if prefix.len() <= max_context_bytes {
            return prefix;
        }
        return truncate_to_max_bytes_with_marker(
            &prefix,
            max_context_bytes,
            CONTEXT_TRUNCATION_MARKER,
        );
    }

    // The AI section is inserted BEFORE tools to preserve the historical
    // section order.
    let mut prefix_without_tools: Vec<String> =
        memory_sections.iter().map(|s| s.to_string()).collect();
    prefix_without_tools.extend(user_lines.iter().cloned());
    let prefix_without_tools = join_sections(&prefix_without_tools);
    let tools_block = if tools_sections.is_empty() {
        String::new()
    } else {
        format!("\n{}", join_sections(&tools_sections))
    };
    let ai_budget = max_context_bytes
        .saturating_sub(prefix_without_tools.len())
        .saturating_sub(tools_block.len())
        .saturating_sub(ai_wrapper.len());
    let bounded_ai = fit_text_responses(text_responses, ai_budget);

    let mut result_parts: Vec<String> = memory_sections.iter().map(|s| s.to_string()).collect();
    result_parts.extend(user_lines.iter().cloned());
    result_parts.push(join_sections(&[
        "## AI Response".to_string(),
        "---".to_string(),
        bounded_ai,
        "---\n".to_string(),
    ]));
    result_parts.extend(tools_sections.iter().cloned());
    let result = join_sections(&result_parts);

    if result.len() <= max_context_bytes {
        return result;
    }
    truncate_to_max_bytes_with_marker(&result, max_context_bytes, CONTEXT_TRUNCATION_MARKER)
}
