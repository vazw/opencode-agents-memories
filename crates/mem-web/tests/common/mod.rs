#![allow(dead_code)]

//! Std-only helpers shared by mem-web integration tests.
//!
//! Everything is hermetic: temp dirs live under `std::env::temp_dir()` only,
//! the process environment is never mutated, and the socket seam talks to a
//! loopback address only.
//!
//! Two seams are exposed:
//!
//! - the **router seam** — `build_router(..)` driven through
//!   `tower::ServiceExt::oneshot`; it never blocks, so it may run on the
//!   current-thread runtime.
//! - the **socket seam** — raw HTTP/1.1 over `std::net::TcpStream` against a
//!   real server bound to `127.0.0.1:0`. The helper is *synchronous* and
//!   blocking, so every socket-seam case MUST run it inside
//!   `tokio::task::spawn_blocking` (a blocking read on the runtime thread
//!   would starve the spawned server task).

pub mod capture;
pub mod fakes;
pub mod real_store;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mem_web::WebServerSettings;
use mem_web::api_auth::LocalTokenSource;
use mem_web::host_allowlist::HostAllowlist;
use mem_web::server::AppState;
use mem_web::web_auth::WebAuth;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A parsed HTTP response (status, headers, body bytes).
#[derive(Debug, Clone)]
pub struct RawResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response headers, in order.
    pub headers: Vec<(String, String)>,
    /// The raw body bytes.
    pub body: Vec<u8>,
}

impl RawResponse {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body decoded as lossy UTF-8.
    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The body read limit used when draining a response.
const BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Converts an axum response into a [`RawResponse`].
pub async fn response_to_raw(response: axum::response::Response) -> RawResponse {
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let body = axum::body::to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();
    RawResponse {
        status,
        headers,
        body,
    }
}

/// The default `Host` the router-level helper injects when the caller does
/// not supply one (an allowed value — see D-8).
pub const DEFAULT_HOST_HEADER: &str = "localhost:4747";

/// The upper bound on a socket-seam read.
pub const SOCKET_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The address a raw request connects to (loopback only).
pub type LoopbackAddr = SocketAddr;

/// A local-token source with a fixed value.
#[derive(Debug, Clone, Default)]
pub struct FixedLocalToken(pub Option<String>);

impl LocalTokenSource for FixedLocalToken {
    fn token(&self) -> Option<String> {
        self.0.clone()
    }
}

/// Builds an [`AppState`] for the router-level tests.
///
/// The bind host is `127.0.0.1`, so the D-8 allowlist enforces the loopback
/// names.
pub fn app_state(
    basic_auth: WebAuth,
    api_token: Option<&str>,
    local_token: Option<&str>,
    dist_dir: PathBuf,
) -> AppState {
    app_state_with_api(
        basic_auth,
        api_token,
        local_token,
        dist_dir,
        throwaway_log_dir(),
        mem_web::api::ports::ApiServices::unavailable(),
        "unknown",
        "127.0.0.1",
    )
}

/// Builds an [`AppState`] with an explicit port bundle, user identity and
/// bind host (the W6.2 tests need all three).
///
/// `log_dir` is the directory the stats handler scans for the daily series;
/// tests that never exercise the scan pass [`throwaway_log_dir`].
#[allow(clippy::too_many_arguments)]
pub fn app_state_with_api(
    basic_auth: WebAuth,
    api_token: Option<&str>,
    local_token: Option<&str>,
    dist_dir: PathBuf,
    log_dir: PathBuf,
    api: mem_web::api::ports::ApiServices,
    user_id: &str,
    bind_host: &str,
) -> AppState {
    AppState {
        basic_auth,
        api_token: api_token.map(str::to_owned),
        local_token: Arc::new(FixedLocalToken(local_token.map(str::to_owned))),
        dist_dir,
        log_dir,
        host_allowlist: HostAllowlist::for_bind_host(bind_host),
        api: Arc::new(api),
        user_id: user_id.to_owned(),
        bind_host: bind_host.to_owned(),
    }
}

/// A unique log directory for [`AppState`] ctors that never exercise the
/// daily-stats scan (created empty under `std::env::temp_dir()`, never
/// written).
pub fn throwaway_log_dir() -> PathBuf {
    temp_dir_unique("stats-logs")
}

/// The router seam for the REST surface: a request carrying the default
/// `Host`, an optional API token and an optional JSON body.
pub async fn api_request(
    router: axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> RawResponse {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", DEFAULT_HOST_HEADER);
    if let Some(token) = token {
        builder = builder.header(mem_web::AUTH_HEADER, token);
    }
    let body = match body {
        Some(payload) => {
            builder = builder.header("content-type", "application/json");
            axum::body::Body::from(payload.to_owned())
        }
        None => axum::body::Body::empty(),
    };
    let request = builder
        .body(body)
        .expect("the test request is a valid HTTP request");
    oneshot_response(router, request).await
}

/// The router seam: drives `router` with one request and collects the
/// response.
///
/// A default `Host: localhost:4747` header is injected when the caller does
/// not supply one. The cases that deliberately exercise a missing or empty
/// `Host` header bypass this helper and build the `axum::http::Request`
/// directly (see `oneshot_response`).
pub async fn router_request(
    router: axum::Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> RawResponse {
    let mut builder = axum::http::Request::builder().method(method).uri(path);
    let mut has_host = false;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        builder = builder.header(*name, *value);
    }
    if !has_host {
        builder = builder.header("host", DEFAULT_HOST_HEADER);
    }
    let request = builder
        .body(axum::body::Body::empty())
        .expect("the test request is a valid HTTP request");
    oneshot_response(router, request).await
}

/// The router seam for a hand-built request (no header injection at all).
pub async fn oneshot_response(
    router: axum::Router,
    request: axum::http::Request<axum::body::Body>,
) -> RawResponse {
    let response = tower::ServiceExt::oneshot(router, request)
        .await
        .expect("the router answers every request");
    response_to_raw(response).await
}

/// The socket seam: sends raw HTTP/1.1 bytes and collects the response.
///
/// The helper injects a `Host: 127.0.0.1:<port>` header when the request
/// does not carry one (D-8 — the gate refuses a request without an allowed
/// `Host`), and a `Connection: close` header so the server closes the
/// connection and the read-to-EOF body parse is deterministic. A
/// caller-supplied `Host` header is preserved verbatim.
pub fn raw_request(addr: SocketAddr, raw: &str) -> RawResponse {
    let mut stream =
        TcpStream::connect(addr).expect("the loopback server must accept the connection");
    stream
        .set_read_timeout(Some(SOCKET_READ_TIMEOUT))
        .expect("the read timeout must be settable");
    let request = with_default_headers(raw, addr.port());
    stream
        .write_all(request.as_bytes())
        .expect("the request must be written");

    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                // The bound is an upper bound: whatever arrived is parsed.
                break;
            }
            Err(error) => panic!("reading the response failed: {error}"),
        }
    }
    parse_response(&buffer)
}

/// Injects the default `Host` and `Connection` headers into a raw request.
fn with_default_headers(raw: &str, port: u16) -> String {
    let (head, body) = split_head_and_body(raw);
    let mut lines: Vec<String> = head
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect();
    if lines.is_empty() {
        lines.push(String::new());
    }

    let mut has_host = false;
    let mut has_connection = false;
    for line in lines.iter().skip(1) {
        if let Some((name, _)) = line.split_once(':') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("host") {
                has_host = true;
            }
            if name.eq_ignore_ascii_case("connection") {
                has_connection = true;
            }
        }
    }
    if !has_host {
        lines.insert(1, format!("Host: 127.0.0.1:{port}"));
    }
    if !has_connection {
        lines.push("Connection: close".to_owned());
    }

    let mut out = lines.join("\r\n");
    out.push_str("\r\n\r\n");
    out.push_str(body);
    out
}

/// Splits a raw request into its head and its body.
fn split_head_and_body(raw: &str) -> (&str, &str) {
    if let Some(index) = raw.find("\r\n\r\n") {
        return (&raw[..index], &raw[index + 4..]);
    }
    if let Some(index) = raw.find("\n\n") {
        return (&raw[..index], &raw[index + 2..]);
    }
    (raw, "")
}

/// Parses a raw HTTP/1.1 response.
fn parse_response(bytes: &[u8]) -> RawResponse {
    let (head, body) = match find_head_end(bytes) {
        Some(index) => (&bytes[..index], &bytes[index + 4..]),
        None => (bytes, &[][..]),
    };
    let head_text = String::from_utf8_lossy(head);
    let mut lines = head_text.lines();
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| {
            panic!("the response carries no status line: {status_line:?} (head: {head_text:?})")
        });
    let headers = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    RawResponse {
        status,
        headers,
        body: body.to_vec(),
    }
}

/// Finds the end of the response head (`\r\n\r\n`).
fn find_head_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Creates a fresh unique temp directory under `std::env::temp_dir()`.
///
/// The name carries the label, the process id, a nanosecond stamp and a
/// monotonic counter, so parallel test binaries never collide.
pub fn temp_dir_unique(label: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "opencode-mem-web-test-{label}-{}-{nanos}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("failed to create the unique temp dir");
    dir
}

/// A temp directory that is removed when the guard is dropped.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a guarded temp directory.
    pub fn new(label: &str) -> TempDir {
        TempDir {
            path: temp_dir_unique(label),
        }
    }

    /// The directory path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Starts a real server for the socket-seam cases.
///
/// The helper is async because `mem_web::server::start` is: the server is
/// spawned onto the current runtime and the handle carries the bound
/// address.
pub async fn spawn_server(
    settings: &WebServerSettings,
    state: AppState,
) -> mem_web::server::WebServerHandle {
    mem_web::server::start(settings, state)
        .await
        .expect("the test server must start")
        .expect("the test server must be enabled")
}
