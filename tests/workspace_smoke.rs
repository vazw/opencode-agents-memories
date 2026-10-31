//! Composition smoke tests: the root binary must print all 12 workspace
//! crate names via `--mode=print-crates` (M31) AND answer `--version` with
//! the pinned string (E4: `opencode-agents-memory 0.1.0 (bridge 1.0.0)`).
//! The r8 wave keeps both assertions (add, never replace).

use std::process::{Command, Stdio};

fn run_mode(mode: &str) -> Result<(String, String, i32), String> {
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(mode)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("spawn failed: {error}"))?;
    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    ))
}

#[test]
fn test_binary_prints_all_workspace_crate_names() {
    let (stdout, stderr, code) =
        run_mode("--mode=print-crates").expect("the binary runs under --mode=print-crates");
    assert_eq!(code, 0, "binary exited {code}: {stderr}");
    assert_eq!(
        stdout,
        "mem-core\nmem-store\nmem-embed\nmem-ai\nmem-profile\nmem-episodic\nmem-facts\nmem-vault\nmem-skills\nmem-portability\nmem-web\nmem-bridge\n"
    );
}

#[test]
fn test_binary_reports_version_string() {
    let (stdout, stderr, code) = run_mode("--version").expect("the binary runs under --version");
    assert_eq!(code, 0, "binary exited {code}: {stderr}");
    assert_eq!(
        stdout.trim_end(),
        format!(
            "opencode-agents-memory {} (bridge 1.0.0)",
            env!("CARGO_PKG_VERSION")
        ),
        "the pinned version line (E4)"
    );
}

/// W7.1/M12 — the MCP refusal must happen BEFORE `Config::load_default`.
///
/// The child gets `env_clear()` plus a scratch `HOME` whose
/// `opencode-mem.jsonc` is deliberately BROKEN: if the binary loaded config
/// first it would exit 70 with a config error. Exit 78 with
/// `mcp.mode_unavailable` is the proof the refusal precedes the config read.
fn run_mcp_mode(mode: &str) -> (String, i32) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let home = std::env::temp_dir().join(format!(
        "opencode-mem-smoke-mcp-{}-{nanos}",
        std::process::id()
    ));
    let config_dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("opencode-mem.jsonc"),
        "{ this is not valid json",
    )
    .expect("write broken config");

    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--mode={mode}"))
        .env_clear()
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("spawn the binary");
    let _ = std::fs::remove_dir_all(&home);
    (
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn test_binary_refuses_mcp_modes_before_loading_config() {
    for mode in ["mcp-stdio", "mcp-http"] {
        let (stderr, code) = run_mcp_mode(mode);
        assert_eq!(code, 78, "{mode} must exit 78, stderr: {stderr}");
        assert!(
            stderr.contains(r#""event":"mcp.mode_unavailable""#),
            "{mode}: {stderr}"
        );
        assert!(
            stderr.contains(&format!(r#""mode":"{mode}""#)),
            "{mode}: {stderr}"
        );
        assert!(
            !stderr.contains("config load failed"),
            "{mode} must refuse BEFORE config load: {stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// `--help`/`-h` and unknown flags are usage, never an engine start (§6.4)
// ---------------------------------------------------------------------------

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "opencode-mem-smoke-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Runs the binary with a scratch `HOME` AND a scratch `--storage-root`, so a
/// binary that (wrongly) falls through into engine mode can never reach the
/// live store — the exact hazard `--help` carried: it was "ignored"
/// (`src/main.rs` `parse_args`), became a real stdio engine on the DEFAULT
/// root, lost the lock and wrote a bogus `degraded` status line into the live
/// store.
fn run_cli(flag: &str, root: &std::path::Path) -> (String, String, i32) {
    let home = scratch_dir("cli-home");
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(flag)
        .arg("--storage-root")
        .arg(root)
        .env_clear()
        .env("HOME", &home)
        .env("OPENCODE_MEM_STORAGE_ROOT", root)
        .stdin(Stdio::null())
        .output()
        .expect("spawn the binary");
    let _ = std::fs::remove_dir_all(&home);
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn test_help_prints_usage_and_exits_zero() {
    for flag in ["--help", "-h"] {
        let root = scratch_dir("help-root").join("storage");
        let (stdout, stderr, code) = run_cli(flag, &root);
        assert_eq!(code, 0, "{flag} must exit 0, stderr: {stderr}");
        assert!(
            stdout.contains("usage: opencode-agents-memory"),
            "{flag} must print the usage block on stdout, got: {stdout:?} / {stderr:?}"
        );
        assert!(
            !root.exists(),
            "{flag} must never start an engine (the root stays uncreated)"
        );
    }
}

#[test]
fn test_unknown_flag_exits_two_without_starting_an_engine() {
    let root = scratch_dir("unknown-flag-root").join("storage");
    let (stdout, stderr, code) = run_cli("--definitely-not-a-flag", &root);
    assert_eq!(
        code, 2,
        "an unknown flag is a usage error, not an engine start: {stdout:?} / {stderr:?}"
    );
    assert!(
        stderr.contains("--definitely-not-a-flag"),
        "the refusal names the offending flag: {stderr:?}"
    );
    assert!(
        !root.exists(),
        "the refused invocation never creates the root"
    );
}

/// The unknown-flag refusal is not mode-dependent: `--mode=print-crates`
/// returns synchronously BEFORE any engine exists, and it used to return
/// before the refusal too, so `--mode=print-crates --nope` exited 0 and said
/// nothing. An unknown token is a usage error in every mode.
#[test]
fn test_unknown_flag_after_a_sync_mode_exits_two() {
    let root = scratch_dir("unknown-flag-mode-root").join("storage");
    let home = scratch_dir("unknown-flag-mode-home");
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .args([
            "--mode=print-crates",
            "--definitely-not-a-flag",
            "--storage-root",
        ])
        .arg(&root)
        .env_clear()
        .env("HOME", &home)
        .env("OPENCODE_MEM_STORAGE_ROOT", &root)
        .stdin(Stdio::null())
        .output()
        .expect("spawn the binary");
    let _ = std::fs::remove_dir_all(&home);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.code(),
        Some(2),
        "print-crates must not swallow an unknown flag: {stdout:?} / {stderr:?}"
    );
    assert!(
        stdout.is_empty(),
        "a refused invocation prints no crate names: {stdout:?}"
    );
    assert!(
        stderr.contains("--definitely-not-a-flag"),
        "the refusal names the offending flag: {stderr:?}"
    );
    assert!(
        !root.exists(),
        "the refused invocation never creates the root"
    );
}
