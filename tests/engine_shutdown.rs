//! Root-package spawn oracles (W5.1 integration lane; M4/M13/M19/M20/M21/M22/M23, E3/E4).
//!
//! `CARGO_BIN_EXE_opencode-agents-memory` is defined ONLY for the package
//! that declares the binary — this is why the process-level exit-code-75
//! oracle lives HERE, not in `crates/mem-bridge/tests/` (M13). Library-level
//! oracles live in `crates/mem-bridge/tests/shutdown.rs` and
//! `crates/mem-store/tests/engine_lock.rs`.
//!
//! Signal escalation uses `std::process::Command::new("kill")` — no root
//! dev-dependency (the root `Cargo.toml` is frozen; `Child::kill()` is
//! SIGKILL only, E3). The suite runs SERIALLY via a shared mutex: each test
//! spawns live engine processes; a panic mid-test would otherwise leak them
//! (and a /proc-based orphan probe would see a previous test's stragglers).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mem_store::serde_json;

/// The version line the packaged binary prints (E4; asserted, never re-pinned).
const VERSION_LINE: &str = "opencode-agents-memory 0.1.0 (bridge 1.0.0)";

/// The serial harness: every spawning test takes this for its whole body.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    let mutex = SERIAL.get_or_init(|| Mutex::new(()));
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("root-w51-{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// An engine child that is KILLED when this guard drops — a panic mid-test
/// never leaks a live engine into the next test's /proc probes.
struct Guarded {
    child: Option<Child>,
}

impl Guarded {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("child alive")
    }

    fn into_inner(mut self) -> Child {
        self.child.take().expect("child alive")
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Spawns the engine in stdio mode with a fresh storage root and its own
/// web port (no cross-test race over 3742).
fn spawn_engine(root: &Path, extra_env: &[(&str, &str)]) -> Guarded {
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        .arg(format!("--storage-root={}", root.display()))
        .arg(format!("--web-port={}", free_port()))
        .env("OPENCODE_MEM_WEB_HOST", DEFAULT_WEB_HOST)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    Guarded::new(command.spawn().expect("engine spawns"))
}

/// Sends one request line, returns the response line.
fn round_trip(stdin: &mut impl Write, stdout: &mut impl Read, line: &str) -> String {
    stdin.write_all(line.as_bytes()).expect("write request");
    // NDJSON framing (docs/bridge-schema.md:21): one message per line,
    // terminated by EXACTLY one '\n'. Without this byte the engine's
    // read_line never completes a frame and both sides block forever.
    if !line.ends_with('\n') {
        stdin.write_all(b"\n").expect("write NDJSON terminator");
    }
    stdin.flush().expect("flush request");
    let mut response = String::new();
    let mut buffer = [0u8; 1];
    loop {
        let read = stdout.read(&mut buffer).expect("read response");
        assert!(read > 0, "stdout hit EOF before a response");
        response.push(buffer[0] as char);
        if response.ends_with('\n') {
            break;
        }
    }
    response
}

/// Waits for exit within `bound`, returning the status.
fn wait_bounded(child: &mut Child, bound: Duration) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        assert!(
            started.elapsed() < bound,
            "process did not exit within {bound:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// /proc orphan probe: SURVIVING processes whose cmdline contains the marker.
fn find_surviving(marker: &str) -> Vec<u32> {
    let mut survivors = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return survivors;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(cmdline) = std::fs::read_to_string(entry.path().join("cmdline")) else {
            continue;
        };
        if cmdline.contains(marker) {
            survivors.push(name.parse::<u32>().expect("pid digits"));
        }
    }
    survivors
}

const HELLO_LINE: &str = r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}"#;
const DEFAULT_WEB_HOST: &str = "127.0.0.1";

// ---------------------------------------------------------------------------
// stdin-EOF + process shutdown (M19/M22/M21, E3)
// ---------------------------------------------------------------------------

#[test]
fn test_engine_exits_on_stdin_eof() {
    let _lock = serial();
    let root = temp_dir("stdin-eof");
    let spawn_guard = spawn_engine(&root, &[]);
    let mut child = spawn_guard.into_inner();
    let _hello = round_trip(
        child.stdin.as_mut().expect("stdin"),
        child.stdout.as_mut().expect("stdout"),
        HELLO_LINE,
    );
    // EOF: drop the parent's write end.
    drop(child.stdin.take());
    let status = wait_bounded(&mut child, Duration::from_secs(6));
    assert_eq!(
        status.code(),
        Some(0),
        "EOF is a clean exit (web auxiliary)"
    );
    // DB flushed + lock released: the lifetime lock file is gone after exit.
    assert!(
        !root.join(".engine.lock").exists(),
        "the lock releases with the engine's life"
    );
}

#[test]
fn test_shutdown_graceful_within_bounds() {
    let _lock = serial();
    let root = temp_dir("graceful");
    let spawn_guard = spawn_engine(&root, &[]);
    let mut child = spawn_guard.into_inner();
    let (mut stdin, mut stdout) = (
        child.stdin.take().expect("stdin"),
        child.stdout.take().expect("stdout"),
    );
    round_trip(&mut stdin, &mut stdout, HELLO_LINE);
    let started = Instant::now();
    let response = round_trip(
        &mut stdin,
        &mut stdout,
        r#"{"id":"s-1","jsonrpc":"2.0","method":"shutdown","params":{"session_id":"sess-a"}}"#,
    );
    assert!(
        response.contains("\"ok\":true"),
        "shutdown acked: {response}"
    );
    let status = wait_bounded(&mut child, Duration::from_secs(4));
    assert!(
        started.elapsed() <= Duration::from_millis(2200),
        "exited within the 2 s D-45 total + tolerance: {:?}",
        started.elapsed()
    );
    assert_eq!(status.code(), Some(0));
    assert!(
        !root.join(".engine.lock").exists(),
        "graceful shutdown released the lock"
    );
}

#[test]
fn test_shutdown_sigterm_fallback() {
    let _lock = serial();
    let root = temp_dir("sigterm-fallback");
    let spawn_guard = spawn_engine(
        &root,
        &[
            ("OPENCODE_MEM_HANG_ON_SHUTDOWN", "1"),
            ("OPENCODE_MEM_MODE", "hang-on-shutdown"),
        ],
    );
    let mut child = spawn_guard.into_inner();
    let (mut stdin, mut stdout) = (
        child.stdin.take().expect("stdin"),
        child.stdout.take().expect("stdout"),
    );
    round_trip(&mut stdin, &mut stdout, HELLO_LINE);
    round_trip(
        &mut stdin,
        &mut stdout,
        r#"{"id":"s-1","jsonrpc":"2.0","method":"shutdown","params":{"session_id":"sess-a"}}"#,
    );
    // The env-gated hang-on-shutdown engine parked after its ack (M21).
    // SIGTERM is deliverable and terminates: the fallback works on stdio.
    std::thread::sleep(Duration::from_millis(400));
    let kill = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill runs");
    assert!(kill.success(), "SIGTERM was deliverable");
    let status = wait_bounded(&mut child, Duration::from_secs(5));
    assert!(
        status.code().is_none(),
        "stdio SIGTERM is the DEFAULT disposition (signaled, not graceful): {status:?}"
    );
}

#[test]
fn test_shutdown_no_orphan() {
    let _lock = serial();
    let root = temp_dir("no-orphan");
    // The probe is TEST-LOCAL and strong: after the guarded child's death,
    // no process whose cmdline contains OUR storage root may remain.
    let mut spawn_guard = spawn_engine(&root, &[]);
    {
        let child = spawn_guard.child();
        round_trip(
            child.stdin.as_mut().expect("stdin"),
            child.stdout.as_mut().expect("stdout"),
            r#"{"id":"s-1","jsonrpc":"2.0","method":"shutdown","params":{"session_id":"sess-b"}}"#,
        );
    }
    // The guard kills/reaps on drop; take it explicitly for the EOF reaping.
    let mut child = spawn_guard.into_inner();
    let status = wait_bounded(&mut child, Duration::from_secs(4));
    assert_eq!(status.code(), Some(0), "exited cleanly");
    if let Some(mut stdout) = child.stdout.take() {
        let mut rest = String::new();
        let _ = stdout.read_to_string(&mut rest);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let mut rest = String::new();
        let _ = stderr.read_to_string(&mut rest);
    }
    let _ = child.wait();
    let marker = format!("--storage-root={}", root.display());
    let survivors = find_surviving(&marker);
    assert!(
        survivors.is_empty(),
        "no surviving process with our engine cmdline: {survivors:?}"
    );
}

// ---------------------------------------------------------------------------
// Engine lock loser: the process-level exit-75 oracle (M13)
// ---------------------------------------------------------------------------

#[test]
fn test_engine_lock_loser_exits_75() {
    let _lock = serial();
    let root = temp_dir("loser-75");
    // The live instance holds the lock (the web port is its own).
    let mut holding_guard = spawn_engine(&root, &[]);
    let holder = holding_guard.child();
    round_trip(
        holder.stdin.as_mut().expect("stdin"),
        holder.stdout.as_mut().expect("stdout"),
        HELLO_LINE,
    );
    assert!(root.join(".engine.lock").exists(), "holder wrote its lock");
    let lock_body = std::fs::read_to_string(root.join(".engine.lock")).expect("lock body");
    // The LOSER exits 75, never opening the DB, status file names the holder.
    let loser_output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg(format!("--web-port={}", free_port()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("loser spawns");
    assert_eq!(
        loser_output.status.code(),
        Some(75),
        "the lock loser exits with code 75 (M13): stderr {}",
        String::from_utf8_lossy(&loser_output.stderr)
    );
    let stderr = String::from_utf8_lossy(&loser_output.stderr);
    assert!(
        stderr.contains("\"event\":\"lock_lost\""),
        "structured lock_lost warning: {stderr}"
    );
    let status = std::fs::read_to_string(root.join("status")).expect("status file written");
    assert!(
        status.starts_with("degraded: "),
        "status grammar: {status:?}"
    );
    let holder_pid = extract_json_pid(&lock_body);
    assert!(
        status.contains(&format!("pid={holder_pid}")),
        "the loser named the HOLDER's pid ({holder_pid}): {status:?}"
    );
    assert!(
        root.join(".engine.lock").exists(),
        "the winner's lock survives"
    );
}

/// Reads `.engine.lock`'s LockState pid.
fn extract_json_pid(body: &str) -> String {
    let begin = body.find("\"pid\":").expect("pid key") + "\"pid\":".len();
    let rest = &body[begin..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].to_owned()
}

// ---------------------------------------------------------------------------
// E3: stdio mode installs NO graceful handler
// ---------------------------------------------------------------------------

#[test]
fn test_engine_stdio_sigterm_terminates() {
    let _lock = serial();
    let root = temp_dir("stdio-sigterm");
    let spawn_guard = spawn_engine(&root, &[]);
    let mut child = spawn_guard.into_inner();
    round_trip(
        child.stdin.as_mut().expect("stdin"),
        child.stdout.as_mut().expect("stdout"),
        HELLO_LINE,
    );
    // A pending stdin read; SIGTERM must still terminate at default
    // disposition — no graceful handler is allowed in stdio (E3).
    let kill = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill runs");
    assert!(kill.success());
    let status = wait_bounded(&mut child, Duration::from_secs(5));
    assert!(
        status.code().is_none(),
        "default termination (no graceful handler, E3): {status:?}"
    );
}

// ---------------------------------------------------------------------------
// Version pin (E4) + mechanical security oracles (M23)
// ---------------------------------------------------------------------------

#[test]
fn test_engine_version_string() {
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg("--version")
        .output()
        .expect("version runs");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        VERSION_LINE
    );
}

#[test]
fn test_security_no_secret_reads() {
    // Grep the committed JS files for the mandate's patterns (M23).
    for name in [
        "plugin/src/plugin.js",
        "plugin/dist/plugin.js",
        "plugin/index.js",
    ] {
        let source = std::fs::read_to_string(name)
            .unwrap_or_else(|error| panic!("{name} unreadable: {error}"));
        for expression in [
            ".auth-token",
            "OPENCODE_MEM_TOKEN",
            "Authorization",
            "env://",
            "file://",
            "http://",
            "https://",
            "fetch(",
            "exec(",
        ] {
            assert!(
                !source.contains(expression),
                "{name} must never contain {expression:?}"
            );
        }
    }
}

#[test]
fn test_stdout_protocol_only() {
    // No println!/print! anywhere in mem-bridge/src; main.rs's println! is
    // restricted to the version/help/print-crates SYNC paths. `eprintln!`
    // CONTAINS the substring, so the probe checks the character before it.
    //
    // W7.1 exemption: `crates/mem-bridge/src/purge.rs` is the `purge-session`
    // CLI runner, a SEPARATE process mode that must print exactly one JSON
    // result line (the engine's stdio protocol is not running in that mode).
    // The exemption is narrow: the only permitted stdout call there is the
    // single `println!("{line}")` in `print_completed`, and the count below
    // pins it at exactly ONE. This is a blueprint gap being bridged (the wave
    // adds a second stdout MODE), not a change to the product's stdout pin.
    let mut allowed_stdout_calls = 0;
    for entry in walk_dir(Path::new("crates/mem-bridge/src")) {
        let source = std::fs::read_to_string(&entry).expect("read module");
        let is_purge_cli = entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "purge.rs");
        for line in source.lines() {
            let violated = line.match_indices("print").any(|(index, _)| {
                let before = line[..index].chars().last().unwrap_or(' ');
                let rest = &line[index..];
                !before.is_alphabetic()
                    && (rest.starts_with("println!(") || rest.starts_with("print!("))
            });
            if violated && is_purge_cli {
                assert_eq!(
                    line.trim(),
                    "println!(\"{line}\");",
                    "{}: the purge CLI's only stdout call is the single JSON line",
                    entry.display()
                );
                allowed_stdout_calls += 1;
                continue;
            }
            assert!(
                !violated,
                "{} writes to stdout (protocol-only violation, M23): {line:?}",
                entry.display()
            );
        }
    }
    assert_eq!(
        allowed_stdout_calls, 1,
        "exactly one stdout call is permitted in the crate (the purge CLI's single JSON line)"
    );
    let main = std::fs::read_to_string("src/main.rs").expect("read main.rs");
    for line in main.lines() {
        let is_stdout_print = line.match_indices("print").any(|(index, _)| {
            let before = line[..index].chars().last().unwrap_or(' ');
            let rest = &line[index..];
            !before.is_alphabetic()
                && (rest.starts_with("println!(") || rest.starts_with("print!("))
        });
        if is_stdout_print {
            assert!(
                line.contains("version_line") || line.contains("name") || line.contains("--help"),
                "main.rs stdout is restricted to version/help/print-crates: {line:?}"
            );
        }
    }
}

fn walk_dir(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).expect("src dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk_dir(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn test_capture_stderr_has_no_transcript_canary() {
    let _lock = serial();
    let root = temp_dir("canary");
    let spawn_guard = spawn_engine(&root, &[]);
    let mut child = spawn_guard.into_inner();
    let canary = "TRANSCRIPT-CANARY-w51-marker";
    let capture = format!(
        "{{\"id\":\"c-1\",\"jsonrpc\":\"2.0\",\"method\":\"capture\",\"params\":{{\"agent_id\":\"a\",\"agent_name\":\"build\",\"capture_id\":\"019a2b3c-4d5e-7f00-8a9b-0c1d2e3f4a5b\",\"metadata\":{{\"turn\":1}},\"parent_session_id\":null,\"project_id\":\"proj-1\",\"session_id\":\"sess-canary\",\"summary\":\"greeting\",\"transcript\":\"{canary} hello world\"}}}}"
    );
    round_trip(
        child.stdin.as_mut().expect("stdin"),
        child.stdout.as_mut().expect("stdout"),
        &capture,
    );
    drop(child.stdin.take());
    wait_bounded(&mut child, Duration::from_secs(6));
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    assert!(
        !stderr.contains(canary),
        "the transcript canary MUST never reach stderr (M23): {stderr}"
    );
    assert!(
        !stderr.contains("transcript"),
        "stderr carries no transcript field: {stderr:?}"
    );
}

// ---------------------------------------------------------------------------
// Packaging (M26; step 17's tarball oracle)
// ---------------------------------------------------------------------------

#[test]
fn test_package_tarball_contains_engine() {
    let _lock = serial();
    // Stage the REAL engine (the script cd's to plugin/ itself).
    let staged = Command::new("bash")
        .arg("plugin/scripts/stage-engine.sh")
        .output()
        .expect("stage-engine runs");
    assert!(
        staged.status.success(),
        "stage-engine must succeed: {}",
        String::from_utf8_lossy(&staged.stderr)
    );
    let platform_dir = std::fs::read_dir("plugin/engine")
        .expect("staged engine dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| {
            name.contains('-')
                && Path::new("plugin/engine")
                    .join(name)
                    .join("opencode-agents-memory")
                    .exists()
        })
        .expect("a staged <platform>-<arch> dir with the binary");

    // TEMP copy of the package (green on a clean checkout; engine is
    // gitignored, M29) — copy the just-staged engine in.
    let pack_dir = temp_dir("pack");
    std::fs::create_dir_all(pack_dir.join("src")).expect("src dir");
    std::fs::create_dir_all(pack_dir.join("dist")).expect("dist dir");
    for entry in ["package.json", "index.js", ".gitignore"] {
        std::fs::copy(format!("plugin/{entry}"), pack_dir.join(entry))
            .unwrap_or_else(|error| panic!("{entry} unreadable: {error}"));
    }
    std::fs::copy("plugin/src/plugin.js", pack_dir.join("src/plugin.js")).expect("src copy");
    std::fs::copy("plugin/dist/plugin.js", pack_dir.join("dist/plugin.js")).expect("dist copy");
    copy_dir(Path::new("plugin/engine"), &pack_dir.join("engine"));

    let pack = Command::new("npm")
        .current_dir(&pack_dir)
        .args(["pack", "--dry-run", "--json"])
        .output()
        .expect("npm pack runs");
    assert!(
        pack.status.success(),
        "npm pack --dry-run must succeed: {} {}",
        String::from_utf8_lossy(&pack.stderr),
        String::from_utf8_lossy(&pack.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&pack.stdout).expect("pack json");
    // `npm pack --json` emits a top-level ARRAY of package objects.
    let package = if json.is_array() {
        json.as_array()
            .and_then(|v| v.first())
            .expect("npm pack emits one package")
    } else {
        &json
    };
    let files: Vec<String> = package["files"]
        .as_array()
        .expect("files array")
        .iter()
        .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect();
    let staged_rel = format!("engine/{platform_dir}/opencode-agents-memory");
    assert!(
        files.iter().any(|path| path.ends_with(&staged_rel)),
        "the tarball's file list must contain the staged engine: {files:?}"
    );
    assert_eq!(
        package["name"].as_str(),
        Some("opencode-mem"),
        "drop-in name (E5)"
    );
    assert_eq!(
        package["version"].as_str(),
        Some("0.1.0"),
        "the version mirrors [workspace.package] (N7)"
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("target dir");
    let entries = std::fs::read_dir(from).expect("source dir");
    for entry in entries.flatten() {
        let source = entry.path();
        let dest = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &dest);
        } else {
            std::fs::copy(&source, &dest).expect("copy file");
        }
    }
}
