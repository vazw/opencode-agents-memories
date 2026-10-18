//! Port of upstream `example/opencode-mem/tests/context-limit.test.ts`
//! (v2.31.3 @ 906038e), test-for-test: every upstream `it` case is traced,
//! plus justified boundary extras pinning the branch order and byte
//! adjustments that roadmap R9 (W5.3 context injection) depends on.
//!
//! Upstream mirror cases:
//! - "counts UTF-8 bytes, not JS string length" → `test_utf8_byte_length_counts_bytes_not_chars`
//! - "returns text unchanged when under the budget" → `test_truncate_returns_text_unchanged_when_under_budget`
//! - "keeps head and tail when truncating" → `test_truncate_keeps_head_and_tail_with_marker`
//! - "slices multi-byte characters on UTF-8 boundaries" → `test_slice_utf8_bytes_never_splits_multibyte_chars`

use mem_core::utils::context_limit::{
    DEFAULT_TRUNCATION_MARKER, slice_utf8_bytes, truncate_to_max_bytes,
    truncate_to_max_bytes_with_marker, utf8_byte_length,
};

/// Upstream "counts UTF-8 bytes, not JS string length" plus boundary extras.
#[test]
fn test_utf8_byte_length_counts_bytes_not_chars() {
    assert_eq!(utf8_byte_length("a"), 1);
    assert_eq!(utf8_byte_length("ä"), 2);
    assert_eq!(utf8_byte_length("你好"), 6);
    assert_eq!(utf8_byte_length(""), 0);
    assert_eq!(utf8_byte_length("🙂"), 4);
}

/// Upstream "returns text unchanged when under the budget".
#[test]
fn test_truncate_returns_text_unchanged_when_under_budget() {
    assert_eq!(truncate_to_max_bytes("hello world", 100), "hello world");
    // Exact-budget boundary: still unchanged.
    assert_eq!(truncate_to_max_bytes("abc", 3), "abc");
}

/// Upstream "keeps head and tail when truncating".
#[test]
fn test_truncate_keeps_head_and_tail_with_marker() {
    let text = "AAAA".to_string() + &"x".repeat(200) + "BBBB";
    let truncated = truncate_to_max_bytes_with_marker(&text, 40, "|TRUNC|");
    assert!(utf8_byte_length(&truncated) <= 40);
    assert!(truncated.starts_with("AAAA"));
    assert!(truncated.contains("|TRUNC|"));
    assert!(truncated.ends_with("BBBB"));
}

/// Upstream "slices multi-byte characters on UTF-8 boundaries", strengthened
/// with the exact expected result `"ä"` (byte length ≤ 3).
#[test]
fn test_slice_utf8_bytes_never_splits_multibyte_chars() {
    let text = "ä".repeat(10);
    let sliced = slice_utf8_bytes(&text, 0, Some(3));
    assert_eq!(sliced, "ä");
    assert!(utf8_byte_length(&sliced) <= 3);
    assert!(!sliced.contains('\u{FFFD}'));
}

/// Advance `from` off a continuation byte (upstream branch translation).
#[test]
fn test_slice_utf8_bytes_advances_start_off_continuation_byte() {
    // "aäb" = [0x61, 0xC3, 0xA4, 0x62]; start 2 is the continuation byte of "ä".
    assert_eq!(slice_utf8_bytes("aäb", 2, Some(4)), "b");
}

/// Back `end` off a continuation byte (upstream branch translation).
#[test]
fn test_slice_utf8_bytes_backs_end_off_continuation_byte() {
    assert_eq!(slice_utf8_bytes("aäb", 1, Some(3)), "ä");
}

/// Clamping and crossed/empty ranges all yield `""`.
#[test]
fn test_slice_utf8_bytes_returns_empty_for_empty_or_crossed_ranges() {
    assert_eq!(slice_utf8_bytes("ä", 0, Some(1)), "");
    assert_eq!(slice_utf8_bytes("abc", 2, Some(2)), "");
    assert_eq!(slice_utf8_bytes("abc", 3, Some(1)), "");
    assert_eq!(slice_utf8_bytes("abc", 5, None), "");
}

/// `end` beyond input length clamps to the input length (JS `undefined &
/// 0xc0` end-of-input quirk translation).
#[test]
fn test_slice_utf8_bytes_clamps_end_beyond_length() {
    assert_eq!(slice_utf8_bytes("abc", 0, Some(99)), "abc");
}

/// Budget at or below marker size → head slice only (no marker, no tail).
#[test]
fn test_truncate_with_budget_at_or_below_marker_returns_head_slice() {
    let text = "a".repeat(30);
    // Default marker "\n[... truncated ...]\n" is 21 bytes.
    assert_eq!(utf8_byte_length(DEFAULT_TRUNCATION_MARKER), 21);
    assert_eq!(truncate_to_max_bytes(&text, 19), "a".repeat(19));
    assert_eq!(truncate_to_max_bytes(&text, 5), "a".repeat(5));
    assert_eq!(truncate_to_max_bytes(&text, 0), "");
}

/// Two-arg form uses the byte-exact default marker.
#[test]
fn test_truncate_uses_default_marker_when_two_arg_form() {
    let text = "H".repeat(100) + &"T".repeat(100);
    let truncated = truncate_to_max_bytes(&text, 60);
    assert!(truncated.contains("\n[... truncated ...]\n"));
    assert!(utf8_byte_length(&truncated) <= 60);
    assert!(truncated.starts_with('H'));
    assert!(truncated.ends_with('T'));
}

/// A tail slice that lands inside a multi-byte char drops the partial bytes.
#[test]
fn test_truncate_tail_slice_drops_partial_multibyte_bytes() {
    // "ä".repeat(5) is 10 bytes; marker "|T|" is 3; max 4 → available 1 →
    // tail slice starts inside a 2-byte char and collapses to "".
    let text = "ä".repeat(5);
    assert_eq!(truncate_to_max_bytes_with_marker(&text, 4, "|T|"), "|T|");
}

/// Byte-loop guard: the result never exceeds the budget for any step.
#[test]
fn test_truncate_result_never_exceeds_budget() {
    let text = "AAAA".to_string() + &"ä".repeat(50) + "BBBB";
    for max_bytes in 1..=64usize {
        let truncated = truncate_to_max_bytes(&text, max_bytes);
        assert!(
            utf8_byte_length(&truncated) <= max_bytes,
            "budget {max_bytes} exceeded by truncation of length {}",
            utf8_byte_length(&truncated)
        );
    }
}
