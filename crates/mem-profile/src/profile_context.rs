//! Port of upstream `src/services/user-profile/profile-context.ts`: the
//! `<user_profile>` block the injector prepends ahead of the episodic history
//! block.
//!
//! Upstream reference (READ-ONLY behavioural contract):
//! `example/opencode-mem/src/services/user-profile/profile-context.ts` (the
//! four sections, `dedupByCategory`, `scoreByRecency`, `escapeContextText`)
//! and `example/opencode-mem/src/utils/profile.ts` (`isFrozen`,
//! `sortProfileItems`). The two ranking helpers live here because that is where
//! upstream defines them — `profile_utils.rs` ports `profile-utils.ts`
//! (embedding stripping), a different file.
//!
//! Invariants this module keeps:
//! - **Pure and clock-injected:** the bytes are a function of
//!   `(ProfileData, now_millis, ProfileRenderCaps)` only; no wall clock, no
//!   store, no I/O.
//! - **One escaping funnel:** [`escape_profile_text`] is the only path from a
//!   profile-derived string into the block; it sanitizes (newlines collapse,
//!   trim) BEFORE it escapes.
//! - **Total order:** every ranked section is totally ordered, so identical
//!   input yields identical bytes; learning paths stay input-ordered, as
//!   upstream.
//!
//! Recorded divergences from upstream (all deliberate, all pinned):
//! - **D-44-1** (Q6): a `0` item cap renders no items. Upstream's
//!   `dedupByCategory` pushes one item before it breaks, so a `0` cap meant
//!   "1" — a bug, not a contract.
//! - **D-44-2** (Q7): `\n`/`\r` are collapsed and trimmed inside
//!   [`escape_profile_text`]; upstream leaks raw newlines, so a description can
//!   forge a section header.
//! - **D-44-3** (Q8): no profile payload is ever logged here (upstream logs
//!   80-char snippets), and the `profile inject` counter carries no data.
//! - **D-44-4** (Q10): an item with a missing/null `description` fails the
//!   stored-row parse, so the whole row is [`StoredProfileRender::Unreadable`]
//!   where upstream renders `- [] `. Fail-soft is the contract; the DTO is
//!   not patched (no schema change).

use std::collections::HashSet;

use mem_core::config::Config;
use serde_json::Value as JsonValue;

use crate::decay::MILLIS_PER_DAY;
use crate::profile_store::{ProfileData, ProfileItem};

/// The block's opening tag (upstream injects the text between the tags).
pub const PROFILE_BLOCK_OPEN: &str = "<user_profile>";

/// The block's closing tag.
pub const PROFILE_BLOCK_CLOSE: &str = "</user_profile>";

/// The section headers, in the frozen emission order (upstream
/// `profile-context.ts:80,87,96,106`).
const HEADING_PREFERENCES: &str = "User Preferences:";
const HEADING_PATTERNS: &str = "User Patterns:";
const HEADING_WORKFLOWS: &str = "User Workflows:";
const HEADING_LEARNING_PATHS: &str = "Learning Paths:";

/// Upstream `profile-context.ts:107` — `learning_paths.slice(0, 3)`, with no
/// sort and no config knob.
const LEARNING_PATHS_TAKE: usize = 3;

/// The three item caps (upstream `userProfileInjectPreferences` /
/// `userProfileInjectPatterns` / `userProfileInjectWorkflows`).
///
/// `maxProfileItems` is deliberately NOT a cap here: it is parsed for upstream
/// compatibility and has no consumer (44.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileRenderCaps {
    pub preferences: usize,
    pub patterns: usize,
    pub workflows: usize,
}

/// The injector-side policy: whether the tier runs at all, plus the caps the
/// renderer applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileInjectPolicy {
    /// Upstream `injectProfile` (`mem_core::config::Config::inject_profile`).
    pub enabled: bool,
    pub caps: ProfileRenderCaps,
}

impl ProfileInjectPolicy {
    /// Copies the profile-injection slice out of a resolved `mem_core` config.
    ///
    /// Only the three `userProfileInject*` knobs feed the caps; the
    /// `maxProfileItems` field is read nowhere (44.9).
    pub fn from_core(config: &Config) -> Self {
        Self {
            enabled: config.inject_profile,
            caps: ProfileRenderCaps {
                preferences: usize_from(config.user_profile_inject_preferences),
                patterns: usize_from(config.user_profile_inject_patterns),
                workflows: usize_from(config.user_profile_inject_workflows),
            },
        }
    }
}

/// The renderer cap conversion (a `u32` knob into `usize` without a panic).
fn usize_from(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Upstream `escapeContextText` (`profile-context.ts:31-36`) + divergence
/// **D-44-2**: sanitize, then escape.
///
/// The single emission funnel — every profile-derived string (category,
/// description, workflow step, learning-path topic and description) reaches the
/// block only through this function, so no call site can skip the collapse or
/// the escaping. A per-call-site helper is exactly how two of the three forge
/// paths get missed.
///
/// `\n`/`\r` collapse to spaces and the result is trimmed BEFORE the three
/// escapes (so a description cannot manufacture a section header), and the
/// escape order is load-bearing: `&` first, so an input `&lt;` becomes
/// `&amp;lt;` (and not the other way round), and `>` last, so nothing
/// re-escapes the `&amp;`/`&lt;` this function has just produced.
pub fn escape_profile_text(value: &str) -> String {
    value
        .replace('\n', " ")
        .replace('\r', "")
        .trim()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Renders the block BODY (the text between the tags).
pub fn render_profile_context(
    data: &ProfileData,
    now_millis: i64,
    caps: &ProfileRenderCaps,
) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();

    let preferences: Vec<String> =
        select_preferences(&data.preferences, now_millis, caps.preferences)
            .into_iter()
            .map(|item| {
                format!(
                    "- [{}] {}",
                    escape_profile_text(item.category.as_deref().unwrap_or_default()),
                    escape_profile_text(&item.description)
                )
            })
            .collect();
    if !preferences.is_empty() {
        sections.push(section(HEADING_PREFERENCES, preferences));
    }

    let patterns: Vec<String> = select_by_category(
        sort_profile_items(&data.patterns, Metric::Frequency),
        caps.patterns,
    )
    .into_iter()
    .map(|item| {
        format!(
            "- [{}] {}",
            escape_profile_text(item.category.as_deref().unwrap_or_default()),
            escape_profile_text(&item.description)
        )
    })
    .collect();
    if !patterns.is_empty() {
        sections.push(section(HEADING_PATTERNS, patterns));
    }

    // Upstream `sortProfileItems(profileData.workflows, "frequency")` then
    // `slice(0, injectWfs)` — frequency order, frozen last, NO category dedup.
    let workflows: Vec<String> = sort_profile_items(&data.workflows, Metric::Frequency)
        .into_iter()
        .take(caps.workflows)
        .map(workflow_line)
        .collect();
    if !workflows.is_empty() {
        sections.push(section(HEADING_WORKFLOWS, workflows));
    }
    let learning_paths: Vec<String> = data
        .learning_paths
        .as_deref()
        .unwrap_or_default()
        .iter()
        .take(LEARNING_PATHS_TAKE)
        .map(|path| {
            // Upstream `escapeContextText(path.topic ?? "")`: a non-object
            // entry has neither field and renders `- : ` (Q9), an accepted
            // quirk — do not add a skip rule.
            let topic = path.get("topic").and_then(JsonValue::as_str);
            let description = path.get("description").and_then(JsonValue::as_str);
            format!(
                "- {}: {}",
                escape_profile_text(topic.unwrap_or_default()),
                escape_profile_text(description.unwrap_or_default())
            )
        })
        .collect();
    if !learning_paths.is_empty() {
        sections.push(section(HEADING_LEARNING_PATHS, learning_paths));
    }

    if sections.is_empty() {
        return None;
    }
    // Exactly one blank line between two sections, none after the open tag and
    // none before the close tag.
    Some(sections.join("\n\n"))
}

/// What one stored `profile_data` value renders to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredProfileRender {
    /// The `<user_profile>…</user_profile>` block, ready to prepend.
    Block(String),
    /// The row parsed but every section is empty — no block, never an empty
    /// wrapper.
    Empty,
    /// The row did not parse as `ProfileData` — no block, never a partial one.
    Unreadable,
}

/// Parses a stored `profile_data` value and renders it (upstream
/// `getUserProfileContext`'s parse → render → "no parts" ladder).
///
/// A parse failure is logged **once**, with no payload (D-44-3), and yields
/// [`StoredProfileRender::Unreadable`] — never a partial block.
pub fn render_stored_profile(
    profile_data: &str,
    now_millis: i64,
    caps: &ProfileRenderCaps,
) -> StoredProfileRender {
    let data: ProfileData = match serde_json::from_str(profile_data) {
        Ok(data) => data,
        Err(_) => {
            tracing::warn!(
                outcome = "unreadable",
                cause = "stored profileData is not valid json",
                "profile context: failed to parse stored profileData"
            );
            return StoredProfileRender::Unreadable;
        }
    };
    match render_profile_block(&data, now_millis, caps) {
        Some(block) => StoredProfileRender::Block(block),
        None => {
            tracing::debug!(
                outcome = "empty",
                "profile context: the stored profile renders no items"
            );
            StoredProfileRender::Empty
        }
    }
}

/// Renders the full `<user_profile>` block.
pub fn render_profile_block(
    data: &ProfileData,
    now_millis: i64,
    caps: &ProfileRenderCaps,
) -> Option<String> {
    render_profile_context(data, now_millis, caps).map(|context| {
        format!(
            "{PROFILE_BLOCK_OPEN}
{context}
{PROFILE_BLOCK_CLOSE}"
        )
    })
}

// ---------------------------------------------------------------------------
// Internals (upstream `profile-context.ts` + `utils/profile.ts`)
// ---------------------------------------------------------------------------

/// Which primary metric a section ranks by (upstream
/// `sortProfileItems(items, metric)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    Confidence,
    Frequency,
}

/// Upstream `isFrozen` (`utils/profile.ts:1`).
fn is_frozen(item: &ProfileItem) -> bool {
    item.drift_below_count.unwrap_or(0) >= 2
}

/// Upstream `sortProfileItems` (`utils/profile.ts:5`), extended to a TOTAL
/// order so identical input always yields identical bytes.
///
/// The last two keys (description, then category) are the divergence that makes
/// the ranking total; upstream leaves such ties to `Array.prototype.sort`,
/// which is stable but input-ordered.
fn sort_profile_items(items: &[ProfileItem], metric: Metric) -> Vec<&ProfileItem> {
    let mut sorted: Vec<&ProfileItem> = items.iter().collect();
    match metric {
        Metric::Confidence => sorted.sort_by(|a, b| {
            is_frozen(a)
                .cmp(&is_frozen(b))
                .then_with(|| confidence(b).total_cmp(&confidence(a)))
                // Upstream `frequency || 1` (Q4 — the confidence metric's
                // tie-break treats a zero/missing frequency as one).
                .then_with(|| b.frequency_or_one().cmp(&a.frequency_or_one()))
                .then_with(|| a.description.cmp(&b.description))
                .then_with(|| a.category.cmp(&b.category))
        }),
        Metric::Frequency => sorted.sort_by(|a, b| {
            is_frozen(a)
                .cmp(&is_frozen(b))
                // Upstream `frequency || 0` (Q4 — the frequency metric's zero
                // is a real zero).
                .then_with(|| frequency(b).cmp(&frequency(a)))
                .then_with(|| a.description.cmp(&b.description))
                .then_with(|| a.category.cmp(&b.category))
        }),
    }
    sorted
}

/// Upstream `a.confidence || 0`.
fn confidence(item: &ProfileItem) -> f64 {
    item.confidence.unwrap_or(0.0)
}

/// Upstream `a.frequency || 0`.
fn frequency(item: &ProfileItem) -> i64 {
    item.frequency.unwrap_or(0)
}

/// Upstream `scoreByRecency`'s score:
/// `confidence·0.7 + e^(−age_days/90)·0.3`, with
/// `age_days = (now − lastSeen) / MILLIS_PER_DAY`.
///
/// Q3: a missing `lastSeen` is `0`, i.e. MAXIMALLY OLD — not "now". A
/// `lastSeen` in the future yields an `exp` over a positive argument, which is
/// ordered (saturating to `inf`) rather than a panic.
fn blend_score(item: &ProfileItem, now_millis: i64) -> f64 {
    let last_seen = item.last_seen.unwrap_or(0);
    let age_days = (now_millis as f64 - last_seen as f64) / MILLIS_PER_DAY;
    confidence(item) * 0.7 + (-age_days / 90.0).exp() * 0.3
}

/// Preferences: rank → `slice(0, 2N)` → recency blend → cap.
///
/// The slice runs BEFORE the blend (upstream `sortedPrefs.slice(0, N*2)`), so a
/// low-confidence but fresh item outside the confidence window never renders;
/// deleting the slice would let the blend promote it into the category cap.
///
/// The blend is a STABLE sort by score alone, exactly like upstream's
/// `[...items].sort((a, b) => scoreB - scoreA)`: exact score ties keep the
/// total order the ranking produced.
fn select_preferences(items: &[ProfileItem], now_millis: i64, cap: usize) -> Vec<&ProfileItem> {
    let mut window: Vec<&ProfileItem> = sort_profile_items(items, Metric::Confidence)
        .into_iter()
        .take(cap.saturating_mul(2))
        .collect();
    window.sort_by(|a, b| blend_score(b, now_millis).total_cmp(&blend_score(a, now_millis)));
    select_by_category(window, cap)
}

/// Upstream `dedupByCategory`: the first item of each category wins, and the
/// scan stops at `top_n`.
///
/// Q1: a list that already fits (`len <= top_n`) is returned untouched — a
/// five-item list under a five cap is NOT category-deduped. Q2: items without
/// a `category` share the `None` key, so at most one uncategorised item
/// survives.
///
/// Divergence **D-44-1** (Q6): `top_n == 0` selects nothing. Upstream pushes
/// one item before its `break`, so a zero cap meant "one" — a bug, not a
/// contract.
fn select_by_category(items: Vec<&ProfileItem>, top_n: usize) -> Vec<&ProfileItem> {
    if top_n == 0 {
        return Vec::new();
    }
    if items.len() <= top_n {
        return items;
    }
    let mut seen: HashSet<Option<&str>> = HashSet::new();
    let mut selected: Vec<&ProfileItem> = Vec::new();
    for item in items {
        if !seen.insert(item.category.as_deref()) {
            continue;
        }
        selected.push(item);
        if selected.len() >= top_n {
            break;
        }
    }
    selected
}

/// One rendered workflow item (upstream `profile-context.ts:97-102`).
fn workflow_line(item: &ProfileItem) -> String {
    let frequency = item.frequency_or_one();
    let steps: Vec<String> = item
        .steps
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|step| escape_profile_text(step))
        .collect();
    let description = escape_profile_text(&item.description);
    if steps.is_empty() {
        format!("- {description} ({frequency}x)")
    } else {
        format!("- {description} ({frequency}x: {})", steps.join(" → "))
    }
}

/// A header plus its `- ` lines.
fn section(header: &str, lines: Vec<String>) -> String {
    let mut body = String::from(header);
    for line in lines {
        body.push('\n');
        body.push_str(&line);
    }
    body
}
