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
