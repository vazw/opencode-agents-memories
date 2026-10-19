#![allow(dead_code)]

//! Std-only helpers shared by the `mem-ai` privacy integration tests.
//!
//! Everything is hermetic: temp dirs live under `std::env::temp_dir()` only and
//! the process environment is never mutated (tests inject maps as `EnvSource`).
//!
//! The `#![allow(dead_code)]` header is load-bearing: this module is
//! instantiated once per mounting file (`#[path = "common/mod.rs"] mod common;`
//! inside each `tests/privacy/*.rs`), so helpers used by only some of them are
//! `dead_code` in the others and would otherwise fail the close-out
//! `-D warnings` gate.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Creates a fresh unique temp directory:
/// `opencode-mem-test-<label>-<pid>-<counter>`.
pub fn unique_temp_dir(label: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "opencode-mem-test-{label}-{}-{id}",
        std::process::id()
    ));
    // A leftover directory with the same name (aborted earlier run) must not
    // carry stale files into the test.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("failed to create unique temp dir");
    dir
}

/// Writes `contents` to `<dir>/<name>`, creating any missing parent
/// directories (names may contain path separators).
pub fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("failed to create parent dirs for test file");
    }
    fs::write(&path, contents).expect("failed to write test file");
    path
}

/// Sets POSIX permission bits explicitly, so the process `umask` never decides
/// the fixture's mode.
#[cfg(unix)]
pub fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("failed to chmod fixture");
}

/// Non-unix no-op: POSIX mode bits are not meaningful there.
#[cfg(not(unix))]
pub fn chmod(_path: &Path, _mode: u32) {}

/// True when this process can read a `0o000` file, i.e. it runs with root or
/// `CAP_DAC_OVERRIDE` semantics.
///
/// On such a process no fixture mode can express a read denial, so the
/// mode-denial tests print `SKIP: process bypasses file modes` and return
/// instead of asserting a false invariant. The pure `group_or_other_bits`
/// predicate tests still run everywhere.
pub fn process_bypasses_file_modes() -> bool {
    let dir = unique_temp_dir("dac-probe");
    let path = write_file(&dir, "probe", "x");
    chmod(&path, 0o000);
    let bypasses = fs::File::open(&path).is_ok();
    let _ = fs::remove_dir_all(&dir);
    bypasses
}
