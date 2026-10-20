//! Port of upstream `tests/profile-learning-idle.test.ts` (W3.1).
//!
//! Upstream drives the real plugin's `session.idle` handler in an isolated Bun
//! process and reports which background jobs ran. The Rust port exercises the
//! pure [`IdleScheduler`] with an injected clock — no wall-clock sleeps, no
//! children (the blueprint's Step 13).
//!
//! Upstream cases (traced 1:1 by name, 6) plus 3 debounce-mechanics tests:
//! - "learns from an active instance that does not own the web server"
//! - "learns when the web server is disabled entirely"
//! - "keeps retention cleanup owner-only"
//! - "runs no cleanup on a non-owner"
//! - "skips the plugin's own internal capture sessions"
//! - "does nothing when auto-capture is disabled"
//! - same-session re-idle resets the window
//! - different sessions keep independent debounces
//! - dispose cancels pending arms
//!
//! Upstream waits 10.6 s per scenario; here the clock is injected at `now`,
//! `now + 9.9 s`, `now + 10.1 s`.

use mem_profile::idle_scheduler::{DEBOUNCE_MS, IdleGates, IdleJob, IdleScheduler};

const NOW: i64 = 1_000_000;

fn gates(owner: bool, should_run_cleanup: bool) -> IdleGates {
    IdleGates {
        owner,
        should_run_cleanup,
        ..IdleGates::default()
    }
}

/// Fires the scheduler at `now + DEBOUNCE_MS + 100 ms` and returns the jobs.
fn fire_at(scheduler: &mut IdleScheduler, now: i64) -> Vec<IdleJob> {
    let mut fired = scheduler.poll_due(now + DEBOUNCE_MS + 100);
    assert_eq!(fired.len(), 1, "exactly one arm fires");
    fired.remove(0).1
}

#[test]
fn test_idle_fires_capture_and_learn_for_non_owner() {
    let mut scheduler = IdleScheduler::new();
    assert!(scheduler.on_session_idle("user-session", gates(false, true), NOW));

    // 9.9 s: still debounced.
    assert!(scheduler.poll_due(NOW + 9_900).is_empty());
    // 10.1 s: fires capture + learn.
    let jobs = fire_at(&mut scheduler, NOW);
    assert_eq!(jobs, vec![IdleJob::Capture, IdleJob::Learn]);
}

#[test]
fn test_idle_learns_when_web_server_disabled() {
    // No web server ⇒ no owner at all. Learning must still run (the upstream
    // regression this case pins).
    let mut scheduler = IdleScheduler::new();
    assert!(scheduler.on_session_idle("user-session", gates(false, false), NOW));
    let jobs = fire_at(&mut scheduler, NOW);
    assert_eq!(jobs, vec![IdleJob::Capture, IdleJob::Learn]);
}

#[test]
fn test_idle_keeps_retention_cleanup_owner_only() {
    let mut scheduler = IdleScheduler::new();
    assert!(scheduler.on_session_idle("user-session", gates(true, true), NOW));
    let jobs = fire_at(&mut scheduler, NOW);
    assert_eq!(
        jobs,
        vec![IdleJob::Capture, IdleJob::Learn, IdleJob::Cleanup]
    );
}

#[test]
fn test_idle_skips_cleanup_on_non_owner() {
    let mut scheduler = IdleScheduler::new();
    assert!(scheduler.on_session_idle("user-session", gates(false, true), NOW));
    let jobs = fire_at(&mut scheduler, NOW);
    assert!(!jobs.contains(&IdleJob::Cleanup));
    assert_eq!(jobs, vec![IdleJob::Capture, IdleJob::Learn]);
}

#[test]
fn test_idle_skips_internal_capture_sessions() {
    let mut scheduler = IdleScheduler::new();
    let internal = IdleGates {
        internal_session: true,
        ..IdleGates::default()
    };
    assert!(!scheduler.on_session_idle("internal-session", internal, NOW));
    assert!(scheduler.poll_due(NOW + DEBOUNCE_MS + 100).is_empty());
    assert_eq!(scheduler.pending_len(), 0);
}

#[test]
fn test_idle_no_jobs_when_auto_capture_disabled() {
    let mut scheduler = IdleScheduler::new();
    let disabled = IdleGates {
        auto_capture_enabled: false,
        owner: true,
        should_run_cleanup: true,
        ..IdleGates::default()
    };
    assert!(!scheduler.on_session_idle("user-session", disabled, NOW));
    assert!(scheduler.poll_due(NOW + DEBOUNCE_MS + 100).is_empty());
    assert_eq!(scheduler.pending_len(), 0);
}

#[test]
fn test_same_session_idle_resets_debounce_window() {
    let mut scheduler = IdleScheduler::new();
    assert!(scheduler.on_session_idle("session", gates(false, false), NOW));

    // A second idle mid-window re-arms: the original deadline must NOT fire.
    assert!(scheduler.on_session_idle("session", gates(false, false), NOW + 5_000));
    assert!(
        scheduler.poll_due(NOW + 9_900).is_empty(),
        "the first deadline was replaced by the re-arm"
    );
    assert!(
        scheduler.poll_due(NOW + 14_900).is_empty(),
        "the re-armed deadline is NOW + 5_000 + 10_000, so 100 ms earlier is still debounced"
    );
    let mut fired = scheduler.poll_due(NOW + 15_000);
    assert_eq!(fired.len(), 1);
    assert_eq!(fired.remove(0).0, "session");
}

#[test]
fn test_different_sessions_keep_independent_debounce() {
    let mut scheduler = IdleScheduler::new();
    assert!(scheduler.on_session_idle("a", gates(false, false), NOW));
    assert!(scheduler.on_session_idle("b", gates(false, false), NOW + 3_000));

    let first = scheduler.poll_due(NOW + 10_000);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, "a");

    let second = scheduler.poll_due(NOW + 13_000);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].0, "b");

    assert!(scheduler.poll_due(NOW + 30_000).is_empty());
}

#[test]
fn test_dispose_cancels_pending_arms() {
    let mut scheduler = IdleScheduler::new();
    assert!(scheduler.on_session_idle("a", gates(false, false), NOW));
    assert!(scheduler.on_session_idle("b", gates(false, false), NOW));

    scheduler.dispose();

    assert!(scheduler.is_disposed());
    assert_eq!(scheduler.pending_len(), 0);
    assert!(scheduler.poll_due(NOW + 60_000).is_empty());
    // Never arms after dispose.
    assert!(!scheduler.on_session_idle("c", gates(false, false), NOW));
    assert!(scheduler.poll_due(NOW + 120_000).is_empty());
}
