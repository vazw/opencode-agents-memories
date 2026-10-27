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
/// `type="skip"`). Issue 52's capture seam imports this predicate rather than
/// porting it a second time.
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

    /// Injected regions removed from a flattened text (no part metadata).
    ///
    /// The capture path receives one string (the wrapper has already joined
    /// the parts), so the part-shaped [`Self::is_injected`]/[`Self::filter`]
    /// pair cannot reach it. This is the same rule over the same blocklist and
    /// allowlist — a second module would be the drift issue 52 forbids.
    ///
    /// # Rule (mechanical, total, terminating)
    ///
    /// 1. Empty or allowlisted text is returned verbatim (the allowlist
    ///    exempts the **whole** text, matching [`Self::is_injected`]'s
    ///    precedence). This is the only path on which a marker survives.
    /// 2. The text is split with [`str::split_inclusive`], and one lowercased
    ///    copy per line is used for **both** the opener match and the closer
    ///    search, so opener and closer can never disagree about case.
    /// 3. Each blocklist marker is classified once: a **tag marker** starts
    ///    with `<` and not `</`/`<!` (its name is the run of
    ///    `[A-Za-z0-9_-]` immediately after `<`, lowercased); everything else
    ///    is a **non-tag marker**.
    /// 4. For every line and every marker occurring on it, the marker's region
    ///    end is computed: a non-tag marker drops its own line; a tag marker
    ///    drops its own line when the closer `</{name}` is on it, when the
    ///    first `>` at or after the end of the name is preceded by `/`
    ///    (**self-closing**), and otherwise drops through the first later line
    ///    containing `</{name}` — or to the last line when no closer exists
    ///    (**fail-closed tail swallow**). A line's region end is the maximum
    ///    over its markers.
    /// 5. The marked lines are concatenated back byte-identically (no reflow,
    ///    no trimming, no blank-line collapsing).
    ///
    /// # Boundaries a reviewer must not have to rediscover
    ///
    /// - **Self-closing is a separate case.** `<team_mode_status` is one of
    ///   the eight defaults and is written *without* `>`; without this branch
    ///   an attribute-bearing, self-closing default marker
    ///   (`<team_mode_status mode="team"/>`) would fall into the fail-closed
    ///   branch and drop to EOF. A tag that closes itself on its own line is a
    ///   complete region, so it drops that line only.
    /// - **The tail swallow stays.** A genuine unterminated opener
    ///   (`<system-reminder>` with no closer) must not leak its body.
    ///   Over-removal is the safe direction, the same as `<private>`.
    /// - **Filtering precedes truncation.** `build_markdown_context`
    ///   truncates to `max_context_bytes`; a region cut in half no longer has
    ///   a closer, so a marker can survive truncation. The capture seam
    ///   therefore filters the prompt before the builder.
    ///
    /// # Invariant
    ///
    /// *For every text that is neither empty nor allowlisted — that is, every
    /// text that reaches the region loop — the output contains no blocklist
    /// marker on any line: every marker-bearing line lies inside some computed
    /// region and is dropped by construction.* The allowlisted path returns
    /// the text verbatim by design (D6 precedence), so do **not** assert this
    /// invariant over an allowlisted text and do **not** write it as a
    /// `debug_assert!` over this method's output.
    ///
    /// # Cost
    ///
    /// Two finite nested loops (lines × markers) plus one marking pass; the
    /// worst case is `O(lines²)` when many openers tail-swallow. Inputs are
    /// already byte-capped (`max_context_bytes`, `MAX_RAW_CONTEXT_BYTES`), so
    /// the bound is real and accepted rather than optimised.
    pub fn filter_text(&self, text: &str) -> String {
        if text.is_empty() || self.is_allowed(text) {
            return text.to_owned();
        }
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let lower: Vec<String> = lines.iter().map(|line| line.to_ascii_lowercase()).collect();
        let mut dropped = vec![false; lines.len()];
        for i in 0..lines.len() {
            let mut region_end = i;
            let mut matched = false;
            for marker in &self.blocklist {
                if marker.is_empty() {
                    continue;
                }
                let marker = marker.to_ascii_lowercase();
                if !lower[i].contains(&marker) {
                    continue;
                }
                matched = true;
                region_end = region_end.max(marker_region_end(&marker, &lower, i));
            }
            if matched {
                for drop in dropped.iter_mut().take(region_end + 1).skip(i) {
                    *drop = true;
                }
            }
        }
        let mut out = String::with_capacity(text.len());
        for (line, drop) in lines.iter().zip(dropped) {
            if !drop {
                out.push_str(line);
            }
        }
        out
    }

    /// `text` is non-blank, carries a blocklist marker, is not allowlisted,
    /// and nothing survives [`Self::filter_text`] after trimming.
    ///
    /// Each clause is load-bearing:
    ///
    /// - `!text.trim().is_empty()` — `""` and a marker-free whitespace-only
    ///   text are not "fully injected"; without the `trim()` the
    ///   leading/trailing-blank case gets through.
    /// - `!self.is_allowed(text)` — mirrors [`Self::is_injected`]'s
    ///   precedence and keeps the allowlisted path out of the predicate.
    /// - `contains_injection_marker(..)` — "fully injected" requires something
    ///   to have been removed.
    /// - `filter_text(text).trim().is_empty()` on the **filtered** value — the
    ///   residue of an injected block is often a single newline, so
    ///   `filter_text(text).is_empty()` is a no-op here and must not be used.
    ///
    /// It is **not** `filter_text(text).is_empty()`: the filtered residue of a
    /// block surrounded by newlines is non-empty after a no-trim comparison.
    ///
    /// The input must be **raw** text: run this predicate on an already
    /// filtered value and the first clause answers `false`, silently
    /// disabling the skip it exists to drive.
    pub fn is_fully_injected(&self, text: &str) -> bool {
        !text.trim().is_empty()
            && !self.is_allowed(text)
            && contains_injection_marker(text, &self.blocklist)
            && self.filter_text(text).trim().is_empty()
    }
}

/// The last line of `marker`'s region on line `i`.
///
/// `marker` is the lowercased blocklist entry and `lower` the lowercased
/// lines; the caller has already established that `lower[i]` contains it.
fn marker_region_end(marker: &str, lower: &[String], i: usize) -> usize {
    let is_tag = marker.starts_with('<') && !marker.starts_with("</") && !marker.starts_with("<!");
    if !is_tag {
        return i;
    }
    let name: String = marker[1..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect();
    let closer = format!("</{name}");
    if lower[i].contains(&closer) {
        return i;
    }
    let line = &lower[i];
    let name_end = line
        .find(marker)
        .map(|start| start + 1 + name.len())
        .unwrap_or_default();
    if let Some(offset) = line[name_end..].find('>') {
        let greater = name_end + offset;
        if greater > 0 && line.as_bytes()[greater - 1] == b'/' {
            return i;
        }
    }
    for (j, later) in lower.iter().enumerate().skip(i + 1) {
        if later.contains(&closer) {
            return j;
        }
    }
    lower.len() - 1
}
