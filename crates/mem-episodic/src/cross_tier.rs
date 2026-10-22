//! The cross-tier phase (W7.1, D1c/Q7) — the seam that turns a committed
//! consolidation milestone into `facts.db` rows.
//!
//! # Why this is NOT `promotion.rs`
//!
//! `promotion.rs` is the W4.3 LEGACY promotion: it copies a decision into the
//! tier-1 `memories` shard (`PermanentMemorySink`), byte-compatible with
//! upstream. This module is the five-tier cross-tier phase: after the durable
//! episodic batch commits, it receives the consolidated rows plus the freshly
//! minted milestone id and writes atomic facts into `facts.db` with the
//! milestone as `source_ref`. The two are deliberately distinct; the counters
//! never mix.
//!
//! # Ordering and idempotency
//!
//! [`CrossTierTask::run`] is called AFTER the consolidation batch commits, so
//! a milestone the task names already exists. It is idempotent: a retry over
//! the same milestone finds the facts present and reports them as
//! `already_present`, never duplicating a row.
//!
//! [`CrossTierTask::promotion_due`] is PURE: no I/O, no clock — the `now`
//! argument is the run's clock.

use std::sync::Arc;

use mem_embed::embedder::BoxFuture;

use crate::error::EpisodicError;
use crate::promotion::PromotableFact;
use crate::record::AgentSession;

/// One consolidated session, as the cross-tier phase sees it.
///
/// A fact's provenance is the subset of these whose `row_id` appears in the
/// fact's `source_row_ids`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    /// The source row id (`agent_sessions.id`).
    pub row_id: String,
    /// The session scope id.
    pub session_id: String,
    /// The agent scope id.
    pub agent_id: String,
}

/// The READ-ONLY view a [`CrossTierTask`] receives after the batch committed.
#[derive(Debug, Clone, Copy)]
pub struct CrossTierContext<'a> {
    /// Project scope.
    pub project_id: &'a str,
    /// Agent scope.
    pub agent_id: &'a str,
    /// The milestone the batch just wrote.
    pub milestone_id: &'a str,
    /// The consolidated sessions (row id + logical identity).
    pub sessions: &'a [SessionRef],
    /// The facts selected from those rows.
    pub facts: &'a [PromotableFact],
    /// The run's clock (unix millis).
    pub now: i64,
}

/// What a cross-tier phase did.
///
/// All three counters are surfaced: a bare "promoted" number hid
/// already-present and skipped work (M9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrossTierReport {
    /// Facts newly written.
    pub promoted: u64,
    /// Facts already present with identical content.
    pub already_present: u64,
    /// Candidates deliberately skipped (e.g. over the content bound).
    pub skipped: u64,
}

/// The cross-tier port the consolidator calls after its durable batch.
pub trait CrossTierTask: Send + Sync {
    /// Whether `rows` are due for promotion at `now`.
    ///
    /// PURE: no I/O, no clock read. `false` makes the consolidator skip the
    /// agent BEFORE any promotion or write, leaving the session rows for a
    /// later run.
    fn promotion_due(&self, rows: &[AgentSession], now: i64) -> bool;

    /// Runs the phase over the committed milestone.
    fn run<'a>(
        &'a self,
        context: &'a CrossTierContext<'a>,
    ) -> BoxFuture<'a, Result<CrossTierReport, EpisodicError>>;
}

/// The default, flag-off task: never promotes, writes nothing, and keeps the
/// report at zero.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCrossTierTask;

impl CrossTierTask for NoopCrossTierTask {
    fn promotion_due(&self, _rows: &[AgentSession], _now: i64) -> bool {
        true
    }

    fn run<'a>(
        &'a self,
        _context: &'a CrossTierContext<'a>,
    ) -> BoxFuture<'a, Result<CrossTierReport, EpisodicError>> {
        Box::pin(async { Ok(CrossTierReport::default()) })
    }
}

/// The boxed task type carried by `ConsolidationDeps`.
pub type SharedCrossTierTask = Arc<dyn CrossTierTask>;
