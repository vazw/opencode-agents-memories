//! Injected-prompt filter (upstream `src/services/injected-prompt-filter.ts`).
//!
//! OpenCode surfaces plugin-injected content through the same `chat.message`
//! hook as genuine user input. Without this filter every injected block is
//! concatenated into `user_prompts.content`, which then feeds both auto-capture
//! and user-profile learning — memories and a profile describing another
//! plugin's orchestration boilerplate instead of the user's actual work.
//!
//! Two layers are needed because injected content arrives in two shapes:
//!
//! 1. Whole messages that consist only of injected text (background-task
//!    notifications, continuation directives, orchestration status blocks).
//! 2. Mixed messages where an injected block is prepended to the real prompt.
//!
//! Structural detection ([`InjectionFilterable::part_is_synthetic`]) is checked
//! first but is not sufficient on its own: plugins commonly inject through
//! message paths that do not set the flag, so textual markers carry most of the
//! weight.
//!
//! The marker list and its normalizer are **not** re-ported here: they already
//! live in `mem-core` (W0.2) and are re-exported so the two cannot drift.

use crate::provider::request::ChatMessage;

pub use mem_core::config::{DEFAULT_INJECTION_MARKERS, normalize_injection_markers};

/// Returns true when `text` contains any configured injection marker.
///
/// Matching is case-insensitive so host variations in tag casing (for example
/// `<System-Reminder>`) are still recognized. Empty markers are skipped.
pub fn contains_injection_marker(text: &str, markers: &[String]) -> bool {
    if text.is_empty() {
        return false;
    }
    let haystack = text.to_ascii_lowercase();
    markers.iter().any(|marker| {
        if marker.is_empty() {
            return false;
        }
        haystack.contains(&marker.to_ascii_lowercase())
    })
}

/// [`contains_injection_marker`] against [`DEFAULT_INJECTION_MARKERS`].
pub fn contains_default_injection_marker(text: &str) -> bool {
    contains_injection_marker(text, &default_markers())
}

fn default_markers() -> Vec<String> {
    DEFAULT_INJECTION_MARKERS
        .iter()
        .map(|marker| (*marker).to_owned())
        .collect()
}

/// Minimal shape this module needs from a message part (upstream
/// `InjectionFilterablePart`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessagePart {
    /// Upstream `type` (e.g. `"text"`).
    pub kind: Option<String>,
    /// Upstream `text`.
    pub text: Option<String>,
    /// Upstream `synthetic` — set by the host for machine-generated parts.
    pub synthetic: bool,
}

/// The text + provenance a filterable part exposes.
pub trait InjectionFilterable {
    /// The part's text (`""` when it has none).
    fn part_text(&self) -> &str;

    /// Whether the host flagged the part as machine-generated.
    fn part_is_synthetic(&self) -> bool;
}

impl InjectionFilterable for MessagePart {
    fn part_text(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }

    fn part_is_synthetic(&self) -> bool {
        self.synthetic
    }
}

impl InjectionFilterable for ChatMessage {
    fn part_text(&self) -> &str {
        &self.content
    }

    fn part_is_synthetic(&self) -> bool {
        false
    }
}

/// Returns true when a part was injected rather than typed by the user.
///
/// A part counts as injected when it is flagged `synthetic` by the host, or
/// when its text carries a known injection marker.
pub fn is_injected_part(part: &impl InjectionFilterable, markers: &[String]) -> bool {
    part.part_is_synthetic() || contains_injection_marker(part.part_text(), markers)
}

/// Drops injected parts and returns the user-authored ones, order preserved.
///
/// When every part is injected the result is empty, signalling the caller to
/// skip recording the message entirely.
pub fn filter_injected_parts<T: InjectionFilterable + Clone>(
    parts: &[T],
    markers: &[String],
) -> Vec<T> {
    parts
        .iter()
        .filter(|part| !is_injected_part(*part, markers))
        .cloned()
        .collect()
}

/// Markers that identify one of the plugin's *own* prompts, exempt from the
/// injection blocklist (D6 — upstream `isStructuredSummaryPromptMessage`).
pub const DEFAULT_INTERNAL_PROMPT_MARKERS: &[&str] =
    &["# User Profile Analysis", "Analyze this conversation."];

/// True when `text` is one of the plugin's own internal prompts (D6).
///
/// Faithful port of upstream `index.ts:41`: the user-profile analysis prompt,
/// or the structured-summary prompt pair (the instruction *and*
/// `type="skip"`). Phase 5's `plugin-host-config` wave must import this
/// predicate rather than port it a second time.
pub fn is_internal_prompt(text: &str) -> bool {
    text.contains("# User Profile Analysis")
        || (text.contains("Analyze this conversation.") && text.contains("type=\"skip\""))
}

/// The allowlist-aware filter: an injected part is dropped unless its text is
/// an internal prompt or matches the allowlist.
#[derive(Debug, Clone, Default)]
pub struct InjectionFilter {
    blocklist: Vec<String>,
    allowlist: Vec<String>,
}

impl InjectionFilter {
    /// The default filter: [`DEFAULT_INJECTION_MARKERS`] against
    /// [`DEFAULT_INTERNAL_PROMPT_MARKERS`].
    pub fn with_defaults() -> Self {
        Self {
            blocklist: default_markers(),
            allowlist: DEFAULT_INTERNAL_PROMPT_MARKERS
                .iter()
                .map(|marker| (*marker).to_owned())
                .collect(),
        }
    }

    /// An explicit blocklist/allowlist pair.
    pub fn new(blocklist: Vec<String>, allowlist: Vec<String>) -> Self {
        Self {
            blocklist,
            allowlist,
        }
    }

    /// The configured blocklist.
    pub fn blocklist(&self) -> &[String] {
        &self.blocklist
    }

    /// The configured allowlist.
    pub fn allowlist(&self) -> &[String] {
        &self.allowlist
    }

    /// True when `text` is exempt from the injection blocklist: an internal
    /// prompt, or a case-insensitive allowlist substring match.
    pub fn is_allowed(&self, text: &str) -> bool {
        if is_internal_prompt(text) {
            return true;
        }
        let haystack = text.to_ascii_lowercase();
        self.allowlist
            .iter()
            .filter(|marker| !marker.is_empty())
            .any(|marker| haystack.contains(&marker.to_ascii_lowercase()))
    }

    /// True when `part` is injected and not allowlisted.
    pub fn is_injected(&self, part: &impl InjectionFilterable) -> bool {
        if self.is_allowed(part.part_text()) {
            return false;
        }
        part.part_is_synthetic() || contains_injection_marker(part.part_text(), &self.blocklist)
    }

    /// Drop injected parts, preserving order.
    pub fn filter<T: InjectionFilterable + Clone>(&self, parts: &[T]) -> Vec<T> {
        parts
            .iter()
            .filter(|part| !self.is_injected(*part))
            .cloned()
            .collect()
    }
}
