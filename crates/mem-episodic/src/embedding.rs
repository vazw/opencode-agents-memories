//! Embedded-document composition and embedder glue (W4.1; D-4).
//!
//! The template is locked by the acceptance tests:
//!
//! ```text
//! Session {session_id} by {agent_id} on {branch}: {summary}. Completed: {completed_work}. Decisions: {decisions}.
//! ```
//!
//! - `{session_id}` is the scope-triple session id from the record, never the
//!   minted row id (D-4);
//! - a missing/empty branch renders the pinned literal
//!   [`UNKNOWN_BRANCH`];
//! - work items render `"{wire_kind}: {description}"` and list entries join
//!   with `"; "`;
//! - [`apply_task_prefix`] with [`PrefixOptions::default`] is applied to the
//!   composed document before the embedder sees it (currently a no-op prefix,
//!   pinned by the FakeEmbedder test so a future prefix change is visible);
//! - the raw `summary` stays the stored column.

use mem_embed::embedder::{EmbedTask, Embedder};
use mem_embed::prefix::{PrefixOptions, apply_task_prefix};

use crate::error::EpisodicError;
use crate::record::AgentSession;

/// Rendered when the record carries no git branch.
pub const UNKNOWN_BRANCH: &str = "unknown branch";

/// Joins list entries for the embedded document.
const LIST_SEPARATOR: &str = "; ";

/// Composes the document embedded for `session` (before prefixing).
pub fn compose_embedded_document(session: &AgentSession) -> String {
    let branch = session
        .metadata
        .git_branch
        .as_deref()
        .filter(|branch| !branch.is_empty())
        .unwrap_or(UNKNOWN_BRANCH);
    let completed = session
        .metadata
        .completed_work
        .iter()
        .map(|item| format!("{}: {}", item.kind.as_wire_name(), item.description))
        .collect::<Vec<_>>()
        .join(LIST_SEPARATOR);
    let decisions = session
        .metadata
        .decisions
        .iter()
        .map(|decision| decision.description.as_str())
        .collect::<Vec<_>>()
        .join(LIST_SEPARATOR);
    format!(
        "Session {} by {} on {}: {}. Completed: {}. Decisions: {}.",
        session.session_id, session.agent_id, branch, session.summary, completed, decisions
    )
}

/// The exact text handed to the embedding provider (prefix applied).
pub fn embedded_text(session: &AgentSession) -> String {
    apply_task_prefix(
        &compose_embedded_document(session),
        EmbedTask::Document,
        &PrefixOptions::default(),
    )
}

/// Embeds the composed document for `session`.
pub async fn embed_document(
    embedder: &dyn Embedder,
    session: &AgentSession,
) -> Result<Vec<f32>, EpisodicError> {
    let text = embedded_text(session);
    let vector = embedder.embed(&text).await?;
    Ok(vector)
}

/// The query-side mirror of [`embedded_text`] (D-15): the same
/// [`PrefixOptions`], differing only in task. The query string is raw user
/// text, so no list rendering or wire-kind formatting is involved.
pub fn query_embedded_text(query: &str) -> String {
    apply_task_prefix(query, EmbedTask::Query, &PrefixOptions::default())
}

/// Embeds a timeline query string for the semantic read path.
pub async fn embed_query(embedder: &dyn Embedder, query: &str) -> Result<Vec<f32>, EpisodicError> {
    let text = query_embedded_text(query);
    let vector = embedder.embed(&text).await?;
    Ok(vector)
}
