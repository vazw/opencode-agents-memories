//! Port of upstream `src/utils/context-limit.ts` (v2.31.3 @ 906038e).
//!
//! Trims text to a UTF-8 byte budget, preferring to keep both the head and
//! the tail so summaries retain opening context and closing conclusions.
//! Feeds W5.3's context injection (roadmap R9): the branch order and the
//! default marker literal are the frozen contract — do not reorder branches
//! or "improve" the split.

/// Byte-exact upstream default marker literal (leading and trailing
/// newlines included).
pub const DEFAULT_TRUNCATION_MARKER: &str = "\n[... truncated ...]\n";

/// UTF-8 byte length of a string (NOT char count — the budget math breaks
/// for multi-byte text if this ever becomes a char count).
pub fn utf8_byte_length(text: &str) -> usize {
    text.len()
}

/// Decode a UTF-8 byte slice without splitting multi-byte characters.
///
/// Upstream JS quirk translation: the `bytes.get(to)` guard stands in for JS
/// reading `bytes[to]`, which is `undefined` at end-of-input (and
/// `undefined & 0xc0` is falsy), so the loop must stop at end-of-input — the
/// `from` side never needs a guard (`from < to <= bytes.len()`).
pub fn slice_utf8_bytes(text: &str, start: usize, end: Option<usize>) -> String {
    let bytes = text.as_bytes();
    let mut from = start.min(bytes.len());
    let mut to = end.unwrap_or(bytes.len()).min(bytes.len());
    if to <= from {
        return String::new();
    }

    // If `from` lands inside a multi-byte character, advance to the next lead byte.
    while from < to && (bytes[from] & 0xC0) == 0x80 {
        from += 1;
    }

    // If `to` lands inside a multi-byte character, back up to that character's start.
    while to > from && bytes.get(to).is_some_and(|b| (b & 0xC0) == 0x80) {
        to -= 1;
    }

    // text.get(from..to) with the existing empty-slice fallback: total for non-boundary ranges (ADR-0013 D-3)
    let Some(slice) = text.get(from..to) else {
        return String::new();
    };
    slice.to_string()
}

/// Truncate text to at most `max_bytes` UTF-8 bytes, using the default
/// marker when head+tail splitting is needed.
pub fn truncate_to_max_bytes(text: &str, max_bytes: usize) -> String {
    truncate_to_max_bytes_with_marker(text, max_bytes, DEFAULT_TRUNCATION_MARKER)
}

/// Truncate text to at most `max_bytes` UTF-8 bytes.
///
/// Branch order is the upstream contract:
/// 1. `max == 0` (upstream `<= 0`; `usize` cannot be negative) → empty.
/// 2. Under or exactly at budget → text unchanged.
/// 3. `max <= marker_bytes` → head slice only (NO marker, NO tail).
/// 4. Otherwise → floor(available/2)-byte head + marker + remaining tail.
pub fn truncate_to_max_bytes_with_marker(text: &str, max_bytes: usize, marker: &str) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let marker_bytes = marker.len();
    if max_bytes <= marker_bytes {
        return slice_utf8_bytes(text, 0, Some(max_bytes));
    }

    let available = max_bytes - marker_bytes;
    let head_bytes = available / 2;
    let tail_bytes = available - head_bytes;
    let total = text.len();

    let mut out = slice_utf8_bytes(text, 0, Some(head_bytes));
    out.push_str(marker);
    out.push_str(&slice_utf8_bytes(text, total - tail_bytes, None));
    out
}
