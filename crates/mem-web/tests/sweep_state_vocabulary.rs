//! Issue 142 — the sweep-state vocabulary the HTTP DTO declares and the ADR that publishes it are one
//! contract. `legacyPeerLive` is retired (2026-11-18): the sweep's block record is report-only and the
//! stamp table stores successes only, so no answer could ever carry the value.
//!
//! The declared set is prose in both artifacts — no `const` exists, and adding one for this test would
//! create a declared set the emit literals (`prompts.rs:99,101,111,114,115,116`) ignore. So the test reads
//! what a client author and an ADR reader actually see, at compile time: an absent or moved path is a
//! compile error, which is the loud failure.
//!
//! Shape contract enforced here: exactly ONE backticked `` `sweep_state: a | b | …` `` statement per file, on
//! one line. The marker carries the leading backtick on purpose: `dto.rs:360` and `prompts.rs:79` declare
//! `sweep_state: &'static str` fields and must not be read as vocabulary.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeSet;

const DTO_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/api/dto.rs"));
const ADR_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/adr/0019-erasure-posture.md"
));

/// Every backticked `` `sweep_state: a | b | c` `` statement in `text`, as label sets. The span ends at the
/// closing backtick or the end of the line, so a wrapped list cannot swallow neighbouring prose; tokens that
/// are not bare camelCase ASCII identifiers are dropped.
fn vocab_sets(text: &str) -> Vec<BTreeSet<String>> {
    const MARK: &str = "`sweep_state: ";
    let mut sets = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(MARK) {
        let tail = &rest[at + MARK.len()..];
        let end = tail.find(['`', '\n']).unwrap_or(tail.len());
        let set: BTreeSet<String> = tail[..end]
            .split('|')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .filter(|t| t.starts_with(|c: char| c.is_ascii_lowercase()))
            .filter(|t| t.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(str::to_owned)
            .collect();
        sets.push(set);
        rest = &rest[at + MARK.len()..];
    }
    sets
}

fn expected() -> BTreeSet<String> {
    ["swept", "notYetSwept", "sweepFailed", "noSweepForTier"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn test_the_dto_declares_no_legacy_peer_live() {
    assert!(
        !DTO_SRC.contains("legacyPeerLive"),
        "the retired label must not reappear in dto.rs; history lives in ADR-0019 (issue 142)"
    );
}

#[test]
fn test_the_dto_and_the_adr_state_the_same_four_labels() {
    let dto = vocab_sets(DTO_SRC);
    let adr = vocab_sets(ADR_SRC);
    assert_eq!(
        dto.len(),
        1,
        "dto.rs must carry exactly one backticked `sweep_state: ` statement, on one line: {dto:?}"
    );
    assert_eq!(
        adr.len(),
        1,
        "ADR-0019 D-19-3 must carry exactly one backticked `sweep_state: ` statement, on one line: {adr:?}"
    );
    assert_eq!(
        dto[0],
        expected(),
        "the DTO's declared set is the four emitted labels"
    );
    assert_eq!(
        adr[0],
        expected(),
        "ADR-0019 D-19-3 publishes the same four labels the DTO declares (AC3: ADR, DTO, test agree)"
    );
}

#[test]
fn test_the_retired_label_is_recorded_as_a_historical_note() {
    let at = ADR_SRC.find("Amended (issue 142").expect(
        "ADR-0019 must record the retirement as an `Amended (issue 142` note, not a silent deletion",
    );
    let tail = &ADR_SRC[at..];
    let end = tail.find("\n\n").unwrap_or(tail.len());
    assert!(
        tail[..end].contains("legacyPeerLive"),
        "the amendment must name the retired label so a reader learns it was never reachable"
    );
}
