//! The web server: shared state, router assembly and lifecycle.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::timeout::TimeoutLayer;

use crate::api_auth::{LocalTokenSource, assert_network_auth};
use crate::error::WebError;
use crate::health::health_handler;
use crate::host_allowlist::HostAllowlist;
use crate::lifecycle::{HTTP_IDLE_TIMEOUT, panic_response};
use crate::settings::WebServerSettings;
use crate::static_hosting::{index_handler, not_found_handler, static_fallback};
use crate::web_auth::WebAuth;

/// The bound on the graceful shutdown of a running server.
const SHUTDOWN_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// The state shared by the request handlers.
pub struct AppState {
    /// The Basic-auth gate.
    pub basic_auth: WebAuth,
    /// The configured API token for the `/api/*` surface.
    pub api_token: Option<String>,
    /// The source of the local token injected into the shell.
    pub local_token: Arc<dyn LocalTokenSource>,
    /// The directory holding the built SPA bundle.
    pub dist_dir: PathBuf,
    /// The directory scanned for the daily stats series (upstream
    /// `defaultLogDir`, resolved at the composition root).
    pub log_dir: PathBuf,
    /// The D-8 Host allowlist — derived from the bound host by `start`.
    pub host_allowlist: HostAllowlist,
    /// The eight port seams behind the REST surface (W6.2).
    pub api: Arc<crate::api::ports::ApiServices>,
    /// The resolved user identity for the profile endpoints (upstream
    /// `getTags(process.cwd()).user.userEmail || "unknown"`).
    ///
    /// Resolved by the composition root through the inject path's
    /// `ProjectScopeResolver`; `"unknown"` only when nothing resolves — never a
    /// literal at the wiring site (issue 115).
    pub user_id: String,
    /// The **configured** bind host (D-11's loopback rule reads it). `start`
    /// fills it from `settings.host`.
    pub bind_host: String,
}

/// Builds the router: routes, SPA fallback and the request gate.
///
/// The gate wraps the WHOLE router — the routes *and* the fallback — so a
/// request for an unknown path is gated too. The timeout and the panic
/// catcher sit inside the gate.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health_handler))
        // Upstream answers `HEAD` with 404, not with the `GET` body.
        .route("/", get(index_handler).head(not_found_handler))
        .route("/index.html", get(index_handler).head(not_found_handler))
        .fallback(static_fallback)
        .method_not_allowed_fallback(not_found_handler)
        // W6.2: the REST surface. `api_router()` carries its own 1 MiB body
        // cap (G1/D-21) and registers every non-health API path.
        .merge(crate::api::api_router())
        .with_state(state.clone())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            HTTP_IDLE_TIMEOUT,
        ))
        .layer(CatchPanicLayer::custom(|_| panic_response()))
        .layer(axum::middleware::from_fn_with_state(
            state,
            crate::gate::gate,
        ))
}

/// Starts the server (or returns `Ok(None)` when it is disabled).
///
/// The order matters: the network-auth guard runs *before* any socket is
/// created, and the Host allowlist is derived from the host that is
/// actually bound — the bound address is the ground truth, so the enforced
/// names can never drift from a stale caller value.
pub async fn start(
    settings: &WebServerSettings,
    state: AppState,
) -> Result<Option<WebServerHandle>, WebError> {
    if !settings.enabled {
        return Ok(None);
    }

    assert_network_auth(
        &settings.host,
        settings.api_token.as_deref(),
        state.basic_auth.is_enabled(),
    )?;

    let mut state = state;
    state.host_allowlist = HostAllowlist::for_bind_host(&settings.host);
    // D-11 reads the configured bind host from the state.
    state.bind_host = settings.host.clone();

    let listener = tokio::net::TcpListener::bind((settings.host.as_str(), settings.port))
        .await
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::AddrInUse => WebError::AddrInUse {
                host: settings.host.clone(),
                port: settings.port,
            },
            _ => WebError::Bind {
                host: settings.host.clone(),
                port: settings.port,
                source,
            },
        })?;

    let local_addr = listener.local_addr().map_err(|source| WebError::Bind {
        host: settings.host.clone(),
        port: settings.port,
        source,
    })?;

    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel();
    let router = build_router(Arc::new(state));
    let join = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_signal.await;
            })
            .await
    });

    Ok(Some(WebServerHandle {
        local_addr,
        shutdown: Some(shutdown),
        join: Some(join),
    }))
}

/// A handle on a running server.
#[derive(Debug)]
pub struct WebServerHandle {
    pub(crate) local_addr: SocketAddr,
    pub(crate) shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) join: Option<tokio::task::JoinHandle<Result<(), std::io::Error>>>,
}

impl WebServerHandle {
    /// The address the server actually bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops the server (bounded at 5 s).
    ///
    /// The shutdown signal is sent first; the serve task is then awaited
    /// with a 5 s bound. On timeout the task is aborted and the stop still
    /// succeeds — a stuck connection must not wedge the caller.
    pub async fn stop(self) -> Result<(), WebError> {
        let WebServerHandle {
            local_addr: _,
            shutdown,
            join,
        } = self;
        if let Some(shutdown) = shutdown {
            let _ = shutdown.send(());
        }
        let Some(mut join) = join else {
            return Ok(());
        };
        match tokio::time::timeout(SHUTDOWN_BOUND, &mut join).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(source))) => Err(WebError::Serve { source }),
            // The serve task panicked or was cancelled — the server is gone
            // either way.
            Ok(Err(_)) => Ok(()),
            Err(_) => {
                join.abort();
                Ok(())
            }
        }
    }
}
