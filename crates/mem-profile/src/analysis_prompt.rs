//! Profile-analysis prompt construction (W3.2) — port of upstream
//! `src/services/user-memory-learning.ts` L392-450 (`buildUserAnalysisContext`),
//! L347-390 (`buildCategorySummary`), L704-719 (`buildProfileSystemPrompt`) and
//! the `createUserProfileAnalysisSchema` surface.
//!
//! Two transports serve the same analysis (a runtime mode, not a config
//! toggle): the OpenCode structured-output path (`PromptMode::Json`, whose
//! prompt must not mention tools) and the external tool-call path
//! (`PromptMode::Tool`, which requires the `update_user_profile` sentence).
//!
//! Truncation has two caps, both char-safe (never a byte slice on a UTF-8
//! string):
//! - [`MAX_PROMPT_CHARS`] per prompt content, applied before numbering;
//! - the whole assembled context capped head-only at the caller's
//!   `userProfileMaxContextBytes`-derived char budget, then closed with
//!   [`CONTEXT_TRUNCATION_MARKER`]. Upstream uses `substring` (head-only), so
//!   the head+tail `mem_core` helper would change profile behavior — this
//!   module never calls it.

use crate::profile_store::ProfileData;
use crate::prompt::UserPrompt;

/// Upstream's per-prompt cap (PRD: "4096 chars before LLM call").
pub const MAX_PROMPT_CHARS: usize = 4096;

/// Upstream `CONFIG.userProfileMaxContextBytes ?? 32768`.
pub const DEFAULT_MAX_CONTEXT_CHARS: usize = 32768;

/// Upstream's literal `"[... context truncated to userProfileMaxContextBytes ...]"`
/// marker, appended (after a newline, byte-for-byte parity) when the assembled
/// context was over the cap.
pub const CONTEXT_TRUNCATION_MARKER: &str =
    "[... context truncated to userProfileMaxContextBytes ...]";

/// The transport the analysis is served through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    /// OpenCode structured output (no tools exist → JSON-object instruction).
    Json,
    /// External API tool call (`update_user_profile` → tool instruction).
    Tool,
}

/// Upstream `buildProfileSystemPrompt(existingProfile, langName, mode)`.
pub fn build_system_prompt(existing: bool, lang_name: &str, mode: PromptMode) -> String {
    let action = if existing { "update" } else { "create" };
    let closing = match mode {
        PromptMode::Tool => format!(
            "Use the update_user_profile tool to save the {} profile.",
            if existing { "updated" } else { "new" }
        ),
        PromptMode::Json => format!(
            "Return the {} profile as the JSON object described in the request. Reply with JSON only.",
            if existing { "updated" } else { "new" }
        ),
    };
    format!(
        "You are a user behavior analyst for a coding assistant.

Your task is to analyze user prompts and {action} a comprehensive user profile.

CRITICAL: You MUST write all descriptions, categories, and text in {lang_name}.

CRITICAL: All JSON string values MUST escape double quotes with backslash. Do NOT use unescaped quotation marks inside string values.

{closing}"
    )
}

/// Upstream `buildUserAnalysisContext(prompts, existingProfile)`.
///
/// Each prompt content is capped at [`MAX_PROMPT_CHARS`] chars (no marker)
/// before numbering; the assembled text is then capped head-only at
/// `max_chars` chars, in which case [`CONTEXT_TRUNCATION_MARKER`] is appended.
pub fn build_user_prompt(
    prompts: &[UserPrompt],
    existing: Option<&ProfileData>,
    max_chars: usize,
) -> String {
    let mut assembled = String::new();
    assembled.push_str("# User Profile Analysis\n\n");
    assembled.push_str(&format!(
        "Analyze {} user prompts to {} the user profile.\n",
        prompts.len(),
        if existing.is_some() {
            "update"
        } else {
            "create"
        }
    ));
    assembled.push_str(if existing.is_some() {
        "The merge system will automatically connect your observations to existing profile entries — you only need to describe what you see in these recent prompts."
    } else {
        "Create a new user profile from scratch based on the prompts below."
    });
    assembled.push_str("\n\n");
    if let Some(profile) = existing {
        assembled.push_str(&build_category_summary(profile));
    }
    assembled.push_str("\n## Recent Prompts\n\n");
    for (index, prompt) in prompts.iter().enumerate() {
        if index > 0 {
            assembled.push_str("\n\n");
        }
        assembled.push_str(&format!(
            "{}. {}",
            index + 1,
            truncate_chars(&prompt.content, MAX_PROMPT_CHARS)
        ));
    }
    assembled.push_str("\n\n");
    assembled.push_str(&analysis_guidelines(existing.is_some()));
    truncate_context(&assembled, max_chars)
}

/// The `## Analysis Guidelines` + few-shot block, verbatim from upstream.
fn analysis_guidelines(existing: bool) -> String {
    format!(
        "## Analysis Guidelines

Identify and {action}:

 1. **Preferences**
   - Code style, communication style, tool preferences
   - Assign confidence 0.3-0.5 based on evidence strength in these recent prompts
   - Include 1-3 example prompts as evidence
   - **Revealed preferences**: when the user chooses one approach over alternatives (e.g. picks simpler solution, skips certain steps), capture the choice as a lower-confidence preference (0.3-0.5). What the user does NOT do is also a signal.

 2. **Patterns**
   - Recurring topics, problem domains, technical interests seen in these prompts
   - Track frequency of occurrence

 3. **Workflows**
    - Distinct, named step sequences the user follows repeatedly
    - Each workflow should represent a DIFFERENT activity (different purpose, different steps)
    - Break down into 3-6 concrete, observable steps, NOT abstract phases
    - Do NOT repeat the same workflow every cycle — only output when you observe a NEW recurring sequence
    - Examples of distinct workflows: \"debugging workflow\", \"code review workflow\", \"learning workflow\", \"refactoring workflow\"

CRITICAL: Only output observations grounded in the RECENT PROMPTS above. Write descriptions in your own words — the system matches by embedding similarity, not exact wording. Do NOT output entries that lack evidence in recent prompts. Put the core semantics at the beginning of each description, keeping descriptions concise and specific (under 120 characters). Do NOT extract one-time debugging tasks, environment setup issues, or specific error investigations as preferences — these are transient events, not behavioral patterns.

## Few-Shot Examples

❌ Do NOT extract as preference:
- \"User is debugging a NullPointerException in auth service\" (one-time debugging task)
- \"User installed Redis for the first time\" (one-time setup event)
- \"User ran npm audit fix\" (routine maintenance, not a behavioral pattern)

✅ DO extract as preference:
- \"User prefers functional programming style over OOP\"
- \"User consistently writes tests before implementation\"
- \"User asks for explanations before accepting code changes\"

✅ DO extract as workflow (distinct, non-overlapping):
- Debugging workflow: \"reproduce the error → check logs → grep source code → trace call chain → propose fix → verify fix\"
- Code review workflow: \"read the diff → check edge cases → verify consistency with existing patterns → report issues → suggest alternatives\"
- Learning workflow: \"ask for explanation → request examples → test understanding with a small task → apply to real problem\"

❌ Do NOT extract as workflow:
- \"User analyzes problems and verifies solutions\" (too abstract — not a concrete step sequence)
- \"User writes code and tests it\" (too generic — covers everything)",
        action = if existing { "report" } else { "create" }
    )
}

/// Upstream `buildCategorySummary(profileData)` — existing category names,
/// existing workflows, and the profile size line.
fn build_category_summary(profile: &ProfileData) -> String {
    let preference_categories = unique_categories(&profile.preferences);
    let pattern_categories = unique_categories(&profile.patterns);

    let mut category_parts: Vec<String> = Vec::new();
    if !preference_categories.is_empty() {
        let counts = preference_categories
            .iter()
            .map(|category| {
                let count = profile
                    .preferences
                    .iter()
                    .filter(|item| item_category(item) == *category)
                    .count();
                format!("{category} ({count})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        category_parts.push(format!("Preference categories: {counts}"));
    }
    if !pattern_categories.is_empty() {
        let counts = pattern_categories
            .iter()
            .map(|category| {
                let count = profile
                    .patterns
                    .iter()
                    .filter(|item| item_category(item) == *category)
                    .count();
                format!("{category} ({count})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        category_parts.push(format!("Pattern categories: {counts}"));
    }

    let category_section = if category_parts.is_empty() {
        String::new()
    } else {
        format!(
            "## Existing Categories\nUse these exact category names when your observation fits:\n\n{}\n",
            category_parts.join("\n")
        )
    };

    let mut workflow_parts: Vec<String> = Vec::new();
    if !profile.workflows.is_empty() {
        workflow_parts.push(
            "## Existing Workflows\nFor reference — only report a workflow when recent prompts show a genuinely NEW sequence, NOT a minor variant of an existing one:"
                .to_string(),
        );
        for (index, workflow) in profile.workflows.iter().enumerate() {
            let steps = match workflow.steps.as_ref().filter(|steps| !steps.is_empty()) {
                Some(steps) => format!(
                    " (freq {}x: {})",
                    workflow.frequency_or_one(),
                    steps.join(" → ")
                ),
                None => String::new(),
            };
            workflow_parts.push(format!("{}. {}{steps}", index + 1, workflow.description));
        }
    }

    let count_section = format!(
        "## Profile Size\nPreferences: {} | Patterns: {} | Workflows: {}\n\nNew observations are matched via embedding cosine similarity — write descriptions in your own words; do not reuse existing wording.",
        profile.preferences.len(),
        profile.patterns.len(),
        profile.workflows.len()
    );

    let mut parts: Vec<String> = Vec::new();
    if !category_section.is_empty() {
        parts.push(category_section);
    }
    parts.extend(workflow_parts);
    parts.push(count_section);
    parts.join("\n")
}

/// The item's category (`None` reads as the empty category name).
fn item_category(item: &crate::profile_store::ProfileItem) -> String {
    item.category.clone().unwrap_or_default()
}

/// Distinct category names in first-seen order (upstream's `[...new Set(...)]`).
fn unique_categories(items: &[crate::profile_store::ProfileItem]) -> Vec<String> {
    let mut categories: Vec<String> = Vec::new();
    for item in items {
        let category = item_category(item);
        if !categories.contains(&category) {
            categories.push(category);
        }
    }
    categories
}

/// The assembled context cap: head-only (upstream `substring`) plus the
/// upstream marker, both char-safe — never a byte slice on a UTF-8 string
/// (D-W2: `s.substring(0, maxBytes) + "\n[...]"` byte parity).
fn truncate_context(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        format!(
            "{}\n{CONTEXT_TRUNCATION_MARKER}",
            truncate_chars(text, max_chars)
        )
    } else {
        text.to_string()
    }
}

/// Char-safe head: the first `max_chars` characters (never a byte slice).
fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// Upstream `createUserProfileAnalysisSchema` — the structured-output JSON
/// Schema for the analysis response.
pub fn analysis_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["preferences", "patterns", "workflows"],
        "properties": {
            "preferences": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["category", "description", "confidence", "evidence"],
                    "properties": {
                        "category": { "type": "string" },
                        "description": { "type": "string" },
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                        "evidence": { "type": "array", "items": { "type": "string" } }
                    }
                }
            },
            "patterns": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["category", "description"],
                    "properties": {
                        "category": { "type": "string" },
                        "description": { "type": "string" }
                    }
                }
            },
            "workflows": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["description", "steps"],
                    "properties": {
                        "description": { "type": "string" },
                        "steps": { "type": "array", "items": { "type": "string" } }
                    }
                }
            }
        }
    })
}
