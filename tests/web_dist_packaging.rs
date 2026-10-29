//! W6.9 packaged-layout oracles for issue 36: the engine resolves and serves
//! the bundle **staged beside its executable** with no setting and no flag, and
//! a packaged engine with no bundle at all degrades loudly through the existing
//! structured path.
//!
//! This file complements `tests/web_dist_wiring.rs`: that suite drives the pure
//! resolution module directly and spawns the engine with an EXPLICIT setting and
//! synthetic `exe_dir`s; this one spawns the real binary from the real packaged
//! directory shape (`<exe_dir>/web-ui/dist`) and asserts the exe-relative
//! default end to end over raw HTTP and NDJSON.
//!
//! House rules (`docs/agents/testing.md`): child stdout/stderr go to FILES
//! (never pipes) and every wait is bounded (20 ms poll, 30 s deadline; the panic
//! prints the observed file contents); `std::env::set_var` is never called — the
//! setting under test is injected per child; each spawn test takes the shared
//! serial mutex, its own `--storage-root` and its own free port, and kills AND
//! reaps its child on drop.
//!
//! ## Scratch root
//!
//! `<repo>/.tmp/scratch/web-dist-packaging/<prefix>-<pid>-<nanos>-<counter>`.
//! `docs/agents/testing.md:56` prescribes `std::env::temp_dir()`, and this wave
//! mandates `TMPDIR=<repo>/.tmp/scratch` on every cargo command — so
//! `temp_dir()` resolves into the *same* tree. Hardcoding
//! `CARGO_MANIFEST_DIR/.tmp/…` therefore deviates from that rule in letter only.
//! It matches the sibling wiring tests (`tests/web_dist_wiring.rs`,
//! `tests/production_api_wiring.rs`, `tests/skills_live_wiring.rs`), which also
//! write under `<repo>/.tmp/scratch`, so the root package's fixtures share one
//! scratch root. The root is gitignored (`.gitignore:6`, `/.tmp/`) and left
//! in place — the wave deletes nothing. Nothing large is ever written here: the
//! engine binary is `hard_link`ed first, and the ~75 MB `copy` happens only if
//! the link fails.
//!
//! ## Why the liveness mutation is genuine (stated, not asserted)
//!
//! `is_ready()` requires `index.html` as a regular file (`src/web_dist.rs:191-193`)
//! -> renaming the fixture makes candidate 1 `ResolvedDist::Missing`
//! (`src/web_dist.rs:170-177`) -> `default_candidates` then falls through to
//! candidate 2 (`src/web_dist.rs:151-156`), which in the first test's tree holds
//! the `CHECKOUT-DECOY` bundle — so `GET /` still returns 200 and the failure
//! surfaces on the BODY assertions (the body carries `CHECKOUT-DECOY` and lacks
//! `PACKAGED-FIXTURE`), not on a connection refusal. With BOTH candidates absent
//! (the second test's layout) the chain is: `RootWebLauncher::launch` emits
//! `web.dist_missing` and returns `Err` **before** any bind
//! (`src/main.rs:114-124`) -> the port is never bound -> `web.launch_failed`
//! follows from the engine's degradation
//! (`crates/mem-bridge/src/engine.rs:917-938`).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use opencode_agents_memory::web_dist;

/// The marker the packaged candidate-1 fixture carries.
const PACKAGED_MARKER: &str = "PACKAGED-FIXTURE";

/// The marker the candidate-2 (cargo-checkout) decoy carries.
const DECOY_MARKER: &str = "CHECKOUT-DECOY";

/// The 5 s bound on every socket read/write (R4).
const SOCKET_BOUND: Duration = Duration::from_secs(5);

/// The startup bound: the engine composes services before it serves, so the
/// connect poll may need longer than the HTTP bound.
const STARTUP_BOUND: Duration = Duration::from_secs(30);

/// The poll interval shared by every bounded wait.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------
// Scratch layout
// ---------------------------------------------------------------------------

/// A unique, gitignored scratch root under `<repo>/.tmp/scratch` (never `/tmp`).
fn scratch_root(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp")
        .join("scratch")
        .join("web-dist-packaging")
        .join(format!("{prefix}-{}-{nanos}-{serial}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch root");
    dir
}

/// `<platform>-<arch>`, mirroring the wrapper's derivation
/// (`plugin/src/plugin.js`: `darwin`/`linux`, `x64`/`arm64`).
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

/// The packaged layout: `<root>/pkg/engine/<platform>-<arch>/` holding the
/// engine binary. `exe_dir.join("../../web-ui/dist")` resolves exactly to
/// `<root>/pkg/web-ui/dist`, which is candidate 2.
struct Packaged {
    root: PathBuf,
    exe_dir: PathBuf,
}

/// Builds the packaged layout and places the real engine binary in `exe_dir`.
/// The binary is `hard_link`ed (same filesystem, no 75 MB copy); only if the
/// link fails is it copied, with mode `0o755`.
fn packaged_layout(prefix: &str) -> Packaged {
    let root = scratch_root(prefix);
    let exe_dir = root.join("pkg").join("engine").join(platform_dir_name());
    std::fs::create_dir_all(&exe_dir).expect("create packaged exe dir");
    let source = Path::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    let dest = exe_dir.join("opencode-agents-memory");
    if std::fs::hard_link(source, &dest).is_err() {
        std::fs::copy(source, &dest).expect("copy the engine binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&dest)
                .expect("stat the copied engine binary")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dest, perms).expect("chmod the copied engine binary");
        }
    }
    Packaged { root, exe_dir }
}

/// Writes a realistic bundle: `index.html` carrying `marker` plus one hashed
/// asset (trunk hashes asset names, so a bundle is never just `index.html`).
fn write_bundle(dir: &Path, marker: &str) {
    std::fs::create_dir_all(dir).expect("create bundle dir");
    let html = format!(
        "<!doctype html><html><head><title>W6.9 packaged fixture</title></head>\
         <body><div id=\"w69-marker\">{marker}</div></body></html>"
    );
    std::fs::write(dir.join("index.html"), html).expect("write index.html");
    std::fs::write(dir.join("web-ui-abc123.js"), format!("// {marker} asset\n"))
        .expect("write the hashed asset");
}

// ---------------------------------------------------------------------------
// Helpers (copy-adapted from `tests/web_dist_wiring.rs`)
// ---------------------------------------------------------------------------

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// The serial harness: every spawning test takes this for its whole body so no
/// orphan from one test is visible to the next one's assertions.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An engine child that is killed AND reaped when the guard drops — the only
/// teardown path, so no zombie survives a panicking test.
struct Guarded {
    child: Child,
}

impl Guarded {
    /// Polls `try_wait` until the child exits, within a bounded deadline.
    fn wait_bounded(&mut self, deadline: Duration) -> ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait the engine") {
                return status;
            }
            assert!(
                started.elapsed() < deadline,
                "the engine never exited within {deadline:?}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls `TcpStream::connect` until the port accepts, within a bounded deadline.
fn wait_for_port(port: u16) -> TcpStream {
    let started = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(error) => {
                assert!(
                    started.elapsed() < STARTUP_BOUND,
                    "the web port {port} never accepted a connection within {STARTUP_BOUND:?}: {error}"
                );
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// One bounded HTTP/1.1 exchange. The socket's read AND write timeouts are set
/// to 5 s before the request is written, and the request asks for
/// `Connection: close`, so a keep-alive server cannot hang the test.
///
/// Returns `(status, raw_response)`; on a socket deadline the raw bytes read so
/// far are part of the panic message.
fn http_get(port: u16, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(SOCKET_BOUND))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(SOCKET_BOUND))
        .expect("set write timeout");
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(error) => {
                let seen = String::from_utf8_lossy(&raw);
                panic!(
                    "HTTP read deadline ({SOCKET_BOUND:?}) on {path}: {error}; bytes so far: {seen}"
                );
            }
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no HTTP status line in {text:?}"));
    (status, text)
}

/// Polls a child output FILE (never a pipe) until `predicate` accepts its
/// contents, within a bounded deadline. On expiry the file's contents are
/// printed by the panic. Generalises `wait_for_stderr` from
/// `tests/web_dist_wiring.rs` (stdout is polled the same way here).
fn wait_for_file(path: &Path, predicate: impl Fn(&str) -> bool) -> String {
    let started = Instant::now();
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if predicate(&text) {
            return text;
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "{} never matched within {STARTUP_BOUND:?}; contents: {text}",
            path.display()
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Spawns the PACKAGED engine with the wrapper's default argv shape: no
/// `--mode` (stdio is the default; the web launcher runs in every mode), no
/// `--web-dist`, and the setting env var removed so only the exe-relative
/// default can resolve. stdin is PIPED (the caller holds it open, which keeps a
/// stdio engine alive); stdout/stderr go to files.
fn spawn_packaged(
    exe_dir: &Path,
    storage_root: &Path,
    port: u16,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Child {
    let stdout = std::fs::File::create(stdout_path).expect("create stdout file");
    let stderr = std::fs::File::create(stderr_path).expect("create stderr file");
    Command::new(exe_dir.join("opencode-agents-memory"))
        .arg(format!("--storage-root={}", storage_root.display()))
        .arg(format!("--web-port={port}"))
        // The parent test runner's environment is never trusted: the setting
        // under test is absent, per child.
        .env_remove(web_dist::ENV_VAR)
        // Isolate the process-global file log from the real home.
        .env(
            "OPENCODE_MEM_LOG_FILE",
            storage_root.join("opencode-mem.log"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("the packaged engine spawns")
}

// ---------------------------------------------------------------------------
// C1 — packaged default: the exe-relative bundle is served with no setting/flag
// ---------------------------------------------------------------------------

#[test]
fn test_packaged_engine_serves_bundle_from_exe_relative_default_without_setting_or_flag() {
    let _lock = serial();
    let pkg = packaged_layout("packaged-default");

    // Candidate 1: the bundle staged beside the binary.
    let packaged_dist = pkg.exe_dir.join("web-ui").join("dist");
    write_bundle(&packaged_dist, PACKAGED_MARKER);

    // Candidate 2 (the decoy): `exe_dir/../../web-ui/dist` resolves to exactly
    // `<root>/pkg/web-ui/dist`. Its presence makes this an ORDER assertion, not
    // a mere existence assertion: only an order-correct `default_candidates`
    // serves the packaged marker.
    let decoy_dist = pkg.root.join("pkg").join("web-ui").join("dist");
    write_bundle(&decoy_dist, DECOY_MARKER);

    let storage = pkg.root.join("storage");
    let port = free_port();
    let stdout_path = pkg.root.join("engine.out");
    let stderr_path = pkg.root.join("engine.err");
    let mut child = spawn_packaged(&pkg.exe_dir, &storage, port, &stdout_path, &stderr_path);
    // Held open for the test's whole body: an EOF would end a stdio engine.
    let _stdin = child.stdin.take().expect("piped stdin");
    let _child = Guarded { child };

    drop(wait_for_port(port));

    let (status, raw) = http_get(port, "/");
    assert_eq!(status, 200, "GET / must serve the packaged bundle: {raw}");
    assert!(
        raw.to_ascii_lowercase().contains("content-type: text/html"),
        "the packaged shell is HTML: {raw}"
    );
    assert!(
        raw.contains(PACKAGED_MARKER),
        "GET / serves the bundle staged beside the executable, with no setting and no flag: {raw}"
    );
    assert!(
        !raw.contains(DECOY_MARKER),
        "the exe-relative packaged candidate takes precedence over the checkout decoy: {raw}"
    );

    let (status, raw) = http_get(port, "/api/health");
    assert_eq!(
        status, 200,
        "the auxiliary API is live beside the bundle: {raw}"
    );
    assert!(
        raw.contains("\"status\":\"ok\""),
        "GET /api/health reports the engine alive: {raw}"
    );
}

// ---------------------------------------------------------------------------
// C2 — packaged loud degradation: no bundle anywhere, still no panic, no socket
// ---------------------------------------------------------------------------

#[test]
fn test_packaged_engine_without_bundle_degrades_loudly_and_never_panics() {
    let _lock = serial();
    // No `web-ui/dist` anywhere under `pkg/`: neither candidate resolves.
    let pkg = packaged_layout("packaged-missing");

    let storage = pkg.root.join("storage");
    let port = free_port();
    let stdout_path = pkg.root.join("engine.out");
    let stderr_path = pkg.root.join("engine.err");
    let mut child = spawn_packaged(&pkg.exe_dir, &storage, port, &stdout_path, &stderr_path);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let hello = r#"{"jsonrpc":"2.0","method":"hello","id":"probe-1","params":{"client":"packaging-test","min_protocol":"1.0.0","protocol":"1.0.0"}}"#;
    stdin.write_all(hello.as_bytes()).expect("write hello");
    stdin.write_all(b"\n").expect("write hello newline");
    stdin.flush().expect("flush hello");
    let mut child = Guarded { child };

    // Both structured lines, in the packaged layout.
    let stderr = wait_for_file(&stderr_path, |text| {
        text.contains("\"event\":\"web.dist_missing\"")
            && text.contains("\"event\":\"web.launch_failed\"")
    });
    let dist_missing_line = stderr
        .lines()
        .find(|line| line.contains("\"event\":\"web.dist_missing\""))
        .expect("a web.dist_missing line");
    assert!(
        dist_missing_line.contains("\"dist_candidates\""),
        "the diagnostic payload carries the candidate list: {dist_missing_line}"
    );
    for candidate in web_dist::default_candidates(&pkg.exe_dir) {
        assert!(
            dist_missing_line.contains(&candidate.display().to_string()),
            "the diagnostic names the packaged candidate {}: {dist_missing_line}",
            candidate.display()
        );
    }
    assert!(
        dist_missing_line.contains(web_dist::ENV_VAR)
            && dist_missing_line.contains(web_dist::CLI_FLAG),
        "the remediation names the setting and the flag: {dist_missing_line}"
    );

    // The stdio purpose survived: the hello handshake is answered on stdout.
    let stdout = wait_for_file(&stdout_path, |text| {
        text.contains("\"id\":\"probe-1\"") && text.contains("\"protocol\":\"1.0.0\"")
    });
    assert!(
        stdout.contains("\"id\":\"probe-1\""),
        "the engine still answers stdio after the loud degradation: {stdout}"
    );

    // Refusing the launch means NO socket: the port stays closed, never a
    // bind-and-404.
    let refused = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    );
    assert!(
        refused.is_err(),
        "a bundle-less package must refuse the launch, never bind and 404: {refused:?}"
    );

    // Never a panic: the degradation is a structured refusal, not a crash.
    assert!(
        !stderr.contains("panicked at"),
        "the packaged degradation must not panic: {stderr}"
    );
    assert!(
        !stderr.contains("RUST_BACKTRACE"),
        "the packaged degradation must not trip the panic hook: {stderr}"
    );

    // EOF after the loud degradation is a clean exit (a panic would surface as
    // 101/abort, not a 0).
    drop(stdin);
    let status = child.wait_bounded(STARTUP_BOUND);
    assert_eq!(
        status.code(),
        Some(0),
        "stdin EOF is a clean exit after the loud degradation: {status:?}"
    );
}
