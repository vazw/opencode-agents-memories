//! Internal capture session tracking (upstream `internal-capture-sessions.test.ts`).
//!
//! The guard's job: a session created for an internal structured-output prompt
//! must never be re-captured (no feedback loop), and the tracked set is
//! process-global so every capture path sees the same state.

use std::time::Duration;

use mem_ai::provider::{
    INTERNAL_CAPTURE_SESSION_TITLE, InternalCaptureSessions, is_internal_capture_session_title,
};

#[test]
fn test_internal_capture_session_not_recaptured() {
    let sessions = InternalCaptureSessions::with_grace(Duration::from_secs(60));

    assert!(sessions.should_capture("ses_other"));
    assert!(!sessions.is_tracked("ses_live"));

    sessions.track("ses_live");
    assert!(sessions.is_tracked("ses_live"));
    // While tracked, the capture path must skip this session entirely.
    assert!(!sessions.should_capture("ses_live"));

    // Untrack keeps the id in the grace window so a late `session.idle` still
    // matches (upstream 60s grace), i.e. still not re-captured.
    sessions.untrack("ses_live");
    assert!(sessions.is_tracked("ses_live"));
    assert!(!sessions.should_capture("ses_live"));

    // With no grace, untracking releases immediately.
    let immediate = InternalCaptureSessions::with_grace(Duration::ZERO);
    immediate.track("ses_now");
    assert!(!immediate.should_capture("ses_now"));
    immediate.untrack("ses_now");
    assert!(immediate.should_capture("ses_now"));

    // The shared title constant identifies internal sessions.
    assert!(is_internal_capture_session_title(
        INTERNAL_CAPTURE_SESSION_TITLE
    ));
    assert!(!is_internal_capture_session_title("normal session"));
}

#[test]
fn test_internal_session_set_global() {
    let first = InternalCaptureSessions::global();
    let second = InternalCaptureSessions::global();

    first.clear();
    first.track("ses_global");

    // Both handles observe the same process-global set.
    assert!(second.is_tracked("ses_global"));
    assert!(!second.should_capture("ses_global"));

    second.untrack("ses_global");
    assert!(first.is_tracked("ses_global"), "grace keeps it visible");

    first.clear();
    assert!(!second.is_tracked("ses_global"));
}
