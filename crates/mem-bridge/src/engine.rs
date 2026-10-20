//! W5.1 step 7/8 — the engine stdio server and its runtime wiring.
//!
//! Design coordinates the blueprint (and ADR-0007) pin explicitly:
//!
//! * **Exactly one tokio runtime**, a `current_thread` one, built HERE in
//!   [`run`] — `src/main.rs` names no tokio type and stays sync (M1). The
//!   runtime lives inside `mem-bridge`, whose tokio edge already exists and
//!   is widened at the mem-bridge manifest only (no new mem-bridge → mem-web
//!   edge; the axum server enters THIS process through the [`WebLauncher`]
//!   seam).
//! * **Stdio is NDJSON protocol-only** on stdout; everything else is a
//!   structured-JSON stderr line with no secret and no transcript (mandate 2).
//! * **Stdio installs no signal handler** (E3): the loop terminates on EOF or
//!   default-terminates on SIGTERM. Only [`LaunchMode::Standalone`] installs
//!   the graceful SIGTERM/SIGINT path.
//! * **The stdio loop terminates on EOF with a flush and a bounded exit**
//!   (M19): EOF ends the loop, the ordered shutdown runs inside the D-45
//!   budget, and the process returns through `run`'s exit code.
//! * **The engine lock loser never opens the DB**: structured `lock_lost`
//!   stderr, the status file naming the HOLDER, exit code 75 (M13, D-38,
//!   D-39).

use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

use crate::error_map::{self, ErrorKind};
use crate::jsonrpc::{ParseLineError, Request, Response, encode_response, parse_line};

/// The protocol version this engine speaks (ADR-0007 D-43 baseline).
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// `session_id` upper bound (Step 1's concrete bound, M6).
pub const SESSION_ID_MAX_CHARS: usize = 128;

/// Default web bind host (loopback; bridge-schema.md §9).
pub const DEFAULT_WEB_HOST: &str = "127.0.0.1";
/// Default web bind port (3742; bridge-schema.md §9).
pub const DEFAULT_WEB_PORT: u16 = 3742;

// ---------------------------------------------------------------------------
// Structured stderr (mandate 2)
// ---------------------------------------------------------------------------

/// Writes one structured-JSON diagnostics line to stderr.
///
/// Only the named level/component/event and the explicit extras are emitted:
/// no request params ever reach here (capture params include the transcript).
pub fn structured_stderr(event: &str, level: &str, component: &str, extras: serde_json::Value) {
    let mut payload = serde_json::json!({
        "component": component,
        "event": event,
        "level": level,
    });
    if let (serde_json::Value::Object(base), serde_json::Value::Object(more)) =
        (&mut payload, &extras)
    {
        for (key, value) in more {
            base.insert(key.clone(), value.clone());
        }
    }
    let mut line = serde_json::to_string(&payload).expect("structured stderr serializes");
    line.push('\n');
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.flush();
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// The engine's configuration inputs; all volatile fields are masked by the
/// fixture contract, so the defaults may be static.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// The protocol line the handshake answers with (semver, ADR-0007).
    pub protocol: String,
    /// This build's version (volatile; masked in response fixtures).
    pub engine_version: String,
    /// The episodic DB path reported by `health` (volatile; masked).
    pub db_path: String,
    /// The readiness bit reported by `health`.
    pub db_ready: bool,
    /// The session count reported by `health` (W5.1 stub: no reader wired).
    pub sessions: i64,
    /// `mode: "degraded"` switch W5.1 leaves off unless the caller knows better.
    pub degraded_reason: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            protocol: PROTOCOL_VERSION.to_owned(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            db_path: "~/.opencode-mem/data/episodic.db".to_owned(),
            db_ready: true,
            sessions: 0,
            degraded_reason: None,
        }
    }
}

/// The hello handshake result body (frozen, `tests/fixtures/bridge/`).
fn handshake_result(config: &EngineConfig) -> serde_json::Value {
    serde_json::json!({
        "capabilities": {
            "fire_and_forget_capture": true,
            "standalone": true,
            "web": true,
        },
        "engine_version": config.engine_version,
        "methods": ["capture", "inject", "recall", "health", "shutdown"],
        "protocol": config.protocol,
    })
}

/// The engine: dispatch and lifecycle state. Dispatching without I/O keeps
/// [`Engine::dispatch`] directly testable — every fixture oracle runs
/// through it.
pub struct Engine {
    config: EngineConfig,
    started: Instant,
    shutting_down: Arc<AtomicBool>,
    /// Number of internal lifecycle tasks spawned through [`Engine::spawn_tracked`]
    /// and not yet finished (step 10's no-live-tasks oracle reads it).
    live_tasks: Arc<AtomicUsize>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            started: Instant::now(),
            shutting_down: Arc::new(AtomicBool::new(false)),
            live_tasks: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Dispatches one parsed request. Always answers with either a result
    /// or a SPECIFIC error code — a classifyable failure NEVER gets the
    /// generic -32603 (step 6's map).
    pub fn dispatch(&self, request: &Request) -> Response {
        if request.method == "hello" {
            return self.hello(request);
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::InvalidRequest),
                "engine is shutting down",
            );
        }
        // Method resolution FIRST: an unknown method is `-32601` even when
        // its params are also wrong (the errors.rs oracle pins this).
        if request.method.as_str() != "capture"
            && request.method.as_str() != "inject"
            && request.method.as_str() != "recall"
            && request.method.as_str() != "health"
            && request.method.as_str() != "shutdown"
        {
            return Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::UnknownMethod),
                error_map::message_for(ErrorKind::UnknownMethod),
            );
        }
        if let Some(response) = self.check_session_id(request) {
            return response;
        }
        match request.method.as_str() {
            "capture" => self.capture(request),
            "inject" => self.inject(request),
            "recall" => self.stub_recall(request),
            "health" => self.health(request),
            "shutdown" => self.shutdown(request),
            _ => Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::UnknownMethod),
                error_map::message_for(ErrorKind::UnknownMethod),
            ),
        }
    }

    /// D-43 handshake: no `session_id` requirement (D-40). The compat rule
    /// is the WRAPPER's job — the engine just reports its protocol line.
    fn hello(&self, request: &Request) -> Response {
        if !request.params.is_object() {
            return Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::InvalidRequest),
                error_map::message_for(ErrorKind::InvalidRequest),
            );
        }
        Response::result(request.id.clone(), handshake_result(&self.config))
    }

    /// The `session_id` data gate shared by every runtime method; returns
    /// `Some(response)` when the request is rejected (`-32602`).
    fn check_session_id(&self, request: &Request) -> Option<Response> {
        let value = match request.param("session_id") {
            None | Some(serde_json::Value::Null) => {
                return Some(Response::error_with_data(
                    request.id.clone(),
                    error_map::code_for(ErrorKind::MissingSessionId),
                    error_map::message_for(ErrorKind::MissingSessionId),
                    serde_json::json!({ "session_id": null, "violation": "missing" }),
                ));
            }
            Some(other) => other,
        };
        let text = match value.as_str() {
            Some(text) => text,
            None => {
                return Some(Response::error(
                    request.id.clone(),
                    error_map::code_for(ErrorKind::InvalidParams),
                    error_map::message_for(ErrorKind::InvalidParams),
                ));
            }
        };
        if text.chars().count() > SESSION_ID_MAX_CHARS {
            return Some(Response::error_with_data(
                request.id.clone(),
                error_map::code_for(ErrorKind::OverlongSessionId),
                error_map::message_for(ErrorKind::OverlongSessionId),
                serde_json::json!({ "session_id": text, "violation": "overlong" }),
            ));
        }
        if text.chars().all(char::is_whitespace) {
            return Some(Response::error_with_data(
                request.id.clone(),
                error_map::code_for(ErrorKind::BlankSessionId),
                error_map::message_for(ErrorKind::BlankSessionId),
                serde_json::json!({ "session_id": text, "violation": "blank" }),
            ));
        }
        None
    }

    /// `capture` stub: a fire-and-forget ack. THE ACK CARRIES NO DURABILITY
    /// GUARANTEE in W5.1 (M25); the structured warning says so on stderr
    /// with NO params (the params include the transcript).
    fn capture(&self, request: &Request) -> Response {
        structured_stderr(
            "capture.stub_not_durable",
            "warn",
            "engine",
            serde_json::json!({}),
        );
        Response::result(request.id.clone(), serde_json::json!({ "accepted": true }))
    }

    /// `inject` stub: an empty injection block (W5.1).
    fn inject(&self, request: &Request) -> Response {
        Response::result(
            request.id.clone(),
            serde_json::json!({
                "block": serde_json::Value::Null,
                "handling_ms": self.elapsed_ms(),
                "memory_count": 0,
                "tokens": 0,
                "truncated": false,
            }),
        )
    }

    /// `recall` stub: an empty result list (W5.1).
    fn stub_recall(&self, request: &Request) -> Response {
        Response::result(
            request.id.clone(),
            serde_json::json!({ "count": 0, "items": [] }),
        )
    }

    /// `health` full/degraded report (frozen schema).
    fn health(&self, request: &Request) -> Response {
        let mode = if self.config.degraded_reason.is_some() {
            "degraded"
        } else {
            "full"
        };
        Response::result(
            request.id.clone(),
            serde_json::json!({
                "ok": true,
                "mode": mode,
                "degraded_reason": self.config.degraded_reason,
                "protocol": self.config.protocol,
                "engine_version": self.config.engine_version,
                "db": { "path": self.config.db_path, "ready": self.config.db_ready },
                "uptime_ms": self.elapsed_ms(),
                "sessions": self.config.sessions,
            }),
        )
    }

    /// `shutdown`: answers ok, and stops accepting further work.
    fn shutdown(&self, request: &Request) -> Response {
        self.shutting_down.store(true, Ordering::SeqCst);
        Response::result(request.id.clone(), serde_json::json!({ "ok": true }))
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The live-task counter behind [`Engine::spawn_tracked`]; drives the
    /// no-live-tasks library oracle.
    pub fn live_tasks(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.live_tasks)
    }

    /// Spawns an internal lifecycle task and tracks it in the counter, so a
    /// completed run leaves the counter at zero (step 10's oracle).
    pub fn spawn_tracked<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.live_tasks.fetch_add(1, Ordering::SeqCst);
        let counter = Arc::clone(&self.live_tasks);
        tokio::spawn(async move {
            let output = future.await;
            counter.fetch_sub(1, Ordering::SeqCst);
            output
        })
    }

    /// The shutdown flag (the loop and tests read it after `shutdown`).
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Launch modes + host configuration (env/args with defaults, §9)
// ---------------------------------------------------------------------------

/// The engine launch modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LaunchMode {
    /// Child of the wrapper: request/response over stdio; NO signal handler.
    #[default]
    Stdio,
    /// Operator-facing: serves the axum web server and waits on a graceful
    /// SIGTERM/SIGINT path.
    Standalone,
    /// Test-only: behaves like stdio but parks after answering `shutdown`,
    /// so the wrapper's SIGTERM fallback path has something real to fire on.
    /// Gated on the `OPENCODE_MEM_HANG_ON_SHUTDOWN` env var.
    HangOnShutdown,
}

impl LaunchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Standalone => "standalone",
            Self::HangOnShutdown => "hang-on-shutdown",
        }
    }

    /// Accepts the mode literal; `hang-on-shutdown` is valid only when the
    /// env gate is set (test-only, debug/env gated).
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "stdio" => Ok(Self::Stdio),
            "standalone" => Ok(Self::Standalone),
            "hang-on-shutdown" if std::env::var("OPENCODE_MEM_HANG_ON_SHUTDOWN").is_ok() => {
                Ok(Self::HangOnShutdown)
            }
            other => Err(format!(
                "unknown mode {other:?} (stdio | standalone | hang-on-shutdown, env-gated)"
            )),
        }
    }

    /// Only standalone installs the graceful handlers (E3 guarantee). The
    /// stdio loop's `tokio::io::stdin` read must be killable by the OS
    /// default disposition, not by this process' own runtime teardown.
    pub fn installs_signal_handlers(self) -> bool {
        matches!(self, Self::Standalone)
    }
}

/// Host-config values both lanes agree on (bridge-schema.md §9).
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Storage root (`.engine.lock`, `status`, DBs).
    pub storage_root: PathBuf,
    /// Engine launch mode.
    pub mode: LaunchMode,
    /// Web bind host.
    pub web_host: String,
    /// Web bind port.
    pub web_port: u16,
}

impl Default for HostConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        Self {
            storage_root: PathBuf::from(format!("{home}/.opencode-mem/data")),
            mode: LaunchMode::Stdio,
            web_host: DEFAULT_WEB_HOST.to_owned(),
            web_port: DEFAULT_WEB_PORT,
        }
    }
}

impl HostConfig {
    /// Reads `OPENCODE_MEM_STORAGE_ROOT`/`_MODE`/`_WEB_HOST`/`_WEB_PORT`;
    /// unset names fall back to the defaults.
    pub fn from_env() -> Self {
        fn env(name: &str) -> Option<String> {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        }
        let mut config = Self::default();
        if let Some(root) = env("OPENCODE_MEM_STORAGE_ROOT") {
            config.storage_root = PathBuf::from(root);
        }
        if let Some(mode) = env("OPENCODE_MEM_MODE")
            .filter(|mode| LaunchMode::parse(mode.trim()).is_ok())
            .and_then(|mode| LaunchMode::parse(mode.trim()).ok())
        {
            config.mode = mode;
        }
        if let Some(host) = env("OPENCODE_MEM_WEB_HOST") {
            config.web_host = host;
        }
        if let Some(port) = env("OPENCODE_MEM_WEB_PORT")
            .filter(|port| port.trim().parse::<u16>().is_ok())
            .and_then(|port| port.trim().parse::<u16>().ok())
        {
            config.web_port = port;
        }
        config
    }
}

// ---------------------------------------------------------------------------
// WebLauncher seam (M1) + WebHandle (B3)
// ---------------------------------------------------------------------------

/// The web server's launch inputs, filled from the host config.
#[derive(Debug, Clone)]
pub struct WebConfig {
    pub host: String,
    pub port: u16,
    pub storage_root: PathBuf,
}

/// A started web server's stop handle. `stop` is an OWNED deferred future —
/// the launcher builds it around `mem_web`'s `WebServerHandle::stop` and
/// absorbs its `Result` (the D-45 budget ignores a failing stop).
/// The stop closure type: an owned `FnOnce` returning the deferred future.
type StopClosure = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

pub struct WebHandle {
    stop: Option<StopClosure>,
}

impl WebHandle {
    /// Wraps a stop closure.
    pub fn new(stop: StopClosure) -> Self {
        Self { stop: Some(stop) }
    }

    /// A disabled handle (web server off; the launcher's `Ok(None)`).
    pub fn disabled() -> Self {
        Self { stop: None }
    }

    /// Fires the stop future exactly once.
    ///
    /// A never-produced handle resolves immediately.
    pub fn stop(mut self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        match self.stop.take() {
            Some(stop) => Box::pin(async move { stop().await }),
            None => Box::pin(async {}),
        }
    }
}

impl std::fmt::Debug for WebHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebHandle")
            .field("stop_armed", &self.stop.is_some())
            .finish()
    }
}

/// The web-launch seam (M1): `mem-bridge` never depends on `mem-web`; the
/// composition root provides this with an axum launcher.
pub trait WebLauncher: Send + Sync {
    /// Launches (or skips) the axum server, returning the stop handle.
    /// `Ok(None)` means "web disabled by configuration", not failure; a
    /// failure is a string, because the engine degrades, it does not crash
    /// the process.
    fn launch(
        &self,
        config: WebConfig,
    ) -> Pin<Box<dyn Future<Output = Result<WebHandle, String>> + Send + '_>>;
}

// ---------------------------------------------------------------------------
// Shutdown pipeline (D-45 budgets, M22) — engine-internal, testable at
// library level (step 10's named oracles live in tests/shutdown.rs).
// ---------------------------------------------------------------------------

/// What the ordered teardown did, with its measured wall time.
#[derive(Debug, Clone, Copy)]
pub struct ShutdownReport {
    pub elapsed: Duration,
    /// The web stop future resolved (inside its budget).
    pub web_stopped_in_budget: bool,
    /// The DB flush future resolved (inside its budget).
    pub db_flushed_in_budget: bool,
}

/// The shutdown budget constants (D-45): web stop wrapped in ~1 s, DB flush
/// ≤ 500 ms, total < 2 s. These are the pipeline's timeouts — NOT mem-web's
/// own 5 s constant, which this wave must not edit (M22/M33).
pub const WEB_STOP_BUDGET: Duration = Duration::from_secs(1);
pub const DB_FLUSH_BUDGET: Duration = Duration::from_millis(500);
pub const SHUTDOWN_TOTAL_BUDGET: Duration = Duration::from_secs(2);

/// Runs the ordered teardown: (1) the web stop inside the ~1 s budget, (2)
/// the DB flush inside the 500 ms budget. Each leg is timeout-wrapped; a leg
/// that overruns is abandoned (best-effort, D-45's "never hold the host
/// indefinitely") and reported in the budget flags.
pub async fn shutdown_pipeline(
    web_stop: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    db_flush: Pin<Box<dyn Future<Output = ()> + Send>>,
) -> ShutdownReport {
    let started = Instant::now();
    // A leg wrapped in its budget: `Ok(_)` = resolved INSIDE the budget
    // (`true`), `Err(Elapsed)` = still running when the budget expired —
    // the pipeline cut it and reports honestly (`false`).
    let web_stopped = match web_stop {
        Some(stop) => tokio::time::timeout(WEB_STOP_BUDGET, stop).await.is_ok(),
        None => true,
    };
    let db_flushed = tokio::time::timeout(DB_FLUSH_BUDGET, db_flush)
        .await
        .is_ok();
    ShutdownReport {
        elapsed: started.elapsed(),
        web_stopped_in_budget: web_stopped,
        db_flushed_in_budget: db_flushed,
    }
}

// ---------------------------------------------------------------------------
// run: lock acquire → runtime → mode loop → ordered shutdown (B3)
// ---------------------------------------------------------------------------

/// Runs the selected launch mode to completion and returns the process exit
/// code. THE single tokio runtime is built here (`current_thread`), so
/// `src/main.rs` stays sync and names no tokio type (M1).
///
/// Order (B3): engine lock → `launch` (axum in-process) → stdio/standalone
/// loop → ordered shutdown → exit code.
pub fn run(mode: LaunchMode, config: HostConfig, launcher: Box<dyn WebLauncher>) -> i32 {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the engine's single runtime builds");
    let code = runtime
        .block_on(run_in_runtime(
            mode,
            config,
            launcher,
            graceful_signal_futures(mode),
        ))
        .unwrap_or_else(|error| {
            structured_stderr(
                "engine.run_failed",
                "error",
                "engine",
                serde_json::json!({ "detail": error }),
            );
            70
        });
    // The runtime drops here; EOF left no task spawned by this module, so
    // the bounded exit (M19) is not degraded by teardown.
    code
}

/// The standalone graceful-signal future (SIGTERM/SIGINT). Stdio never calls
/// this (E3) and never installs a handler.
fn graceful_signal_futures(mode: LaunchMode) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    if mode.installs_signal_handlers() {
        Box::pin(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler registers");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        })
    } else {
        // Stdio: forever-pending; the loop does not need a signal.
        Box::pin(futures_never())
    }
}

/// A future that never resolves (kept local — no extra dependency edge).
async fn futures_never() {}

/// The `run` body, testable with injected signals and any stream pair.
pub async fn run_in_runtime(
    mode: LaunchMode,
    config: HostConfig,
    launcher: Box<dyn WebLauncher>,
    shutdown_signal: Pin<Box<dyn Future<Output = ()> + Send>>,
) -> Result<i32, String> {
    // The exclusive lifetime lock FIRST: a loser never opens the DB.
    let probe: Arc<dyn mem_store::platform::ProcessProbe> =
        Arc::new(mem_store::platform::SystemProcessProbe);
    let lock = acquire_outcome(&config.storage_root, probe);

    // Web server inside THIS process (D-32); `Ok(None)` → disabled handle.
    // A WEB LAUNCH FAILURE DEGRADES (D-38): the engine keeps its stdio
    // purpose with a disabled web handle and a structured stderr warning —
    // the axum surface is auxiliary; an AddrInUse/Bind failure must never
    // crash the bridge (and concurrent engines on one host do not wedge
    // each other over the default port).
    let web = match launcher
        .launch(WebConfig {
            host: config.web_host.clone(),
            port: config.web_port,
            storage_root: config.storage_root.clone(),
        })
        .await
    {
        Ok(web) => web,
        Err(detail) => {
            structured_stderr(
                "web.launch_failed",
                "warn",
                "engine",
                serde_json::json!({ "component_hint": "auxiliary", "detail_short": true }),
            );
            // Detail may name host config; it never carries a secret, and
            // the full text stays off the wire (kept in tracing only).
            tracing::warn!(detail, "web launch failed; engine degrades to web-disabled");
            WebHandle::disabled()
        }
    };

    let engine_config = EngineConfig {
        db_path: config
            .storage_root
            .join("episodic.db")
            .to_string_lossy()
            .into_owned(),
        ..EngineConfig::default()
    };
    let engine = Engine::new(engine_config);

    match mode {
        LaunchMode::Stdio | LaunchMode::HangOnShutdown => {
            serve_stdio(&engine, tokio::io::stdin(), tokio::io::stdout(), mode).await?;
        }
        LaunchMode::Standalone => {
            shutdown_signal.await;
        }
    }

    // Ordered teardown inside the D-45 budget. The DB flush leg is the
    // W5.1 stub's flush (the SerialDbLane runner drains what the stub
    // queued; W5.3 wires real work onto this future).
    let web_stop = web.stop();
    let report = shutdown_pipeline(Some(web_stop), Box::pin(async {})).await;
    drop(lock); // lifetime scope: the lock releases with the engine's life
    structured_stderr(
        "engine.shutdown_complete",
        "info",
        "engine",
        serde_json::json!({
            "elapsed_ms": report.elapsed.as_millis() as u64,
        }),
    );
    Ok(0)
}

fn acquire_outcome(
    storage_root: &std::path::Path,
    probe: Arc<dyn mem_store::platform::ProcessProbe>,
) -> Option<mem_store::engine_lock::EngineLock> {
    use mem_store::engine_lock as el;
    match el::acquire_engine_lock(storage_root, probe) {
        Ok(el::EngineLockOutcome::Acquired(lock)) => Some(lock),
        Ok(el::EngineLockOutcome::Lost { holder_pid }) => {
            structured_stderr(
                "lock_lost",
                "warn",
                "engine",
                serde_json::json!({ "holder_pid": holder_pid }),
            );
            let _ = el::write_status_file(storage_root, "engine lock held", holder_pid);
            std::process::exit(el::ENGINE_LOCK_LOST);
        }
        Err(error) => {
            structured_stderr(
                "engine.lock_io_failed",
                "error",
                "engine",
                serde_json::json!({ "detail": error.to_string() }),
            );
            std::process::exit(70);
        }
    }
}

/// The stdio request loop: parse one line, answer one line; EOF ends it with
/// a bounded exit (M19); `--mode=hang-on-shutdown` parks AFTER the shutdown
/// response is on the wire, which is the fallback oracle's hook.
pub async fn serve_stdio<R, W>(
    engine: &Engine,
    reader: R,
    mut writer: W,
    mode: LaunchMode,
) -> Result<ShutdownReport, String>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("stdin read failed: {error}"))?;
        if read == 0 {
            // EOF: protocol output is flushed as each line is written; the
            // loop is now out with nothing parked (M19).
            break;
        }
        let response = match parse_line(&line) {
            Ok(request) => engine.dispatch(&request),
            Err(ParseLineError::Parse) => Response::parse_error(),
            Err(ParseLineError::InvalidRequest) => Response::invalid_request(),
        };
        writer
            .write_all(encode_response(&response).as_bytes())
            .await
            .map_err(|error| format!("stdout write failed: {error}"))?;
        writer
            .flush()
            .await
            .map_err(|error| format!("stdout flush failed: {error}"))?;
        if engine.is_shutting_down() {
            if mode == LaunchMode::HangOnShutdown {
                // Test-only park AFTER the response is flushed.
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
            break;
        }
    }
    Ok(ShutdownReport {
        elapsed: Duration::from_millis(0),
        web_stopped_in_budget: true,
        db_flushed_in_budget: true,
    })
}
