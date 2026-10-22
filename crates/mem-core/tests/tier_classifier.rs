//! W7.1 — the rule-based fact-tier classifier (Q3): pure, total, fixed
//! precedence, no score ties, no LLM, no config keywords, no I/O.

use mem_core::tier::{FactTier, classify_fact_tier};

/// The 20 pinned rows (Q3). Rows 1–6 are RESOURCE (an exact-artifact marker
/// beats every imperative rule — row 4 proves it), 7–12 PROCEDURAL, 13–20
/// SEMANTIC (the declarative default).
#[test]
fn test_fact_tier_classifier_table() {
    let table: [(&str, FactTier); 20] = [
        (
            "The build uses `F32_BLOB(768)` for embeddings.",
            FactTier::Resource,
        ),
        (
            "Config lives at ~/.config/opencode/opencode-mem.jsonc",
            FactTier::Resource,
        ),
        (
            "See https://example.com/adr/0010 for the invariant",
            FactTier::Resource,
        ),
        (
            "Run `cargo test -p mem-core` before merging",
            FactTier::Resource,
        ),
        ("fn split_segments(raw: &str)", FactTier::Resource),
        (
            "The vault key is at `{storage_root}/vault-keys` ",
            FactTier::Resource,
        ),
        (
            "Always run the migration tests before merging",
            FactTier::Procedural,
        ),
        (
            "Install the ONNX runtime before the first build",
            FactTier::Procedural,
        ),
        (
            "Never regenerate an existing encryption key",
            FactTier::Procedural,
        ),
        (
            "First create the ledger; then run the suite",
            FactTier::Procedural,
        ),
        (
            "Configure the promotion TTL to seven days",
            FactTier::Procedural,
        ),
        ("Step 2 creates the retention tables", FactTier::Procedural),
        ("The vault key is per project scope", FactTier::Semantic),
        (
            "Raw context is the authoritative representation",
            FactTier::Semantic,
        ),
        (
            "We chose option C because the blast radius is smaller",
            FactTier::Semantic,
        ),
        (
            "Turso is a SQLite-compatible database engine",
            FactTier::Semantic,
        ),
        ("Promotion is idempotent across runs", FactTier::Semantic),
        ("The default TTL is seven days", FactTier::Semantic),
        ("Segments are derived data", FactTier::Semantic),
        (
            "Reviewer agents may read scoped episodic rows",
            FactTier::Semantic,
        ),
    ];
    for (index, (text, expected)) in table.iter().enumerate() {
        assert_eq!(
            classify_fact_tier(text),
            *expected,
            "row {} ({text:?})",
            index + 1
        );
    }
}

#[test]
fn test_fact_tier_classifier_is_deterministic() {
    let samples = [
        "Run `cargo test` first",
        "Always check the lock",
        "Turso keeps the invariant",
        "",
        "   ",
        "1. do the first thing",
        "2. then the second",
    ];
    for text in samples {
        let first = classify_fact_tier(text);
        for _ in 0..50 {
            assert_eq!(classify_fact_tier(text), first, "{text:?} is total");
        }
    }
    // Empty input is declarative: Semantic, never a panic.
    assert_eq!(classify_fact_tier(""), FactTier::Semantic);
    assert_eq!(classify_fact_tier("   "), FactTier::Semantic);
}
