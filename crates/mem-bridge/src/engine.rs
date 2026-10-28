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

use mem_episodic::store::EpisodicStore;
use mem_profile::profile_store::ProfileStore;
use mem_profile::prompt::UserPromptManager;
use mem_store::shard_manager::ShardManager;

use crate::capture::{CaptureParams, CaptureService};
use crate::error_map::{self, ErrorKind};
use crate::inject::{InjectParams, InjectService};
use crate::jsonrpc::{ParseLineError, Request, Response, encode_response, parse_line};
use crate::recall::{RecallRequest, RecallService};

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

/// The fixed fallback emitted when the diagnostics payload cannot be
/// serialized: still one valid JSON object, so the NDJSON framing survives.
const STDERR_SERIALIZE_FALLBACK: &str =
    r#"{"component":"bridge","event":"stderr.serialize_failed","level":"error"}"#;

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
    // stderr logger: a serialization failure emits a fixed valid JSON line, never breaking NDJSON framing (ADR-0013 D-3)
    let mut line = match serde_json::to_string(&payload) {
        Ok(line) => line,
        Err(error) => {
            tracing::error!(error = %error, "structured stderr payload failed to serialize");
            STDERR_SERIALIZE_FALLBACK.to_string()
        }
    };
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
    /// Whether the mixed-version open guard is currently bypassed (issue 45 O4):
    /// the env leg `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` or the config key
    /// `allowUnguardedOpen`, env winning. Reported by `health` so an operator can
    /// tell a guarded engine from an overridden one.
    pub open_guard_override: bool,
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
            open_guard_override: false,
        }
    }
}

/// The hello handshake result body (frozen, `tests/fixtures/bridge/`).
fn handshake_result(config: &EngineConfig) -> serde_json::Value {
    serde_json::json!({
        "capabilities": {
            // Issue 68 (D-43): this build's `inject` honours `include.compaction`
            // (inject.rs:341-347 and :589-591 — the leg renders the session tier
            // or returns an empty block, never the chat block). A build predating
            // issue 33 omitted the key; the wrapper then no-ops both hooks.
            "compaction": true,
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
    /// The W5.3 runtime services plus the W5.4 one-tool read service. `None`
    /// is the FIXTURE-only engine (`Engine::new`): `capture`/`inject`/`recall`
    /// degrade (-32001) and never touch storage; `hello`/`health`/`shutdown`
    /// are unchanged. The legacy `recall` shape needs no runtime (it reads
    /// nothing).
    runtime: Option<Arc<EngineRuntime>>,
}

/// The dependency-bearing runtime: composed ONCE by the engine root
/// (`run_in_runtime`), shared by dispatch.
pub struct EngineRuntime {
    pub capture: Arc<CaptureService>,
    pub inject: Arc<InjectService>,
    pub recall: Arc<RecallService>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            started: Instant::now(),
            shutting_down: Arc::new(AtomicBool::new(false)),
            live_tasks: Arc::new(AtomicUsize::new(0)),
            runtime: None,
        }
    }

    /// The production constructor: fixture fields + the runtime services.
    pub fn with_runtime(config: EngineConfig, runtime: Arc<EngineRuntime>) -> Self {
        let mut engine = Self::new(config);
        engine.runtime = Some(runtime);
        engine
    }

    /// Dispatches one parsed request. Always answers with either a result
    /// or a SPECIFIC error code — a classifyable failure NEVER gets the
    /// generic -32603 (step 6's map).
    pub async fn dispatch(&self, request: &Request) -> Response {
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
            "capture" => self.call_capture(request).await,
            "inject" => self.call_inject(request).await,
            "recall" => self.call_recall(request).await,
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

    /// `capture`: the D-48 durable path — validated params, the inbox
    /// enqueue is AWAITED (durable), only then `{"accepted":true}` comes
    /// back; a full inbox is `-32002`; degraded mode refuses writes with
    /// `-32001` and NO inbox write.
    async fn call_capture(&self, request: &Request) -> Response {
        let runtime = match &self.runtime {
            Some(runtime) => runtime,
            None => return degraded_capture(request),
        };
        if self.config.degraded_reason.is_some() {
            return degraded_capture(request);
        }

        // A2, WRITE path: `capture` with a missing or blank `project_id`
        // is `-32602` BEFORE any enqueue.
        let project_id = request
            .param("project_id")
            .and_then(serde_json::Value::as_str);
        match project_id {
            Some(project_id) if !project_id.trim().is_empty() => project_id.to_string(),
            _ => {
                return Response::error_with_data(
                    request.id.clone(),
                    error_map::code_for(ErrorKind::InvalidParams),
                    error_map::message_for(ErrorKind::InvalidParams),
                    serde_json::json!({ "project_id": null, "violation": "missing" }),
                );
            }
        };
        if let Some(response) = capture_validator(request) {
            return response;
        }

        let params: CaptureParams =
            serde_json::from_value(request.params.clone()).unwrap_or_default();
        use crate::capture::ObjError;
        match runtime.capture.enqueue(&params).await {
            Ok(()) => {}
            Err(ObjError::Full) => {
                return Response::error(
                    request.id.clone(),
                    error_map::code_for(ErrorKind::InboxFull),
                    error_map::message_for(ErrorKind::InboxFull),
                );
            }
            Err(ObjError::Store(_)) => {
                return Response::error(
                    request.id.clone(),
                    error_map::code_for(ErrorKind::Internal),
                    error_map::message_for(ErrorKind::Internal),
                );
            }
        }

        // Fire-and-forget drain — spawn-tracked so the shutdown oracle
        // sees it; the ACK remains the durability guarantee (queued now).
        let capture = runtime.capture.clone();
        self.spawn_tracked(async move {
            let _ = capture.drain_once().await;
        });

        Response::result(request.id.clone(), serde_json::json!({ "accepted": true }))
    }

    /// `inject`: the A2 absent-param-window — a missing or blank
    /// `project_id` never errors (-32602) and never queries; the assembly's
    /// empty result IS the answer.
    async fn call_inject(&self, request: &Request) -> Response {
        let runtime = match &self.runtime {
            Some(runtime) => runtime,
            None => return empty_inject(request, self.elapsed_ms()),
        };
        let inject_params: InjectParams =
            serde_json::from_value(request.params.clone()).unwrap_or_default();
        match runtime.inject.assemble(&inject_params).await {
            Ok(result) => Response::result(
                request.id.clone(),
                serde_json::json!({
                    "block": result.block,
                    "handling_ms": result.handling_ms,
                    "memory_count": result.memory_count,
                    "tokens": result.tokens,
                    "truncated": result.truncated,
                }),
            ),
            Err(crate::inject::InjectError::Overflow) => Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::InvalidParams),
                error_map::message_for(ErrorKind::InvalidParams),
            ),
            Err(crate::inject::InjectError::Scope(_)) => Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::InvalidParams),
                error_map::message_for(ErrorKind::InvalidParams),
            ),
            Err(crate::inject::InjectError::Store(_)) => Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::Internal),
                error_map::message_for(ErrorKind::Internal),
            ),
        }
    }

    /// `recall`: two accepted parameter shapes (D3). The legacy shape answers
    /// one logged empty page and never needs a runtime (D4); the canonical
    /// shape degrades without one (-32001) and answers the project-scoped page
    /// otherwise.
    async fn call_recall(&self, request: &Request) -> Response {
        let recall_request = match RecallRequest::from_params(&request.params) {
            Ok(recall_request) => recall_request,
            Err(_) => {
                return Response::error(
                    request.id.clone(),
                    error_map::code_for(ErrorKind::InvalidParams),
                    error_map::message_for(ErrorKind::InvalidParams),
                );
            }
        };
        let args = match recall_request {
            RecallRequest::Legacy => {
                return Response::result(request.id.clone(), RecallService::legacy_page());
            }
            RecallRequest::Canonical(args) => args,
        };
        let runtime = match &self.runtime {
            Some(runtime) => runtime,
            None => {
                return Response::error(
                    request.id.clone(),
                    error_map::code_for(ErrorKind::Degraded),
                    error_map::message_for(ErrorKind::Degraded),
                );
            }
        };
        match runtime.recall.recall(&args).await {
            Ok(page) => Response::result(request.id.clone(), page),
            Err(crate::recall::RecallError::InvalidParams) => Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::InvalidParams),
                error_map::message_for(ErrorKind::InvalidParams),
            ),
            Err(crate::recall::RecallError::Internal) => Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::Internal),
                error_map::message_for(ErrorKind::Internal),
            ),
        }
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
                "open_guard": { "override_active": self.config.open_guard_override },
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
    /// W7.1: the MCP transport over stdio — PARSED but not implemented. The
    /// binary refuses with exit 78 before it reads any config (M12); the
    /// protocol lands in W7.6.
    McpStdio,
    /// W7.1: the MCP transport over HTTP — parsed, refused with exit 78.
    McpHttp,
}

impl LaunchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Standalone => "standalone",
            Self::HangOnShutdown => "hang-on-shutdown",
            Self::McpStdio => "mcp-stdio",
            Self::McpHttp => "mcp-http",
        }
    }

    /// Accepts the mode literal; `hang-on-shutdown` is valid only when the
    /// env gate is set (test-only, debug/env gated). The MCP modes parse so
    /// the refusal is a typed, testable outcome rather than an unknown-mode
    /// parse error.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "stdio" => Ok(Self::Stdio),
            "standalone" => Ok(Self::Standalone),
            "mcp-stdio" => Ok(Self::McpStdio),
            "mcp-http" => Ok(Self::McpHttp),
            "hang-on-shutdown" if std::env::var("OPENCODE_MEM_HANG_ON_SHUTDOWN").is_ok() => {
                Ok(Self::HangOnShutdown)
            }
            other => Err(format!(
                "unknown mode {other:?} (stdio | standalone | hang-on-shutdown | mcp-stdio | mcp-http, env-gated)"
            )),
        }
    }

    /// Whether this process can actually serve the mode. The two MCP modes
    /// are recognised spellings that are REFUSED (exit 78) until W7.6.
    pub fn is_implemented(self) -> bool {
        !matches!(self, Self::McpStdio | Self::McpHttp)
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

/// The stores the ROOT composition root turns into `ApiServices::production(...)`.
///
/// mem-bridge never depends on mem-web (ADR-0007:584); the root crate owns that
/// edge and builds the mem-web services itself. Every field here is already a
/// mem-bridge dependency (`mem-store` / `mem-profile` / `mem-embed`), so this
/// struct adds no manifest entry and no new edge. Every field is `Send + Sync`,
/// so the boxed launcher future keeps its bounds.
#[derive(Clone)]
pub struct WebStoreDeps {
    /// The composed shard manager (a CLONE of the one `build_composed_runtime`
    /// opened — its manual `Clone` shares the connection manager, the init cell
    /// and the scope write locks, so it is still one manager).
    pub shards: ShardManager,
    /// The user-prompt store (opened by the composition root for the first time
    /// in W6.7).
    pub prompts: Arc<UserPromptManager>,
    /// The user-profile store (likewise first opened in W6.7).
    pub profile: Arc<ProfileStore>,
    /// The composed episodic store — a CLONE of `ComposedRuntime.store`, the
    /// same store capture and inject write through. The root crate wraps it in
    /// `episodic_session_port(..)` for `.with_sessions(..)`.
    pub episodic: Arc<EpisodicStore>,
    /// The engine's embedder, wrapped by the root into the mem-web embed port.
    pub embedder: Arc<dyn mem_embed::embedder::Embedder>,
}

/// The web server's launch inputs, filled from the host config.
///
/// No `Debug`: none of `ShardManager` / `UserPromptManager` / `ProfileStore` /
/// `dyn Embedder` is `Debug`, and this struct is never rendered or logged.
#[derive(Clone)]
pub struct WebConfig {
    pub host: String,
    pub port: u16,
    pub storage_root: PathBuf,
    /// The composed stores, or `None` when composition failed: the launcher then
    /// wires the documented unavailable stub and the engine degrades.
    pub deps: Option<WebStoreDeps>,
}

/// A started web server's stop handle. `stop` is an OWNED deferred future —
/// the launcher builds it around `mem-web`'s `WebServerHandle::stop` and
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
/// Refuses an unimplemented launch mode BEFORE any config is read (W7.1/M12).
///
/// Exits the process with 78 and one structured stderr event. Kept here (not
/// in `src/main.rs`) so the refusal payload and the `run` guard share one
/// implementation and one message.
pub fn refuse_unimplemented_mode(mode: LaunchMode) {
    if mode.is_implemented() {
        return;
    }
    structured_stderr(
        "mcp.mode_unavailable",
        "error",
        "engine",
        serde_json::json!({
            "mode": mode.as_str(),
            "detail": "MCP transport lands in W7.6",
        }),
    );
    std::process::exit(78);
}

pub fn run(
    mode: LaunchMode,
    config: HostConfig,
    core_config: mem_core::config::Config,
    launcher: Box<dyn WebLauncher>,
) -> i32 {
    // W7.1/M12 — the MCP refusal is the FIRST statement: a direct caller can
    // never fall through to a runtime, a lock or a config read.
    if !mode.is_implemented() {
        structured_stderr(
            "mcp.mode_unavailable",
            "error",
            "engine",
            serde_json::json!({
                "mode": mode.as_str(),
                "detail": "MCP transport lands in W7.6",
            }),
        );
        return 78;
    }
    // the runtime build is the only way in: on failure report and return the existing run-failure code (ADR-0013 D-3)
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        structured_stderr(
            "engine.runtime_failed",
            "error",
            "engine",
            serde_json::json!({ "detail": "the engine's single runtime could not be built" }),
        );
        return 70;
    };
    let code = runtime
        .block_on(run_in_runtime(
            mode,
            config,
            core_config,
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
            // SIGTERM registration: on failure log and await ctrl_c() only, so SIGINT still shuts down (ADR-0013 D-3)
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut term) => {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = term.recv() => {}
                    }
                }
                Err(error) => {
                    tracing::error!(error = %error, "SIGTERM handler failed to register");
                    if let Err(ctrl_c_error) = tokio::signal::ctrl_c().await {
                        tracing::error!(
                            error = %ctrl_c_error,
                            "SIGINT handler failed to register"
                        );
                    }
                }
            }
        })
    } else {
        // Stdio: forever-pending; the loop does not need a signal.
        Box::pin(futures_never())
    }
}

/// A future that never resolves (kept local — no extra dependency edge).
async fn futures_never() {}

fn degraded_capture(request: &Request) -> Response {
    Response::error(
        request.id.clone(),
        error_map::code_for(ErrorKind::Degraded),
        error_map::message_for(ErrorKind::Degraded),
    )
}

fn empty_inject(request: &Request, handling_ms: u64) -> Response {
    Response::result(
        request.id.clone(),
        serde_json::json!({
            "block": serde_json::Value::Null,
            "handling_ms": handling_ms,
            "memory_count": 0,
            "tokens": 0,
            "truncated": false,
        }),
    )
}

/// The `capture_id`/A2 bind check (bridge-schema §3): required, non-blank,
/// ≤ 64 characters. Returns an EMPTY vec on success.
fn capture_validator(request: &Request) -> Option<Response> {
    match request
        .param("capture_id")
        .and_then(serde_json::Value::as_str)
    {
        Some(value) if !value.trim().is_empty() && value.chars().count() <= 64 => None,
        _ => Some(Response::error_with_data(
            request.id.clone(),
            error_map::code_for(ErrorKind::InvalidParams),
            error_map::message_for(ErrorKind::InvalidParams),
            serde_json::json!({ "capture_id": null, "violation": "missing" }),
        )),
    }
}

/// The `run` body, testable with injected signals and any stream pair.
///
/// `core_config` is the resolved `mem_core::Config` loaded SYNCHRONOUSLY by
/// `src/main.rs` (the resolved config has no other carrier into the engine —
/// `HostConfig` is the host §9 surface only); it is a parameter so
/// `main.rs` never touches the runtime or the scheduler.
pub async fn run_in_runtime(
    mode: LaunchMode,
    config: HostConfig,
    core_config: mem_core::config::Config,
    launcher: Box<dyn WebLauncher>,
    shutdown_signal: Pin<Box<dyn Future<Output = ()> + Send>>,
) -> Result<i32, String> {
    // The exclusive lifetime lock FIRST: a loser never opens the DB.
    let probe: Arc<dyn mem_store::platform::ProcessProbe> =
        Arc::new(mem_store::platform::SystemProcessProbe);
    let lock = acquire_outcome(&config.storage_root, probe);

    // The production composition root: every W5.3 service over the resolved
    // config. A composition failure DEGRADES (D-38): the engine keeps its
    // stdio purpose, `capture`/`inject` answer -32001, and the warning is
    // structured on stderr. Fixture-only `Engine::new` remains the no-I/O
    // constructor.
    //
    // W6.7: composition runs BEFORE the web launch, because the launcher now
    // receives the composed stores (`WebStoreDeps`) and the ROOT crate turns
    // them into `ApiServices::production(...)`. The engine lock is still
    // acquired first (§B3), and a composition failure still launches the web
    // server with `deps: None` — the existing degrade path.
    let composed = match crate::composition::build_composed_runtime(
        &config.storage_root,
        &core_config,
        crate::composition::embedder_from_config(&core_config),
        None,
    )
    .await
    {
        Ok(runtime) => Some(Arc::new(runtime)),
        Err(detail) => {
            structured_stderr(
                "engine.composition_failed",
                "warn",
                "engine",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(
                detail,
                "runtime composition failed; engine degrades to no-write"
            );
            None
        }
    };

    // Web server inside THIS process (D-32); `Ok(None)` → disabled handle.
    // A WEB LAUNCH FAILURE DEGRADES (D-38): the engine keeps its stdio
    // purpose with a disabled web handle and a structured stderr warning —
    // the axum surface is auxiliary; an AddrInUse/Bind failure must never
    // crash the bridge (and concurrent engines on one host do not wedge
    // each other over the default port).
    let deps = composed.as_ref().map(|runtime| WebStoreDeps {
        // The deref-clone is required: `ComposedRuntime.shards` is
        // `Arc<ShardManager>` while `WebStoreDeps.shards` is by value. The
        // manual `Clone` shares the connection manager, the init cell and the
        // scope write locks, so the composition root and the web surface still
        // use exactly ONE manager.
        shards: (*runtime.shards).clone(),
        prompts: runtime.prompts.clone(),
        profile: runtime.profile.clone(),
        episodic: runtime.store.clone(),
        embedder: runtime.embedder.clone(),
    });
    let web = match launcher
        .launch(WebConfig {
            host: config.web_host.clone(),
            port: config.web_port,
            storage_root: config.storage_root.clone(),
            deps,
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

    // The override is read ONCE at startup: the env leg wins over the config key
    // (O5), and `health` reports the resolved answer rather than re-reading env.
    let open_guard_env = std::env::var(mem_store::open_guard::ALLOW_UNGUARDED_OPEN_ENV).ok();
    let engine_config = EngineConfig {
        db_path: config
            .storage_root
            .join("episodic.db")
            .to_string_lossy()
            .into_owned(),
        open_guard_override: mem_store::open_guard::override_active(
            open_guard_env.as_deref(),
            core_config.allow_unguarded_open,
        ),
        ..EngineConfig::default()
    };

    let engine = match &composed {
        Some(runtime) => Engine::with_runtime(
            engine_config,
            Arc::new(EngineRuntime {
                capture: runtime.capture.clone(),
                inject: runtime.inject.clone(),
                recall: Arc::new(RecallService::new(
                    runtime.store.clone(),
                    runtime.embedder.clone(),
                )),
            }),
        ),
        None => Engine::new(engine_config),
    };

    // The startup capture drain stays AWAITED before the stdio loop: Step 7
    // requires durable replay to complete before the first `inject` reads.
    // The Q8 consolidation lifecycle is SCHEDULED, not awaited: the startup
    // tick scans up to 64 projects and awaits an LLM summarizer per project,
    // while the plugin's `hello` handshake is timeout-bounded — so awaiting it
    // here would let an aged store push the handshake past that deadline and
    // degrade the plugin to no-engine. Injection stays correct meanwhile: the
    // tick's RAII guard sets `consolidation_active` for the duration of every
    // `Consolidator::run`, which makes `InjectService` skip ONLY the tier-1
    // leg (blueprint Step 9). Consequence, by design: the first `inject` after
    // launch may run before promoted facts exist and may skip the tier-1 leg
    // while a tick is in flight (ADR-0008 D-W5.3-8).
    if let Some(runtime) = &composed {
        let _ = runtime.capture.drain_once().await;
        let scheduler = Arc::new(ConsolidationScheduler::new(
            runtime.store.clone(),
            runtime.scope_resolver.clone(),
            runtime.consolidation_runner.clone(),
            runtime.consolidation_active.clone(),
            Arc::clone(&engine.shutting_down),
            Duration::from_secs(CONSOLIDATION_INTERVAL_SECS),
            CONSOLIDATION_MAX_PROJECTS,
        ));
        spawn_consolidation_lifecycle(&engine, scheduler);
    }

    match mode {
        LaunchMode::Stdio | LaunchMode::HangOnShutdown => {
            serve_stdio(&engine, tokio::io::stdin(), tokio::io::stdout(), mode).await?;
        }
        LaunchMode::Standalone => {
            shutdown_signal.await;
        }
        // The sync dispatch refuses these first; a direct `run_in_runtime`
        // caller gets a typed error, never an accidental serve.
        LaunchMode::McpStdio | LaunchMode::McpHttp => {
            return Err("MCP transport lands in W7.6".to_string());
        }
    }

    // Ordered teardown inside the D-45 budget. The DB flush leg drains only
    // queued entries whose summary was ALREADY supplied — the shutdown path
    // never invokes an LLM and leaves other entries queued for the next
    // start's durable replay.
    let web_stop = web.stop();
    let flush_leg: Pin<Box<dyn Future<Output = ()> + Send>> = match &composed {
        Some(runtime) => {
            let capture = runtime.capture.clone();
            Box::pin(async move {
                let _ = capture.flush_supplied_summary_queued().await;
            })
        }
        None => Box::pin(async {}),
    };
    let report = shutdown_pipeline(Some(web_stop), flush_leg).await;
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
            Ok(request) => engine.dispatch(&request).await,
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

// ---------------------------------------------------------------------------
// Q8 — the consolidation scheduler (startup + periodic, bounded, busy-safe)
// ---------------------------------------------------------------------------

/// The periodic consolidation interval (Q8). The episodic config exposes the
/// age threshold, not a schedule interval, so the scheduler's cadence is a
/// W5.3-new constant (ADR-0008 D-W5.3-8 records it beside the batch bound).
pub const CONSOLIDATION_INTERVAL_SECS: u64 = 3600;

/// The per-tick project-scan bound. It is deliberately DIFFERENT from
/// `DEFAULT_BATCH_LIMIT = 200` (rows per agent batch): 64 bounds how many
/// projects one tick visits, 200 bounds the rows claimed per agent
/// (ADR-0008 D-W5.3-8).
pub const CONSOLIDATION_MAX_PROJECTS: usize = 64;

/// The per-tick consolidation runner seam. `Ok(true)` is a busy skip (another
/// run holds a lock), `Ok(false)` a completed run, `Err` a logged failure.
pub trait ConsolidationRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        project_id: &'a str,
        scope: &'a crate::project_scope::ResolvedProjectScope,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<bool, String>>;
}

/// The production runner: one `Consolidator::run` per project under the
/// shared scope (the same `{container_tag, scope_hash}` the capture writer
/// and the injector read with).
pub struct DefaultConsolidationRunner {
    consolidator: mem_episodic::consolidation::Consolidator,
}

impl DefaultConsolidationRunner {
    pub fn new(consolidator: mem_episodic::consolidation::Consolidator) -> Self {
        Self { consolidator }
    }
}

impl ConsolidationRunner for DefaultConsolidationRunner {
    fn run<'a>(
        &'a self,
        project_id: &'a str,
        scope: &'a crate::project_scope::ResolvedProjectScope,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<bool, String>> {
        let promotion = mem_episodic::consolidation::PromotionScope {
            container_tag: scope.container_tag.clone(),
            scope_hash: scope.scope_hash.clone(),
        };
        Box::pin(async move {
            match self.consolidator.run(project_id, &promotion).await {
                Ok(report) => Ok(report.busy),
                Err(error) => Err(error.to_string()),
            }
        })
    }
}

/// RAII: sets the shared `consolidation_active` flag for the WHOLE run and
/// clears it on every return/error (so `InjectService` skips tier-1 only
/// while a run really is in flight).
struct ConsolidationActiveGuard {
    flag: Arc<AtomicBool>,
}

impl ConsolidationActiveGuard {
    fn set(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for ConsolidationActiveGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// What one scheduler tick did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedulerTickReport {
    /// Project ids returned by the bounded scan.
    pub projects_scanned: usize,
    /// Runs that completed (not busy).
    pub runs: usize,
    /// Runs that found a live lock (clean skip).
    pub busy: usize,
    /// Runs that failed (logged; capture/inject continue).
    pub failed: usize,
}

/// The Q8 scheduler: startup tick + periodic ticks, bounded project scans,
/// a clean busy skip, and prompt stop on shutdown.
pub struct ConsolidationScheduler {
    store: Arc<mem_episodic::store::EpisodicStore>,
    resolver: Arc<dyn crate::scope_resolver::ProjectScopeResolver>,
    runner: Arc<dyn ConsolidationRunner>,
    consolidation_active: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
    interval: Duration,
    max_projects: usize,
}

impl ConsolidationScheduler {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<mem_episodic::store::EpisodicStore>,
        resolver: Arc<dyn crate::scope_resolver::ProjectScopeResolver>,
        runner: Arc<dyn ConsolidationRunner>,
        consolidation_active: Arc<AtomicBool>,
        shutting_down: Arc<AtomicBool>,
        interval: Duration,
        max_projects: usize,
    ) -> Self {
        Self {
            store,
            resolver,
            runner,
            consolidation_active,
            shutting_down,
            interval,
            max_projects,
        }
    }

    /// The bounded `SELECT DISTINCT project_id` scan (one SQL locus: the
    /// statement lives in `mem_store::episodic_sql`).
    pub async fn project_ids(&self) -> Result<Vec<String>, mem_episodic::error::EpisodicError> {
        let db = self.store.ready().await?;
        let rows = db
            .query(
                mem_store::episodic_sql::DISTINCT_LIVE_PROJECT_IDS,
                vec![mem_store::db::Value::Integer(
                    i64::try_from(self.max_projects).unwrap_or(i64::MAX),
                )],
            )
            .await
            .map_err(mem_episodic::error::EpisodicError::Store)?;
        Ok(rows
            .iter()
            .filter_map(|row| row.text("project_id"))
            .collect())
    }

    /// One tick: at most `max_projects` scans, at most one run per id.
    ///
    /// **Loop invariant:** every visited project id has exactly one explicit
    /// outcome (run / busy / failed), the index advances once per finite list
    /// item, and the scan is already truncated to `max_projects`.
    pub async fn run_tick(
        &self,
    ) -> Result<SchedulerTickReport, mem_episodic::error::EpisodicError> {
        let project_ids = self.project_ids().await?;
        let mut report = SchedulerTickReport {
            projects_scanned: project_ids.len(),
            ..Default::default()
        };
        for project_id in project_ids {
            if self.shutting_down.load(Ordering::SeqCst) {
                break;
            }
            let scope = match self.resolver.resolve(&project_id) {
                Ok(scope) => scope,
                Err(_) => {
                    report.failed += 1;
                    continue;
                }
            };
            let _active = ConsolidationActiveGuard::set(self.consolidation_active.clone());
            match self.runner.run(&project_id, &scope).await {
                Ok(true) => report.busy += 1,
                Ok(false) => report.runs += 1,
                Err(error) => {
                    report.failed += 1;
                    structured_stderr(
                        "consolidation.run_failed",
                        "warn",
                        "consolidation",
                        serde_json::json!({ "detail_short": true }),
                    );
                    tracing::warn!(error, "consolidation run failed; capture/inject continue");
                }
            }
        }
        Ok(report)
    }

    /// The periodic loop (the caller schedules the startup tick in the SAME
    /// tracked task immediately before this). Sleeps in small slices so a
    /// shutdown stops it promptly.
    pub async fn run_periodic(self: Arc<Self>) {
        let slice = Duration::from_millis(50);
        let mut elapsed = Duration::ZERO;
        while !self.shutting_down.load(Ordering::SeqCst) {
            tokio::time::sleep(slice).await;
            if self.shutting_down.load(Ordering::SeqCst) {
                break;
            }
            elapsed += slice;
            if elapsed < self.interval {
                continue;
            }
            elapsed = Duration::ZERO;
            let _ = self.run_tick().await;
        }
    }
}

/// The Q8 lifecycle: ONE tracked task — the startup tick, then the periodic
/// loop.
///
/// The startup tick is deliberately NOT awaited before `serve_stdio`: it
/// iterates up to 64 projects and awaits an LLM per project, and the plugin's
/// `hello` handshake is timeout-bounded (an aged store would otherwise push
/// the handshake past that deadline and degrade the plugin to no-engine).
/// Injection stays correct because the tick's RAII guard sets
/// `consolidation_active` for the duration of every `Consolidator::run`,
/// which makes `InjectService` skip only the tier-1 leg. A failed tick never
/// kills the periodic loop.
pub fn spawn_consolidation_lifecycle(engine: &Engine, scheduler: Arc<ConsolidationScheduler>) {
    engine.spawn_tracked(async move {
        match scheduler.run_tick().await {
            Ok(report) => structured_stderr(
                "consolidation.startup_tick",
                "info",
                "consolidation",
                serde_json::json!({
                    "projects_scanned": report.projects_scanned,
                    "runs": report.runs,
                    "busy": report.busy,
                    "failed": report.failed,
                }),
            ),
            Err(error) => {
                structured_stderr(
                    "consolidation.startup_failed",
                    "warn",
                    "consolidation",
                    serde_json::json!({ "detail_short": true }),
                );
                tracing::warn!(%error, "startup consolidation tick failed; engine continues");
            }
        }
        // No `?`: a failed tick must never kill the periodic loop.
        scheduler.run_periodic().await;
    });
}
