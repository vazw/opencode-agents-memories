//! Task-prefix application: custom prefixes (including explicit `""`) win
//! over the Nomic constants, which apply only when task prefixes are enabled
//! (upstream `resolveEmbeddingTaskPrefix` / `applyEmbeddingTaskPrefix`).

use crate::embedder::EmbedTask;

/// Prefix precedence inputs: custom prefixes (including `""`) win over the
/// Nomic constants, which apply only when task prefixes are enabled.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrefixOptions {
    pub use_task_prefixes: bool,
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
}

/// Nomic-style document prefix.
pub const NOMIC_DOCUMENT_PREFIX: &str = "search_document: ";
/// Nomic-style query prefix.
pub const NOMIC_QUERY_PREFIX: &str = "search_query: ";

/// Resolve the prefix for `task` under `options`: a custom prefix
/// (`Some(..)`, including `""`) wins unconditionally; otherwise enabling
/// task prefixes yields the Nomic constant; otherwise no prefix.
pub fn resolve_task_prefix(task: EmbedTask, options: &PrefixOptions) -> String {
    let custom = match task {
        EmbedTask::Query => &options.query_prefix,
        EmbedTask::Document => &options.document_prefix,
    };
    if let Some(prefix) = custom {
        return prefix.clone();
    }
    if options.use_task_prefixes {
        return match task {
            EmbedTask::Document => NOMIC_DOCUMENT_PREFIX.to_owned(),
            EmbedTask::Query => NOMIC_QUERY_PREFIX.to_owned(),
        };
    }
    String::new()
}

/// Prefix `text` for `task` without ever double-prefixing: an empty resolved
/// prefix or text already carrying the prefix passes through unchanged.
pub fn apply_task_prefix(text: &str, task: EmbedTask, options: &PrefixOptions) -> String {
    let prefix = resolve_task_prefix(task, options);
    if prefix.is_empty() || text.starts_with(&prefix) {
        return text.to_owned();
    }
    format!("{prefix}{text}")
}
