//! W5.5 — the compaction seam's renderer: the session-scoped memory block the
//! wrapper appends before compaction (`ingest`) and re-injects after it
//! (`restore`).
//!
//! **One production entry point:** [`render`]. The wrapper never renders
//! anything itself (it has no store and no config path), so this module is the
//! whole byte contract; the `inject` method reaches it through
//! `InjectService::compaction_leg`.
//!
//! Byte source of truth (restore only — the ingest half is port-only):
//! `example/opencode-mem/src/index.ts:1425-1441` and the footer stripper at
//! `:1392-1423`. The heading is the only thing the mode selects.
//!
//! The renderer logs nothing: it has no `session_id`, so the
//! `COMPACTION_MEMORY_INJECTED` emission belongs to the leg, which holds both
//! the id and the count.

use mem_core::config::Compaction;
use mem_store::db::Row;

/// Which half of the seam asked for the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionMode {
    /// Before compaction: appended to the host's compaction prompt.
    Ingest,
    /// After compaction: re-injected as one synthetic no-reply message.
    Restore,
}

/// The two config keys the lane reads (`compaction.enabled`,
/// `compaction.memory_limit`), resolved once at composition time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPolicy {
    /// `compaction.enabled`; a disabled policy renders nothing at all.
    pub enabled: bool,
    /// `compaction.memory_limit`; rows are truncated to this after sorting.
    pub memory_limit: u32,
}

impl Default for CompactionPolicy {
    /// Pinned to the core defaults (`crates/mem-core/src/config/schema.rs:617-618`:
    /// `compaction_enabled: true`, `compaction_memory_limit: 10`). The step-8
    /// mutation relies on exactly these values.
    fn default() -> Self {
        Self {
            enabled: true,
            memory_limit: 10,
        }
    }
}

impl CompactionPolicy {
    /// Copies the two configured values. A field copy — no clamp, no second
    /// default table.
    pub fn from_core(core: &Compaction) -> Self {
        Self {
            enabled: core.enabled,
            memory_limit: core.memory_limit,
        }
    }
}

/// Renders one block. `Some` exactly when the policy is enabled and at least
/// one row survives, after sorting `created_at` DESC and truncating to
/// `policy.memory_limit`.
pub fn render(mode: CompactionMode, rows: Vec<Row>, policy: &CompactionPolicy) -> Option<String> {
    if !policy.enabled {
        return None;
    }

    let mut memories: Vec<Memory> = rows
        .into_iter()
        .filter_map(|row| {
            Some(Memory {
                content: row.text("content")?,
                tags: split_tags(row.text("tags").as_deref()),
                created_at: row.i64("created_at").unwrap_or(0),
            })
        })
        .collect();
    memories.sort_by_key(|memory| std::cmp::Reverse(memory.created_at));
    let limit = usize::try_from(policy.memory_limit).unwrap_or(usize::MAX);
    memories.truncate(limit);
    if memories.is_empty() {
        return None;
    }

    let heading = match mode {
        CompactionMode::Ingest => "## Session Memory Before Compaction",
        CompactionMode::Restore => "## Restored Session Memory",
    };
    let mut output = String::new();
    output.push_str(heading);
    output.push_str("\n\n");
    for (index, memory) in memories.iter().enumerate() {
        let body = if memory.tags.is_empty() {
            memory.content.clone()
        } else {
            strip_matching_embedded_tags_footer(&memory.content, &memory.tags)
        };
        output.push_str("### Memory ");
        output.push_str(&(index + 1).to_string());
        output.push('\n');
        output.push_str(&body);
        output.push_str("\n\n");
        if !memory.tags.is_empty() {
            output.push_str("Tags: ");
            output.push_str(&memory.tags.join(", "));
            output.push_str("\n\n");
        }
    }
    Some(output)
}

/// One renderable row, after the column reads.
struct Memory {
    content: String,
    tags: Vec<String>,
    created_at: i64,
}

/// `input.tags.join(",")` stores the comma-joined form (`capture.rs:258`); the
/// renderer splits it back and drops empty entries (upstream's `tags` array).
fn split_tags(tags: Option<&str>) -> Vec<String> {
    tags.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

/// Port of upstream `stripMatchingEmbeddedTagsFooter`
/// (`example/opencode-mem/src/index.ts:1392-1423`):
/// `EMBEDDED_TAGS_FOOTER_RE = /\n*Tags: ([^\n]*)\s*$/`.
///
/// The regex is scanned left to right and the function acts on the FIRST match
/// only: a matching tag set strips the footer (including the newlines that
/// precede `Tags:`), a mismatching set leaves the content byte-identical. The
/// tag sets are compared after trim/filter/sort, joined with NUL.
fn strip_matching_embedded_tags_footer(memory: &str, tags: &[String]) -> String {
    const FOOTER_MARKER: &str = "Tags: ";
    let mut search_from = 0usize;
    while let Some(relative) = memory[search_from..].find(FOOTER_MARKER) {
        let occurrence = search_from + relative;
        let mut start = occurrence;
        while start > 0 && memory.as_bytes()[start - 1] == b'\n' {
            start -= 1;
        }
        let value_start = occurrence + FOOTER_MARKER.len();
        let line_end = memory[value_start..]
            .find('\n')
            .map(|offset| value_start + offset)
            .unwrap_or(memory.len());
        let footer_value = &memory[value_start..line_end];
        let tail = &memory[line_end..];
        // `$` (no multiline flag) anchors the end of input; `\s*` may consume
        // the newlines/whitespace after the footer line, so a non-whitespace
        // tail means this occurrence is not the anchored footer.
        if tail.chars().all(char::is_whitespace) {
            let embedded = split_tags(Some(footer_value));
            if normalize_tags_key(&embedded) == normalize_tags_key(tags) {
                return memory[..start].to_string();
            }
            return memory.to_string();
        }
        search_from = value_start;
    }
    memory.to_string()
}

/// Upstream `normalizeTagsKey`: trim, drop empties, sort, join with NUL.
fn normalize_tags_key(tags: &[String]) -> String {
    let mut normalized: Vec<&str> = tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .collect();
    normalized.sort_unstable();
    normalized.join("\u{0}")
}
