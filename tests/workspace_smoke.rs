//! Composition smoke test: the root binary must print all 8 workspace crate names.

use std::process::Command;

#[test]
fn test_binary_prints_all_workspace_crate_names() {
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .output()
        .expect("failed to run opencode-agents-memory binary");
    assert!(
        output.status.success(),
        "binary exited with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        "mem-core\nmem-store\nmem-embed\nmem-ai\nmem-profile\nmem-episodic\nmem-web\nmem-bridge\n"
    );
}
