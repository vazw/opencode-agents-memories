//! W7.1 — segment splitting: the derived representation must partition the
//! raw bytes exactly, with contiguous offsets and no segment over the bound.

use mem_episodic::segments::{MAX_SEGMENT_BYTES, RawSegment, split_segments};

fn assert_partitions(raw: &str, segments: &[RawSegment], max_bytes: usize) {
    let mut offset = 0i64;
    let mut rebuilt = String::new();
    for (index, segment) in segments.iter().enumerate() {
        assert_eq!(
            segment.segment_index, index as i64,
            "segment_index is the 0-based position"
        );
        assert_eq!(segment.byte_offset, offset, "contiguous offsets");
        assert!(segment.byte_len >= 0);
        assert!(
            segment.byte_len as usize <= max_bytes,
            "segment {index} is {} bytes, bound {max_bytes}",
            segment.byte_len
        );
        assert_eq!(
            segment.byte_len as usize,
            segment.raw_content.len(),
            "byte_len is the exact UTF-8 length"
        );
        assert!(raw.is_char_boundary(segment.byte_offset as usize));
        assert!(raw.is_char_boundary((segment.byte_offset + segment.byte_len) as usize));
        rebuilt.push_str(&segment.raw_content);
        offset += segment.byte_len;
    }
    assert_eq!(offset as usize, raw.len(), "offsets cover the input");
    assert_eq!(rebuilt, raw, "concatenation is byte-equal to the input");
}

#[test]
fn test_split_segments_partitions_the_input_byte_for_byte() {
    let raw = "first paragraph
with two lines

second paragraph

third paragraph
with a trailing line";
    let segments = split_segments(raw, 40);
    assert!(!segments.is_empty());
    assert_partitions(raw, &segments, 40);
    // A small bound still splits; every boundary lands after a blank line
    // when one is reachable.
    assert!(segments.len() > 1);

    // A bound larger than the input is one segment.
    let whole = split_segments(raw, MAX_SEGMENT_BYTES);
    assert_eq!(whole.len(), 1);
    assert_eq!(whole[0].raw_content, raw);
    assert_eq!(whole[0].byte_offset, 0);

    // No trailing newline: still partitions.
    let no_newline = "a".repeat(100);
    let segments = split_segments(&no_newline, 30);
    assert_partitions(&no_newline, &segments, 30);

    // Blank-line separators are preserved verbatim.
    let with_blank = "one

two";
    let segments = split_segments(with_blank, 4);
    assert_partitions(with_blank, &segments, 4);
}

#[test]
fn test_split_segments_cuts_over_long_paragraphs_at_a_char_boundary() {
    // Three times the segment bound, no newline, multi-byte UTF-8.
    let paragraph = "é🦀ab".repeat(MAX_SEGMENT_BYTES);
    let raw = paragraph.as_str();
    assert!(raw.len() >= 3 * MAX_SEGMENT_BYTES);
    let segments = split_segments(raw, MAX_SEGMENT_BYTES);
    assert!(segments.len() >= 3, "{} segments", segments.len());
    assert_partitions(raw, &segments, MAX_SEGMENT_BYTES);
    for segment in &segments {
        // A char-boundary cut never yields a replacement character.
        assert!(!segment.raw_content.contains('\u{FFFD}'));
    }

    // A bound smaller than one character is widened to one char boundary
    // rather than emitting an empty (non-progressing) segment.
    let small = split_segments("éé", 1);
    assert_eq!(small.len(), 2);
    assert_eq!(small[0].raw_content, "é");
    assert_eq!(small[1].raw_content, "é");
}

#[test]
fn test_split_segments_of_empty_input_is_empty() {
    assert!(split_segments("", MAX_SEGMENT_BYTES).is_empty());
    assert!(split_segments("", 16).is_empty());
}
