//! Integration test verifying startup inert configuration warnings (Issue 128).
//!
//! Spawns the binary subprocess with temporary homes containing either inert
//! configuration keys or clean configurations, asserting structured_stderr output.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mem_store::serde_json::{self, Value};

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "startup-inert-{prefix}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_config(home: &Path, content: &str) {
    let config_dir = home.join(".config/opencode");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(config_dir.join("opencode-mem.jsonc"), content).expect("write config file");
}

fn run_engine(home: &Path) -> (std::process::ExitStatus, String, String) {
    let root = temp_dir("root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        .arg(format!("--storage-root={}", root.display()))
        .arg("--web-port=0")
        .env("HOME", home)
        .env("OPENCODE_MEM_WEB_HOST", "127.0.0.1")
        .env("OPENCODE_MEM_LOG_FILE", root.join("opencode-mem.log"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().expect("engine spawns");
    if let Some(mut stdin) = child.stdin.take() {
        let req = serde_json::json!({
            "id": "shutdown-1",
            "jsonrpc": "2.0",
            "method": "shutdown",
            "params": {}
        });
        let _ = writeln!(stdin, "{}", req);
        let _ = stdin.flush();
    }

    let output = child.wait_with_output().expect("wait for engine child");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status, stdout, stderr)
}

#[test]
fn test_startup_warning_emitted_on_inert_keys() {
    let home = temp_dir("home-inert");
    write_config(
        &home,
        r#"{
            "similarityThreshold": 0.8,
            "maxProfileItems": 5
        }"#,
    );

    let (status, _stdout, stderr) = run_engine(&home);
    assert!(status.success(), "engine should exit successfully");

    let inert_event = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|json| json.get("event").and_then(Value::as_str) == Some("config.inert_keys"));

    let event = inert_event.expect("expected config.inert_keys structured_stderr event");
    assert_eq!(event.get("level").and_then(Value::as_str), Some("warn"));
    assert_eq!(
        event.get("component").and_then(Value::as_str),
        Some("engine")
    );
    assert_eq!(event.get("count").and_then(Value::as_u64), Some(2));

    let keys: Vec<&str> = event
        .get("inert_keys")
        .and_then(Value::as_array)
        .expect("inert_keys array")
        .iter()
        .filter_map(Value::as_str)
        .collect();

    assert_eq!(keys, vec!["maxProfileItems", "similarityThreshold"]);
}

#[test]
fn test_no_startup_warning_on_clean_config() {
    let home = temp_dir("home-clean");
    write_config(
        &home,
        r#"{
            "embeddingModel": "Xenova/nomic-embed-text-v1"
        }"#,
    );

    let (status, _stdout, stderr) = run_engine(&home);
    assert!(status.success(), "engine should exit successfully");

    let inert_event = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|json| json.get("event").and_then(Value::as_str) == Some("config.inert_keys"));

    assert!(
        inert_event.is_none(),
        "clean config should not emit config.inert_keys event, found: {:?}",
        inert_event
    );
}
