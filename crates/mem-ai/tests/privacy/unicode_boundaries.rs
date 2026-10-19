//! Multi-byte slice safety for the hand-written byte scanner (AC2).
//!
//! These are the only tests that exercise the scanner at `char` boundaries —
//! the malformed-tag cases are all ASCII. Every case asserts the **exact**
//! output string, so a mid-codepoint slice fails either as a panic or as a
//! mangled (lossy-replaced) string, with 2-, 3- and 4-byte characters before,
//! after and inside a private region and adjacent to a truncated tag.

#[path = "common/mod.rs"]
mod common;

use mem_ai::privacy::tags::{REDACTED, is_fully_private, strip_private_content};

#[test]
fn test_multibyte_text_before_and_after_private_region_is_preserved() {
    // 4-byte 🎌 before, 2-byte é inside {} no — `café` before, 3-byte ☃ after.
    let input = "🎌 café <private>s</private> ☃";
    assert_eq!(strip_private_content(input), "🎌 café [REDACTED] ☃");
}

#[test]
fn test_multibyte_text_inside_private_region_is_redacted_whole() {
    let input = "<private>日本語 secret</private>tail";
    let out = strip_private_content(input);

    assert!(!out.contains("日本語"));
    assert_eq!(out, format!("{REDACTED}tail"));
}

#[test]
fn test_multibyte_text_in_unclosed_region_is_fully_redacted() {
    let input = "<private>秘密";
    let out = strip_private_content(input);

    assert!(!out.contains("秘密"));
    assert_eq!(out, REDACTED);
    assert!(is_fully_private(input));
}

#[test]
fn test_multibyte_text_adjacent_to_truncated_tag_is_literal() {
    // `é` follows `privat`, so the tag never completes; nothing changes.
    let input = "a<privaté";
    assert_eq!(strip_private_content(input), input);
}

#[test]
fn test_multibyte_text_around_tag_like_non_tag_is_preserved() {
    let input = "日本 <private-ish> 語 <private>x</private>";
    assert_eq!(
        strip_private_content(input),
        format!("日本 <private-ish> 語 {REDACTED}")
    );
}
