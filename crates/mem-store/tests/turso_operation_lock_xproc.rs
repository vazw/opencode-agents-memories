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

#[cfg(target_os = "linux")]
#[test]
fn test_stale_lock_from_dead_pid_is_reclaimed() {
    for attempt in 0..3 {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn naturally exiting child");
        let status = child.wait().expect("reap child");
        let dead_pid = child.id();
        assert!(status.success(), "child status {status}");
        let target = libc::pid_t::try_from(dead_pid).expect("representable child PID");
        assert!(target > 0);
        let absent = || {
            match std::fs::metadata(format!("/proc/{dead_pid}")) {
                Ok(_) => return false,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("fixture metadata failed: {error}"),
            }
            // SAFETY: checked positive child PID; signal0 delivers no signal.
            if unsafe { libc::kill(target, 0) } == 0 {
                return false;
            }
            let error = std::io::Error::last_os_error();
            assert_eq!(error.raw_os_error(), Some(libc::ESRCH), "fixture signal0");
            true
        };
        if !absent() {
            continue;
        }
        let storage = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/scratch/issue131-fixture-r4")
            .join(format!("stale-{}-{dead_pid}-{attempt}", std::process::id()));
        std::fs::create_dir_all(&storage).expect("fixture directory");
        let path = common::write_lock_state(&storage, OPERATION_LOCK_FILE, dead_pid, Some("stale"));
        assert!(path.exists(), "stale fixture must exist before acquire");
        let result =
            acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "reclaim");
        if !absent() {
            drop(result);
            common::cleanup_temp_dir(&storage).expect("cleanup reused fixture");
            continue;
        }
        println!(
            "reaped pid={dead_pid} status={status} pre/post proc=NotFound signal0=ESRCH attempt={attempt}"
        );
        let lock = result.expect("a dead-pid lock must be reclaimed, not propagated");
        let body = std::fs::read_to_string(&path).expect("read reclaimed lock file");
        let state: LockState = serde_json::from_str(&body).expect("parse reclaimed lock file");
        assert_eq!(
            state.pid,
            std::process::id(),
            "the stale lock must be replaced by the acquiring process"
        );
        assert_ne!(state.pid, dead_pid);
        drop(lock);
        common::cleanup_temp_dir(&storage).expect("cleanup temp dir");
        return;
    }
    panic!("fixture unavailable: all three reaped PIDs reused");
}

#[test]
fn test_unrepresentable_holder_pid_is_not_reclaimed() {
    let storage = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/scratch/issue131-fixture-r4")
        .join(format!("invalid-{}", std::process::id()));
    std::fs::create_dir_all(&storage).expect("fixture directory");
    let path = common::write_lock_state(&storage, OPERATION_LOCK_FILE, u32::MAX, None);
    let bytes = std::fs::read(&path).expect("original lock bytes");
    let holder = read_live_lock(&storage, OPERATION_LOCK_FILE, &SystemProcessProbe);
    assert!(
        holder.is_some(),
        "unrepresentable holder lock must be retained"
    );
    assert_eq!(holder.expect("retained holder").pid, u32::MAX);
    assert_eq!(std::fs::read(&path).expect("retained lock bytes"), bytes);
    let error = acquire_turso_operation_lock(&storage, Arc::new(SystemProcessProbe), "invalid")
        .err()
        .expect("invalid holder must block acquisition");
    assert!(matches!(
        error,
        StoreError::MigrationLocked { pid: u32::MAX, .. }
    ));
    assert_eq!(std::fs::read(&path).expect("blocked lock bytes"), bytes);
    common::cleanup_temp_dir(&storage).expect("cleanup fixture");
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
