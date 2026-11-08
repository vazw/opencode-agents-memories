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

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
use mem_embed::{EmbeddingService, EmbeddingServiceOptions};
use mem_episodic::CRATE_NAME as MEM_EPISODIC;
use mem_facts::CRATE_NAME as MEM_FACTS;
use mem_portability::CRATE_NAME as MEM_PORTABILITY;
use mem_profile::CRATE_NAME as MEM_PROFILE;
use mem_skills::CRATE_NAME as MEM_SKILLS;
use mem_skills::SkillsManager;
use mem_store::CRATE_NAME as MEM_STORE;
use mem_vault::CRATE_NAME as MEM_VAULT;
use mem_web::CRATE_NAME as MEM_WEB;
use mem_web::api::deduplication::production_deduplication_port_with_cancel;
use mem_web::api::ports::ApiServices;
use mem_web::api::store_adapter::{
    ProductionDeps, ProductionEmbed, episodic_milestone_port, episodic_session_port,
    production_skill_port,
};
use opencode_agents_memory::portability_backend::ProductionPortabilityFactory;
use opencode_agents_memory::web_dist::{self, ResolvedDist};

/// The workspace crates in composition order — printed by
/// `--mode=print-crates` (M31); asserted byte-exactly by
/// `tests/workspace_smoke.rs`.
fn workspace_crates() -> [&'static str; 12] {
    [
        MEM_CORE,
        MEM_STORE,
        MEM_EMBED,
        MEM_AI,
        MEM_PROFILE,
        MEM_EPISODIC,
        MEM_FACTS,
        MEM_VAULT,
        MEM_SKILLS,
        MEM_PORTABILITY,
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

/// The sync usage block printed by `-h`/`--help` (pinned by
/// `tests/workspace_smoke.rs`). It is printed BEFORE any config load or store
/// open: a `--help` probe that fell through into a real stdio engine on the
/// default root took the live engine's lock and wrote a bogus `degraded`
/// `status` line into the live store.
const USAGE: &str = "\
usage: opencode-agents-memory [--mode=<mode>] [--storage-root <path>] [options]

Modes (--mode=<mode>): stdio (default), standalone, print-crates,
  purge-session, reembed-scope, reap-shards, sweep. hang-on-shutdown is accepted
  only when OPENCODE_MEM_HANG_ON_SHUTDOWN is set; mcp-stdio and mcp-http are
  recognised and refused with exit 78.

Options:
  --storage-root <path>   storage root (default $HOME/.opencode-mem/data)
  --web-host <host>       web UI bind host (default 127.0.0.1)
  --web-port <port>       web UI bind port (default 3742)
  --web-dist <path>       explicit web UI build directory
  --project <id>          purge-session: project scope
  --session <id>          purge-session: session id
  --agent <id>            purge-session: agent id
  --confirm <token>       purge-session: the printed confirmation token
  --scope <tag>           reembed-scope: container tag to re-embed
  --apply                 reap-shards: perform the reap (dry run otherwise);
                          sweep: required (the sweep mutates)
  -h, --help              print this help and exit
  --version               print the version and exit
";

/// The root composition's web launcher: an axum server (mem-web) inside
/// this process. The boxed future is returned through the seam; sync
/// composition never awaits a future, and no tokio type is named (M1) —
/// the `Pin<Box<dyn Future>>` trait object is std's, not tokio's.
///
/// The launcher owns the RESOLVED dist directory (W6.8). A `Ready` build is
/// handed to mem-web; a `Missing` one refuses the launch — with a structured
/// `web.dist_missing` line naming the candidates — so the engine's existing
/// `web.launch_failed` degradation fires and no silent 404 shell is served.
///
/// W6.7: the launcher is ALSO the mem-web composition root. mem-bridge hands it
/// the composed stores (`WebConfig::deps`) because mem-bridge may never depend
/// on mem-web (ADR-0007:584); the root crate owns that edge and is the only
/// place that calls [`ApiServices::production`]. With no deps (composition
/// failed) it wires the documented unavailable stub and the engine degrades.
struct RootWebLauncher {
    dist: ResolvedDist,
    config: mem_core::config::Config,
    /// The daily-stats scan directory, derived from the log sink at the
    /// composition root so the writer and the reader cannot disagree.
    log_dir: PathBuf,
    /// The launch working directory, resolved ONCE here (issue 115). The web
    /// surface's identity is upstream's `getTags(process.cwd())`, i.e. this
    /// directory, not the per-call session directory inject resolves; reading
    /// it inside the async launch would let the two disagree.
    cwd: PathBuf,
}

impl WebLauncher for RootWebLauncher {
    fn launch(
        &self,
        config: WebConfig,
    ) -> Pin<Box<dyn Future<Output = Result<WebHandle, String>> + Send + '_>> {
        let WebConfig {
            host,
            port,
            storage_root,
            deps,
        } = config;
        let dist = self.dist.clone();
        let core_config = self.config.clone();
        let cwd = self.cwd.clone();
        Box::pin(async move {
            let dist_dir = match &dist {
                ResolvedDist::Ready { dir } => dir.clone(),
                ResolvedDist::Missing { .. } => {
                    // The engine's own `web.launch_failed` line is deliberately
                    // short (`detail_short: true`). The composition root emits
                    // the diagnosable line itself, from the one executable
                    // payload seam (W6.8/R1): `structured_stderr` always
                    // reaches the process stderr, while `tracing` events only
                    // reach the file sink through the `mem_core::logging`
                    // facade target.
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
            // The mem-web production wiring lives HERE, in the root crate,
            // because mem-bridge must not take a mem-web edge.
            // W6.4: the skills manager is wired in BOTH branches — it needs
            // only the storage root, so `/api/skills` answers 200 even when
            // the tier-1 composition failed and the other ports degrade.
            let skills_port = {
                // The store config is DERIVED from the resolved config (the
                // same rule the bridge's composition root follows): only the
                // storage path is owned here.
                let mut store_config = mem_store::config::from_core(&core_config);
                store_config.storage_path = storage_root.clone();
                production_skill_port(Arc::new(SkillsManager::new(
                    storage_root.clone(),
                    store_config,
                )))
            };
            // One flag, two readers: the deduplication port (a running or
            // queued near scan returns at its next check) and the stop
            // closure below (which sets it before stopping the server). It is
            // declared above the `match deps` so both arms can see it; the
            // `None` arm has no deduplication port to cancel.
            let deduplication_cancelled = Arc::new(AtomicBool::new(false));
            let api = match deps {
                Some(deps) => {
                    let service = EmbeddingService::new(
                        EmbeddingServiceOptions::from_config(&core_config),
                        deps.embedder.clone(),
                    );
                    Arc::new(
                        ApiServices::production(ProductionDeps {
                            shards: deps.shards.clone(),
                            prompts: deps.prompts.clone(),
                            profile: deps.profile.clone(),
                            embed: Arc::new(ProductionEmbed::new(Arc::new(service))),
                        })
                        .with_skills(skills_port)
                        .with_sessions(episodic_session_port(deps.episodic.clone()))
                        // W6.6: the milestone projection joins the same
                        // `facts.db` the cross-tier task writes through;
                        // `None` when the promotion pipeline is off.
                        .with_milestones(episodic_milestone_port(
                            deps.episodic.clone(),
                            deps.facts.clone(),
                        ))
                        .with_maintenance(
                            production_deduplication_port_with_cancel(
                                deps.shards.clone(),
                                core_config.deduplication_enabled,
                                core_config.deduplication_similarity_threshold,
                                Arc::clone(&deduplication_cancelled),
                            ),
                        ),
                    )
                }
                // The documented degrade path: no composed stores means the
                // endpoints answer `PortError::Unavailable`, exactly as before
                // W6.7. The skills port is still live.
                None => Arc::new(ApiServices::unavailable().with_skills(skills_port)),
            };
            let state = mem_web::server::AppState {
                basic_auth: mem_web::web_auth::WebAuth::new(None, None, "engine"),
                api_token: None,
                local_token: Arc::new(mem_web::api_auth::FileLocalToken::new(
                    storage_root.join(".auth-token"),
                )),
                dist_dir,
                // The daily-stats scan directory, derived from the SAME sink
                // the composition root installed (W-log-signature): the
                // writer and the reader share one resolution path and cannot
                // disagree. `default_log_dir()` remains a reader-side helper.
                log_dir: self.log_dir.clone(),
                host_allowlist: mem_web::host_allowlist::HostAllowlist::for_bind_host(&host),
                api,
                // Issue 115: the web surface uses the inject path's own
                // resolver and config fields over the LAUNCH cwd. A literal
                // here is the bug this line replaces.
                user_id: opencode_agents_memory::web_identity::resolve_user_id(&core_config, &cwd),
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
                                // FIRST, before the server is stopped: a near
                                // scan running or queued on the blocking pool
                                // returns at its next check, so the graceful
                                // drain does not wait for detached CPU work.
                                deduplication_cancelled.store(true, Ordering::SeqCst);
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

/// The parsed argv surface: mode, storage root, web host/port, the explicit
/// web-dist directory, and the `purge-session` scope/confirmation flags.
#[derive(Debug, Default)]
struct ParsedArgs {
    mode: Option<String>,
    storage_root: Option<String>,
    web_host: Option<String>,
    web_port: Option<u16>,
    web_dist: Option<String>,
    project: Option<String>,
    session: Option<String>,
    agent: Option<String>,
    confirm: Option<String>,
    scope: Option<String>,
    /// The bare `--apply` confirmation (reap-shards only).
    apply: bool,
    /// `--apply=<value>` was seen: a usage error in reap mode.
    apply_inline: bool,
    /// Arguments the sync parser does not know. A usage error (exit 2), never
    /// silently ignored: an ignored flag used to fall through into a real
    /// engine on the DEFAULT live root.
    unknown: Vec<String>,
}

/// The sync arg-parse (M1): mode, storage root, web host/port overrides, the
/// explicit web-dist directory and the purge-session flags over the
/// env-resolved defaults (bridge-schema.md §9).
fn parse_args<I: Iterator<Item = (String, Option<String>)>>(args: I) -> ParsedArgs {
    let mut parsed = ParsedArgs::default();
    for (name, inline) in args {
        match name.as_str() {
            "--mode" | "mode" => parsed.mode = inline,
            "--mode=print-crates" => parsed.mode = Some("print-crates".to_owned()),
            "--mode=stdio" => parsed.mode = Some("stdio".to_owned()),
            "--mode=standalone" => parsed.mode = Some("standalone".to_owned()),
            "--mode=hang-on-shutdown" => parsed.mode = Some("hang-on-shutdown".to_owned()),
            "--mode=reap-shards" => parsed.mode = Some("reap-shards".to_owned()),
            "--mode=sweep" => parsed.mode = Some("sweep".to_owned()),
            "--storage-root" | "storage-root" => parsed.storage_root = inline,
            "--web-host" | "web-host" => parsed.web_host = inline,
            "--web-port" | "web-port" => {
                parsed.web_port = inline.and_then(|port| port.trim().parse::<u16>().ok())
            }
            "--web-dist" | "web-dist" => parsed.web_dist = inline,
            "--project" | "project" => parsed.project = inline,
            "--session" | "session" => parsed.session = inline,
            "--agent" | "agent" => parsed.agent = inline,
            "--confirm" | "confirm" => parsed.confirm = inline,
            "--scope" | "scope" => parsed.scope = inline,
            "--apply" => match inline {
                None => parsed.apply = true,
                Some(_) => parsed.apply_inline = true,
            },
            other => parsed.unknown.push(other.to_owned()),
        }
    }
    parsed
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
        // `--apply` is a bare boolean: it never swallows a following value.
        // `--apply false` must be a usage error, never "apply and discard
        // false" (the value stays a separate argument for the trap check).
        if arg == "--apply" {
            let next = raw.get(index + 1).cloned();
            let swallows_value = next.as_deref().is_some_and(|value| !value.starts_with('-'));
            argv.push((arg, None));
            index += 1;
            if swallows_value && let Some(value) = next {
                argv.push((value, None));
                index += 1;
            }
            continue;
        }
        if !arg.contains('=')
            && (arg == "--mode"
                || arg == "--storage-root"
                || arg == "--web-host"
                || arg == "--web-port"
                || arg == "--web-dist"
                || arg == "--project"
                || arg == "--session"
                || arg == "--agent"
                || arg == "--confirm"
                || arg == "--scope")
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

    // `--apply <token>` (token not starting with `-`) is the value trap: a
    // bare boolean must never swallow a value and then apply.
    let apply_value_trap = raw
        .windows(2)
        .any(|pair| pair[0] == "--apply" && !pair[1].starts_with('-'));

    // A reap-foreign flag is a usage error on PRESENCE, not value: a bare
    // `--scope` at end-of-argv parses to `None`, and `--apply --scope` would
    // otherwise apply a root-wide reap with the filter silently dropped. Scan
    // the raw argv so a foreign token swallowed as another flag's value is
    // still refused.
    let reap_foreign_flag = raw.iter().any(|arg| {
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        matches!(
            name,
            "--scope"
                | "scope"
                | "--project"
                | "project"
                | "--session"
                | "session"
                | "--agent"
                | "agent"
                | "--confirm"
                | "confirm"
        )
    });

    // `--help`/`-h` is usage, handled sync and BEFORE any runtime exists: it
    // used to be "ignored" here and fell through into a real `Stdio` engine on
    // the DEFAULT root, which lost the live lock and wrote a `degraded`
    // `status` line into the live store.
    if raw.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return;
    }

    // Special literal modes handled sync, BEFORE any runtime exists.
    if raw.iter().any(|a| a == "--version") {
        println!("{}", version_line());
        return;
    }

    // Compose the host config from env + argv, then dispatch.
    let mut config = HostConfig::from_env();
    let args = parse_args(
        argv.iter()
            .map(|(name, inline)| (name.clone(), inline.clone())),
    );

    // reap-shards validates its OWN usage first: EX_USAGE (64) for a valued
    // `--apply`, a foreign scope flag, or `--apply` swallowing a value. This
    // runs before the generic unknown-flag refusal so a token the reap rules
    // own keeps the reap verdict instead of the generic 2.
    let reap_mode = args.mode.as_deref().map(str::trim) == Some("reap-shards");
    if reap_mode && (args.apply_inline || apply_value_trap || reap_foreign_flag) {
        eprintln!(
            "usage: opencode-agents-memory --mode=reap-shards [--apply] [--storage-root <path>]"
        );
        std::process::exit(64);
    }

    // An unknown argument is a usage error, never a silent "ignore" that falls
    // through into a real engine. This runs BEFORE the mode dispatch — a sync
    // mode that returns early (`print-crates`) must not swallow one either —
    // and BEFORE any config load or store open.
    if !args.unknown.is_empty() {
        eprintln!(
            "usage: opencode-agents-memory [--mode=<mode>] [--storage-root <path>] [options]"
        );
        for argument in &args.unknown {
            eprintln!("unknown argument {argument:?}");
        }
        eprintln!("run `opencode-agents-memory --help` for the full list");
        std::process::exit(2);
    }

    if let Some(root) = args
        .storage_root
        .clone()
        .filter(|root| !root.trim().is_empty())
    {
        config.storage_root = PathBuf::from(root);
    }
    // HostConfig storage_root: make the engine's ONE storage root absolute so
    // every holder agrees. This sits AFTER argv resolution (above) and the
    // `HostConfig::from_env()` default (the `$HOME/...` fallback is already
    // absolute but is normalized with the same expression), and BEFORE the
    // purge/reembed/reap dispatches and the engine runtime — one point that
    // covers argv + env + default. A relative `--storage-root` used to break
    // ShardManager/ConnectionManager containment parity: the manager
    // canonicalizes its own copy (`shard_manager.rs`), the connection manager
    // kept the raw value, and `assert_path_inside_storage` refused the
    // engine's own `metadata.db` — absolute target vs relative root — so
    // every store-backed route answered a silent 500 (debug:
    // engine-silent-500.debug-1). `canonicalize` resolves the real path for
    // an existing root; `std::path::absolute` keeps a not-yet-existing root
    // working (first-run-on-new-root, exactly the smoke scenario) without
    // touching the filesystem; the last fallback keeps the raw value (an
    // engine that cannot even resolve its cwd degrades the way it did
    // before this fix rather than refusing to start).
    config.storage_root = std::fs::canonicalize(&config.storage_root)
        .or_else(|_| std::path::absolute(&config.storage_root))
        .unwrap_or_else(|_| config.storage_root.clone());
    if let Some(host) = args.web_host.clone().filter(|host| !host.trim().is_empty()) {
        config.web_host = host;
    }
    if let Some(port) = args.web_port {
        config.web_port = port;
    }
    // The purge request, built and confirmed at parse time. `None` for every
    // other mode.
    let mut purge_request: Option<mem_bridge::purge::PurgeRequest> = None;
    // The reembed-scope tag: validated by the CLI runner AFTER the config
    // load (it needs no config to reject a bad tag, but does need it to
    // compose the embedder).
    let mut reembed_tag: Option<String> = None;
    // The reap confirmation, built at parse time. `None` for every other mode.
    let mut reap_request: Option<mem_bridge::reap::ReapRequest> = None;
    // The sweep request, built at parse time. `None` for every other mode.
    let mut sweep_request: Option<mem_bridge::sweep::SweepRequest> = None;
    if let Some(mode) = args.mode.clone() {
        let mode = mode.trim().to_owned();
        if mode == "print-crates" {
            // print-crates is the sync side-effecting composition mode
            // (M31); it is dispatched here, before any runtime exists.
            for name in workspace_crates() {
                println!("{name}");
            }
            return;
        }
        if mode == "purge-session" {
            // W7.1/M19: the confirmation is validated HERE, before
            // `Config::load_default` and before any store is opened.
            let request = mem_bridge::purge::PurgeRequest {
                project_id: args.project.clone().unwrap_or_default(),
                session_id: args.session.clone().unwrap_or_default(),
                agent_id: args.agent.clone().unwrap_or_default(),
            };
            let expected = request.confirmation();
            match args.confirm.as_deref() {
                None => mem_bridge::purge::refuse_missing_confirmation(&expected),
                Some(value) if value != expected => {
                    mem_bridge::purge::refuse_mismatched_confirmation(&expected)
                }
                Some(_) => {}
            }
            purge_request = Some(request);
        } else if mode == "reembed-scope" {
            // W3: the scope tag travels as-is; an absent flag is the empty
            // tag, which the runner rejects with 64 (invalid scope).
            reembed_tag = Some(args.scope.clone().unwrap_or_default());
        } else if mode == "reap-shards" {
            // The usage verdicts were already taken above, before the generic
            // unknown-flag refusal; only the request is built here.
            reap_request = Some(mem_bridge::reap::ReapRequest { apply: args.apply });
        } else if mode == "sweep" {
            // The sweep mutates, so `--apply` is mandatory; the runner answers
            // 64 (not a dry run) when it is absent.
            sweep_request = Some(mem_bridge::sweep::SweepRequest { apply: args.apply });
        } else {
            match LaunchMode::parse(&mode) {
                Ok(parsed) => {
                    // W7.1/M12: refuse the MCP modes BEFORE
                    // `Config::load_default` (the next statements would read
                    // the config home).
                    mem_bridge::engine::refuse_unimplemented_mode(parsed);
                    config.mode = parsed;
                }
                Err(message) => {
                    eprintln!("{message}");
                    std::process::exit(64);
                }
            }
        }
    }

    // The web UI's location is a composition-root setting (W6.8): CLI flag >
    // env var > exe-relative default. It is deliberately NOT a
    // `mem_core::Config` field — see `web_dist`'s module docs. Blank values
    // fall through exactly like unset ones, and the default is derived from
    // the running binary, never from a hard-coded repo path.
    let cli_dist = args
        .web_dist
        .clone()
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

    // The resolved core config is loaded SYNCHRONOUSLY here (M1) and carried
    // into the engine by value. W6.7 also needs it in the launcher (the
    // embedding service options for the mem-web embed port), so it is hoisted
    // and cloned rather than loaded twice.
    let core_config = mem_core::config::Config::load_default(&mem_core::env::ProcessEnv)
        .unwrap_or_else(|error| {
            eprintln!("config load failed: {error}");
            std::process::exit(70);
        });

    // W7.1 — the purge dispatch sits BEFORE the engine runtime. The root is
    // the HostConfig value (`--storage-root` → `OPENCODE_MEM_STORAGE_ROOT` →
    // `$HOME/.opencode-mem/data`); the JSONC `storagePath` is not the engine
    // root and is never consulted here.
    if let Some(request) = purge_request {
        std::process::exit(mem_bridge::purge::run(
            &core_config,
            &config.storage_root,
            &request,
        ));
    }

    // W3 — the re-embed dispatch also sits BEFORE the engine runtime: never
    // start the stdio loop, the web server or the engine lock in this mode.
    if let Some(container_tag) = reembed_tag {
        std::process::exit(mem_bridge::reembed::run(
            &core_config,
            &config.storage_root,
            &mem_bridge::reembed::ReembedRequest { container_tag },
        ));
    }

    // W7 — the reap dispatch also sits BEFORE the engine runtime and BEFORE
    // the log sink: never start the stdio loop, the web server or an engine
    // runtime in this mode.
    if let Some(request) = reap_request {
        std::process::exit(mem_bridge::reap::run(
            &core_config,
            &config.storage_root,
            &request,
        ));
    }

    // W6c — the erasure sweep dispatch also sits BEFORE the engine runtime: it
    // takes the engine lock itself, so starting the engine here would make it
    // refuse with 75 against its own process.
    if let Some(request) = sweep_request {
        std::process::exit(mem_bridge::sweep::run(
            &core_config,
            &config.storage_root,
            &request,
        ));
    }

    // W-log-signature — install the structured-log sink ONCE at the
    // composition root, and derive the stats scan directory from that same
    // sink. Both stdio and standalone modes reach this point; the sync modes
    // (`--version`, `print-crates`, `purge-session`) returned above.
    //
    // Degraded observability: a failed install must not abort the engine (a
    // zero counter is strictly better than no engine); the existing
    // machine-readable stderr seam reports it.
    let env = mem_core::env::ProcessEnv;
    let home = mem_core::env::home_dir(&env).unwrap_or_else(|| PathBuf::from("."));
    let sink = mem_core::logging::LogSink::from_env(&env, &home);
    let log_dir = sink
        .path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if let Err(error) = mem_core::logging::init_process_logging(sink) {
        structured_stderr(
            "logging.init_failed",
            "warn",
            "engine",
            mem_store::serde_json::json!({ "error": error.to_string() }),
        );
    }

    if !core_config.inert_keys_present.is_empty() {
        mem_core::tracing::warn!(
            inert_keys = ?core_config.inert_keys_present,
            count = core_config.inert_keys_present.len(),
            "inert config keys set in JSONC; these have no runtime effect"
        );
        structured_stderr(
            "config.inert_keys",
            "warn",
            "engine",
            mem_store::serde_json::json!({
                "inert_keys": core_config.inert_keys_present,
                "count": core_config.inert_keys_present.len(),
            }),
        );
    }

    let exit = engine_run(
        config.mode,
        config,
        core_config.clone(),
        Box::new(RootWebLauncher {
            dist,
            config: core_config,
            log_dir,
            cwd,
        }),
        // Issue 70 wave 1: the ONE portability factory the engine seam
        // consults; export/import degrade (-32001) only if it builds none.
        Some(Box::new(ProductionPortabilityFactory)),
    );
    std::process::exit(exit);
}

#[cfg(test)]
mod tests {
    use super::{USAGE, parse_args};

    /// Issue 43 W6c — the sweep mode is accepted by the same parse table as
    /// `reap-shards`, and `--apply` travels into the request unchanged.
    #[test]
    fn test_mode_sweep_parses_and_apply_travels() {
        let parsed = parse_args(
            vec![
                ("--mode=sweep".to_string(), None),
                ("--apply".to_string(), None),
            ]
            .into_iter(),
        );
        assert_eq!(parsed.mode.as_deref(), Some("sweep"));
        assert!(parsed.apply, "--apply reaches the sweep request");
        assert!(
            !parsed.apply_inline,
            "a bare --apply is not an inline value"
        );
        assert!(USAGE.contains("sweep"), "the usage text names the mode");
    }
}
