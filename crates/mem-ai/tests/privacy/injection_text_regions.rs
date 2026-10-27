//! Issue 52 — the injected-prompt filter's *text* entry points.
//!
//! The capture path receives one flattened string, so the part-based
//! `InjectionFilter::is_injected`/`filter` pair is not reachable there. These
//! tests pin the text rule (`filter_text`) and its "is this text nothing but
//! injected content?" predicate (`is_fully_injected`) over the SAME marker
//! list — no second filter module and no second marker list (52.1).

use std::collections::BTreeMap;

use mem_ai::privacy::injection::DEFAULT_INTERNAL_PROMPT_MARKERS;
use mem_ai::privacy::{InjectionFilter, PrivacyConfig};

const BLOCK: &str = "<system-reminder>\n[BACKGROUND TASK RESULT READY]\n</system-reminder>";

fn filter() -> InjectionFilter {
    InjectionFilter::with_defaults()
}

// ---------------------------------------------------------------------------
// Test 1 — a multiline region in a mixed text
// ---------------------------------------------------------------------------

#[test]
fn test_filter_text_drops_a_multiline_marker_region_and_keeps_the_authored_line() {
    // Wrong implementations this kills, and the bytes that differ:
    // - whole-text drop      -> `""` instead of `"fix the failing test\n"`;
    // - a no-op filter       -> the input verbatim;
    // - drop-marker-line-only -> `[BACKGROUND TASK RESULT READY]\n` survives.
    let mixed = format!("fix the failing test\n{BLOCK}");
    let filtered = filter().filter_text(&mixed);
    assert_eq!(filtered, "fix the failing test\n");
    assert!(!filter().is_fully_injected(&mixed));
}

// ---------------------------------------------------------------------------
// Test 2 — an entirely injected text
// ---------------------------------------------------------------------------

#[test]
fn test_filter_text_drops_an_entirely_injected_text_and_is_fully_injected_reports_it() {
    // A line-only rule (drops marker lines, keeps body lines) leaves
    // `"[BACKGROUND TASK RESULT READY]\n"` behind.
    let filter = filter();
    assert_eq!(filter.filter_text(BLOCK), "");
    assert!(filter.is_fully_injected(BLOCK));
}

// ---------------------------------------------------------------------------
// Test 2b — the predicate is whitespace-insensitive
// ---------------------------------------------------------------------------

#[test]
fn test_is_fully_injected_is_whitespace_insensitive() {
    // The wrong implementation this row exists to kill is
    // `filter_text(text).is_empty()`, not a missing clause. It answers
    // `false` here because the residue of the leading/trailing-blank fixture
    // is `"\n"` — non-empty — so the final clause's `trim()` is the
    // load-bearing call. (The two falsifiers an earlier revision named —
    // "without the marker clause" on `"   "`, "without the `!text.trim()`
    // clause" on `"\n\n"` — are unreachable: clause 1 already makes `"   "`
    // false, and no blocklist marker makes `"\n\n"` false. Kept as a comment
    // so nobody weakens the fixture trying to reach them.)
    let filter = filter();
    let block = format!("\n{BLOCK}\n");
    // `split_inclusive('\n')` yields four segments; the region is index 1
    // through 3 and index 0 is the only survivor.
    assert_eq!(filter.filter_text(&block), "\n");
    assert!(filter.is_fully_injected(&block));
    assert!(!filter.is_fully_injected(""));
    assert!(!filter.is_fully_injected("   "));
    assert!(!filter.is_fully_injected("\n\n"));
}

// ---------------------------------------------------------------------------
// Test 3 — the allowlist exemption (D6 precedence)
// ---------------------------------------------------------------------------

#[test]
fn test_filter_text_exempts_an_internal_prompt_that_carries_a_marker() {
    // Without the `is_allowed` check the marker region is dropped and the
    // output is `"# User Profile Analysis\n"`; the whole text must survive
    // verbatim instead, matching `is_injected`'s precedence.
    let text = "# User Profile Analysis\n<system-reminder>\nboiler\n</system-reminder>";
    assert_eq!(filter().filter_text(text), text);
}

// ---------------------------------------------------------------------------
// Test 4 — clean text is byte-identical (pure `#[test]`, nothing to wire)
// ---------------------------------------------------------------------------

#[test]
fn test_filter_text_leaves_clean_text_byte_identical() {
    // A trimming/reflowing filter changes the whitespace-rich fixture; a
    // blank-treated-as-injected definition answers `true` for `""`.
    let filter = filter();
    for clean in ["plain words, no tags", "", "  leading\n\n  words  \n"] {
        assert_eq!(filter.filter_text(clean), clean);
        assert!(!filter.is_fully_injected(clean));
    }
}

// ---------------------------------------------------------------------------
// Test 5 — a non-tag marker drops one line, not the tail
// ---------------------------------------------------------------------------

#[test]
fn test_filter_text_drops_a_comment_marker_line_without_swallowing_its_neighbours() {
    // Run-to-EOF for a non-tag marker -> `"before\n"` (drops `after` too).
    let text = "before\n<!-- OMO_INTERNAL_INITIATOR -->\nafter";
    assert_eq!(filter().filter_text(text), "before\nafter");
}

// ---------------------------------------------------------------------------
// Test 5d — case-insensitive opener AND closer
// ---------------------------------------------------------------------------

#[test]
fn test_filter_text_matches_a_case_variant_opener_and_closer() {
    // A case-sensitive closer search fails closed to EOF -> `"fix it\n"`.
    let text = "fix it\n<System-Reminder>\nboiler\n</System-Reminder>\nkeep this line";
    assert_eq!(filter().filter_text(text), "fix it\nkeep this line");
}

// ---------------------------------------------------------------------------
// Test 5e — a self-closing marker line is a complete region
// ---------------------------------------------------------------------------

#[test]
fn test_a_self_closing_marker_line_is_dropped_alone_and_does_not_swallow_the_tail() {
    // `<team_mode_status` is one of the eight defaults and is written without
    // `>`; without the self-closing branch the fail-closed tail swallow
    // yields `"keep\n"` — the byte-identical outcome of the rejected
    // whole-text drop.
    let text = "keep\n<team_mode_status mode=\"team\" count=\"2\"/>\nafter";
    assert_eq!(filter().filter_text(text), "keep\nafter");
    assert!(!filter().is_fully_injected(text));
}

// ---------------------------------------------------------------------------
// Test 5f — a genuine unterminated opener swallows the tail (pinned)
// ---------------------------------------------------------------------------

#[test]
fn test_an_unterminated_opening_tag_swallows_the_tail() {
    // "Drop the opener line only" would leak the body as `"keep\nboiler"`.
    let text = "keep\n<system-reminder>\nboiler";
    assert_eq!(filter().filter_text(text), "keep\n");
}

// ---------------------------------------------------------------------------
// Test 6 — `PrivacyConfig::from_core` reads both resolved keys
// ---------------------------------------------------------------------------

#[test]
fn test_from_core_reads_both_chat_message_keys_and_keeps_the_internal_prompt_allowlist() {
    // Wrong implementations this kills:
    // - `PrivacyConfig::default()` (ignores both keys) -> flag `true` and a
    //   blocklist without `<my-banner>`;
    // - assigning the `&[&str]` const directly -> does not compile (the field
    //   is `Vec<String>`).
    let home = std::env::temp_dir().join(format!(
        "mem-ai-privacy-from-core-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&home).expect("create home");
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.to_string_lossy().to_string())]
        .into_iter()
        .collect();
    let mut config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(&home), &env)
            .expect("the default config loads");
    config.chat_message.filter_injected_prompts = false;
    config
        .chat_message
        .injection_markers
        .push("<my-banner>".to_string());

    let privacy = PrivacyConfig::from_core(&config);

    assert!(!privacy.filter_injected_prompts);
    assert_eq!(privacy.blocklist, config.chat_message.injection_markers);
    assert_eq!(
        privacy.allowlist,
        DEFAULT_INTERNAL_PROMPT_MARKERS
            .iter()
            .map(|marker| (*marker).to_owned())
            .collect::<Vec<String>>()
    );
    assert!(privacy.strip_private);

    let _ = std::fs::remove_dir_all(&home);
}
