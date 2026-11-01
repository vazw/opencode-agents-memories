//! W5.3 — the capture summary prompt/schema/skip parsing (upstream
//! `auto-capture.ts:572-614` + the capture system prompt).
//!
//! The upstream base output is `{summary, type, tags}` with
//! `type == "skip"` and normalized (lowercase/trimmed) tags; the port's
//! episodic extension (`files_touched`, `completed_work`, `decisions`,
//! `open_threads`) is accepted and parsed when present.

use std::sync::{Arc, Mutex};

use mem_embed::embedder::BoxFuture;

use mem_ai::provider::error::wire_reason;
use mem_ai::provider::{ProviderError, ProviderErrorKind};

use mem_core::sync::lock_or_recover;

use mem_episodic::record::{Decision, WorkItem, WorkKind};

use crate::capture::{CaptureExtension, CaptureSummaryOutput, SkipReason};

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

/// One NAMED summarizer failure: the closed capture reason plus the provider
/// classification. `error_kind` is a [`ProviderErrorKind`] label and `status`
/// the HTTP status, when the failure came from a response.
///
/// `retryable` is decided by the CLASSIFIED token (never by sniffing a
/// message), `retry_after_ms` carries the host's hint, and `detail` is the
/// decoded operator payload — copied verbatim, never parsed for a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummarizerFailure {
    /// One of the capture reasons, or a host-resolution reason.
    pub reason: &'static str,
    /// `ProviderErrorKind::as_str()` label.
    pub error_kind: &'static str,
    /// HTTP status when the host answered.
    pub status: Option<u16>,
    /// Whether another attempt may succeed.
    pub retryable: bool,
    /// The host's `retryAfterMs` hint, when it sent one.
    pub retry_after_ms: Option<u64>,
    /// Decoded operator detail (bounded, never the reply).
    pub detail: Option<String>,
}

/// A `retryAfterMs` above this many milliseconds dead-letters the capture
/// instead of holding it for a retry. A policy choice, not a measurement: the
/// drain path must not park a capture behind one long outage.
pub const HOST_RETRY_AFTER_CEILING_MS: u64 = 30_000;

/// The summarizer seam for captures: `context` is the bounded markdown
/// context (or the raw transcript when the wrapper sent none). The
/// implementation runs OFF the serial DB lane.
pub trait CaptureSummarizer: Send + Sync {
    fn summarize<'a>(
        &'a self,
        context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, mem_episodic::error::EpisodicError>>;

    /// The failure of the LAST `summarize` call, when it failed on the provider.
    ///
    /// A JSON/parse failure reports `None`: the record is cleared on entry, so
    /// a stale provider reason can never name a later parse failure. Defaulted
    /// (never required) so non-provider implementations stay unchanged.
    fn last_failure(&self) -> Option<SummarizerFailure> {
        None
    }
}

/// Parses one raw provider output into the structured result.
///
/// The text goes through the same three-shape recovery the host's schema-free
/// generate route needs (direct JSON → fenced block → last balanced object;
/// `mem_ai::provider::remote::parse_structured_lenient`), so the tree keeps
/// exactly one scanner. Everything after the decode is the unchanged capture
/// contract: fail-closed without a summary, `type: "skip"` short-circuits,
/// tags are normalized (lowercase + trim) and deduplicated.
pub fn parse_capture_output(
    raw: &str,
) -> Result<CaptureSummaryOutput, mem_episodic::error::EpisodicError> {
    let value =
        mem_ai::provider::remote::parse_structured_lenient("mem-bridge", raw).map_err(|error| {
            mem_episodic::error::EpisodicError::Summary {
                message: format!("capture summary JSON parse failed: {error}"),
            }
        })?;
    parse_capture_value(value)
}

/// The value path: the same normalizers, over an already-parsed [`Value`].
///
/// This is the arm the structured completion feeds (ADR-0029): the provider
/// hands back a `Value`, so there is no string to decode and no `{text}` join.
/// Everything below is the unchanged capture contract — `skip` short-circuits
/// before the empty-summary check, an empty summary fails closed, tags are
/// lowercased and deduplicated, and the extension is sanitized.
pub fn parse_capture_value(
    value: mem_store::serde_json::Value,
) -> Result<CaptureSummaryOutput, mem_episodic::error::EpisodicError> {
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
        return Ok(CaptureSummaryOutput::Skip(SkipReason::ProviderSkipped));
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

// ---------------------------------------------------------------------------
// The production provider adapter (the engine composition root composes it)
// ---------------------------------------------------------------------------

/// The provider-backed capture summarizer: ONE schema-bound structured
/// completion over the bounded summary prompt, parsed by
/// [`parse_capture_value`] fail-closed.
///
/// It holds no `AiProvider` at all (ADR-0029): the streaming arm is not merely
/// unused, it is unreachable, so a schema-free capture path cannot be written
/// here. A `StructuredCapture` that answers `Unsupported` surfaces as a named
/// failure with no streaming fallback — reintroducing one would re-open the
/// exact defect (7 of 8 `complete` impls drop `request.json_schema`).
pub struct ProviderCaptureSummarizer {
    completion: Arc<dyn crate::capture_provider::StructuredCapture>,
    model: String,
    max_context_bytes: usize,
    /// The last provider failure; cleared on entry to every `summarize`.
    last_failure: Mutex<Option<SummarizerFailure>>,
}

impl ProviderCaptureSummarizer {
    pub fn new(
        completion: Arc<dyn crate::capture_provider::StructuredCapture>,
        model: impl Into<String>,
        max_context_bytes: usize,
    ) -> Self {
        Self {
            completion,
            model: model.into(),
            max_context_bytes,
            last_failure: Mutex::new(None),
        }
    }
}

/// Classify one provider failure onto the closed capture reason set.
///
/// Branch order is load-bearing: the closed wire token FIRST (it supplies both
/// the reason AND retryability), then Auth, then WireShape, then a
/// Config-wiring reason, then a model-output miss, then the default.
///
/// `is_retryable()` is consulted ONLY on the untagged default path: it returns
/// true for any 5xx `HttpStatus`, which would erase the difference between the
/// two 502 classes if a tagged failure used it.
pub fn classify_provider_error(error: &ProviderError) -> SummarizerFailure {
    if let Some(wire_reason) = error.wire_reason {
        let (reason, retryable) = match wire_reason {
            wire_reason::INVALID_REQUEST => ("host_invalid_request", false),
            wire_reason::UNAUTHORIZED => ("host_auth_rejected", false),
            wire_reason::PROVIDER_NOT_FOUND => ("host_provider_not_found", false),
            wire_reason::MODEL_NOT_FOUND => ("host_model_not_found", false),
            wire_reason::LOCATION_NOT_FOUND => ("host_location_not_found", false),
            wire_reason::MODEL_UNSUPPORTED => ("host_model_unsupported", false),
            wire_reason::PROVIDER_REQUEST_RETRYABLE => ("host_provider_request_error", true),
            wire_reason::PROVIDER_REQUEST_TERMINAL => ("host_provider_request_error", false),
            wire_reason::SERVICE_UNAVAILABLE => ("host_service_unavailable", true),
            wire_reason::ROUTE_MOVED => ("host_route_moved", false),
            // `wire_reason::ALL` is the closed set; a token outside it can only
            // come from a future decoder, so it takes the defensive class
            // rather than panicking inside the drain loop.
            _ => ("host_unrecognised_error", error.is_retryable()),
        };
        return SummarizerFailure {
            reason,
            error_kind: error.kind.as_str(),
            status: error.status,
            retryable,
            retry_after_ms: error.retry_after_ms,
            detail: error.detail.as_deref().map(str::to_owned),
        };
    }

    let (reason, retryable, detail) =
        if error.kind == ProviderErrorKind::Auth || matches!(error.status, Some(401) | Some(403)) {
            ("host_auth_rejected", true, None)
        } else if error.kind == ProviderErrorKind::WireShape {
            ("host_wire_shape_mismatch", true, None)
        } else if error.kind == ProviderErrorKind::Config {
            let wired = crate::host_endpoint::HostWiringError::all()
                .into_iter()
                .find(|candidate| candidate.reason() == error.message)
                .map(|candidate| candidate.reason());
            (wired.unwrap_or("summarizer_failed"), true, None)
        } else if error.kind == ProviderErrorKind::ModelOutput {
            (
                "capture_output_not_json",
                true,
                Some(format!(
                    "{}; no JSON of any of the three shapes",
                    error.detail.as_deref().unwrap_or("reply 0 bytes")
                )),
            )
        } else {
            ("summarizer_failed", error.is_retryable(), None)
        };
    SummarizerFailure {
        reason,
        error_kind: error.kind.as_str(),
        status: error.status,
        retryable,
        retry_after_ms: error.retry_after_ms,
        detail,
    }
}

impl CaptureSummarizer for ProviderCaptureSummarizer {
    fn summarize<'a>(
        &'a self,
        context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, mem_episodic::error::EpisodicError>> {
        Box::pin(async move {
            // Entry clear: a failure record belongs to ONE attempt only.
            *lock_or_recover(&self.last_failure) = None;
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
            let schema: mem_store::serde_json::Value =
                mem_store::serde_json::from_str(schema_json)?;
            request.json_schema = Some(schema.clone());
            match self.completion.complete_structured(request, schema).await {
                Ok(value) => parse_capture_value(value),
                Err(error) => {
                    *lock_or_recover(&self.last_failure) = Some(classify_provider_error(&error));
                    Err(mem_episodic::error::EpisodicError::Summary {
                        message: format!("capture summarizer provider failed: {error}"),
                    })
                }
            }
        })
    }

    fn last_failure(&self) -> Option<SummarizerFailure> {
        lock_or_recover(&self.last_failure).clone()
    }
}
