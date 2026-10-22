//! The five-tier memory model (ADR-0009) and the ADR-0010 D-3 compression
//! budget as executable code, plus the rule-based fact-tier classifier (Q3).
//!
//! # The tiers
//!
//! `Profile | Semantic | Procedural | Resource | Episodic | Vault`. The first
//! four are durable knowledge tiers; `Episodic` is the session tier; `Vault`
//! is the secret tier. The Vault crate itself is carved into W7.2 — this
//! module pins the model row, not the tier's storage.
//!
//! # The classifier (Q3)
//!
//! [`classify_fact_tier`] is **pure, total and deterministic**: no I/O, no
//! clock, no randomness, no score ties, no LLM, no config keywords. Fixed
//! precedence, first match wins:
//!
//! 1. **Resource** — an exact-artifact marker is present (a URL, an
//!    absolute/`~/` path token, a triple-backtick fence, or a backticked
//!    token);
//! 2. **Procedural** — the first word is an imperative verb, or a step
//!    marker (`1.`, `2.`, `step `) appears;
//! 3. **Semantic** — the declarative default.
//!
//! Artifact rules beat imperative rules on purpose: ``Run `cargo test` `` is
//! a pointer at an exact artifact, not a workflow recipe.

/// One memory tier (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryTier {
    /// User-profile preferences and patterns.
    Profile,
    /// Atomic declarative facts.
    Semantic,
    /// How-to / workflow facts.
    Procedural,
    /// Exact artifacts: paths, commands, URLs, code.
    Resource,
    /// Per-project agent session memory.
    Episodic,
    /// Encrypted secrets (the crate lands in W7.2).
    Vault,
}

impl MemoryTier {
    /// Every tier, in ADR order.
    pub const ALL: [MemoryTier; 6] = [
        MemoryTier::Profile,
        MemoryTier::Semantic,
        MemoryTier::Procedural,
        MemoryTier::Resource,
        MemoryTier::Episodic,
        MemoryTier::Vault,
    ];

    /// The canonical wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Semantic => "semantic",
            Self::Procedural => "procedural",
            Self::Resource => "resource",
            Self::Episodic => "episodic",
            Self::Vault => "vault",
        }
    }

    /// Exact, case-sensitive lookup of the wire string.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|tier| tier.as_str() == text)
    }

    /// Whether this tier lives in `facts.db` (the three fact tiers).
    pub fn is_fact_tier(self) -> bool {
        matches!(self, Self::Semantic | Self::Procedural | Self::Resource)
    }
}

/// The ADR-0010 D-3 compression budget level (`L0` = no compression).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompressionLevel {
    /// No compression (the vault keeps its bytes verbatim).
    L0,
    /// Light compression (atomic facts).
    L1,
    /// Medium compression (profile entries).
    L2,
    /// Reserved by the ADR table.
    L3,
    /// The strongest budget (episodic transcripts).
    L4,
}

impl CompressionLevel {
    /// The canonical wire string (`L0`..`L4`, uppercase).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L0 => "L0",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::L3 => "L3",
            Self::L4 => "L4",
        }
    }

    /// Exact, case-sensitive lookup (`l0` is rejected).
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "L0" => Some(Self::L0),
            "L1" => Some(Self::L1),
            "L2" => Some(Self::L2),
            "L3" => Some(Self::L3),
            "L4" => Some(Self::L4),
            _ => None,
        }
    }
}

/// The D-3 budget table, as code (not config — one source of truth).
pub fn compression_level_for(tier: MemoryTier) -> CompressionLevel {
    match tier {
        MemoryTier::Profile => CompressionLevel::L2,
        MemoryTier::Semantic | MemoryTier::Procedural | MemoryTier::Resource => {
            CompressionLevel::L1
        }
        MemoryTier::Episodic => CompressionLevel::L4,
        MemoryTier::Vault => CompressionLevel::L0,
    }
}

/// The three tiers an atomic fact can live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FactTier {
    /// A declarative fact.
    Semantic,
    /// A how-to / workflow fact.
    Procedural,
    /// A pointer at an exact artifact.
    Resource,
}

impl FactTier {
    /// The memory tier this fact tier lives in.
    pub fn as_tier(self) -> MemoryTier {
        match self {
            Self::Semantic => MemoryTier::Semantic,
            Self::Procedural => MemoryTier::Procedural,
            Self::Resource => MemoryTier::Resource,
        }
    }

    /// The canonical wire string (the `facts.tier` column value).
    pub fn as_str(self) -> &'static str {
        self.as_tier().as_str()
    }
}

/// The imperative verbs that open a procedural fact (fixed list, Q3).
pub const IMPERATIVE_VERBS: [&str; 17] = [
    "run",
    "use",
    "set",
    "install",
    "add",
    "remove",
    "always",
    "never",
    "first",
    "then",
    "must",
    "create",
    "configure",
    "enable",
    "disable",
    "prefer",
    "avoid",
];

/// True when `token` is an absolute or home-relative path (`/x`, `~/x`).
fn is_path_token(token: &str) -> bool {
    let candidate = token.trim_matches(|character: char| {
        matches!(
            character,
            '`' | '\'' | '"' | '(' | ')' | '[' | ']' | ',' | ';' | '.' | ':'
        )
    });
    if let Some(rest) = candidate.strip_prefix("~/") {
        return !rest.is_empty();
    }
    if let Some(rest) = candidate.strip_prefix('/') {
        // A lone slash is punctuation, not a path.
        return !rest.is_empty() && rest.chars().any(|character| character.is_alphanumeric());
    }
    false
}

/// True when the text carries an exact-artifact marker (rule 1).
///
/// A marker is a URL, a triple-backtick fence, a backticked token, an
/// absolute/`~/` path token, or a Rust `fn` signature head. The `fn` head is
/// part of the pinned row `fn split_segments(raw: &str)`: a bare function
/// signature is an exact artifact even without backticks.
fn has_artifact_marker(text: &str) -> bool {
    if text.contains("://") || text.contains("```") {
        return true;
    }
    if text
        .split_whitespace()
        .any(|token| token == "fn" || token.starts_with("fn("))
    {
        return true;
    }
    // A backticked token: a pair of backticks with content between them.
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        match after_open.find('`') {
            Some(close) if close > 0 => return true,
            Some(_) => {
                rest = &after_open[1..];
            }
            None => break,
        }
    }
    text.split_whitespace().any(is_path_token)
}

/// True when the text opens with an imperative verb (rule 2, first half).
fn opens_with_imperative(text: &str) -> bool {
    let Some(first) = text.split_whitespace().next() else {
        return false;
    };
    let word = first
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_ascii_lowercase();
    IMPERATIVE_VERBS.contains(&word.as_str())
}

/// True when a step marker appears anywhere (rule 2, second half).
fn has_step_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("step ") || lower.contains("1.") || lower.contains("2.")
}

/// Classifies a fact's text into its fact tier (Q3).
///
/// Pure, total, deterministic; fixed precedence. See the module docs.
pub fn classify_fact_tier(text: &str) -> FactTier {
    if has_artifact_marker(text) {
        return FactTier::Resource;
    }
    if opens_with_imperative(text) || has_step_marker(text) {
        return FactTier::Procedural;
    }
    FactTier::Semantic
}
