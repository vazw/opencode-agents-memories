//! Raw-context segmentation (W7.1).
//!
//! `episodic_segments` is DERIVED and regenerable: [`split_segments`] cuts the
//! raw text only at:
//!
//! 1. a blank-line (`\n\n`) boundary, when one is reachable inside the bound;
//! 2. otherwise the last `\n` inside the bound;
//! 3. otherwise the last UTF-8 `char` boundary inside the bound (an over-long
//!    paragraph).
//!
//! The invariants the tests pin:
//!
//! - `concat(segments.raw_content) == raw_context` byte for byte;
//! - `offset[i] + len[i] == offset[i+1]` (contiguous, covering offsets);
//! - `segment_index` is the 0-based position and reads are ordered by it;
//! - every segment is at most `max_bytes` — except the degenerate bound
//!   smaller than one character, where the cut widens to the next char
//!   boundary rather than emitting an empty, non-progressing segment.

/// The per-segment byte bound (16 KiB).
pub const MAX_SEGMENT_BYTES: usize = 16_384;

/// One derived segment of a raw context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSegment {
    /// 0-based position.
    pub segment_index: i64,
    /// The exact UTF-8 slice.
    pub raw_content: String,
    /// Byte offset into the raw context.
    pub byte_offset: i64,
    /// Byte length (equal to `raw_content.len()`).
    pub byte_len: i64,
}

/// Splits `raw` into segments of at most `max_bytes`, at the documented
/// boundaries.
pub fn split_segments(raw: &str, max_bytes: usize) -> Vec<RawSegment> {
    if raw.is_empty() {
        return Vec::new();
    }
    let max_bytes = max_bytes.max(1);
    let bytes = raw.as_bytes();
    let mut segments: Vec<RawSegment> = Vec::new();
    let mut start = 0usize;
    while start < bytes.len() {
        let remaining = bytes.len() - start;
        let cut = if remaining <= max_bytes {
            bytes.len()
        } else {
            cut_point(raw, start, start + max_bytes)
        };
        // Defensive progress guarantee: the cut always advances.
        let cut = cut.max(next_boundary_after(raw, start));
        let cut = cut.min(bytes.len());
        let text = &raw[start..cut];
        segments.push(RawSegment {
            segment_index: segments.len() as i64,
            raw_content: text.to_string(),
            byte_offset: start as i64,
            byte_len: text.len() as i64,
        });
        start = cut;
    }
    segments
}

/// The preferred cut position in `(start, window]`.
fn cut_point(raw: &str, start: usize, window: usize) -> usize {
    let hard = window.min(raw.len());
    // Search only inside a valid char boundary; a multi-byte character may
    // straddle the byte bound.
    let mut search_end = hard;
    while search_end > start && !raw.is_char_boundary(search_end) {
        search_end -= 1;
    }
    if search_end > start {
        let slice = &raw[start..search_end];
        // 1. The last blank-line separator that ends inside the window.
        if let Some(position) = slice.rfind("\n\n") {
            return start + position + 2;
        }
        // 2. The last newline inside the window.
        if let Some(position) = slice.rfind('\n') {
            return start + position + 1;
        }
        // 3. The last char boundary at or before the window.
        return search_end;
    }
    // The window is smaller than the next character: widen to its boundary.
    next_boundary_after(raw, start)
}

/// The first char boundary strictly after `start`.
fn next_boundary_after(raw: &str, start: usize) -> usize {
    let mut index = start + 1;
    while index < raw.len() && !raw.is_char_boundary(index) {
        index += 1;
    }
    index.min(raw.len())
}
