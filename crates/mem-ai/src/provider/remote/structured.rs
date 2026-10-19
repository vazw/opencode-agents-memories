//! Incremental structured-output assembly.
//!
//! Structured completions arrive (or accumulate) as text that should be a JSON
//! document; models wrap it in markdown fences, and streamed chunks split it
//! arbitrarily. [`StructuredAccumulator`] buffers chunks and only parses at the
//! end, so partial chunks — including a chunk split mid-string or mid-escape —
//! never panic. A malformed final document surfaces as
//! [`ProviderErrorKind::Protocol`].

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

/// Assemble a single text blob into a JSON value.
pub fn parse_structured(provider: &str, text: &str) -> Result<Value, ProviderError> {
    let mut accumulator = StructuredAccumulator::new();
    accumulator.push(text);
    accumulator.finish(provider)
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
}
