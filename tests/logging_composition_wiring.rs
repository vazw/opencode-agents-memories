//! AC6 — the composition root installs the log sink at
//! `OPENCODE_MEM_LOG_FILE`, and the stats scan directory is derived from the
//! SAME sink.
//!
//! This is the regression oracle for the whole wiring: if `src/main.rs` kept
//! `mem_web::api::stats::default_log_dir()` (the real home) instead of
//! `sink.path().parent()`, the lines land in the scratch file while the reader
//! scans `$HOME/.opencode-mem` — the counters stay 0 and every assertion here
//! goes RED.
//!
//! House rules: child stdout is piped ONLY for the JSON-RPC acks; stderr goes
//! to a FILE; every wait is bounded; `std::env::set_var` is never called — the
//! log path is injected per child; the child is killed AND reaped on drop.

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use mem_store::serde_json::{self, Value};

/// How long startup (composition + web bind) may take.
const STARTUP_BOUND: Duration = Duration::from_secs(30);
/// How long any socket read/write may take.
const SOCKET_BOUND: Duration = Duration::from_secs(5);
/// The poll interval for the port, the banner, the token and the stats poll.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How long the JSON-RPC ack read may take.
const ACK_BOUND: Duration = Duration::from_secs(10);

const HELLO_LINE: &str = r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"log-wiring-test","min_protocol":"1.0.0","protocol":"1.0.0"}}"#;
const SKIPPED_CAPTURE: &str = r#"{"id":"capture-skip","jsonrpc":"2.0","method":"capture","params":{"session_id":"sess-log-wiring","capture_id":"019a0000-0000-7f00-8000-00000000c001","project_id":"/srv/log-wiring","agent_id":"agent-1"}}"#;
const DEAD_CAPTURE: &str = r#"{"id":"capture-dead","jsonrpc":"2.0","method":"capture","params":{"session_id":"sess-log-wiring","capture_id":"019a0000-0000-7f00-8000-00000000c002","project_id":"/srv/log-wiring","agent_id":"agent-1","agent_role":"bogus"}}"#;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique scratch dir under the repo-local `.tmp/scratch` (never `/tmp`).
fn scratch_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::current_dir().expect("repo root");
    let dir = root
        .join(".tmp")
        .join("scratch")
        .join("logging-composition-wiring")
        .join(format!("{prefix}-{}-{nanos}-{serial}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the scratch dir is creatable");
    dir
}

/// A child that is killed AND reaped when the guard drops.
struct Guarded {
    child: Child,
}

impl Drop for Guarded {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// A minimal shell: the launcher refuses to start without an `index.html`.
fn write_shell(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create the dist dir");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><body><div id=\"log-wiring\">fixture</div></body></html>",
    )
    .expect("write index.html");
}

/// Sends one NDJSON request and reads exactly one response line.
fn round_trip(stdin: &mut impl Write, stdout: &mut impl Read, line: &str) -> String {
    stdin.write_all(line.as_bytes()).expect("write request");
    if !line.ends_with('\n') {
        stdin.write_all(b"\n").expect("write NDJSON terminator");
    }
    stdin.flush().expect("flush request");
    let started = Instant::now();
    let mut response = String::new();
    let mut buffer = [0_u8; 1];
    loop {
        let read = stdout.read(&mut buffer).expect("read response");
        assert!(read > 0, "stdout hit EOF before a response");
        response.push(buffer[0] as char);
        if response.ends_with('\n') {
            return response;
        }
        assert!(
            started.elapsed() < ACK_BOUND,
            "no JSON-RPC ack within {ACK_BOUND:?}: {response:?}"
        );
    }
}

/// Polls `TcpStream::connect` until the port accepts, within the bound.
fn wait_for_port(port: u16) {
    let started = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                drop(stream);
                return;
            }
            Err(error) => {
                assert!(
                    started.elapsed() < STARTUP_BOUND,
                    "the web port {port} never accepted within {STARTUP_BOUND:?}: {error}"
                );
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// Polls a file until its contents satisfy `predicate`.
fn wait_for_file(path: &Path, predicate: impl Fn(&str) -> bool, what: &str) -> String {
    let started = Instant::now();
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if predicate(&text) {
            return text;
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "{what} never appeared at {path:?} within {STARTUP_BOUND:?}; contents: {text:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One bounded raw HTTP/1.1 GET on loopback.
fn http_get(port: u16, path: &str, token: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(SOCKET_BOUND))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(SOCKET_BOUND))
        .expect("set write timeout");
    let mut request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(token) = token {
        request.push_str(&format!("{}: {token}\r\n", mem_web::AUTH_HEADER));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(error) => panic!("HTTP read deadline on {path}: {error}"),
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

fn body_of(raw: &str) -> &str {
    raw.split_once("\r\n\r\n")
        .map(|(_head, body)| body)
        .unwrap_or_else(|| panic!("no body in {raw:?}"))
}

/// The daily `field` summed over the LAST TWO days. The midnight hardening
/// (same reasoning as AC5): the writer's stamp and the reader's clock may fall
/// on either side of local midnight, so the value may land on today or the day
/// before — but on exactly one of them.
fn last_two_days_sum(body: &Value, field: &str) -> u64 {
    let daily = body["data"]["daily"]
        .as_array()
        .expect("the stats answer carries the daily series");
    assert_eq!(daily.len(), 14, "the series is a 14-day window");
    daily[daily.len() - 2..]
        .iter()
        .map(|day| day[field].as_u64().unwrap_or(0))
        .sum()
}

/// The error bucket summed over the LAST TWO days.
fn last_two_days_bucket(body: &Value, name: &str) -> u64 {
    let daily = body["data"]["daily"]
        .as_array()
        .expect("the stats answer carries the daily series");
    daily[daily.len() - 2..]
        .iter()
        .map(|day| day["errors"][name].as_u64().unwrap_or(0))
        .sum()
}

#[test]
fn test_engine_writes_env_log_and_stats_reads_the_same_directory() {
    let root = scratch_dir("composed");
    let storage = root.join("storage");
    let dist = root.join("dist");
    let logs = root.join("logs");
    let log_file = logs.join("opencode-mem.log");
    std::fs::create_dir_all(&storage).expect("create the storage root");
    write_shell(&dist);

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let stderr = std::fs::File::create(&stderr_path).expect("create stderr file");

    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        // stdio mode: the engine serves JSON-RPC AND launches the web server.
        .arg("--mode=stdio")
        .arg(format!("--storage-root={}", storage.display()))
        .arg(format!("--web-port={port}"))
        .arg(format!("--web-dist={}", dist.display()))
        .env("OPENCODE_MEM_LOG_FILE", &log_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr));
    let mut guard = Guarded {
        child: command.spawn().expect("the engine binary spawns"),
    };
    let mut stdin = guard.child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(guard.child.stdout.take().expect("child stdout"));

    // The composition root installed the sink and primed the session banner at
    // the ENV path (not at `$HOME/.opencode-mem`).
    wait_for_file(
        &log_file,
        |text| text.contains("--- Session started: "),
        "the session banner",
    );

    wait_for_port(port);

    // hello → two captures (skipped + immediate dead-letter).
    round_trip(&mut stdin, stdout.get_mut(), HELLO_LINE);
    let skipped_ack = round_trip(&mut stdin, stdout.get_mut(), SKIPPED_CAPTURE);
    assert!(
        skipped_ack.contains("\"accepted\":true"),
        "the skipped capture is durably queued: {skipped_ack}"
    );
    let dead_ack = round_trip(&mut stdin, stdout.get_mut(), DEAD_CAPTURE);
    assert!(
        dead_ack.contains("\"accepted\":true"),
        "the dead capture is durably queued: {dead_ack}"
    );

    // The token is acquired the only honest way: a token-less request answers
    // 401 and is what makes the server write `.auth-token`.
    let (status, raw) = http_get(port, "/api/stats", None);
    assert_eq!(
        status, 401,
        "a token-less /api/stats request answers 401 and writes the token: {raw}"
    );
    let token = wait_for_file(
        &storage.join(".auth-token"),
        |text| !text.trim().is_empty(),
        "the local auth token",
    )
    .trim()
    .to_string();

    // Poll until BOTH signatures are visible through the reader that scans the
    // sink's directory.
    let started = Instant::now();
    let body: Value = loop {
        let (status, raw) = http_get(port, "/api/stats", Some(&token));
        assert_eq!(status, 200, "GET /api/stats answers 200: {raw}");
        let body: Value = serde_json::from_str(body_of(&raw)).expect("the stats answer is JSON");
        if last_two_days_sum(&body, "captureSkipped") == 1
            && last_two_days_bucket(&body, "Auto-capture final error") == 1
        {
            break body;
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "the counters never appeared within {STARTUP_BOUND:?}; last body: {body}"
        );
        std::thread::sleep(POLL_INTERVAL);
    };

    // The scan directory really is derived from the sink: the counters are
    // non-zero, and (as an extra guard) the persisted capture is still zero —
    // neither capture was persisted.
    assert_eq!(last_two_days_sum(&body, "captureSkipped"), 1, "{body}");
    assert_eq!(
        last_two_days_bucket(&body, "Auto-capture final error"),
        1,
        "{body}"
    );
    assert_eq!(last_two_days_sum(&body, "captures"), 0, "{body}");

    // Both lines are in the ENV-path file (writer side).
    let content = std::fs::read_to_string(&log_file).expect("the env log file is readable");
    assert!(
        content.contains("] Auto-capture skipped: {\""),
        "the skipped line is at the env path: {content}"
    );
    assert!(
        content.contains("] Auto-capture final error: {\""),
        "the final-error line is at the env path: {content}"
    );

    drop(stdin);
    drop(stdout);
    drop(guard);
}
