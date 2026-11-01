//! Incremental structured-output assembly.
//!
//! Structured completions arrive (or accumulate) as text that should be a JSON
//! document; models wrap it in markdown fences, and streamed chunks split it
//! arbitrarily. [`StructuredAccumulator`] buffers chunks and only parses at the
//! end, so partial chunks — including a chunk split mid-string or mid-escape —
//! never panic. A malformed final document surfaces as
//! [`ProviderErrorKind::Protocol`].
//!
//! Two entry points with different strictness, deliberately:
//!
//! - [`parse_structured`] / [`StructuredAccumulator::finish`] are **strict**.
//!   The remote provider adapters use them, and every one of those routes
//!   enforces `json_schema` server-side, so a reply that is not a bare JSON
//!   document is a genuine protocol error there. Leniency on those routes is
//!   unmeasured and would mask a real contract break.
//! - [`parse_structured_lenient`] is the **three-shape recovery** for routes
//!   that cannot enforce a schema (the host's stateless generate route). It
//!   mirrors upstream `legacy-client.ts`'s `parseJson`: direct JSON, then the
//!   first fenced block, then every balanced top-level object keeping the last
//!   that parses. A miss is [`ProviderErrorKind::ModelOutput`] — a named,
//!   retryable outcome rather than an anonymous protocol failure.

use serde_json::Value;

use crate::provider::error::{ProviderError, ProviderErrorKind};

/// Buffers structured-output text across chunks and parses it once complete.
#[derive(Debug, Default, Clone)]
pub struct StructuredAccumulator {
    buffer: String,
}

impl StructuredAccumulator {
    /// An empty accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one chunk (may be an arbitrary substring of the document).
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    /// The raw buffered text.
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Whether nothing has been buffered yet.
    pub fn is_empty(&self) -> bool {
        self.buffer.trim().is_empty()
    }

    /// Try to parse the current buffer as a complete JSON document.
    ///
    /// Returns `None` while the document is still incomplete (or malformed so
    /// far) — callers keep pushing. Never panics.
    pub fn try_parse(&self) -> Option<Value> {
        let cleaned = strip_code_fences(&self.buffer);
        serde_json::from_str::<Value>(cleaned.trim()).ok()
    }

    /// Finish assembly, parsing the buffer or reporting a protocol error.
    pub fn finish(self, provider: &str) -> Result<Value, ProviderError> {
        let cleaned = strip_code_fences(&self.buffer).trim().to_owned();
        if cleaned.is_empty() {
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "structured output was empty",
            )
            .with_provider(provider));
        }
        serde_json::from_str::<Value>(&cleaned).map_err(|error| {
            ProviderError::new(
                ProviderErrorKind::Protocol,
                format!("structured output was not valid JSON: {error}"),
            )
            .with_provider(provider)
        })
    }
}

/// Assemble a single text blob into a JSON value, strict (see the module doc).
pub fn parse_structured(provider: &str, text: &str) -> Result<Value, ProviderError> {
    let mut accumulator = StructuredAccumulator::new();
    accumulator.push(text);
    accumulator.finish(provider)
}

/// Recover a JSON value from a reply that may be wrapped in prose or fences.
///
/// Three shapes, tried in upstream's order (`example/opencode-mem/src/v2/
/// legacy-client.ts:23-83`): the whole text as JSON, then the first fenced
/// block, then every balanced top-level `{…}` span (string/escape aware),
/// keeping the **last** one that parses — models often emit a stub or an
/// example before the final answer. A miss is a [`ProviderErrorKind::ModelOutput`]
/// naming all three shapes; the message carries only the reply's byte length.
pub fn parse_structured_lenient(provider: &str, text: &str) -> Result<Value, ProviderError> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }
    let fenced = first_fenced_block(trimmed);
    if let Some(inner) = fenced
        && let Ok(value) = serde_json::from_str::<Value>(inner)
    {
        return Ok(value);
    }
    if let Some(value) = last_balanced_object(fenced.unwrap_or(trimmed)) {
        return Ok(value);
    }
    Err(ProviderError::new(
        ProviderErrorKind::ModelOutput,
        format!(
            "structured output matched none of: direct JSON, fenced block, balanced object (reply {} bytes)",
            text.len()
        ),
    )
    .with_provider(provider))
}

/// The first ```/```json fenced block's inner text, when the reply has one.
fn first_fenced_block(text: &str) -> Option<&str> {
    let open = text.find("```")?;
    let after_open = &text[open + 3..];
    let body_start = after_open.find('\n')? + 1;
    let body = &after_open[body_start..];
    let close = body.find("```")?;
    Some(body[..close].trim())
}

/// Every balanced top-level `{…}` span's last parseable value.
///
/// Braces inside JSON strings and escaped quotes are ignored. The search
/// offset always advances past a closing brace (or stops at an unterminated
/// object), so termination is guaranteed.
fn last_balanced_object(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let mut last = None;
    let mut search_from = 0usize;
    while search_from < bytes.len() {
        let start = match text[search_from..].find('{') {
            Some(index) => search_from + index,
            None => break,
        };
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (cursor, byte) in bytes.iter().enumerate().skip(start) {
            let byte = *byte;
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(cursor);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        if let Ok(value) = serde_json::from_str::<Value>(&text[start..=end]) {
            last = Some(value);
        }
        search_from = end + 1;
    }
    last
}

/// Strip a single leading ```/```json fence and its trailing ``` fence.
///
/// Only the outermost fence is removed; text without fences is returned
/// unchanged (borrowed when no trimming is needed).
pub fn strip_code_fences(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop the optional language tag on the opening fence line.
    let rest = match rest.find('\n') {
        Some(index) => &rest[index + 1..],
        None => return trimmed,
    };
    match rest.trim_end().strip_suffix("```") {
        Some(inner) => inner.trim(),
        None => rest.trim_end(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::error::ProviderErrorKind;

    #[test]
    fn assembles_across_partial_chunks_without_panicking() {
        let mut accumulator = StructuredAccumulator::new();
        accumulator.push("{\"topic\":");
        accumulator.push("\"auth\",\"co");
        assert!(accumulator.try_parse().is_none(), "incomplete JSON");
        accumulator.push("unt\":3}");
        let value = accumulator.finish("p").expect("parses once complete");
        assert_eq!(value["topic"], "auth");
        assert_eq!(value["count"], 3);
    }

    #[test]
    fn strips_markdown_fences() {
        assert_eq!(strip_code_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fences("```\n[1,2]\n```"), "[1,2]");
        assert_eq!(strip_code_fences("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn malformed_final_is_a_protocol_error() {
        let error = parse_structured("p", "{not json").expect_err("malformed");
        assert_eq!(error.kind, ProviderErrorKind::Protocol);
        let error = parse_structured("p", "   ").expect_err("empty");
        assert_eq!(error.kind, ProviderErrorKind::Protocol);
    }

    #[test]
    fn split_mid_multibyte_char_does_not_panic() {
        let mut accumulator = StructuredAccumulator::new();
        accumulator.push("{\"t\":\"caf");
        accumulator.push("é\"}");
        let value = accumulator.finish("p").expect("parses");
        assert_eq!(value["t"], "café");
    }

    /// The measured 308-byte markdown reply that the 2.2.5 host returned to a
    /// capture-sized prompt (`.tmp/scratch/probe-stream.out`). Byte count is
    /// asserted so the fixture cannot silently drift.
    const MEASURED_MARKDOWN_REPLY: &str = "## Request\nBound the unbounded inject barrier to 2 seconds and add logging for timeout events.\n\n## Outcome\nImplemented `INJECT_BARRIER_BUDGET` and an `AtomicBool` latch in `engine.rs` to enforce the 2-second timeout and log when the barrier is exceeded.\n\n**Tags**: `rust`, `concurrency`, `timeout`, `logging`";

    #[test]
    fn test_lenient_parse_takes_direct_json() {
        assert_eq!(
            MEASURED_MARKDOWN_REPLY.len(),
            308,
            "fixture is the measured reply"
        );
        let value = parse_structured_lenient("p", "{\"a\":1}").expect("direct JSON parses");
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn test_lenient_parse_recovers_a_fenced_block() {
        let value = parse_structured_lenient("p", "here you go:\n```json\n{\"a\":1}\n```\n")
            .expect("fenced block parses");
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn test_lenient_parse_prefers_the_last_balanced_object() {
        let value = parse_structured_lenient("p", "x {\"a\":1} y {\"summary\":\"real\"}")
            .expect("a balanced object parses");
        assert_eq!(value["summary"], "real", "the LAST parseable object wins");
    }

    #[test]
    fn test_lenient_parse_miss_names_all_three_shapes() {
        let error = parse_structured_lenient("p", MEASURED_MARKDOWN_REPLY)
            .expect_err("prose is not structured output");
        assert_eq!(error.kind, ProviderErrorKind::ModelOutput);
        assert!(error.message.contains("direct JSON"), "{}", error.message);
        assert!(error.message.contains("fenced block"), "{}", error.message);
        assert!(
            error.message.contains("balanced object"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("reply 308 bytes"),
            "{}",
            error.message
        );
        assert!(
            !error.message.contains("INJECT_BARRIER_BUDGET"),
            "the reply text must never be echoed: {}",
            error.message
        );
    }
}
