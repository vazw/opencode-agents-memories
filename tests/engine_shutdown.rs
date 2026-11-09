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
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
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
        // Isolate the process-global file log: never the developer's real
        // `~/.opencode-mem/opencode-mem.log`.
        .env("OPENCODE_MEM_LOG_FILE", root.join("opencode-mem.log"))
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

/// Watchdog timeout for engine process exit on stdin EOF.
///
/// Measured distributions (issue 103, 2026-11-04):
/// - `test_engine_exits_on_stdin_eof` (hello only, no queued capture):
///   typical idle exit 0.15–0.22 s (mean 0.19 s).
/// - `test_capture_stderr_has_no_transcript_canary` (capture queued, embedding + DB flush):
///   typical idle exit 3.69–5.32 s (mean 4.58 s); under concurrent cargo load spikes to 6.18–6.27 s.
///
/// An arbitrary 6 s bound had only a 1.25x margin over idle and 0.96x under load (causing gate flakes).
/// A 30 s watchdog provides a 4.78x margin over the worst-case loaded exit (6.27 s) and 6.55x over mean idle,
/// acting strictly as a hang/deadlock detector rather than a machine-load discriminator.
const SHUTDOWN_WATCHDOG: Duration = Duration::from_secs(30);

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
    // Watchdog bound: idle teardown with empty inbox takes ~0.19 s because
    // no embeddings or inbox flushes are performed.
    let status = wait_bounded(&mut child, SHUTDOWN_WATCHDOG);
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
        .env("OPENCODE_MEM_LOG_FILE", root.join("opencode-mem.log"))
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
    // W7.1 / W3 / issue 43 W7 / issue 43 W6c exemption:
    // `crates/mem-bridge/src/purge.rs`, `crates/mem-bridge/src/reembed.rs`,
    // `crates/mem-bridge/src/reap.rs` and `crates/mem-bridge/src/sweep.rs` are
    // SEPARATE process modes (`purge-session`, `reembed-scope`, `reap-shards`,
    // `sweep`) that must print exactly one JSON result line each (the engine's
    // stdio protocol is not running in those modes). The exemption is narrow:
    // the only permitted stdout call in each is the single `println!("{line}")`
    // in its completion printer, and the count below pins it at exactly FOUR.
    // This is a blueprint gap being bridged (the waves add stdout MODES), not a
    // change to the product's stdout pin.
    let mut allowed_stdout_calls = 0;
    for entry in walk_dir(Path::new("crates/mem-bridge/src")) {
        let source = std::fs::read_to_string(&entry).expect("read module");
        let is_cli_runner = entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name == "purge.rs"
                    || name == "reembed.rs"
                    || name == "reap.rs"
                    || name == "sweep.rs"
            });
        for line in source.lines() {
            let violated = line.match_indices("print").any(|(index, _)| {
                let before = line[..index].chars().last().unwrap_or(' ');
                let rest = &line[index..];
                !before.is_alphabetic()
                    && (rest.starts_with("println!(") || rest.starts_with("print!("))
            });
            if violated && is_cli_runner {
                assert_eq!(
                    line.trim(),
                    "println!(\"{line}\");",
                    "{}: the CLI's only stdout call is the single JSON line",
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
        allowed_stdout_calls, 4,
        "exactly one stdout call is permitted per CLI mode (the purge, reembed, reap and sweep single JSON lines)"
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
            // `print!("{USAGE}")` is the `--help`/`-h` block — the help mode
            // the message below already names. Pinned as the exact line: a
            // substring token would let any future line mentioning `USAGE`
            // through the scan.
            if line.trim() == r#"print!("{USAGE}");"# {
                continue;
            }
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
    wait_bounded(&mut child, SHUTDOWN_WATCHDOG);
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    assert!(
        stderr.contains("engine.shutdown_complete"),
        "the engine completed structured shutdown: {stderr}"
    );
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

/// The synthetic bundle the scratch package stages (an `index.html` the script
/// requires and one asset proving the copy is recursive).
const STAGE_FIXTURE_INDEX: &str = "<!doctype html>\n<title>STAGE-ENGINE-FIXTURE</title>\n";

/// A second file in the synthetic bundle, so the `cp -R` leg is exercised.
const STAGE_FIXTURE_ASSET: &str = "// stage-engine fixture asset\n";

/// The bound on one `stage-engine.sh` run.
const STAGE_BOUND: Duration = Duration::from_secs(120);

/// The bound on `npm pack --dry-run`.
const PACK_BOUND: Duration = Duration::from_secs(120);

/// One bounded child run: its exit status and its captured output.
struct BoundedRun {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

/// A unique, gitignored scratch root under `<repo>/.tmp/scratch/engine-stage`
/// (never `/tmp`). Unique by pid + nanos + atomic counter so two concurrent
/// copies of this test binary cannot stage into the same tree — a second stage
/// into one tree is exit 7 by contract, and this file has already been polluted
/// once by concurrent copies of the binary.
fn stage_scratch_root(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp")
        .join("scratch")
        .join("engine-stage")
        .join(format!("{prefix}-{}-{nanos}-{serial}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the engine-stage scratch root");
    dir
}

/// The repository root, so no fixture source path depends on the runner's cwd.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Writes an executable file (parents created, mode 0o755).
fn write_exec(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the executable's parent dir");
    }
    std::fs::write(path, body).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
}

/// `<platform>-<arch>`, mirroring the script's `uname` arms exactly
/// (`plugin/scripts/stage-engine.sh:57-66`). Anything else makes the script
/// exit 4, which the staging assertion reports with that code — it never
/// silently passes.
fn platform_dir_name() -> String {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        _ => "linux",
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x64",
    };
    format!("{platform}-{arch}")
}

/// Runs `command` with its stdout/stderr redirected to files under `log_dir`
/// (never pipes: a chatty child must not deadlock the parent) and waits at most
/// `bound`, polling every 20 ms.
///
/// Residual, documented rather than hidden: `kill()` + `wait()` reaps the
/// `bash` child only and does **not** reap grandchildren. Today the `cargo`
/// stub exits instantly and `npm pack` finishes on its own, but a future real
/// `npm` would spawn `node` that could outlive the bound; a process-group kill
/// needs a `libc`/`unsafe` edge this wave forbids.
fn run_bounded(command: &mut Command, bound: Duration, label: &str, log_dir: &Path) -> BoundedRun {
    std::fs::create_dir_all(log_dir).expect("create the stage log dir");
    let stdout_path = log_dir.join(format!("{label}.stdout"));
    let stderr_path = log_dir.join(format!("{label}.stderr"));
    let stdout_file = std::fs::File::create(&stdout_path)
        .unwrap_or_else(|error| panic!("{label}: create {}: {error}", stdout_path.display()));
    let stderr_file = std::fs::File::create(&stderr_path)
        .unwrap_or_else(|error| panic!("{label}: create {}: {error}", stderr_path.display()));
    let mut child = command
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .unwrap_or_else(|error| panic!("{label}: cannot spawn — environment fault: {error}"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if started.elapsed() >= bound {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{label} did not finish within {bound:?} — killed; this is an environment fault, not a pass"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(
        &std::fs::read(&stdout_path)
            .unwrap_or_else(|error| panic!("{label}: read {}: {error}", stdout_path.display())),
    )
    .into_owned();
    let stderr = String::from_utf8_lossy(
        &std::fs::read(&stderr_path)
            .unwrap_or_else(|error| panic!("{label}: read {}: {error}", stderr_path.display())),
    )
    .into_owned();
    BoundedRun {
        status,
        stdout,
        stderr,
    }
}

/// The packaged tarball carries the real engine binary and the bundle staged
/// beside it.
///
/// # Why the scratch package tree
///
/// The old body ran `plugin/scripts/stage-engine.sh` in the ambient tree, whose
/// verdict therefore depended on ambient state: with a bundle already staged it
/// exits 7 (`already holds a staged bundle`), and on a clean checkout it exits 5
/// (no `../web-ui/dist/index.html`, since `web-ui/dist` is gitignored). Neither
/// says anything about the engine. This body builds a hermetic package tree
/// under `<repo>/.tmp/scratch/engine-stage/…`, stages into it, and packs it, so
/// both ambient states are irrelevant to the verdict.
///
/// The scratch root is under `<repo>/.tmp/scratch` because this wave forbids
/// writes under `/tmp` — a deliberate, documented deviation from
/// `docs/agents/testing.md`'s `temp_dir()` rule, matching
/// `tests/web_dist_packaging.rs:31-43`. The root is gitignored (`.gitignore:6`)
/// and left in place: this wave deletes nothing.
///
/// # What is real, and what is stubbed
///
/// Only `cargo` is stubbed, and only for that child (via `Command::env`, never
/// `std::env::set_var`, which would race other tests). The real script, its
/// bundle guards, the `cp`, the post-copy integrity check and the `--version`
/// smoke all run. A mis-shadowed `cargo` fails loudly: the real build would then
/// run under `set -e` in a directory with no `Cargo.toml`.
///
/// `hard_link(env!("CARGO_BIN_EXE_opencode-agents-memory"))` is correct here
/// because this oracle lives in the integration test of the package that
/// declares the binary — the same variable the file header (`:3-5`) and the
/// spawn oracles (lines 87/322/403) already use. The artifact under test is the
/// real engine; only the release build is elided, the hermeticity choice already
/// recorded at `plugin/tests/stage-engine.test.mjs:9-11`.
///
/// What the harness stopped proving, in these words: **no release build is
/// performed; the staged binary is the debug build of the same bin target,
/// planted at the release path; exit 3 is not exercised**.
#[test]
fn test_package_tarball_contains_engine() {
    let _lock = serial();
    let repo = repo_root();
    let scratch = stage_scratch_root("tarball");
    let logs = scratch.join("logs");
    std::fs::create_dir_all(&logs).expect("create the stage log dir");

    // The synthetic built bundle the script stages (`../web-ui/dist` from the
    // scratch package), so no ambient `web-ui/dist` is read.
    let bundle = scratch.join("web-ui/dist");
    std::fs::create_dir_all(&bundle).expect("create the synthetic bundle dir");
    std::fs::write(bundle.join("index.html"), STAGE_FIXTURE_INDEX)
        .expect("write the synthetic bundle index.html");
    std::fs::write(bundle.join("web-ui-fixture.js"), STAGE_FIXTURE_ASSET)
        .expect("write the synthetic bundle asset");

    // The scratch package: the package's non-engine surface plus the REAL
    // script, copied in. The copy is what is invoked — never the repo copy,
    // which `cd`s to its own `..` and would mutate `plugin/engine/`.
    let pkg = scratch.join("plugin");
    std::fs::create_dir_all(pkg.join("src")).expect("create the scratch src dir");
    std::fs::create_dir_all(pkg.join("dist")).expect("create the scratch dist dir");
    std::fs::create_dir_all(pkg.join("scripts")).expect("create the scratch scripts dir");
    for entry in ["package.json", "index.js", ".gitignore"] {
        std::fs::copy(repo.join("plugin").join(entry), pkg.join(entry))
            .unwrap_or_else(|error| panic!("plugin/{entry} unreadable: {error}"));
    }
    std::fs::copy(repo.join("plugin/src/plugin.js"), pkg.join("src/plugin.js")).expect("src copy");
    std::fs::copy(
        repo.join("plugin/dist/plugin.js"),
        pkg.join("dist/plugin.js"),
    )
    .expect("dist copy");
    let script = pkg.join("scripts/stage-engine.sh");
    std::fs::copy(repo.join("plugin/scripts/stage-engine.sh"), &script).expect("script copy");

    // Toolchain: only `cargo` is stubbed; the real engine binary is planted at
    // the release path the script looks for (hard link first — the debug engine
    // is ~484 MB — with a copy + chmod fallback across filesystems).
    let bin_dir = scratch.join("bin");
    write_exec(&bin_dir.join("cargo"), "#!/usr/bin/env bash\nexit 0\n");
    let engine_bin = PathBuf::from(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    let engine_len = std::fs::metadata(&engine_bin)
        .unwrap_or_else(|error| panic!("engine binary metadata: {error}"))
        .len();
    let planted_dir = scratch.join("cargo-target/release");
    // `logs` was the only directory created so far; `hard_link`/`copy` do not
    // create parents, so the literal implementation would panic with ENOENT.
    std::fs::create_dir_all(&planted_dir).expect("create the planted release dir");
    let planted = planted_dir.join("opencode-agents-memory");
    if std::fs::hard_link(&engine_bin, &planted).is_err() {
        std::fs::copy(&engine_bin, &planted).expect("copy the engine binary to the release path");
        std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o755))
            .expect("chmod the planted engine binary");
    }
    // A failed or truncated plant would otherwise surface as the script's exit
    // 3 ("built binary is missing/not executable") and be misread as a
    // mis-built tree; diagnose it here as a harness fault instead.
    assert_eq!(
        std::fs::metadata(&planted)
            .expect("planted binary metadata")
            .len(),
        engine_len,
        "the planted engine binary must be the whole debug engine, not a truncated plant"
    );

    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // First stage: the real script, into the hermetic tree.
    let mut stage = Command::new("bash");
    stage
        .arg(&script)
        .current_dir(&scratch)
        .env("PATH", &path_env)
        .env("CARGO_TARGET_DIR", scratch.join("cargo-target"));
    let first = run_bounded(&mut stage, STAGE_BOUND, "stage-1", &logs);
    assert!(
        first.status.success(),
        "stage-engine must succeed in the scratch tree (exit {:?}); exits 5/6/7 here mean the harness built the wrong tree — an environment fault, not a broken engine; stderr: {}",
        first.status.code(),
        first.stderr
    );

    // The staged artifact IS the engine binary, executable, with the real len.
    let platform = platform_dir_name();
    let staged_dir = pkg.join("engine").join(&platform);
    let staged_bin = staged_dir.join("opencode-agents-memory");
    assert!(
        staged_bin.is_file(),
        "the script must stage the engine binary at {} (exit was {:?}); stderr: {}",
        staged_bin.display(),
        first.status.code(),
        first.stderr
    );
    let staged_meta = std::fs::metadata(&staged_bin).expect("staged binary metadata");
    assert_ne!(
        staged_meta.permissions().mode() & 0o111,
        0,
        "the staged engine must be executable"
    );
    assert_eq!(
        staged_meta.len(),
        engine_len,
        "the staged binary must be the engine binary (same length as CARGO_BIN_EXE_opencode-agents-memory)"
    );

    // The script discards the test binary's stdout, so run it here: "ran and
    // printed the version line" must be distinguishable from "ran and printed
    // garbage".
    let mut version = Command::new(&staged_bin);
    version.arg("--version");
    let run = run_bounded(
        &mut version,
        Duration::from_secs(20),
        "staged-version",
        &logs,
    );
    assert!(
        run.status.success(),
        "the staged engine's --version must succeed (exit {:?}); stderr: {}",
        run.status.code(),
        run.stderr
    );
    assert_eq!(
        run.stdout.trim(),
        VERSION_LINE,
        "the staged engine must print the version line, not arbitrary output"
    );

    let staged_index = staged_dir.join("web-ui/dist/index.html");
    assert_eq!(
        std::fs::read_to_string(&staged_index).expect("read the staged bundle index.html"),
        STAGE_FIXTURE_INDEX,
        "the staged bundle must be the synthetic fixture, byte for byte"
    );

    // The wave's exit-7 contract, exercised in the same hermetic tree: a second
    // stage refuses (replace-never-merge) and rewrites nothing.
    let mut stage_again = Command::new("bash");
    stage_again
        .arg(&script)
        .current_dir(&scratch)
        .env("PATH", &path_env)
        .env("CARGO_TARGET_DIR", scratch.join("cargo-target"));
    let second = run_bounded(&mut stage_again, STAGE_BOUND, "stage-2", &logs);
    assert_eq!(
        second.status.code(),
        Some(7),
        "a second stage into the same tree must refuse as already-staged (exit Some(7)); stderr: {}",
        second.stderr
    );
    let staged_bundle_rel = format!("engine/{platform}/web-ui/dist");
    assert!(
        second.stderr.contains(&staged_bundle_rel),
        "the exit-7 message must name the already-staged bundle `{staged_bundle_rel}`; stderr: {}",
        second.stderr
    );
    assert_eq!(
        std::fs::read_to_string(&staged_index).expect("re-read the staged bundle index.html"),
        STAGE_FIXTURE_INDEX,
        "replace-never-merge: the refused second stage must leave the staged bundle untouched"
    );

    // Pack the hermetic package. `--ignore-scripts` keeps a future `prepare`
    // script out of the test; the cache stays inside the scratch tree.
    let npm_cache = scratch.join("npm-cache");
    let mut pack = Command::new("npm");
    pack.current_dir(&pkg)
        .args([
            "pack",
            "--dry-run",
            "--json",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-update-notifier",
            "--cache",
        ])
        .arg(&npm_cache);
    let packed = run_bounded(&mut pack, PACK_BOUND, "npm-pack", &logs);
    assert!(
        packed.status.success(),
        "npm pack --dry-run must succeed (exit {:?}); stderr: {}",
        packed.status.code(),
        packed.stderr
    );
    let json: serde_json::Value = serde_json::from_str(&packed.stdout).expect("pack json");
    // `npm pack --json` emits a top-level ARRAY of package objects.
    let package = if json.is_array() {
        json.as_array()
            .and_then(|value| value.first())
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
    let staged_rel = format!("engine/{platform}/opencode-agents-memory");
    assert!(
        files.iter().any(|path| path.ends_with(&staged_rel)),
        "the tarball's file list must contain the staged engine: {files:?}"
    );
    let index_rel = format!("engine/{platform}/web-ui/dist/index.html");
    assert!(
        files.iter().any(|path| path.ends_with(&index_rel)),
        "the tarball must carry the staged bundle beside the engine: {files:?}"
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
