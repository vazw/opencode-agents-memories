//! W5.1 step 9 — the serial DB lane (`C2`, settled point 1).
//!
//! A `DbLane` keeps the scheduler swappable for W5.x; the W5.1
//! implementation is [`SerialDbLane`]: a single ordered drain — the agent
//! queue first, then the web queue — with the drain holding ownership of
//! the drain session, so DB writes NEVER overlap. LLM summarisation is
//! rejected at the door: it never rides the lane (D-33), so a slow
//! summariser can never block the DB writer or reorder agent captures.

use std::future::Future;
use std::pin::Pin;

/// One queued piece of DB work. The boxed future IS the work; the lane only
/// decides WHEN it runs (never before the prior job is done).
pub type Job = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Which queue the work rides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    /// The agent's own write path (highest priority, D-33).
    Agent,
    /// The web/API-driven read path.
    Web,
    /// LLM summarisation — NEVER accepted (a scheduler-independent lane
    /// keeps the summariser off the writer's critical path).
    LlmSummarisation,
}

/// Why a submit was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionReason {
    /// Work that this lane must never execute.
    LlmSummarisationOffLane,
}

/// A submit rejection; the caller logs (structured stderr upstream of the
/// wire) and drops the work — a rejection is NOT a crash.
#[derive(Debug, Clone, Copy)]
pub struct Rejection {
    pub reason: RejectionReason,
}

/// What a drain ran.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainedCounts {
    pub agent: usize,
    pub web: usize,
}

/// The lane seam: submit (queue) and drain (run). The W5.x scheduler
/// swaps implementations behind this trait; W5.1 ships [`SerialDbLane`].
pub trait DbLane {
    /// Queues one job, or rejects it.
    fn submit(
        &self,
        kind: WorkKind,
        job: Job,
    ) -> impl Future<Output = Result<(), Rejection>> + Send;
    /// Drains both queues serially (agent first) and reports the counts.
    fn drain(&self) -> impl Future<Output = DrainedCounts> + Send;
    /// The `(agent, web)` queue lengths (diagnostics/tests).
    fn queue_lengths(&self) -> impl Future<Output = (usize, usize)> + Send;
}

/// The lane's interiors, single-owner via the mutex: agent queue, web queue
/// and the drain mutex (a second drain WAITS for the first — writes never
/// overlap, and a second caller never re-runs a drained job).
#[derive(Default)]
struct LaneState {
    agent: std::collections::VecDeque<Job>,
    web: std::collections::VecDeque<Job>,
}

/// The W5.1 lane: everything is filed under ONE `tokio::sync::Mutex` and a
/// drain runs the queues agent-first while holding the lock — overlap is
/// structurally impossible (the two queues share the one drain).
pub struct SerialDbLane {
    state: tokio::sync::Mutex<LaneState>,
}

impl SerialDbLane {
    pub fn new() -> Self {
        Self {
            state: tokio::sync::Mutex::new(LaneState::default()),
        }
    }

    /// The lane's static provenance string (diagnostics only).
    pub fn banner() -> &'static str {
        "serial-db-lane agent-first"
    }
}

impl Default for SerialDbLane {
    fn default() -> Self {
        Self::new()
    }
}

impl DbLane for SerialDbLane {
    async fn submit(&self, kind: WorkKind, job: Job) -> Result<(), Rejection> {
        match kind {
            WorkKind::LlmSummarisation => {
                // Rejected AT THE DOOR: summarisation never rides the DB
                // lane (settled point 1); the future is dropped here, so a
                // caller cannot wait on it either.
                Err(Rejection {
                    reason: RejectionReason::LlmSummarisationOffLane,
                })
            }
            WorkKind::Agent => {
                let mut state = self.state.lock().await;
                state.agent.push_back(job);
                Ok(())
            }
            WorkKind::Web => {
                let mut state = self.state.lock().await;
                state.web.push_back(job);
                Ok(())
            }
        }
    }

    async fn drain(&self) -> DrainedCounts {
        let mut state = self.state.lock().await;
        let mut counts = DrainedCounts::default();
        // Agent first (D-33): every agent job completes before the web
        // queue turns.
        while let Some(job) = state.agent.pop_front() {
            job.await;
            counts.agent += 1;
        }
        while let Some(job) = state.web.pop_front() {
            job.await;
            counts.web += 1;
        }
        counts
    }

    async fn queue_lengths(&self) -> (usize, usize) {
        let state = self.state.lock().await;
        (state.agent.len(), state.web.len())
    }
}
