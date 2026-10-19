//! Multiprocess WAL — port of upstream `tests/turso-multiprocess.test.ts`
//! (blueprint W1.5 step 6, lane sL).
//!
//! Scaffolded empty by the captain (step 2); the lane fills it RED-first.
//!
//! Cases 1–2 are pure value assertions over the `ConnectOptions` /
//! open-error seams (no runtime platform gate). Case 3 pins the real
//! cross-process handshake: two `lock_holder` example processes with
//! overlapping hold windows must SERIALIZE through the operation lock's
//! bounded retry budget (`--wait-ms`), leaving no lock file behind.

mod common;

use std::path::Path;
use std::time::Duration;

use mem_store::db::{
    ConnectOptions, ExperimentalFeature, is_multi_process_lock_error, wrap_turso_open_error,
};
use mem_store::operation_lock::OPERATION_LOCK_FILE;
use mem_store::platform::Platform;

#[test]
fn test_connect_options_enable_encryption_and_multiprocess_wal_on_unix() {
    let unix = ConnectOptions::for_config(&common::store_config("mp-opts"), Platform::Unix);
    assert!(
        unix.experimental.contains(&ExperimentalFeature::Encryption),
        "encryption flag is always on"
    );
    assert!(
        unix.experimental
            .contains(&ExperimentalFeature::MultiprocessWal),
        "multiprocess WAL must be on for Unix"
    );

    let windows = ConnectOptions::for_config(&common::store_config("mp-opts"), Platform::Windows);
    assert!(
        windows
            .experimental
            .contains(&ExperimentalFeature::Encryption),
        "encryption flag is always on"
    );
    assert!(
        !windows
            .experimental
            .contains(&ExperimentalFeature::MultiprocessWal),
        "multiprocess WAL must stay off for Windows"
    );
}

#[test]
fn test_wrap_turso_open_error_classifies_multi_process_lock() {
    let message = "failed to open database /tmp/x.db: Locking error: \
                   Failed locking file. File is locked by another process";
    assert!(
        is_multi_process_lock_error(message),
        "the upstream lock fragments must be recognized"
    );
    assert!(
        !is_multi_process_lock_error("unrelated failure"),
        "non-lock text must not be classified as a lock error"
    );

    let wrapped = wrap_turso_open_error(Path::new("/tmp/x.db"), message, Platform::Unix, false);
    let text = wrapped.message();
    assert!(
        text.contains("Failed to open database /tmp/x.db"),
        "wrapper must name the database path: {text}"
    );
    assert!(
        text.contains("Another OpenCode session") || text.contains("multiprocess_wal"),
        "wrapper must carry the unix lock hint: {text}"
    );
}

#[test]
fn test_two_concurrent_processes_serialize_via_operation_lock() {
    // Flake detector: two overlapping holders must serialize over 10 rounds.
    const ITERATIONS: usize = 10;

    let storage = common::temp_dir("mp-serialize");
    let path = storage.join(OPERATION_LOCK_FILE);
    let mut completed = 0usize;

    for iteration in 0..ITERATIONS {
        // Both holders budget 3000ms of re-attempts, each holds for 200ms:
        // the loser of the first race retries until the winner releases.
        let mut first = common::spawn_lock_holder(&storage, 200, 3000);
        let mut second = common::spawn_lock_holder(&storage, 200, 3000);

        let status_first = first.wait(Duration::from_secs(10)).unwrap_or_else(|| {
            panic!(
                "iteration {iteration}: first holder did not exit (stderr: {})",
                first.captured_stderr()
            )
        });
        let status_second = second.wait(Duration::from_secs(10)).unwrap_or_else(|| {
            panic!(
                "iteration {iteration}: second holder did not exit (stderr: {})",
                second.captured_stderr()
            )
        });

        assert!(
            status_first.success(),
            "iteration {iteration}: first holder exited {status_first:?} (stderr: {})",
            first.captured_stderr()
        );
        assert!(
            status_second.success(),
            "iteration {iteration}: second holder exited {status_second:?} (stderr: {})",
            second.captured_stderr()
        );
        assert!(
            !path.exists(),
            "iteration {iteration}: operation lock file left behind after both holders exited"
        );

        completed += 1;
    }

    assert_eq!(completed, ITERATIONS, "every iteration must pass");
    println!("serialization iterations completed: {completed}/{ITERATIONS}");
    common::cleanup_temp_dir(&storage).expect("cleanup temp dir");
}
