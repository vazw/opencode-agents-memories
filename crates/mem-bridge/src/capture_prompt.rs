//! W5.3 — the capture summary prompt/schema/skip parsing (upstream
//! `auto-capture.ts:572-614` + the capture system prompt).
//!
//! The upstream base output is `{summary, type, tags}` with
//! `type == "skip"` and normalized (lowercase/trimmed) tags; the port's
//! episodic extension (`files_touched`, `completed_work`, `decisions`,
//! `open_threads`) is accepted and parsed when present.

use std::sync::Arc;

use mem_embed::embedder::BoxFuture;

use mem_episodic::record::{Decision, WorkItem, WorkKind};

use crate::capture::{CaptureExtension, CaptureSummaryOutput};

/// The capture system prompt (upstream's capture system prompt with the
/// language line resolved by the configured `auto_capture_language`, whose
/// port default is "English" — upstream's `auto` resolves via a language
/// detector the port does not yet carry).
pub const CAPTURE_SYSTEM_PROMPT: &str = "\
You are a technical memory recorder for a software development project.

RULES:
1. ONLY capture technical work (code, bugs, features, architecture, config)
2. SKIP non-technical by returning type=\"skip\"
3. NO meta-commentary or behavior analysis
4. Include specific file names, functions, technical details
5. Generate 2-4 technical tags (e.g., \"react\", \"auth\", \"bug-fix\")
6. You MUST write the summary in English.

FORMAT:
## Request
[1-2 sentences: what was requested, in English]

## Outcome
[1-2 sentences: what was done, include files/functions, in English]

SKIP if: greetings, casual chat, no code/decisions made
CAPTURE if: code changed, bug fixed, feature added, decision made";

/// The frozen JSON schema the provider must validate against (upstream
/// `z.object({ summary, type, tags })`).
pub const CAPTURE_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "summary": {"type": "string"},
    "type": {"type": "string"},
    "tags": {"type": "array", "items": {"type": "string"}},
    "filesTouched": {"type": "array", "items": {"type": "string"}},
    "completedWork": {"type": "array", "items": {
        "type": "object",
        "properties": {"kind": {"type": "string"}, "description": {"type": "string"}},
        "required": ["kind", "description"]
    }},
    "decisions": {"type": "array", "items": {
        "type": "object",
        "properties": {"description": {"type": "string"}},
        "required": ["description"]
    }},
    "openThreads": {"type": "array", "items": {"type": "string"}}
  },
  "required": ["summary", "type", "tags"]
}"#;

/// The summarizer seam for captures: `context` is the bounded markdown
/// context (or the raw transcript when the wrapper sent none). The
/// implementation runs OFF the serial DB lane.
pub trait CaptureSummarizer: Send + Sync {
    fn summarize<'a>(
        &'a self,
        context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, mem_episodic::error::EpisodicError>>;
}

/// Parses one raw provider output into the structured result.
///
/// Fail-closed: nothing that looks like a capture without a summary parses;
/// `type: "skip"` short-circuits to Skip regardless of other fields; tags
/// are normalized (lowercase + trim) and deduplicated.
pub fn parse_capture_output(
    raw: &str,
) -> Result<CaptureSummaryOutput, mem_episodic::error::EpisodicError> {
    let wrapped = mem_store::serde_json::from_str::<mem_store::serde_json::Value>(raw)
        .or_else(|_| parse_trailing_json(raw));
    let value = match wrapped {
        Ok(value) => value,
        Err(error) => {
            return Err(mem_episodic::error::EpisodicError::Summary {
                message: format!("capture summary JSON parse failed: {error}"),
            });
        }
    };
    if !value.is_object() {
        return Err(mem_episodic::error::EpisodicError::Summary {
            message: "capture summary output is not a JSON object".to_string(),
        });
    }
    let memory_type = value
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .trim();
    if memory_type.eq_ignore_ascii_case("skip") {
        return Ok(CaptureSummaryOutput::Skip);
    }
    let summary = value
        .get("summary")
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if summary.is_empty() {
        return Err(mem_episodic::error::EpisodicError::Summary {
            message: "capture summary output has an empty summary".to_string(),
        });
    }
    let mut tags = Vec::new();
    for tag in value
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
    {
        if let Some(tag) = tag.as_str() {
            let tag = tag.trim().to_lowercase();
            if !tag.is_empty() && !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    Ok(CaptureSummaryOutput::Capture {
        summary,
        memory_type: memory_type.to_string(),
        tags,
        extension: parse_extension(&value),
    })
}

/// Parses the episodic extension (`filesTouched`, `completedWork`,
/// `decisions`, `openThreads`).
///
/// Every entry is sanitized and bounded: `mem-episodic`'s validation rejects
/// empty, NUL-bearing and oversized entries, so an unsanitized provider reply
/// would fail the WHOLE episodic commit and burn a retry.
fn parse_extension(value: &mem_store::serde_json::Value) -> CaptureExtension {
    use mem_episodic::validation::{
        MAX_DECISIONS, MAX_FILE_PATH_LEN, MAX_FILES, MAX_THREADS, MAX_WORK_DESC_LEN, MAX_WORK_ITEMS,
    };

    let strings = |key: &str, max_bytes: usize, limit: usize| -> Vec<String> {
        value
            .get(key)
            .and_then(|items| items.as_array())
            .map(|items| items.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter_map(|item| item.as_str())
            .filter_map(|text| sanitize_extension_text(text, max_bytes))
            .take(limit)
            .collect()
    };

    let completed_work = value
        .get("completedWork")
        .and_then(|items| items.as_array())
        .map(|items| items.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| {
            let description = item.get("description").and_then(|d| d.as_str())?;
            Some(WorkItem {
                kind: work_kind(item.get("kind").and_then(|k| k.as_str())),
                description: sanitize_extension_text(description, MAX_WORK_DESC_LEN)?,
            })
        })
        .take(MAX_WORK_ITEMS)
        .collect();

    let decisions = value
        .get("decisions")
        .and_then(|items| items.as_array())
        .map(|items| items.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| {
            let description = item.get("description").and_then(|d| d.as_str())?;
            Some(Decision {
                description: sanitize_extension_text(description, MAX_WORK_DESC_LEN)?,
            })
        })
        .take(MAX_DECISIONS)
        .collect();

    CaptureExtension {
        files_touched: strings("filesTouched", MAX_FILE_PATH_LEN, MAX_FILES),
        completed_work,
        decisions,
        open_threads: strings("openThreads", MAX_WORK_DESC_LEN, MAX_THREADS),
    }
}

/// The explicit `kind` mapping: the three documented wire names map to their
/// `WorkKind`; EVERYTHING else is `Other` (never a parse failure — one
/// unexpected kind must not discard a whole capture).
fn work_kind(kind: Option<&str>) -> WorkKind {
    match kind.unwrap_or_default() {
        "feature" => WorkKind::Feature,
        "bugFix" => WorkKind::BugFix,
        "command" => WorkKind::Command,
        _ => WorkKind::Other,
    }
}

/// Trims one extension string; `None` when empty or NUL-bearing, otherwise
/// bounded at a UTF-8 char boundary.
fn sanitize_extension_text(raw: &str, max_bytes: usize) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return None;
    }
    if trimmed.len() <= max_bytes {
        return Some(trimmed.to_string());
    }
    let mut end = max_bytes;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(trimmed[..end].to_string())
}

/// Lenient fallback: the first balanced-JSON object in free text.
fn parse_trailing_json(raw: &str) -> Result<mem_store::serde_json::Value, String> {
    let bytes = raw.as_bytes();
    let start = raw.find('{').ok_or("no opening brace")?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        match (escaped, byte, in_string) {
            (true, _, _) => escaped = false,
            (_, b'\\', true) => escaped = true,
            (_, b'"', false) => in_string = true,
            (_, b'"', true) => in_string = false,
            (_, b'{', false) => depth += 1,
            (_, b'}', false) => {
                depth -= 1;
                if depth == 0 {
                    let candidate = &raw[start..=offset];
                    return mem_store::serde_json::from_str(candidate).map_err(|e| e.to_string());
                }
            }
            _ => {}
        }
    }
    Err("unbalanced JSON in capture output".to_string())
}

// ---------------------------------------------------------------------------
// The production provider adapter (the engine composition root composes it)
// ---------------------------------------------------------------------------

/// The provider-backed capture summarizer: one structured completion over
/// the bounded summary prompt; `{text}` deltas are joined, `{"json": v}`
/// short-circuits to the parsed object, and everything parses through
/// [`parse_capture_output`] fail-closed.
pub struct ProviderCaptureSummarizer {
    provider: Arc<dyn mem_ai::provider::AiProvider>,
    model: String,
    max_context_bytes: usize,
}

impl ProviderCaptureSummarizer {
    pub fn new(
        provider: Arc<dyn mem_ai::provider::AiProvider>,
        model: impl Into<String>,
        max_context_bytes: usize,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            max_context_bytes,
        }
    }
}

impl CaptureSummarizer for ProviderCaptureSummarizer {
    fn summarize<'a>(
        &'a self,
        context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, mem_episodic::error::EpisodicError>> {
        Box::pin(async move {
            use mem_episodic::futures::StreamExt;
            let schema_json = CAPTURE_OUTPUT_SCHEMA;
            let user_prompt = crate::capture_context::build_bounded_summary_prompt(
                context,
                CAPTURE_SYSTEM_PROMPT,
                schema_json,
                self.max_context_bytes,
            );
            let mut request = mem_ai::provider::request::CompletionRequest::new(self.model.clone())
                .with_system(CAPTURE_SYSTEM_PROMPT)
                .with_message(mem_ai::provider::request::ChatMessage::user(user_prompt));
            // CAPTURE_OUTPUT_SCHEMA parse: propagate into EpisodicError::Json, whose From<serde_json::Error> exists (ADR-0013 D-3)
            request.json_schema = Some(mem_store::serde_json::from_str(schema_json)?);
            let mut stream = self.provider.complete(request);
            let mut text = String::new();
            while let Some(delta) = stream.next().await {
                match delta {
                    Ok(chunk) => {
                        if let Some(text_delta) = chunk.get("text").and_then(|t| t.as_str()) {
                            text.push_str(text_delta);
                        }
                        if let Some(json) = chunk.get("json") {
                            let serialized = mem_store::serde_json::to_string(json)
                                .unwrap_or_else(|_| String::new());
                            return parse_capture_output(&serialized);
                        }
                    }
                    Err(error) => {
                        return Err(mem_episodic::error::EpisodicError::Summary {
                            message: format!("capture summarizer provider failed: {error}"),
                        });
                    }
                }
            }
            parse_capture_output(&text)
        })
    }
}
