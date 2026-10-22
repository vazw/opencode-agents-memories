//! Raw context (W7.1): the authoritative per-session bytes.
//!
//! # One representation only
//!
//! The authoritative representation is the `episodic_raw_context` TABLE,
//! keyed by the session's logical identity `(project_id, session_id,
//! agent_id)`. There is deliberately NO `agent_sessions.raw_context` column:
//! consolidation hard-deletes a session ROW, and the identity — not the row —
//! is what the raw bytes hang off (M1).
//!
//! # Immutability with exactly one exit
//!
//! Writes are first-write-wins ([`RawContextWrite::AlreadyPresent`] on a
//! second write, whatever the bytes). There is no overwrite path; the only
//! delete is the explicit, owner-initiated `purge_session` (D6). Consolidation
//! and every lifecycle stage never destroy raw context.

use crate::error::EpisodicError;
use crate::segments::MAX_SEGMENT_BYTES;

/// The hard bound on one raw context (256 KiB).
pub const MAX_RAW_CONTEXT_BYTES: usize = 262_144;

/// One stored raw context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawContext {
    /// Project scope.
    pub project_id: String,
    /// Session scope.
    pub session_id: String,
    /// Agent scope.
    pub agent_id: String,
    /// The stored bytes.
    pub raw_context: String,
    /// `raw_context.len()` in bytes.
    pub byte_len: i64,
    /// Capture stamp (unix millis).
    pub captured_at: i64,
}

/// What an [`insert`](crate::store::EpisodicStore::insert_raw_context) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawContextWrite {
    /// The first write for the triple: the raw row and its segments landed.
    Inserted {
        /// How many segments were derived and stored.
        segments: usize,
    },
    /// The triple already has raw context; nothing was written (first write
    /// wins — a recapture never overwrites the authoritative bytes).
    AlreadyPresent,
}

impl RawContextWrite {
    /// Whether this was a first write.
    pub fn is_inserted(self) -> bool {
        matches!(self, Self::Inserted { .. })
    }
}

/// Validates a raw-context payload: non-empty after trim, NUL-free, and at
/// most [`MAX_RAW_CONTEXT_BYTES`].
///
/// Raw context is untrusted input; the error carries the field name and a
/// fixed reason, never the payload.
pub fn validate_raw_context(raw_context: &str) -> Result<(), EpisodicError> {
    if raw_context.trim().is_empty() {
        return Err(EpisodicError::validation(
            "raw_context",
            "must not be empty or whitespace-only",
        ));
    }
    if raw_context.contains('\u{0}') {
        return Err(EpisodicError::validation(
            "raw_context",
            "must not contain NUL bytes",
        ));
    }
    if raw_context.len() > MAX_RAW_CONTEXT_BYTES {
        return Err(EpisodicError::validation(
            "raw_context",
            format!("must be at most {MAX_RAW_CONTEXT_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// The segment bound re-exported beside the raw bound (the two move
/// together).
pub const SEGMENT_BOUND: usize = MAX_SEGMENT_BYTES;
