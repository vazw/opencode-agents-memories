//! The `<private>` tag stripper (upstream `src/services/privacy.ts`).
//!
//! [`strip_private_content`] replaces every `<private>…</private>` region with
//! [`REDACTED`] so a secret the user wrapped in the tag never reaches
//! persistence or an embedding/LLM request.
//!
//! # Tag grammar
//!
//! `<` + optional `/` + ASCII-case-insensitive `private` + zero-or-more tag
//! whitespace characters + `>`. The tag-whitespace predicate is
//! [`is_tag_whitespace`] — `char::is_whitespace()` **or** U+FEFF (see D4 in the
//! module docs of [`crate::privacy`]: JS `\s` matches U+FEFF and Rust's
//! `White_Space` does not, so a bare `char::is_whitespace` would leak
//! `<private\u{FEFF}>secret</private>` verbatim).
//!
//! # Invariants
//!
//! - The scan is a single left-to-right pass that slices only on `char`
//!   boundaries (never mid-codepoint).
//! - At every point the output holds only text emitted from regions already
//!   proven non-private.
//! - Over-redaction is the only safe error direction: any input the scanner
//!   cannot classify is redacted rather than surfaced.
//! - `cursor` and the scan index strictly advance by at least one byte per
//!   iteration, so the loop terminates.

/// Replacement marker for a redacted `<private>` region.
pub const REDACTED: &str = "[REDACTED]";

/// Tag whitespace: `char::is_whitespace()` plus U+FEFF.
///
/// D4 — U+FEFF (ZWNBSP) is `\s` in JS but not `White_Space` in Rust; dropping
/// it here would leave `<private\u{FEFF}>secret</private\u{FEFF}>` unredacted.
/// U+0085 (NEL) is the reverse case and is accepted deliberately: it errs
/// toward redaction (fail-closed).
pub fn is_tag_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

/// Matches a `<private>` / `</private>` tag at `start`.
///
/// Returns `(is_closing, index_after_the_gt)`, or `None` when `content[start..]`
/// does not begin with the tag. `start` must be a `char` boundary.
fn match_private_tag(content: &str, start: usize) -> Option<(bool, usize)> {
    const NAME: &[u8] = b"private";

    let bytes = content.as_bytes();
    if bytes.get(start) != Some(&b'<') {
        return None;
    }

    let mut index = start + 1;
    let is_closing = if bytes.get(index) == Some(&b'/') {
        index += 1;
        true
    } else {
        false
    };

    for expected in NAME {
        match bytes.get(index) {
            Some(actual) if actual.eq_ignore_ascii_case(expected) => index += 1,
            _ => return None,
        }
    }

    // Zero or more tag-whitespace characters, then `>`.
    loop {
        match bytes.get(index) {
            Some(b'>') => return Some((is_closing, index + 1)),
            Some(_) => {
                let c = content[index..].chars().next()?;
                if !is_tag_whitespace(c) {
                    return None;
                }
                index += c.len_utf8();
            }
            None => return None,
        }
    }
}

/// Replaces every `<private>…</private>` region with [`REDACTED`].
///
/// Tags are scanned with a depth counter rather than matched in pairs, so the
/// two malformed shapes fail *closed*:
///
/// - An **unclosed** `<private>` redacts to the end of the input — a paired
///   scan found no closing tag and left the region verbatim, so a typo or a
///   truncated message stored the content.
/// - **Nested** tags close at the *outer* tag, not the first inner one — pair
///   matching released the rest of the outer region and left a stray
///   `</private>` in the output.
///
/// A `</private>` with no opener is markup rather than content, so it is
/// dropped instead of being surfaced as if it were part of the user's text.
pub fn strip_private_content(content: &str) -> String {
    if !content.contains('<') {
        return content.to_owned();
    }

    let mut result = String::new();
    let mut depth = 0usize;
    let mut cursor = 0usize;
    let mut index = 0usize;

    while index < content.len() {
        let Some(offset) = content[index..].find('<') else {
            break;
        };
        let tag_start = index + offset;

        let Some((is_closing, tag_end)) = match_private_tag(content, tag_start) else {
            // Not a tag: keep scanning just past this `<` (1 byte = 1 char
            // boundary, since `<` is ASCII).
            index = tag_start + 1;
            continue;
        };

        if !is_closing {
            if depth == 0 {
                result.push_str(&content[cursor..tag_start]);
            }
            depth += 1;
            cursor = tag_end;
            index = tag_end;
            continue;
        }

        if depth == 0 {
            // Stray closing tag: emit the text before it, drop the tag itself.
            result.push_str(&content[cursor..tag_start]);
            cursor = tag_end;
            index = tag_end;
            continue;
        }

        depth -= 1;
        if depth == 0 {
            result.push_str(REDACTED);
        }
        cursor = tag_end;
        index = tag_end;
    }

    if depth > 0 {
        // Unclosed region: everything from the opening tag onward stays private.
        result.push_str(REDACTED);
    } else {
        result.push_str(&content[cursor..]);
    }

    result
}

/// True when nothing survives redaction.
///
/// The caller refuses such a write rather than storing a message that is
/// private in its entirety.
pub fn is_fully_private(content: &str) -> bool {
    let stripped = strip_private_content(content);
    let trimmed = stripped.trim();
    trimmed == REDACTED || trimmed.is_empty()
}
