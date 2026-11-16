#![cfg(not(feature = "shard-consolidation"))]
//! Off-build behavioural suite: every `consolidate-*` invocation is refused
//! with exit 69 (`EX_UNAVAILABLE`) and the `consolidation.feature_absent`
//! event BEFORE any config load, root access or reservation side effect
//! (blueprint r2 Decision 1 / test plan). Compiles to an empty, passing
//! suite under default features.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Fresh unique scratch dir (hermeticity: `std::env::temp_dir()` only,
/// docs/agents/testing.md).
fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "consolidation-off-{prefix}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Spawn result: exit code plus both captured streams.
struct RunOutcome {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Spawns the binary with `args` plus `--storage-root <storage_root>`,
/// `HOME` set to `home`, `stdin` nulled, both streams captured. `home` and
/// `storage_root` are required parameters (no defaults) so a mode invocation
/// can never fall back to the live `$HOME/.opencode-mem` root.
fn run_mode(home: &Path, storage_root: &Path, args: &[&str]) -> RunOutcome {
    spawn_with_home(Some(home), storage_root, args)
}

/// Same seam with `HOME` removed from the child env entirely: an intercept
/// moved below `Config::load_default` answers 70 (`MissingHomeDirectory`)
/// instead of 69. Live-safe by construction — no home, no root.
fn run_mode_no_home(storage_root: &Path, args: &[&str]) -> RunOutcome {
    spawn_with_home(None, storage_root, args)
}

fn spawn_with_home(home: Option<&Path>, storage_root: &Path, args: &[&str]) -> RunOutcome {
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command.args(args);
    command.arg(format!("--storage-root={}", storage_root.display()));
    match home {
        Some(home) => {
            command.env("HOME", home);
        }
        None => {
            command.env_remove("HOME");
        }
    }
    // Never inherit operator env that could redirect the root or fence the
    // refusal path.
    command.env_remove("OPENCODE_MEM_STORAGE_ROOT");
    command.env_remove("OPENCODE_MEM_MAINTENANCE_NONCE");
    // A stdio-engine fall-through must exit, never block on inherited stdin.
    command.stdin(Stdio::null());
    let output: Output = command.output().expect("binary spawns");
    RunOutcome {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Criterion 1: a consolidate mode on an off build answers 69 with the
/// `consolidation.feature_absent` event and the feature name, and never
/// touches the storage root (the empty isolated dir kills any root-side
/// work: no root, no reservation, no `recovery/`).
#[test]
fn test_consolidate_mode_refused_69_feature_absent() {
    let scratch = unique_temp_dir("refused-69");
    let home = scratch.join("home");
    let isolated = scratch.join("isolated");
    std::fs::create_dir_all(&isolated).expect("create isolated parent");
    let never_created = isolated.join("never-created");
    let plan = scratch.join("x.json");
    let plan_arg = format!("--plan={}", plan.display());
    let outcome = run_mode(
        &home,
        &never_created,
        &["--mode=consolidate-shards", plan_arg.as_str()],
    );
    assert_eq!(
        outcome.code, 69,
        "exit 69 expected; stderr: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("consolidation.feature_absent"),
        "event line missing; stderr: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("shard-consolidation"),
        "feature name missing from refusal; stderr: {}",
        outcome.stderr
    );
    let leftovers = std::fs::read_dir(&isolated).expect("read isolated dir");
    assert_eq!(
        leftovers.count(),
        0,
        "the off build must not touch the storage root"
    );
}

/// Criterion 2: the whole `consolidate-*` namespace is refused by prefix —
/// a real member an exact-name mutant would miss
/// (`consolidate-staged-verify`) AND a nonsense member
/// (`consolidate-not-a-mode`) both answer 69.
#[test]
fn test_consolidate_namespace_refused_prefix_form() {
    let scratch = unique_temp_dir("prefix");
    let home = scratch.join("home");
    let root = scratch.join("root-never-created");
    for mode in ["consolidate-staged-verify", "consolidate-not-a-mode"] {
        let arg = format!("--mode={mode}");
        let outcome = run_mode(&home, &root, &[arg.as_str()]);
        assert_eq!(
            outcome.code, 69,
            "mode {mode} must answer 69; stderr: {}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("consolidation.feature_absent"),
            "event line missing for {mode}; stderr: {}",
            outcome.stderr
        );
    }
}

/// Criterion 3: the intercept must not over-match — a non-consolidate
/// unknown mode keeps the pre-existing usage refusal (64).
#[test]
fn test_unknown_mode_still_exit_64() {
    let scratch = unique_temp_dir("unknown-64");
    let home = scratch.join("home");
    let root = scratch.join("root-never-created");
    let outcome = run_mode(&home, &root, &["--mode=nope"]);
    assert_eq!(
        outcome.code, 64,
        "unknown mode must keep exit 64; stderr: {}",
        outcome.stderr
    );
}

/// Criterion 4: the refusal precedes `Config::load_default` — with HOME
/// removed the off build still answers 69 (a moved intercept answers 70 via
/// `MissingHomeDirectory`).
#[test]
fn test_refusal_precedes_config_load() {
    let scratch = unique_temp_dir("no-home");
    let plan = scratch.join("x.json");
    let plan_arg = format!("--plan={}", plan.display());
    let root = scratch.join("root-never-created");
    let outcome = run_mode_no_home(&root, &["--mode=consolidate-shards", plan_arg.as_str()]);
    assert_eq!(
        outcome.code, 69,
        "expected 69 without HOME (70 would mean the intercept sits below \
         Config::load_default); stderr: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("consolidation.feature_absent"),
        "event line missing; stderr: {}",
        outcome.stderr
    );
}

/// Criterion 5: the ungated composition root still works — `--version`
/// exits 0 and `--mode=print-crates` exits 0 with the 12 workspace crates.
#[test]
fn test_off_build_sync_modes_functional() {
    let scratch = unique_temp_dir("sync");
    let home = scratch.join("home");
    let root = scratch.join("root-never-created");
    let version = run_mode(&home, &root, &["--version"]);
    assert_eq!(
        version.code, 0,
        "--version must exit 0; stderr: {}",
        version.stderr
    );
    let crates = run_mode(&home, &root, &["--mode=print-crates"]);
    assert_eq!(
        crates.code, 0,
        "print-crates must exit 0; stderr: {}",
        crates.stderr
    );
    let lines: Vec<&str> = crates
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        12,
        "print-crates prints the 12 workspace crates; stdout: {:?}",
        lines
    );
}
