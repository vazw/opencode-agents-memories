//! Composition smoke tests: the root binary must print all 8 workspace
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
        "mem-core\nmem-store\nmem-embed\nmem-ai\nmem-profile\nmem-episodic\nmem-web\nmem-bridge\n"
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
