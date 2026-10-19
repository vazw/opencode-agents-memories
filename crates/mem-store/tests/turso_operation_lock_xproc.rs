//! Cross-process operation lock — W1.5 step 5, lane sL.
//!
//! Scaffolded empty by the captain (step 2); the lane fills it RED-first.
//!
//! House law (`docs/agents/testing.md`): hermetic temp dirs, injected seams,
//! no dev-dependencies, no sleeps for synchronization. Cross-process behaviour
//! is exercised by spawning the `lock_holder` example binary (built with
//! `cargo build -p mem-store --example lock_holder`) and waiting on it with an
//! explicit timeout — a hung holder fails fast instead of hanging the suite.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mem_store::error::StoreError;
use mem_store::operation_lock::{
    LockState, OPERATION_LOCK_FILE, acquire_turso_operation_lock, read_live_lock,
};
use mem_store::platform::SystemProcessProbe;

/// Every "stale owner" fixture uses `u32::MAX`: on Linux `pid_max` never
/// reaches it, so the pid cannot be allocated even on a long-running host
/// (the blueprint's literal `999999` may well be alive there).
const DEAD_PID: u32 = u32::MAX;

fn lock_path(storage: &Path) -> PathBuf {
    storage.join(OPERATION_LOCK_FILE)
}

#[test]
fn test_read_live_lock_unlinks_corrupt_and_empty_lock_files() {
    let fixtures: [&[u8]; 3] = [b"not json at all {{{", b"", b"   \n\t  "];
    for (index, bytes) in fixtures.iter().enumerate() {
        let storage = common::temp_dir(&format!("oplock-corrupt-{index}"));
        let path = lock_path(&storage);
        std::fs::write(&path, bytes).expect("write corrupt lock");

        let state = read_live_lock(&storage, OPERATION_LOCK_FILE, &SystemProcessProbe);

        assert!(
            state.is_none(),
            "corrupt/empty lock must read as None (fixture {index}, bytes {bytes:?})"
        );
        assert!(
            !path.exists(),
            "corrupt/empty lock must be unlinked (fixture {index}, bytes {bytes:?})"
        );
        common::cleanup_temp_dir(&storage).expect("cleanup temp dir");
    }
}

#[test]
fn test_acquire_refuses_while_live_holder_present() {
    // Flake detector: the refusal/release handshake must hold over 20 rounds.
    const ITERATIONS: usize = 20;

    let storage = common::temp_dir("oplock-live-holder");
    let path = lock_path(&storage);
    let mut completed = 0usize;

    for iteration in 0..ITERATIONS {
        // `wait_ms = 0`: the holder refuses immediately if it cannot acquire.
        let mut holder = common::spawn_lock_holder(&storage, 300, 0);
        assert!(
            common::wait_for_path(&path, Duration::from_secs(5)),
            "iteration {iteration}: holder never wrote the operation lock file"
        );

        let error = acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "test")
            .err()
            .unwrap_or_else(|| {
                panic!(
                    "iteration {iteration}: acquire must be refused while a live holder owns the lock"
                )
            });
        assert!(
            matches!(error, StoreError::MigrationLocked { .. }),
            "iteration {iteration}: expected MigrationLocked, got {error:?}"
        );
        assert!(
            error.to_string().contains("writes are temporarily blocked"),
            "iteration {iteration}: refusal text missing upstream phrase: {error}"
        );

        let status = holder
            .wait(Duration::from_secs(5))
            .unwrap_or_else(|| panic!("iteration {iteration}: holder did not exit within 5s"));
        assert!(
            status.success(),
            "iteration {iteration}: holder exited {status:?} (stderr: {})",
            holder.captured_stderr()
        );

        let lock = acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "test")
            .unwrap_or_else(|error| {
                panic!("iteration {iteration}: fresh acquire after holder release failed: {error}")
            });
        drop(lock);
        assert!(
            !path.exists(),
            "iteration {iteration}: lock file left behind after release"
        );

        completed += 1;
    }

    assert_eq!(completed, ITERATIONS, "every iteration must pass");
    println!("cross-process live-holder refusals completed: {completed}/{ITERATIONS}");
    common::cleanup_temp_dir(&storage).expect("cleanup temp dir");
}

#[test]
fn test_stale_lock_from_dead_pid_is_reclaimed() {
    let storage = common::temp_dir("oplock-stale");
    let path = common::write_lock_state(&storage, OPERATION_LOCK_FILE, DEAD_PID, Some("stale"));
    assert!(path.exists(), "stale fixture must exist before acquire");

    let lock = acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "reclaim")
        .expect("a dead-pid lock must be reclaimed, not propagated");

    let body = std::fs::read_to_string(&path).expect("read reclaimed lock file");
    let state: LockState = serde_json::from_str(&body).expect("parse reclaimed lock file");
    assert_eq!(
        state.pid,
        std::process::id(),
        "the stale lock must be replaced by the acquiring process"
    );
    assert_ne!(state.pid, DEAD_PID);

    drop(lock);
    common::cleanup_temp_dir(&storage).expect("cleanup temp dir");
}

#[test]
fn test_acquire_creates_lock_with_operation_and_pid() {
    let storage = common::temp_dir("oplock-content");
    let lock = acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "unit-op")
        .expect("acquire over a fresh storage dir");

    let body = std::fs::read_to_string(lock_path(&storage)).expect("read lock file");
    let state: LockState = serde_json::from_str(&body).expect("lock file is valid LockState JSON");
    assert_eq!(state.pid, std::process::id());
    assert_eq!(state.operation.as_deref(), Some("unit-op"));
    assert!(
        !state.timestamp.is_empty(),
        "lock content must carry a timestamp"
    );

    drop(lock);
    common::cleanup_temp_dir(&storage).expect("cleanup temp dir");
}

#[test]
fn test_release_removes_owned_lock_and_drop_fallback() {
    let storage = common::temp_dir("oplock-release");
    let path = lock_path(&storage);

    let lock = acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "release")
        .expect("acquire");
    assert!(path.exists(), "acquire must create the lock file");
    lock.release();
    assert!(
        !path.exists(),
        "explicit release must remove the owned lock"
    );

    {
        let _fallback =
            acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "drop-fallback")
                .expect("re-acquire");
        assert!(
            path.exists(),
            "lock file must exist while the handle is alive"
        );
    }
    assert!(
        !path.exists(),
        "Drop must remove the owned lock even without an explicit release"
    );

    common::cleanup_temp_dir(&storage).expect("cleanup temp dir");
}
