//! Root composition binary for the `opencode-agents-memory` workspace: the
//! V1 engine process (ADR-0007 / W5.1).
//!
//! The root package is the composition root; it owns no logic of its own.
//! This file is **sync arg-parse + dispatch ONLY** and names no tokio type
//! (M1): the single runtime, the stdio loop, the shutdown ordering and the
//! lock lifecycle all live inside `mem_bridge::engine::run`.
//!
//! The web server enters the process through the [`RootWebLauncher`] seam,
//! which wraps `mem_web::server::start` (D-32: axum in the engine process;
//! no new mem-bridge → mem-web edge). A launch failure DEGRADES (D-38):
//! the launcher reports it as a string and the engine keeps its stdio
//! purpose.
//!
//! The launcher is handed the RESOLVED web-UI directory (`web_dist`). When no
//! build resolves it REFUSES the launch — `web.dist_missing` names the
//! candidates and the engine's `web.launch_failed` degradation follows — so a
//! missing build is loud and diagnosable instead of a silent 404 shell
//! (W6.8/AC3).

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use mem_ai::CRATE_NAME as MEM_AI;
use mem_bridge::CRATE_NAME as MEM_BRIDGE;
use mem_bridge::engine::LaunchMode;
use mem_bridge::engine::PROTOCOL_VERSION;
use mem_bridge::engine::WebConfig;
use mem_bridge::engine::WebHandle;
use mem_bridge::engine::WebLauncher;
use mem_bridge::engine::structured_stderr;
use mem_bridge::engine::{HostConfig, run as engine_run};
use mem_core::CRATE_NAME as MEM_CORE;
use mem_embed::CRATE_NAME as MEM_EMBED;
use mem_episodic::CRATE_NAME as MEM_EPISODIC;
use mem_profile::CRATE_NAME as MEM_PROFILE;
use mem_store::CRATE_NAME as MEM_STORE;
use mem_web::CRATE_NAME as MEM_WEB;
use opencode_agents_memory::web_dist::{self, ResolvedDist};

/// The workspace crates in composition order — printed by
/// `--mode=print-crates` (M31); asserted byte-exactly by
/// `tests/workspace_smoke.rs`.
fn workspace_crates() -> [&'static str; 8] {
    [
        MEM_CORE,
        MEM_STORE,
        MEM_EMBED,
        MEM_AI,
        MEM_PROFILE,
        MEM_EPISODIC,
        MEM_WEB,
        MEM_BRIDGE,
    ]
}

/// The version line `--version` prints (pinned by the smoke test, E4:
/// `opencode-agents-memory 0.1.0 (bridge 1.0.0)`).
fn version_line() -> String {
    format!(
        "opencode-agents-memory {} (bridge {PROTOCOL_VERSION})",
        env!("CARGO_PKG_VERSION")
    )
}

/// The root composition's web launcher: an axum server (mem-web) inside
/// this process. The boxed future is returned through the seam; sync
/// composition never awaits a future, and no tokio type is named (M1) —
/// the `Pin<Box<dyn Future>>` trait object is std's, not tokio's.
///
/// The launcher owns the RESOLVED dist directory (W6.8). A `Ready` build is
/// handed to mem-web; a `Missing` one refuses the launch — with a structured
/// `web.dist_missing` line naming the candidates — so the engine's existing
/// `web.launch_failed` degradation fires and no silent 404 shell is served.
struct RootWebLauncher {
    dist: ResolvedDist,
}

impl WebLauncher for RootWebLauncher {
    fn launch(
        &self,
        config: WebConfig,
    ) -> Pin<Box<dyn Future<Output = Result<WebHandle, String>> + Send + '_>> {
        let host = config.host;
        let port = config.port;
        let storage_root = config.storage_root;
        let dist = self.dist.clone();
        Box::pin(async move {
            let dist_dir = match &dist {
                ResolvedDist::Ready { dir } => dir.clone(),
                ResolvedDist::Missing { .. } => {
                    // The engine's own `web.launch_failed` line is deliberately
                    // short (`detail_short: true`) and its detail goes to
                    // `tracing`, which has no subscriber on the run path — so
                    // the composition root emits the diagnosable line itself,
                    // from the one executable payload seam (W6.8/R1).
                    structured_stderr("web.dist_missing", "warn", "engine", dist.missing_payload());
                    return Err(dist.error_message());
                }
            };
            let settings = mem_web::WebServerSettings {
                enabled: true,
                host: host.clone(),
                port,
                auth_password: None,
                auth_username: None,
                api_token: None,
            };
            let state = mem_web::server::AppState {
                basic_auth: mem_web::web_auth::WebAuth::new(None, None, "engine"),
                api_token: None,
                local_token: Arc::new(mem_web::api_auth::FileLocalToken::new(
                    storage_root.join(".auth-token"),
                )),
                dist_dir,
                host_allowlist: mem_web::host_allowlist::HostAllowlist::for_bind_host(&host),
                api: Arc::new(mem_web::api::ports::ApiServices::unavailable()),
                user_id: "unknown".to_owned(),
                bind_host: host.clone(),
            };
            match mem_web::server::start(&settings, state).await {
                Ok(Some(handle)) => {
                    // `WebServerHandle::stop` is
                    // `pub async fn stop(self) -> Result<(), WebError>`
                    // (crates/mem-web/src/server.rs:156); the launcher's
                    // stop future absorbs the Result (blueprint B3).
                    let stop: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send> =
                        Box::new(move || {
                            Box::pin(async move {
                                let _ = handle.stop().await;
                            })
                                as Pin<Box<dyn Future<Output = ()> + Send>>
                        });
                    Ok(WebHandle::new(stop))
                }
                Ok(None) => Ok(WebHandle::disabled()),
                Err(error) => Err(error.to_string()),
            }
        })
    }
}

/// The parsed argv surface: mode, storage root, web host, web port and the
/// explicit web-dist directory.
type ParsedArgs = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<u16>,
    Option<String>,
);

/// The sync arg-parse (M1): mode, storage root, web host/port overrides and
/// the explicit web-dist directory over the env-resolved defaults
/// (bridge-schema.md §9).
fn parse_args<I: Iterator<Item = (String, Option<String>)>>(args: I) -> ParsedArgs {
    let mut mode_text: Option<String> = None;
    let mut storage_root: Option<String> = None;
    let mut web_host: Option<String> = None;
    let mut web_port: Option<u16> = None;
    let mut web_dist: Option<String> = None;
    for (name, inline) in args {
        match name.as_str() {
            "--mode" | "mode" => mode_text = inline,
            "--mode=print-crates" => mode_text = Some("print-crates".to_owned()),
            "--mode=stdio" => mode_text = Some("stdio".to_owned()),
            "--mode=standalone" => mode_text = Some("standalone".to_owned()),
            "--mode=hang-on-shutdown" => mode_text = Some("hang-on-shutdown".to_owned()),
            "--storage-root" | "storage-root" => storage_root = inline,
            "--web-host" | "web-host" => web_host = inline,
            "--web-port" | "web-port" => {
                web_port = inline.and_then(|port| port.trim().parse::<u16>().ok())
            }
            "--web-dist" | "web-dist" => web_dist = inline,
            other => eprintln!("ignoring argument {other:?}"),
        }
    }
    (mode_text, storage_root, web_host, web_port, web_dist)
}

/// The iterator/path: positional pairs (`--mode stdio`) become `Some` inline
/// values by the caller peeking ahead; only inline (`=`) forms are ACTUALLY
/// parsed here. `run` is called with the resolved config; parse-crates is a
/// SYNC dispatch done BEFORE the engine's runtime exists.
fn main() {
    let mut argv: Vec<(String, Option<String>)> = Vec::new();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0usize;
    while index < raw.len() {
        let arg = raw[index].clone();
        if !arg.contains('=')
            && (arg == "--mode"
                || arg == "--storage-root"
                || arg == "--web-host"
                || arg == "--web-port"
                || arg == "--web-dist")
        {
            let value = raw.get(index + 1).cloned();
            let has_value = value.is_some();
            argv.push((arg, value));
            index += if has_value { 2 } else { 1 };
        } else {
            let (name, inline) = arg
                .split_once('=')
                .map(|(name, value)| (name.to_owned(), Some(value.to_owned())))
                .unwrap_or_else(|| (arg.clone(), None));
            argv.push((name, inline));
            index += 1;
        }
    }

    // Special literal modes handled sync, BEFORE any runtime exists.
    if raw.iter().any(|a| a == "--version") {
        println!("{}", version_line());
        return;
    }

    // Compose the host config from env + argv, then dispatch.
    let mut config = HostConfig::from_env();
    let (mode_text, storage_root, web_host, web_port, web_dist_arg) = parse_args(
        argv.iter()
            .map(|(name, inline)| (name.clone(), inline.clone())),
    );
    if let Some(root) = storage_root.filter(|root| !root.trim().is_empty()) {
        config.storage_root = PathBuf::from(root);
    }
    if let Some(host) = web_host.filter(|host| !host.trim().is_empty()) {
        config.web_host = host;
    }
    if let Some(port) = web_port {
        config.web_port = port;
    }
    if let Some(mode) = mode_text {
        let mode = mode.trim().to_owned();
        if mode == "print-crates" {
            // print-crates is the sync side-effecting composition mode
            // (M31); it is dispatched here, before any runtime exists.
            for name in workspace_crates() {
                println!("{name}");
            }
            return;
        }
        match LaunchMode::parse(&mode) {
            Ok(mode) => config.mode = mode,
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(64);
            }
        }
    }

    // The web UI's location is a composition-root setting (W6.8): CLI flag >
    // env var > exe-relative default. It is deliberately NOT a
    // `mem_core::Config` field — see `web_dist`'s module docs. Blank values
    // fall through exactly like unset ones, and the default is derived from
    // the running binary, never from a hard-coded repo path.
    let cli_dist = web_dist_arg
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let env_dist = std::env::var_os(web_dist::ENV_VAR).map(PathBuf::from);
    let explicit = web_dist::explicit_setting(cli_dist.as_deref(), env_dist.as_deref());
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dist = web_dist::resolve(explicit.as_deref(), &exe_dir, &cwd);

    let exit = engine_run(
        config.mode,
        config,
        mem_core::config::Config::load_default(&mem_core::env::ProcessEnv).unwrap_or_else(
            |error| {
                eprintln!("config load failed: {error}");
                std::process::exit(70);
            },
        ),
        Box::new(RootWebLauncher { dist }),
    );
    std::process::exit(exit);
}
