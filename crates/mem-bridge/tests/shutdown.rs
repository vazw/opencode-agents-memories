//! W5.1 step 10 — LIBRARY-LEVEL shutdown oracles (N10, C4).
//!
//! The process-level shutdown trio
//! (`test_shutdown_graceful_within_bounds`, `test_shutdown_sigterm_fallback`,
//! `test_shutdown_no_orphan`) lives in the ROOT package's
//! `tests/engine_shutdown.rs` (M20/M21) — this file owns only the three
//! library shutdown properties:
//!
//! 1. the ordered teardown respects the D-45 budgets;
//! 2. only standalone installs the graceful handlers (E3's)];
//! 3. a completed engine lifecycle leaves no internally-live task.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use mem_bridge::engine::{
    DB_FLUSH_BUDGET, Engine, LaunchMode, SHUTDOWN_TOTAL_BUDGET, WEB_STOP_BUDGET, shutdown_pipeline,
};

/// A stop future that NEVER resolves on its own (a wedged web server); the
/// pipeline must cut it at the budget, not wait.
fn never() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(std::future::pending())
}

fn instant_flush() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async {})
}

#[tokio::test]
async fn test_shutdown_library_within_bounds() {
    let report = shutdown_pipeline(Some(never()), instant_flush()).await;
    // Web stop leg is cut at the ~1 s budget, DB flush inside its 500 ms;
    // the WHOLE pipeline comes back inside the 2 s total.
    assert!(
        !report.web_stopped_in_budget,
        "a wedged server is reported honestly (never cut-and-claim)"
    );
    assert!(
        report.db_flushed_in_budget,
        "an instant flush is inside 500 ms"
    );
    assert!(
        report.elapsed < SHUTDOWN_TOTAL_BUDGET,
        "pipeline total stays inside the 2 s bound: {report:?}"
    );
    assert!(
        report.elapsed >= WEB_STOP_BUDGET,
        "the web budget is actually waited out first, not skipped: {report:?}"
    );
}

#[tokio::test]
async fn test_shutdown_library_sigterm_path() {
    // Standalone installs a graceful handler; stdio NEVER does (E3: an
    // in-flight stdin read must not make runtime teardown wait).
    assert!(LaunchMode::Standalone.installs_signal_handlers());
    assert!(!LaunchMode::Stdio.installs_signal_handlers());
    assert!(!LaunchMode::HangOnShutdown.installs_signal_handlers());
    // And the budgets match the settled constants (D-45) exactly.
    assert_eq!(WEB_STOP_BUDGET, Duration::from_secs(1));
    assert_eq!(DB_FLUSH_BUDGET, Duration::from_millis(500));
    assert_eq!(SHUTDOWN_TOTAL_BUDGET, Duration::from_secs(2));
}

#[tokio::test]
async fn test_shutdown_library_leaves_no_live_tasks() {
    let engine = Engine::new(mem_bridge::engine::EngineConfig::default());
    let live = engine.live_tasks();
    assert_eq!(live.load(std::sync::atomic::Ordering::SeqCst), 0);

    // The tracked spawn helper is the ONLY internal lifecycle spawn: a task
    // that completes brings the counter back to zero, so a finished run
    // leaves nothing pending.
    let work = engine.spawn_tracked(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        42u32
    });
    assert_eq!(
        live.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an in-flight tracked task is visible"
    );
    assert_eq!(work.await.expect("task completes"), 42);
    assert_eq!(
        live.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a completed run leaves no live task behind"
    );
    // And the shutdown decisions of the library itself are all awaited
    // inline: the shutdown pipeline resolved (no task parked behind it).
    let report = shutdown_pipeline(None, instant_flush()).await;
    assert!(report.web_stopped_in_budget && report.db_flushed_in_budget);
    assert_eq!(live.load(std::sync::atomic::Ordering::SeqCst), 0);
    let _ = Arc::clone(&live);
}
