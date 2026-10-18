//! Executable demonstrator of the fixture-translation recipe documented in
//! `docs/agents/testing.md`: upstream worker-script fixtures (e.g.
//! `profile-learning-lock-worker.mts`, `profile-cold-buffer-worker-shortwrite.ts`
//! → W3.1) become child entries in the consuming crate's integration test
//! binary, spawned via self re-exec (`std::env::current_exe()` +
//! `--exact fixture_child_entry --ignored`) with an env-var mode protocol.
//!
//! Evidence-file assertions are the anti-false-green guard: `--exact`
//! matching nothing would otherwise exit 0, so the parent asserts the
//! child's output file contents.

use std::io::ErrorKind;
use std::time::Duration;

use common::{FIXTURE_OUT_ENV, run_fixture_child};

mod common;

/// The child entry point — spawned via current_exe re-exec by the parent
/// fixture tests. Must be exactly `fixture_child_entry`: the shared helper
/// hardcodes this name (every crate copying the recipe names it the same).
#[test]
#[ignore = "fixture child entry: spawned via current_exe re-exec by the parent fixture tests"]
fn fixture_child_entry() {
    let mode = std::env::var(common::FIXTURE_CHILD_ENV).ok();
    let Some(mode) = mode else {
        // No child-mode env set: an accidental run is a harmless no-op.
        return;
    };
    let out_path =
        std::env::var(FIXTURE_OUT_ENV).expect("child output path required in child mode");
    match mode.as_str() {
        "write-marker" => {
            std::fs::write(&out_path, "fixture-child-ran")
                .expect("failed to write fixture child marker");
        }
        "hang" => {
            std::thread::sleep(Duration::from_secs(30));
        }
        other => panic!("unknown fixture child mode: {other}"),
    }
}

/// "write-marker" child runs to completion under the protocol and the
/// evidence file contains exactly the expected payload.
#[test]
fn test_fixture_child_reexec_protocol_roundtrip() {
    let dir = common::unique_temp_dir("fixture-reexec");
    let out = dir.join("child.out");
    let status = run_fixture_child("write-marker", &out, &[], Duration::from_secs(10))
        .expect("fixture child run failed to start");
    assert!(status.success(), "fixture child exited with {status}");
    assert_eq!(
        std::fs::read_to_string(&out).expect("marker file must exist"),
        "fixture-child-ran"
    );
}

/// "hang" child hits the bounded deadline: killed + reaped, and the parent
/// observes a `TimedOut` error — no orphan processes, no unbounded wait.
#[test]
fn test_fixture_child_timeout_kills_and_reports() {
    let dir = common::unique_temp_dir("fixture-timeout");
    let out = dir.join("child.out");
    let err = run_fixture_child("hang", &out, &[], Duration::from_millis(1500))
        .expect_err("hanging child must time out");
    assert_eq!(err.kind(), ErrorKind::TimedOut);
}
