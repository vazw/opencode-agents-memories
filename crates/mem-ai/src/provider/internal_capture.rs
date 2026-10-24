//! Internal capture session loop guard.
//!
//! The auto-capture hook must never fire for the transient sessions the
//! provider itself creates for structured capture — otherwise capture feeds
//! back into capture forever. The tracked set is process-global (upstream's
//! module-level `Set`) so every capture path observes the same state, and untrack
//! keeps a grace window so a late `session.idle` still matches after the
//! session was already deleted.

use mem_core::sync::lock_or_recover;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Title used for transient structured-output sessions (capture / profile learning).
pub const INTERNAL_CAPTURE_SESSION_TITLE: &str = "opencode-mem capture";

/// Upstream grace period so `session.idle` can still match after a
/// best-effort delete.
pub const UNTRACK_GRACE: Duration = Duration::from_secs(60);

/// True when `title` is the reserved internal-capture session title.
pub fn is_internal_capture_session_title(title: &str) -> bool {
    title == INTERNAL_CAPTURE_SESSION_TITLE
}

/// Tracked entries: `None` while the session is live, `Some(deadline)` once
/// untracked (grace window).
#[derive(Debug, Default)]
struct State {
    tracked: HashMap<String, Option<Instant>>,
}

/// Process-global (or handle-local) set of internal capture session ids.
///
/// Cheap to clone: clones share the same underlying set.
#[derive(Debug, Clone)]
pub struct InternalCaptureSessions {
    inner: Arc<Mutex<State>>,
    grace: Duration,
}

impl Default for InternalCaptureSessions {
    fn default() -> Self {
        Self::with_grace(UNTRACK_GRACE)
    }
}

impl InternalCaptureSessions {
    /// A fresh handle with the upstream 60s grace.
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh handle with an explicit grace window (`Duration::ZERO` releases
    /// immediately on untrack).
    pub fn with_grace(grace: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State::default())),
            grace,
        }
    }

    /// The process-global handle shared by every capture path.
    pub fn global() -> &'static InternalCaptureSessions {
        static GLOBAL: OnceLock<InternalCaptureSessions> = OnceLock::new();
        GLOBAL.get_or_init(InternalCaptureSessions::new)
    }

    /// Mark a session as an internal capture session (live).
    pub fn track(&self, session_id: &str) {
        // tracked map: session id to grace deadline; single insert, in-process, nothing persisted (ADR-0013 D-2 D2-R)
        lock_or_recover(&self.inner)
            .tracked
            .insert(session_id.to_owned(), None);
    }

    /// Release a session after the grace window (or immediately when grace is
    /// zero).
    pub fn untrack(&self, session_id: &str) {
        // tracked: insert-or-remove of one entry; worst case one capture skipped (ADR-0013 D-2 D2-R)
        let mut state = lock_or_recover(&self.inner);
        if self.grace.is_zero() {
            state.tracked.remove(session_id);
        } else {
            // checked_add: an overflow stores None ("live"), the fail-safe direction (ADR-0013 D-2 D2-R)
            state.tracked.insert(
                session_id.to_owned(),
                Instant::now().checked_add(self.grace),
            );
        }
    }

    /// True while `session_id` is tracked (live or still inside its grace
    /// window). Expired entries are pruned eagerly.
    pub fn is_tracked(&self, session_id: &str) -> bool {
        // tracked: read plus eager prune of one entry (ADR-0013 D-2 D2-R)
        let mut state = lock_or_recover(&self.inner);
        match state.tracked.get(session_id) {
            None => false,
            Some(None) => true,
            Some(Some(deadline)) => {
                if Instant::now() < *deadline {
                    true
                } else {
                    state.tracked.remove(session_id);
                    false
                }
            }
        }
    }

    /// Whether an auto-capture should run for `session_id`. Internal capture
    /// sessions are never re-captured.
    pub fn should_capture(&self, session_id: &str) -> bool {
        !self.is_tracked(session_id)
    }

    /// Drop all tracked ids.
    pub fn clear(&self) {
        // tracked: one clear; the set is a per-process cache (ADR-0013 D-2 D2-R)
        lock_or_recover(&self.inner).tracked.clear();
    }
}
