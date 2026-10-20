//! Milestone records — the W4.3 consolidation output.
//!
//! A **milestone** is the surviving record of one consolidated
//! `(project_id, agent_id)` window: the merged summary, the list of the
//! source sessions it was built from, and the retained per-source summary
//! text (D-27).
//!
//! Two invariants are load-bearing:
//!
//! - the **contributors** JSON is the only carrier of the retained source
//!   text, so the `contributors` column is `TEXT NOT NULL` and holds the
//!   encoded array — there is no separate column and no v4 migration;
//! - the **embedding is write-only**: it is bound into the INSERT as
//!   `vector32(?)` and is never read back (`#[serde(skip)]` keeps it off the
//!   wire, and the read projection does not select it).

use mem_store::serde::{Deserialize, Serialize};

use crate::error::EpisodicError;
use crate::record::AgentSession;
use crate::summarizer::stripped_source_summary;

/// One source session behind a milestone.
///
/// The `summary` field carries the SOURCE SESSION's own summary text,
/// retained verbatim (privacy-stripped) so the raw per-session text stays
/// recoverable after the source row is hard-deleted (D-27).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub struct Contributor {
    /// The source row's id (`agent_sessions.id`).
    pub session_row_id: String,
    /// The source session's scope id (`agent_sessions.session_id`).
    pub session_id: String,
    /// The source session's parent link, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// The source session's agent scope.
    pub agent_id: String,
    /// The source session's agent display name.
    pub agent_name: String,
    /// The source session's creation stamp (unix millis).
    pub created_at: i64,
    /// The source session's own summary text, privacy-stripped and retained
    /// verbatim (D-27). Empty when the source summary stripped to empty.
    pub summary: String,
}

/// One consolidated milestone row.
///
/// Wire shape is camelCase; the embedding never travels the wire
/// (`#[serde(skip)]`, exactly like [`crate::record::AgentSession`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "mem_store::serde", rename_all = "camelCase")]
pub struct Milestone {
    /// Row id (UUIDv7, minted by the store's id path).
    pub id: String,
    /// Project scope.
    pub project_id: String,
    /// Agent scope (milestones are per `(project_id, agent_id)`).
    pub agent_id: String,
    /// The merged milestone summary.
    pub summary: String,
    /// The source sessions behind this milestone, with their retained
    /// summary text.
    pub contributors: Vec<Contributor>,
    /// How many source sessions were consolidated.
    pub source_count: i64,
    /// The oldest consolidated session's `created_at` (unix millis).
    pub window_start: i64,
    /// The newest consolidated session's `created_at` (unix millis).
    pub window_end: i64,
    /// Embedding of the milestone summary (populated at write time; never
    /// serialized and never read back).
    #[serde(skip)]
    pub embedding: Vec<f32>,
    /// Creation stamp (unix millis).
    pub created_at: i64,
}

/// Decodes the stored contributors JSON (fail-closed on malformed text).
///
/// The list path fails soft around this call — a milestone row whose
/// contributors JSON cannot be decoded is skipped with a warning, exactly
/// like a session row with unreadable metadata.
pub fn decode_contributors(json: &str) -> Result<Vec<Contributor>, EpisodicError> {
    mem_store::serde_json::from_str(json).map_err(|error| EpisodicError::Json {
        message: format!("contributors: {error}"),
    })
}

/// Encodes the contributors as compact, deterministic JSON.
///
/// The field order is the declaration order and the array order is the
/// contributor order, so the same contributors always produce the same
/// bytes — the bound check below measures exactly this string.
pub fn encode_contributors(contributors: &[Contributor]) -> Result<String, EpisodicError> {
    mem_store::serde_json::to_string(contributors).map_err(|error| EpisodicError::Json {
        message: format!("contributors: {error}"),
    })
}

/// The per-row framing allowance of the contributors bound (1_536 bytes).
///
/// The allowance covers the per-row JSON framing on top of the text:
/// `session_row_id` (≤ `MAX_ID_LEN` = 256), `session_id` (≤ 256),
/// `parent_session_id` (≤ 256), `agent_id` (≤ `MAX_AGENT_ID_LEN` = 128),
/// `agent_name` (≤ 128), the `created_at` digits, the JSON keys and the
/// punctuation — and it scales with the row count, so raising `batch_limit`
/// raises the allowance proportionally.
///
/// **The allowance does NOT cover the JSON ESCAPING expansion.** A `"` or
/// `\` becomes 2 bytes and any other control character becomes the 6-byte
/// `\u00XX` form, so the ESCAPED text can be up to 6× the raw text and is
/// NOT bounded by `MAX_TRANSCRIPT_BYTES`. What makes the check airtight is
/// that the measured quantity is the exact byte length of the ASSEMBLED
/// JSON that gets bound into the `contributors` column — no escaping
/// expansion can slip past it. The consequence is recorded in ADR-0006
/// (D-27): an over-bound contributors JSON is a PERMANENT per-agent skip.
pub const MAX_CONTRIBUTORS_FRAMING_BYTES_PER_ROW: usize = 1_536;

/// The byte bound of the assembled contributors JSON for `row_count` rows.
pub fn max_contributors_bytes(row_count: usize) -> usize {
    crate::summarizer::MAX_TRANSCRIPT_BYTES
        + MAX_CONTRIBUTORS_FRAMING_BYTES_PER_ROW.saturating_mul(row_count)
}

/// Plans one milestone row (pure and deterministic).
///
/// No I/O, no clock, no randomness and no id minting: the milestone id
/// arrives as an explicit first parameter (the caller mints it through the
/// store's id path) so this function stays pure.
///
/// - `window_start` / `window_end` are the min/max `created_at` of the
///   CONSOLIDATED rows;
/// - `source_count` is their count;
/// - `contributors` are the source rows ordered `created_at ASC, id ASC`,
///   each carrying its own privacy-stripped, retained summary text.
pub fn plan_milestone(
    id: &str,
    rows: &[AgentSession],
    summary: &str,
    embedding: Vec<f32>,
    now: i64,
) -> Milestone {
    let mut contributors: Vec<Contributor> = rows
        .iter()
        .map(|row| Contributor {
            session_row_id: row.id.clone(),
            session_id: row.session_id.clone(),
            parent_session_id: row.parent_session_id.clone(),
            agent_id: row.agent_id.clone(),
            agent_name: row.metadata.agent_name.clone(),
            created_at: row.created_at,
            summary: stripped_source_summary(&row.summary),
        })
        .collect();
    contributors.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.session_row_id.cmp(&right.session_row_id))
    });

    let window_start = rows
        .iter()
        .map(|row| row.created_at)
        .min()
        .unwrap_or_default();
    let window_end = rows
        .iter()
        .map(|row| row.created_at)
        .max()
        .unwrap_or_default();
    let project_id = rows
        .first()
        .map(|row| row.project_id.clone())
        .unwrap_or_default();
    let agent_id = rows
        .first()
        .map(|row| row.agent_id.clone())
        .unwrap_or_default();

    Milestone {
        id: id.to_string(),
        project_id,
        agent_id,
        summary: summary.to_string(),
        contributors,
        source_count: rows.len() as i64,
        window_start,
        window_end,
        embedding,
        created_at: now,
    }
}
