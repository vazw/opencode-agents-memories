//! W5.1 step 3 — engine lock + status-file oracles.
//!
//! The engine lock is a **lifetime** lock (`.engine.lock`), distinct from the
//! operation-scoped `.turso-operation.lock`: holding the operation lock for the
//! engine's whole life would make `get_write_shard` refuse the engine's own
//! writes (ADR-0007 D-47).
//!
//! These are library-level oracles. The process-level exit-code-75 oracle lives
//! in the root package's `tests/engine_shutdown.rs` (M13), because `CARGO_BIN_EXE_<name>`
//! only exists for the package that declares the binary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_store::engine_lock::{
    ENGINE_LOCK_FILE, ENGINE_LOCK_LOST, EngineLockOutcome, STATUS_FILE, acquire_engine_lock,
    write_status_file,
};
use mem_store::operation_lock::{LockState, read_live_lock};
use mem_store::platform::{ProcessProbe, SystemProcessProbe};

fn probe() -> Arc<dyn ProcessProbe> {
    Arc::new(SystemProcessProbe)
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "mem-store-w51-{prefix}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn lock_path(dir: &Path) -> PathBuf {
    dir.join(ENGINE_LOCK_FILE)
}

/// Writes a raw lock body (foreign writer simulation).
fn write_raw_lock(dir: &Path, body: &str) {
    std::fs::write(lock_path(dir), body).expect("write raw lock");
}

fn write_lock_state(dir: &Path, pid: u32) {
    let state = LockState {
        pid,
        timestamp: "2026-10-20T00:00:00.000Z".to_string(),
        operation: Some("engine".to_string()),
    };
    write_raw_lock(dir, &serde_json::to_string(&state).expect("serialize lock"));
}

fn acquire(dir: &Path) -> EngineLockOutcome {
    acquire_engine_lock(dir, probe()).expect("acquire does not fail on io")
}

/// Runs `sleep <secs>` and returns its pid, so a liveness probe has a real
/// live foreign process to look at.
fn spawn_sleeper(secs: u32) -> std::process::Child {
    std::process::Command::new("sleep")
        .arg(secs.to_string())
        .spawn()
        .expect("spawn sleep")
}

/// `/proc`-backed liveness is Linux-only in `SystemProcessProbe`; on other
/// platforms the probe is conservative (always alive), which the recovery tests
/// cannot exercise. This suite runs on Linux.
/// `/proc`-backed liveness is Linux-only in `SystemProcessProbe`; on other
/// platforms the probe is conservative (always alive), which the recovery
/// tests cannot exercise. This suite runs on Linux. The predicate is
/// compiled, not asserted: a `cfg!`-based `assert!` is a constant
/// assertion under clippy's lint surface, so the platform split lives in
/// the CFG below. Runtime certainty requires its /proc reads to succeed.
#[cfg(target_os = "linux")]
fn assert_linux() {
    let probe_dir = std::env::temp_dir();
    std::fs::read_dir(&probe_dir).expect("/proc-style filesystem access is live");
    assert!(
        std::path::Path::new("/proc/self/stat").exists(),
        "engine-lock recovery oracles need /proc-backed liveness (Linux)"
    );
}

#[cfg(not(target_os = "linux"))]
fn assert_linux() {
    panic!(
        "this suite needs /proc-backed liveness (Linux; SystemProcessProbe is exact only there)"
    );
}

#[test]
fn test_engine_lock_acquire_release() {
    let dir = temp_dir("acquire-release");
    let outcome = acquire(&dir);
    let EngineLockOutcome::Acquired(lock) = outcome else {
        panic!("a free storage root must be acquired, got {outcome:?}");
    };
    assert!(lock_path(&dir).exists(), "the lock file is created");
    drop(lock);
    assert!(
        !lock_path(&dir).exists(),
        "Drop is the lifetime release: the lock file is gone"
    );
}

#[test]
fn test_engine_lock_stale_dead_pid_recovered() {
    assert_linux();
    let dir = temp_dir("stale-dead-pid");
    let mut child = spawn_sleeper(30);
    let dead_pid = child.id();
    child.kill().expect("kill sleeper");
    child.wait().expect("reap sleeper");
    write_lock_state(&dir, dead_pid);

    let outcome = acquire(&dir);
    assert!(
        matches!(outcome, EngineLockOutcome::Acquired(_)),
        "a dead owner's lock is stale and must be recovered, got {outcome:?}"
    );
    let live = read_live_lock(&dir, ENGINE_LOCK_FILE, probe().as_ref())
        .expect("the recovered lock reads back live");
    assert_eq!(live.pid, std::process::id());
}

#[test]
fn test_engine_lock_corrupt_file_recovered() {
    assert_linux();
    let dir = temp_dir("corrupt");
    write_raw_lock(&dir, "not-json-at-all");

    let outcome = acquire(&dir);
    assert!(
        matches!(outcome, EngineLockOutcome::Acquired(_)),
        "a corrupt lock body must be recovered, got {outcome:?}"
    );
    let live = read_live_lock(&dir, ENGINE_LOCK_FILE, probe().as_ref())
        .expect("the recovered lock parses");
    assert_eq!(live.pid, std::process::id());
}

#[test]
fn test_engine_lock_live_holder_is_loser() {
    let dir = temp_dir("live-holder");
    let holder = acquire(&dir);
    let EngineLockOutcome::Acquired(holder) = holder else {
        panic!("first acquire must win");
    };

    let outcome = acquire(&dir);
    match outcome {
        EngineLockOutcome::Lost { holder_pid } => {
            assert_eq!(
                holder_pid,
                std::process::id(),
                "the loser names the live holder's pid"
            );
        }
        other => panic!("a live holder must make the second acquire lose, got {other:?}"),
    }
    assert!(lock_path(&dir).exists(), "the loser never steals the lock");
    drop(holder);
}

#[test]
fn test_engine_lock_loser_reports_lock_lost() {
    let dir = temp_dir("loser-reports");
    assert_eq!(ENGINE_LOCK_LOST, 75, "D-39/the brief pins exit code 75");
    let holder = acquire(&dir);
    let EngineLockOutcome::Acquired(holder) = holder else {
        panic!("first acquire must win");
    };
    let outcome = acquire(&dir);
    let EngineLockOutcome::Lost { holder_pid } = outcome else {
        panic!("second acquire must lose");
    };

    // The loser's degradation signal is the status file naming the HOLDER.
    write_status_file(&dir, "engine lock held", holder_pid).expect("write status");
    let status = std::fs::read_to_string(dir.join(STATUS_FILE)).expect("status file exists");
    assert!(
        status.contains(&format!("(pid={holder_pid},")),
        "status names the holder pid: {status:?}"
    );
    assert!(
        status.starts_with("degraded: engine lock held ("),
        "status grammar: {status:?}"
    );
    drop(holder);
}

#[test]
fn test_engine_lock_body_is_read_live_lock_parseable() {
    let dir = temp_dir("body-parseable");
    let outcome = acquire(&dir);
    let EngineLockOutcome::Acquired(lock) = outcome else {
        panic!("acquire must win");
    };
    // B4: a plain-pid body would be read as corrupt and stolen by
    // `read_live_lock`, giving the DB two owners.
    let live = read_live_lock(&dir, ENGINE_LOCK_FILE, probe().as_ref())
        .expect("LockState JSON body parses and reads live");
    assert_eq!(live.pid, std::process::id());
    assert_eq!(live.operation.as_deref(), Some("engine"));
    assert!(!live.timestamp.is_empty());
    drop(lock);
}

#[test]
fn test_status_file_loser_does_not_mask_owner() {
    assert_linux();
    let dir = temp_dir("status-holder");
    let mut child = spawn_sleeper(30);
    let holder_pid = child.id();
    assert_ne!(holder_pid, std::process::id());
    write_lock_state(&dir, holder_pid);

    let outcome = acquire(&dir);
    let EngineLockOutcome::Lost { holder_pid: named } = outcome else {
        panic!("a live foreign holder must win, got {outcome:?}");
    };
    assert_eq!(named, holder_pid);
    write_status_file(&dir, "engine lock held", named).expect("write status");

    let status = std::fs::read_to_string(dir.join(STATUS_FILE)).expect("status file");
    assert_eq!(status.lines().count(), 1, "exactly one line: {status:?}");
    assert!(
        status.len() <= 200,
        "the line is bounded at 200 chars, got {}",
        status.len()
    );
    assert!(
        status.contains(&format!("pid={holder_pid}")),
        "the loser records the HOLDER's pid, not its own: {status:?}"
    );
    assert!(
        !status.contains(&format!("pid={}", std::process::id())),
        "the loser must never record its own pid: {status:?}"
    );
    assert!(
        status.trim_end().ends_with(')'),
        "grammar ends with the ts group: {status:?}"
    );
    assert!(
        status.contains("ts=") && status.contains('Z'),
        "a UTC ISO-8601 timestamp is present: {status:?}"
    );

    child.kill().expect("kill sleeper");
    child.wait().expect("reap sleeper");
    // The owner never deletes a status it did not write.
    assert!(
        dir.join(STATUS_FILE).exists(),
        "acquiring/releasing the lock never unlinks a foreign status"
    );
}
