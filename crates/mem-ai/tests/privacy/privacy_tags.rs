//! Port of upstream `tests/privacy.test.ts` (AC1).
//!
//! Seam: `mem_ai::privacy::{strip_private_content, is_fully_private, REDACTED}`.

#[path = "common/mod.rs"]
mod common;

use mem_ai::privacy::tags::{REDACTED, is_fully_private, strip_private_content};

const SECRET: &str = "sk-live-abc123";

#[test]
fn test_strip_replaces_private_tag_with_redacted() {
    let input = "Hello <private>secret data</private> world";
    assert_eq!(
        strip_private_content(input),
        format!("Hello {REDACTED} world")
    );
}

#[test]
fn test_strip_handles_multiple_tags() {
    let input = "<private>first</private> and <private>second</private>";
    assert_eq!(
        strip_private_content(input),
        format!("{REDACTED} and {REDACTED}")
    );
}

#[test]
fn test_strip_is_case_insensitive() {
    let input = "<PRIVATE>secret</PRIVATE>";
    assert_eq!(strip_private_content(input), REDACTED);
}

#[test]
fn test_strip_handles_multiline_content() {
    let input = "<private>\nline1\nline2\n</private>";
    assert_eq!(strip_private_content(input), REDACTED);
}

#[test]
fn test_strip_returns_unchanged_without_tags() {
    let input = "no private content here";
    assert_eq!(strip_private_content(input), input);
}

#[test]
fn test_strip_handles_empty_string() {
    assert_eq!(strip_private_content(""), "");
}

#[test]
fn test_strip_handles_adjacent_tags() {
    let input = "<private>a</private><private>b</private>";
    assert_eq!(
        strip_private_content(input),
        format!("{REDACTED}{REDACTED}")
    );
}

#[test]
fn test_is_fully_private_true_for_only_private_tag() {
    assert!(is_fully_private(&format!("<private>{SECRET}</private>")));
}

#[test]
fn test_is_fully_private_true_for_empty_string() {
    assert!(is_fully_private(""));
}

#[test]
fn test_is_fully_private_true_for_whitespace_around_private_tag() {
    assert!(is_fully_private(&format!(
        "  <private>{SECRET}</private>  "
    )));
}

#[test]
fn test_is_fully_private_false_when_visible_content_exists() {
    assert!(!is_fully_private(&format!(
        "visible <private>{SECRET}</private>"
    )));
}

#[test]
fn test_is_fully_private_false_for_plain_text() {
    assert!(!is_fully_private("just normal text"));
}
