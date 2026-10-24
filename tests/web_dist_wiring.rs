//! W6.8 oracles: the composition root points mem-web at the built web UI.
//!
//! Three groups live here, in the order the wave builds them:
//!
//! 1. **Unit oracles** for the pure resolution module (`src/web_dist.rs`):
//!    precedence, default candidates, readiness, absolutization and the
//!    executable diagnostic payload (R1).
//! 2. **Binary-spawn oracles**: a live engine child serving a shell fixture
//!    (`GET /` + SPA fallback), degrading loudly when no build resolves, and
//!    proving the env/CLI precedence end to end.
//! 3. **Repository guards**: `web-ui/dist` stays gitignored build output.
//!
//! House rules obeyed here: child stdout/stderr go to FILES (never pipes) and
//! every wait is bounded (`docs/agents/testing.md:42-43`); `std::env::set_var`
//! is never called (`:57`) — the child env is injected with `Command::env`;
//! each spawn test uses its own `--storage-root` and free port.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use opencode_agents_memory::web_dist::{self, ResolvedDist};

/// The fixture shell's marker attribute, so a served body is recognisable
/// without pinning anything from the real `web-ui/dist`.
const FIXTURE_TITLE: &str = "W6.8 dist fixture";

/// The 5 s bound on every socket read/write and on every poll loop (R4).
const SOCKET_BOUND: Duration = Duration::from_secs(5);

/// The startup bound: the engine composes services before it serves, so the
/// connect poll may need a little longer than the HTTP bound.
const STARTUP_BOUND: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A unique, gitignored scratch directory under the repo's `tmp/scratch`
/// (never `/tmp`: this host's tmpfs truncates under pressure). Absolute, so
/// `resolve` never has to interpret the fixture path relative to a test's
/// pretend cwd.
fn scratch_dir(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::current_dir().expect("repo root");
    let dir = root
        .join("tmp")
        .join("scratch")
        .join("web-dist-wiring")
        .join(format!("{prefix}-{}-{nanos}-{serial}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// A stand-in `exe_dir` two levels below `root`, so
/// `exe_dir/../../web-ui/dist` lands inside the test's OWN scratch root (and
/// never in a sibling test's checkout candidate).
fn fake_exe_dir(root: &Path) -> PathBuf {
    let exe_dir = root.join("target").join("debug");
    std::fs::create_dir_all(&exe_dir).expect("create exe dir");
    exe_dir
}

/// Writes a minimal shell whose body carries `marker`.
fn write_shell(dir: &Path, marker: &str) {
    std::fs::create_dir_all(dir).expect("create dist dir");
    let html = format!(
        "<!doctype html><html><head><title>{FIXTURE_TITLE}</title></head>\
         <body><div id=\"w68-marker\">{marker}</div></body></html>"
    );
    std::fs::write(dir.join("index.html"), html).expect("write index.html");
}

/// Reads back the marker the shell in `dir` carries (proves which candidate
/// actually won).
fn shell_body(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("index.html")).expect("read fixture shell")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// The serial harness: every spawning test takes this for its whole body so
/// no orphan from one test is visible to the next one's assertions.
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

impl Drop for Guarded {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls `TcpStream::connect` until the port accepts, within a bounded
/// deadline (no sleep loops beyond the fixed 20 ms poll interval).
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
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// One bounded HTTP/1.1 exchange. The socket's read AND write timeouts are set
/// to 5 s before the request is written, and the request asks for
/// `Connection: close`, so a keep-alive server cannot hang the test (R4).
///
/// Returns `(status, raw_response)`. On a socket deadline the raw bytes read so
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

/// Polls a stderr FILE (never a pipe) until `predicate` accepts its contents,
/// within a bounded deadline. On expiry the child is killed by the caller's
/// guard and the file's contents are printed by the panic.
fn wait_for_stderr(path: &Path, predicate: impl Fn(&str) -> bool) -> String {
    let started = Instant::now();
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if predicate(&text) {
            return text;
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "stderr never matched within {STARTUP_BOUND:?}; contents: {text}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Spawns a standalone engine with its stderr redirected to `stderr_path`.
fn spawn_standalone(
    storage_root: &Path,
    web_port: u16,
    extras: &[String],
    env: &[(&str, String)],
    stderr_path: &Path,
) -> Guarded {
    let stderr = std::fs::File::create(stderr_path).expect("create stderr file");
    let stdout =
        std::fs::File::create(stderr_path.with_extension("out")).expect("create stdout file");
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        .arg("--mode=standalone")
        .arg(format!("--storage-root={}", storage_root.display()))
        .arg(format!("--web-port={web_port}"))
        // The parent test runner's environment is never trusted: the setting
        // under test is injected per child, and only per child.
        .env_remove(web_dist::ENV_VAR)
        // Isolate the process-global file log from the real home.
        .env(
            "OPENCODE_MEM_LOG_FILE",
            storage_root.join("opencode-mem.log"),
        );
    for extra in extras {
        command.arg(extra);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    Guarded {
        child: command.spawn().expect("engine spawns"),
    }
}

// ---------------------------------------------------------------------------
// Step 2 — unit oracles for the pure resolution module
// ---------------------------------------------------------------------------

#[test]
fn test_explicit_setting_cli_overrides_env() {
    let cli = Path::new("/cli/web-ui/dist");
    let env = Path::new("/env/web-ui/dist");
    assert_eq!(
        web_dist::explicit_setting(Some(cli), Some(env)),
        Some(PathBuf::from("/cli/web-ui/dist")),
        "the CLI flag wins over the env var"
    );
}

#[test]
fn test_explicit_setting_env_used_when_cli_absent() {
    let env = Path::new("/env/web-ui/dist");
    assert_eq!(
        web_dist::explicit_setting(None, Some(env)),
        Some(PathBuf::from("/env/web-ui/dist")),
        "the env var is the fallback when no CLI flag was given"
    );
}

#[test]
fn test_explicit_setting_none_when_both_absent_or_blank() {
    assert_eq!(web_dist::explicit_setting(None, None), None);
    let blank = Path::new("   ");
    assert_eq!(
        web_dist::explicit_setting(Some(blank), Some(blank)),
        None,
        "a whitespace-only value falls through like an unset one"
    );
}

#[test]
fn test_resolve_explicit_ready_when_index_html_present() {
    let dist = scratch_dir("explicit-ready");
    write_shell(&dist, "MARKER-EXPLICIT-READY");
    let resolved = web_dist::resolve(
        Some(&dist),
        Path::new("tmp/scratch/unused-exe"),
        Path::new("/"),
    );
    assert_eq!(
        resolved.dir(),
        Some(dist.as_path()),
        "an explicit dist with an index.html file is READY"
    );
    assert_eq!(resolved.candidates(), std::slice::from_ref(&dist));
}

#[test]
fn test_resolve_explicit_missing_when_index_html_absent() {
    let dist = scratch_dir("explicit-empty");
    let resolved = web_dist::resolve(
        Some(&dist),
        Path::new("tmp/scratch/unused-exe"),
        Path::new("/"),
    );
    assert_eq!(
        resolved.dir(),
        None,
        "an explicit directory without index.html is NOT a usable build"
    );
    assert_eq!(resolved.candidates(), std::slice::from_ref(&dist));
    match &resolved {
        ResolvedDist::Missing { explicit, .. } => {
            assert_eq!(explicit.as_deref(), Some(dist.as_path()));
        }
        ResolvedDist::Ready { dir } => panic!("expected Missing, got Ready at {}", dir.display()),
    }
}

#[test]
fn test_resolve_index_html_directory_is_not_ready() {
    let dist = scratch_dir("index-is-a-dir");
    std::fs::create_dir_all(dist.join("index.html")).expect("create index.html as a directory");
    let resolved = web_dist::resolve(
        Some(&dist),
        Path::new("tmp/scratch/unused-exe"),
        Path::new("/"),
    );
    assert_eq!(
        resolved.dir(),
        None,
        "readiness requires index.html to be a regular FILE"
    );
}

#[test]
fn test_resolve_relative_explicit_is_absolutized_against_cwd() {
    let cwd = scratch_dir("relative-cwd");
    let dist = cwd.join("ui").join("dist");
    write_shell(&dist, "MARKER-RELATIVE");
    let resolved = web_dist::resolve(Some(Path::new("ui/dist")), Path::new("/nowhere/bin"), &cwd);
    assert_eq!(
        resolved.dir(),
        Some(dist.as_path()),
        "a relative explicit path is resolved against cwd, never the process cwd of the caller"
    );
}

#[test]
fn test_resolve_default_prefers_packaged_candidate() {
    let root = scratch_dir("default-packaged");
    let exe_dir = fake_exe_dir(&root);
    let packaged = web_dist::default_candidates(&exe_dir)[0].clone();
    let checkout = web_dist::default_candidates(&exe_dir)[1].clone();
    write_shell(&packaged, "MARKER-PACKAGED");
    write_shell(&checkout, "MARKER-CHECKOUT");
    assert_eq!(
        packaged,
        exe_dir.join("web-ui/dist"),
        "candidate 1 is the packaged layout beside the binary"
    );
    assert_eq!(
        checkout,
        exe_dir.join("../../web-ui/dist"),
        "candidate 2 is the cargo-checkout dev layout"
    );
    let resolved = web_dist::resolve(None, &exe_dir, &root);
    assert_eq!(
        resolved.dir(),
        Some(packaged.as_path()),
        "the packaged candidate wins when both exist"
    );
    assert!(
        shell_body(resolved.dir().expect("ready dir")).contains("MARKER-PACKAGED"),
        "the SERVED candidate is the packaged one"
    );
}

#[test]
fn test_resolve_default_falls_back_to_checkout_candidate() {
    let root = scratch_dir("default-checkout");
    let exe_dir = fake_exe_dir(&root);
    let checkout = web_dist::default_candidates(&exe_dir)[1].clone();
    write_shell(&checkout, "MARKER-CHECKOUT-ONLY");
    let resolved = web_dist::resolve(None, &exe_dir, &root);
    assert_eq!(
        resolved.dir(),
        Some(checkout.as_path()),
        "a missing packaged candidate falls back to the checkout candidate"
    );
    assert!(
        shell_body(resolved.dir().expect("ready dir")).contains("MARKER-CHECKOUT-ONLY"),
        "the SERVED candidate is the checkout one"
    );
}

#[test]
fn test_resolve_default_skips_candidate_without_index_html() {
    let root = scratch_dir("default-partial");
    let exe_dir = fake_exe_dir(&root);
    let candidates = web_dist::default_candidates(&exe_dir);
    std::fs::create_dir_all(&candidates[0]).expect("create stale partial build");
    write_shell(&candidates[1], "MARKER-LATER-CANDIDATE");
    let resolved = web_dist::resolve(None, &exe_dir, &root);
    assert_eq!(
        resolved.dir(),
        Some(candidates[1].as_path()),
        "a stale partial build without index.html is skipped in favour of a later candidate"
    );
}

#[test]
fn test_error_message_lists_default_candidates() {
    let root = scratch_dir("message-default");
    let exe_dir = fake_exe_dir(&root);
    let resolved = web_dist::resolve(None, &exe_dir, &root);
    let message = resolved.error_message();
    for candidate in web_dist::default_candidates(&exe_dir) {
        assert!(
            message.contains(&candidate.display().to_string()),
            "the diagnostic names every candidate it tried: {message:?}"
        );
    }
    assert!(
        message.contains(web_dist::ENV_VAR) && message.contains(web_dist::CLI_FLAG),
        "the diagnostic names the setting that overrides the defaults: {message:?}"
    );
}

#[test]
fn test_error_message_names_explicit_path() {
    let dist = scratch_dir("message-explicit");
    let resolved = web_dist::resolve(
        Some(&dist),
        Path::new("tmp/scratch/unused-exe"),
        Path::new("/"),
    );
    let message = resolved.error_message();
    assert!(
        message.contains(&dist.display().to_string()),
        "the explicit path is named so the misconfiguration is visible: {message:?}"
    );
    assert!(
        message.contains("index.html"),
        "the remediation sentence names the readiness marker: {message:?}"
    );
}

#[test]
fn test_missing_payload_ready_is_null() {
    let dist = scratch_dir("payload-ready");
    write_shell(&dist, "MARKER-PAYLOAD-READY");
    let resolved = web_dist::resolve(
        Some(&dist),
        Path::new("tmp/scratch/unused-exe"),
        Path::new("/"),
    );
    assert_eq!(
        resolved.missing_payload(),
        mem_store::serde_json::Value::Null,
        "a ready build never emits the event, so its payload is Null"
    );
}

#[test]
fn test_missing_payload_missing_carries_candidates_and_reason() {
    let dist = scratch_dir("payload-missing");
    let resolved = web_dist::resolve(
        Some(&dist),
        Path::new("tmp/scratch/unused-exe"),
        Path::new("/"),
    );
    let payload = resolved.missing_payload();
    let candidates = payload["dist_candidates"]
        .as_array()
        .expect("dist_candidates is a JSON array of strings");
    assert_eq!(
        candidates
            .iter()
            .map(|value| value.as_str().expect("candidate is a string").to_owned())
            .collect::<Vec<_>>(),
        resolved.lossy_candidates(),
        "the payload carries the same candidates the diagnostic lists"
    );
    assert_eq!(
        payload["reason"].as_str(),
        Some(resolved.error_message().as_str()),
        "the payload carries the same reason the launcher returns"
    );
    assert!(
        payload["reason"]
            .as_str()
            .expect("reason is a string")
            .contains(&dist.display().to_string())
    );
}

// ---------------------------------------------------------------------------
// Step 4 — binary-spawn oracles (AC1, AC2, AC3, AC5)
// ---------------------------------------------------------------------------

#[test]
fn test_binary_serves_shell_and_spa_fallback_with_explicit_dist() {
    let _lock = serial();
    let root = scratch_dir("spawn-positive");
    let storage = root.join("storage");
    let dist = root.join("dist");
    write_shell(&dist, "MARKER-SPAWN-POSITIVE");
    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_standalone(
        &storage,
        port,
        &[format!("{}={}", web_dist::CLI_FLAG, dist.display())],
        &[],
        &stderr_path,
    );
    drop(wait_for_port(port));

    let (status, raw) = http_get(port, "/");
    assert_eq!(status, 200, "GET / must serve the shell: {raw}");
    assert!(
        raw.to_ascii_lowercase().contains("content-type: text/html"),
        "the shell is HTML: {raw}"
    );
    assert!(
        raw.contains("MARKER-SPAWN-POSITIVE"),
        "GET / serves the fixture the CLI flag named: {raw}"
    );

    let (status, raw) = http_get(port, "/client-route");
    assert_eq!(status, 200, "a client route falls back to the shell: {raw}");
    assert!(
        raw.contains("MARKER-SPAWN-POSITIVE"),
        "the SPA fallback serves the shell: {raw}"
    );

    let (status, raw) = http_get(port, "/missing.js");
    assert_eq!(
        status, 404,
        "an asset-like path must 404, never fall back to the shell: {raw}"
    );
}

#[test]
fn test_binary_degrades_loudly_with_missing_dist() {
    let _lock = serial();
    let root = scratch_dir("spawn-negative");
    let storage = root.join("storage");
    let empty = root.join("empty-dist");
    std::fs::create_dir_all(&empty).expect("create empty dist dir");
    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_standalone(
        &storage,
        port,
        &[format!("{}={}", web_dist::CLI_FLAG, empty.display())],
        &[],
        &stderr_path,
    );

    // Both structured lines, and the composition root's line names the path.
    let stderr = wait_for_stderr(&stderr_path, |text| {
        text.contains("\"event\":\"web.launch_failed\"")
            && text.contains("\"event\":\"web.dist_missing\"")
    });
    assert!(
        stderr.contains(&empty.display().to_string()),
        "web.dist_missing names the offending path: {stderr}"
    );
    let dist_missing_line = stderr
        .lines()
        .find(|line| line.contains("\"event\":\"web.dist_missing\""))
        .expect("web.dist_missing line");
    assert!(
        dist_missing_line.contains("\"dist_candidates\"")
            && dist_missing_line.contains("\"reason\""),
        "the diagnostic payload carries candidates and reason: {dist_missing_line}"
    );

    // Refuse the launch means NO socket: the port stays closed.
    let refused = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    );
    assert!(
        refused.is_err(),
        "a missing dist must refuse the launch, never bind and 404: {refused:?}"
    );
}

#[test]
fn test_binary_env_var_dist_is_served() {
    let _lock = serial();
    let root = scratch_dir("spawn-env");
    let storage = root.join("storage");
    let dist = root.join("dist");
    write_shell(&dist, "MARKER-SPAWN-ENV");
    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_standalone(
        &storage,
        port,
        &[],
        &[(web_dist::ENV_VAR, dist.display().to_string())],
        &stderr_path,
    );
    drop(wait_for_port(port));
    let (status, raw) = http_get(port, "/");
    assert_eq!(status, 200, "the env-named dist is served: {raw}");
    assert!(
        raw.contains("MARKER-SPAWN-ENV"),
        "GET / serves the env fixture with no CLI flag at all: {raw}"
    );
}

#[test]
fn test_binary_cli_dist_overrides_env() {
    let _lock = serial();
    let root = scratch_dir("spawn-cli-wins");
    let storage = root.join("storage");
    let cli_dist = root.join("cli-dist");
    let env_dist = root.join("env-dist");
    write_shell(&cli_dist, "MARKER-CLI-WINS");
    write_shell(&env_dist, "MARKER-ENV-LOSES");
    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_standalone(
        &storage,
        port,
        &[format!("{}={}", web_dist::CLI_FLAG, cli_dist.display())],
        &[(web_dist::ENV_VAR, env_dist.display().to_string())],
        &stderr_path,
    );
    drop(wait_for_port(port));
    let (status, raw) = http_get(port, "/");
    assert_eq!(status, 200, "the CLI-named dist is served: {raw}");
    assert!(
        raw.contains("MARKER-CLI-WINS"),
        "the CLI flag wins over the env var: {raw}"
    );
    assert!(
        !raw.contains("MARKER-ENV-LOSES"),
        "the env fixture is NOT served when the CLI flag is present: {raw}"
    );
}

// ---------------------------------------------------------------------------
// Step 5 — the non-UTF-8 diagnostic oracle (M4/R1)
// ---------------------------------------------------------------------------

/// `json!` interpolates a `PathBuf` through `to_value(..).unwrap()`, and
/// `Path`'s `Serialize` ERRORS on non-UTF-8 bytes — so a payload built from
/// `dist.candidates()` panics inside the launcher and `web.launch_failed` is
/// never emitted. This oracle EXECUTES the payload seam: it is green only
/// because `missing_payload()` is built from lossy strings and `display()`.
#[cfg(unix)]
#[test]
fn test_non_utf8_path_does_not_panic_in_lossy_diagnostics() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let bad = OsStr::from_bytes(b"/var/tmp/w68-\xff-dist");
    let dist = ResolvedDist::Missing {
        candidates: vec![PathBuf::from(bad)],
        explicit: Some(PathBuf::from(bad)),
    };

    let executed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let payload = dist.missing_payload();
        let text = payload.to_string();
        (text, dist.lossy_candidates(), dist.error_message())
    }));
    let (text, lossy, message) =
        executed.expect("the diagnostic payload must not panic on a non-UTF-8 path");

    assert!(
        text.contains("dist_candidates"),
        "the executed payload carries the candidate list: {text}"
    );
    assert!(
        text.contains('\u{fffd}'),
        "the lossy form shows the replacement character: {text}"
    );
    assert!(
        lossy.iter().any(|candidate| candidate.contains('\u{fffd}')),
        "lossy_candidates() is total for non-UTF-8 paths: {lossy:?}"
    );
    assert!(
        message.contains('\u{fffd}'),
        "error_message() is total for non-UTF-8 paths: {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Step 6 — the gitignore guard (AC4/M6)
// ---------------------------------------------------------------------------

/// `web-ui/dist` is BUILD OUTPUT: the wiring consumes it and never commits it,
/// and the build never requires it to exist (W6.8/AC4).
///
/// `web-ui/` is tracked at HEAD, so the first guard is only a cheap precondition
/// and does not fire on a normal checkout. The load-bearing guard is the one
/// below: `web-ui/dist` is ignored build output (`web-ui/.gitignore:2` — `/dist/`,
/// a directory-only rule), so it is absent until the UI is built.
///
/// A present-but-unbuilt tree (`web-ui/` present, `web-ui/dist` absent — the
/// normal state of a clean checkout) SKIPS only the ignore-EFFECT check with a
/// printed reason; the `.gitignore` rule text is asserted before that skip.
#[test]
fn test_web_dist_is_gitignored_build_output() {
    if !Path::new("web-ui").exists() {
        println!(
            "SKIP test_web_dist_is_gitignored_build_output: web-ui/ is absent — \
             this guard presumes W6.5's untracked web-ui/ tree (W6.8/AC4, M6)"
        );
        return;
    }

    // Rule text first: it needs no build output, so it runs on an unbuilt tree too.
    let ignore_body =
        std::fs::read_to_string("web-ui/.gitignore").expect("web-ui/.gitignore readable");
    assert!(
        ignore_body.lines().any(|line| line.trim() == "/dist/"),
        "web-ui/.gitignore must keep its `/dist/` line: {ignore_body:?}"
    );

    if !Path::new("web-ui/dist").is_dir() {
        println!(
            "SKIP test_web_dist_is_gitignored_build_output: web-ui/dist is absent — \
             no build output exists to ignore, so the ignore EFFECT is unverifiable \
             here (clean/unbuilt checkout) (W6.8/AC4, M6)"
        );
        return;
    }

    // PRIMARY oracle: git itself says the dist is ignored and untracked.
    let ignored = Command::new("git")
        .args(["check-ignore", "-q", "--", "web-ui/dist"])
        .status()
        .expect("git check-ignore runs");
    assert!(
        ignored.success(),
        "web-ui/dist must be gitignored build output (AC4)"
    );

    let tracked = Command::new("git")
        .args(["ls-files", "--error-unmatch", "web-ui/dist"])
        .output()
        .expect("git ls-files runs");
    assert!(
        !tracked.status.success(),
        "web-ui/dist must NOT be tracked: {}",
        String::from_utf8_lossy(&tracked.stdout)
    );
}
