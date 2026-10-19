//! Port of upstream `tests/privacy-malformed-tags.test.ts` plus the
//! tag-whitespace parity boundaries (AC2, D4).
//!
//! `strip_private_content` is the last thing between a `<private>` block and
//! persistent storage, so malformed markup must fail *closed*: the caller
//! writes the result to a database, where under-redacting is an unrecoverable
//! disclosure while over-redacting only loses text the user can retype.

#[path = "common/mod.rs"]
mod common;

use mem_ai::privacy::tags::{REDACTED, is_fully_private, strip_private_content};

const SECRET: &str = "sk-live-abc123";

#[test]
fn test_unclosed_tag_redacts_to_end_of_input() {
    // A pair-matching scan finds no `</private>` and leaves the region
    // untouched, so a typo or a truncated message stores the secret verbatim.
    let result = strip_private_content(&format!("my key is <private>{SECRET}"));

    assert!(!result.contains(SECRET));
    assert_eq!(result, "my key is [REDACTED]");
}

#[test]
fn test_unclosed_tag_is_fully_private() {
    // The write path refuses fully-private content; a pair matcher answers
    // `false` here, so the message would be accepted *and* stored unredacted.
    assert!(is_fully_private(&format!("<private>{SECRET}")));
}

#[test]
fn test_truncated_message_region_is_redacted() {
    let result = strip_private_content(&format!("intro <private>{SECRET} and more text"));
    assert!(!result.contains(SECRET));
    assert!(!result.contains("and more text"));
}

#[test]
fn test_nested_region_closes_at_outer_tag() {
    // Pair matching stops at the inner `</private>`, releasing the remainder of
    // the outer region and leaving a stray `</private>` in the stored text.
    let result = strip_private_content(&format!(
        "<private>outer <private>inner</private> {SECRET}</private>"
    ));

    assert!(!result.contains(SECRET));
    assert!(!result.contains("inner"));
    assert!(!result.contains("private>"));
    assert_eq!(result, REDACTED);
}

#[test]
fn test_whitespace_inside_tag_is_recognized() {
    // `<private >` is the same tag; it previously matched nothing and passed
    // through verbatim.
    let result = strip_private_content(&format!("<private >{SECRET}</private >"));

    assert!(!result.contains(SECRET));
    assert_eq!(result, REDACTED);
}

#[test]
fn test_stray_closing_tag_is_dropped() {
    // With no opener there is nothing to redact, but the markup is not the
    // user's text either, so it should not be stored.
    assert_eq!(
        strip_private_content("visible </private> text"),
        "visible  text"
    );
}

#[test]
fn test_mixed_case_tags_work_including_unclosed() {
    assert_eq!(
        strip_private_content(&format!("<PRIVATE>{SECRET}</private>")),
        REDACTED
    );
    assert_eq!(
        strip_private_content(&format!("<Private>{SECRET}")),
        REDACTED
    );
}

#[test]
fn test_well_formed_regions_are_redacted_independently() {
    // The scanner must not merge separate regions; asserted here alongside the
    // malformed cases.
    let result = strip_private_content("a <private>one</private> b <private>two</private> c");
    assert_eq!(result, "a [REDACTED] b [REDACTED] c");
}

#[test]
fn test_no_tags_leaves_angle_brackets_untouched() {
    assert_eq!(
        strip_private_content("use Array<string> and a < b"),
        "use Array<string> and a < b"
    );
    assert_eq!(strip_private_content(""), "");
    assert_eq!(
        strip_private_content("no markup at all"),
        "no markup at all"
    );
}

#[test]
fn test_similarly_named_tag_is_not_redaction() {
    // Only `private` is the redaction tag; `<privateer>` is ordinary text.
    let input = "<privateer>ship</privateer>";
    assert_eq!(strip_private_content(input), input);
}

#[test]
fn test_repeated_calls_do_not_leak_state() {
    // There is no `lastIndex` analogue: the scan keeps a local cursor only, so
    // a second call must not start mid-string.
    let input = format!("<private>{SECRET}</private>");
    assert_eq!(strip_private_content(&input), REDACTED);
    assert_eq!(strip_private_content(&input), REDACTED);
    assert_eq!(strip_private_content(&input), REDACTED);
}

#[test]
fn test_unclosed_region_after_closed_one_is_redacted() {
    let result = strip_private_content(&format!(
        "<private>first</private> middle <private>{SECRET}"
    ));
    assert!(!result.contains(SECRET));
    assert_eq!(result, "[REDACTED] middle [REDACTED]");
}

// ---------------------------------------------------------------------------
// Upstream parity-boundary guards: tag grammar edges that must stay literal.
// ---------------------------------------------------------------------------

#[test]
fn test_self_closing_private_tag_is_literal() {
    // `<private/>` is not `<private\s*>` (the `/` is not whitespace), so the
    // self-closing form is ordinary text; the stray `</private>` is dropped.
    assert_eq!(
        strip_private_content(&format!("<private/>{SECRET}</private>")),
        format!("<private/>{SECRET}")
    );
}

#[test]
fn test_private_tag_with_attributes_is_literal() {
    // `mode="strict"` is not whitespace-then-`>`, so the tag is not recognized.
    assert_eq!(
        strip_private_content(&format!("<private mode=\"strict\">{SECRET}</private>")),
        format!("<private mode=\"strict\">{SECRET}")
    );
}

#[test]
fn test_tab_and_newline_whitespace_in_tag_is_recognized() {
    let result = strip_private_content(&format!("<private\t>{SECRET}</private\n>"));
    assert!(!result.contains(SECRET));
    assert_eq!(result, REDACTED);
}

// ---------------------------------------------------------------------------
// Tag-whitespace parity boundary (D4) — a leak boundary, not a style choice.
// ---------------------------------------------------------------------------

#[test]
fn test_zero_width_no_break_space_inside_tag_is_recognized() {
    // U+FEFF is `\s` in JS but NOT `White_Space` in Rust, so `char::is_whitespace`
    // alone would leave this region unredacted — a leak in exactly the
    // direction this wave forbids. `is_tag_whitespace` recognizes it explicitly.
    let input = format!("<private\u{FEFF}>{SECRET}</private>");
    let result = strip_private_content(&input);

    assert!(!result.contains(SECRET));
    assert_eq!(result, REDACTED);
}

#[test]
fn test_nel_inside_tag_is_recognized_divergence_from_js() {
    // U+0085 (NEL) is `White_Space` in Rust but not `\s` in JS: upstream leaves
    // `<private\u{0085}>secret</private\u{0085}>` verbatim, the port redacts it.
    // Accepted and recorded (D4) because it errs *toward* redaction
    // (fail-closed); the alternative would fail open.
    let input = format!("<private\u{0085}>{SECRET}</private\u{0085}>");
    let result = strip_private_content(&input);

    assert!(!result.contains(SECRET));
    assert_eq!(result, REDACTED);
}
