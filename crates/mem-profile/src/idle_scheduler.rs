//! Pure idle-debounce scheduler (W3.1 step 13) — port of upstream's
//! `session.idle` handler (`src/index.ts` L370-375, L1240-1298).
//!
//! Upstream arms a **10-second per-session debounce** on `session.idle`, then
//! runs capture + learning (and, for the web-server owner only, retention
//! cleanup). There is no 5-minute timer anywhere (D-V4); the only upstream
//! `300_000 ms` constant is an HTTP timeout.
//!
//! This port is pure: the clock is injected, no wall-clock sleeps, no I/O.
//! `poll_due` is the test-visible edge the host calls from its own timer.

use std::collections::BTreeMap;

/// Upstream per-session idle debounce (`10_000 ms`, pinned by D-V4).
pub const DEBOUNCE_MS: i64 = 10_000;

/// A unit of idle work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleJob {
    /// Upstream `performAutoCapture`.
    Capture,
    /// Upstream `performUserProfileLearning`.
    Learn,
    /// Upstream `cleanupService.runCleanup` (web-server owner only).
    Cleanup,
}

/// The host gates resolved at `session.idle` time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleGates {
    /// `CONFIG.autoCaptureEnabled`.
    pub auto_capture_enabled: bool,
    /// The plugin's own internal capture session (must be skipped entirely).
    pub internal_session: bool,
    /// Whether this instance owns the web server (no server ⇒ no owner).
    pub owner: bool,
    /// `cleanupService.shouldRunCleanup()`.
    pub should_run_cleanup: bool,
}

impl Default for IdleGates {
    fn default() -> Self {
        Self {
            auto_capture_enabled: true,
            internal_session: false,
            owner: false,
            should_run_cleanup: false,
        }
    }
}

/// One pending arm. `generation` is the re-arm identity: `poll_due` only
/// removes the entry it observed, so a re-arm between the due scan and the
/// removal is never cancelled by the older tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingArm {
    deadline: i64,
    generation: u64,
    gates: IdleGates,
}

/// The pure debounce scheduler.
#[derive(Debug, Clone)]
pub struct IdleScheduler {
    debounce_ms: i64,
    generation: u64,
    disposed: bool,
    pending: BTreeMap<String, PendingArm>,
}

impl Default for IdleScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl IdleScheduler {
    /// The production scheduler (10 s debounce).
    pub fn new() -> Self {
        Self {
            debounce_ms: DEBOUNCE_MS,
            generation: 0,
            disposed: false,
            pending: BTreeMap::new(),
        }
    }

    /// A scheduler with an explicit debounce (fixtures only).
    pub fn with_debounce(debounce_ms: i64) -> Self {
        Self {
            debounce_ms: debounce_ms.max(0),
            ..Self::new()
        }
    }

    pub fn debounce_ms(&self) -> i64 {
        self.debounce_ms
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed
    }

    /// Number of pending arms (test/inspection helper).
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Whether `session_id` has a pending arm (test/inspection helper).
    pub fn is_armed(&self, session_id: &str) -> bool {
        self.pending.contains_key(session_id)
    }

    /// Handles one `session.idle` event.
    ///
    /// Returns `true` when an arm is now pending. Never arms after
    /// [`IdleScheduler::dispose`]; a same-session re-idle REPLACES the pending
    /// deadline (upstream re-arms the timer); internal sessions and a disabled
    /// auto-capture produce no work at all.
    pub fn on_session_idle(&mut self, session_id: &str, gates: IdleGates, now: i64) -> bool {
        if self.disposed {
            return false;
        }
        if !gates.auto_capture_enabled || gates.internal_session {
            // No work for this event — and no stale arm may survive either.
            self.pending.remove(session_id);
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending.insert(
            session_id.to_string(),
            PendingArm {
                deadline: now.saturating_add(self.debounce_ms),
                generation: self.generation,
                gates,
            },
        );
        true
    }

    /// Fires every arm whose deadline has been reached, in session-id order.
    ///
    /// The job list is `[Capture, Learn]`, plus `Cleanup` iff the arm was
    /// created by the web-server owner and the retention cleanup gate was set.
    /// An arm is removed only while its generation is unchanged.
    pub fn poll_due(&mut self, now: i64) -> Vec<(String, Vec<IdleJob>)> {
        if self.disposed {
            return Vec::new();
        }
        let due: Vec<(String, u64)> = self
            .pending
            .iter()
            .filter(|(_, arm)| now >= arm.deadline)
            .map(|(session_id, arm)| (session_id.clone(), arm.generation))
            .collect();

        let mut fired = Vec::with_capacity(due.len());
        for (session_id, observed_generation) in due {
            let Some(arm) = self.pending.get(&session_id).copied() else {
                continue;
            };
            // Re-arm race mirror: the entry we scanned is the entry we remove.
            if arm.generation != observed_generation {
                continue;
            }
            self.pending.remove(&session_id);
            let mut jobs = vec![IdleJob::Capture, IdleJob::Learn];
            if arm.gates.owner && arm.gates.should_run_cleanup {
                jobs.push(IdleJob::Cleanup);
            }
            fired.push((session_id, jobs));
        }
        fired
    }

    /// Shuts the scheduler down: every pending arm is dropped and no later
    /// idle event can arm again.
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debounce_is_ten_seconds() {
        assert_eq!(DEBOUNCE_MS, 10_000);
        assert_eq!(IdleScheduler::new().debounce_ms(), 10_000);
    }

    #[test]
    fn test_poll_due_boundary_is_inclusive() {
        let mut scheduler = IdleScheduler::new();
        assert!(scheduler.on_session_idle("s", IdleGates::default(), 1_000));
        assert!(scheduler.poll_due(10_999).is_empty());
        assert_eq!(scheduler.poll_due(11_000).len(), 1);
    }

    #[test]
    fn test_generation_guard_keeps_a_rearmed_entry() {
        let mut scheduler = IdleScheduler::new();
        scheduler.on_session_idle("s", IdleGates::default(), 0);
        // A re-arm at the same instant must produce a *different* generation,
        // so an older tick cannot cancel it.
        scheduler.on_session_idle("s", IdleGates::default(), 0);
        assert!(scheduler.poll_due(10_000).len() == 1);
        assert_eq!(scheduler.pending_len(), 0);
    }
}
