#![allow(dead_code)]

//! Std-only helpers shared by mem-core integration tests.
//!
//! Everything is hermetic: temp dirs live under `std::env::temp_dir()` only
//! and process environment is never mutated (tests inject `BTreeMap` envs).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Creates a fresh unique temp directory: `opencode-mem-test-<label>-<pid>-<counter>`.
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

/// Child-mode selector set when re-execing the test binary in child mode.
pub const FIXTURE_CHILD_ENV: &str = "MEM_FIXTURE_CHILD";

/// Child output-file path (evidence file / stderr+stdout redirection base).
pub const FIXTURE_OUT_ENV: &str = "MEM_FIXTURE_OUT";

/// The child entry point's exact test name — the helper hardcodes it, so
/// every crate copying the recipe MUST name its child entry test exactly
/// this.
pub const FIXTURE_CHILD_TEST_NAME: &str = "fixture_child_entry";

/// Fixture-translation recipe helper: spawns this test binary via self
/// re-exec (`current_exe()` + `--exact fixture_child_entry --ignored`),
/// selects the child mode with `FIXTURE_CHILD_ENV=mode`, passes the
/// output-file path through `FIXTURE_OUT_ENV` plus `extra_env`, redirects
/// stdout/stderr to `<out_path>.stdout` / `<out_path>.stderr` (files, never
/// pipes — a chatty child cannot deadlock the parent), polls `try_wait()`
/// every 10 ms until `timeout`, then `kill()` + `wait()` on deadline and
/// returns a `TimedOut` error (no orphan processes, bounded wall time).
pub fn run_fixture_child(
    mode: &str,
    out_path: &Path,
    extra_env: &[(&str, &str)],
    timeout: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let stdout_path = out_path.with_extension("stdout");
    let stderr_path = out_path.with_extension("stderr");
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", FIXTURE_CHILD_TEST_NAME, "--ignored"])
        .env(FIXTURE_CHILD_ENV, mode)
        .env(FIXTURE_OUT_ENV, out_path)
        .envs(extra_env.iter().copied())
        .stdout(Stdio::from(fs::File::create(&stdout_path)?))
        .stderr(Stdio::from(fs::File::create(&stderr_path)?))
        .spawn()?;

    let deadline_started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if deadline_started.elapsed() > timeout {
            child.kill()?;
            child.wait()?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "fixture child exceeded the bounded deadline and was killed",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
