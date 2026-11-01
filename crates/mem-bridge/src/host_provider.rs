//! `LazyHostProvider` — resolve the live host ON USE.
//!
//! `service.json` is rewritten at host start (its mtime equals the host's start
//! second), so a startup-only read can see the previous run's dead pid. The
//! provider resolves lazily, caches on success, and **clears the cache on any
//! inner failure** — `Auth`/`HttpStatus`/`Protocol` included: a foreign process
//! that rebound a recycled port answers `401`/`404`/junk, never `Network`, and
//! caching only on `Network`/`Timeout` would pin a stranger's origin for the
//! engine's life.
//!
//! `warm()` is the startup probe: it resolves once (caching on success) and
//! never caches a failure.

use std::sync::{Arc, RwLock};

use mem_core::env::EnvSource;
use mem_core::logging::log;
use mem_core::logging::signatures::{
    AUTO_CAPTURE_PROVIDER_READY, AUTO_CAPTURE_PROVIDER_UNAVAILABLE,
};
use mem_core::sync::{read_or_recover, write_or_recover};
use mem_episodic::futures::StreamExt as _;
use mem_store::platform::ProcessProbe;
use serde_json::Value;

use mem_ai::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};
use mem_ai::provider::{
    AiProvider, BoxFuture, CompletionStream, HostTransport, OpenCodeHostConfig,
    OpenCodeHostProvider, PROVIDER_NAME, ProviderError, ProviderErrorKind,
};

use crate::host_endpoint::{
    HostWiringError, HostWiringRequest, ProcessClock, resolve_host_wiring_detailed,
};

/// Everything resolution needs, injected (the engine builds it from the
/// resolved config; tests inject recording doubles).
pub struct HostResolverDeps {
    /// `opencodeBaseUrl` (branch A when non-blank).
    pub override_url: Option<String>,
    /// `opencodeProvider`.
    pub provider_id: Option<String>,
    /// `opencodeModel`.
    pub model_id: Option<String>,
    /// `opencodeVariant`.
    pub variant: Option<String>,
    /// The environment seam (`XDG_STATE_HOME`, `HOME`).
    pub env: Arc<dyn EnvSource + Send + Sync>,
    /// Liveness probe for the recorded host pid.
    pub probe: Arc<dyn ProcessProbe>,
    /// Start-time oracle for pid recycling detection.
    pub clock: Arc<dyn ProcessClock>,
    /// The host transport every resolved provider dials through.
    pub transport: Arc<dyn HostTransport>,
}

struct LazyState {
    deps: HostResolverDeps,
    cache: RwLock<Option<Arc<OpenCodeHostProvider>>>,
}

/// A provider that resolves the live OpenCode host on first use.
pub struct LazyHostProvider {
    state: Arc<LazyState>,
}

impl LazyHostProvider {
    /// Build the lazy provider (no I/O: nothing is read until the first use).
    pub fn new(deps: HostResolverDeps) -> Self {
        Self {
            state: Arc::new(LazyState {
                deps,
                cache: RwLock::new(None),
            }),
        }
    }

    /// The cached provider, or a fresh resolution.
    ///
    /// A failure is never cached: the next capture re-resolves (the acceptance
    /// property — a host restart heals without an engine restart).
    fn resolve(&self) -> Result<Arc<OpenCodeHostProvider>, HostWiringError> {
        if let Some(cached) = read_or_recover(&self.state.cache).clone() {
            return Ok(cached);
        }
        let deps = &self.state.deps;
        let request = HostWiringRequest {
            override_url: deps.override_url.as_deref(),
            configured_provider: deps.provider_id.as_deref(),
            configured_model: deps.model_id.as_deref(),
        };
        let wiring = match resolve_host_wiring_detailed(
            deps.env.as_ref(),
            deps.probe.as_ref(),
            deps.clock.as_ref(),
            &request,
        ) {
            Ok(wiring) => wiring,
            Err(failure) => {
                log_provider_unavailable(failure.error.reason(), failure.host_version.as_deref());
                return Err(failure.error);
            }
        };

        let provider = Arc::new(OpenCodeHostProvider::new(
            OpenCodeHostConfig {
                base_url: wiring.base_url.clone(),
                provider_id: wiring.provider_id.clone(),
                // The WIRING's model: the provider ignores
                // `CompletionRequest.model` and names its own `model_ref()` in
                // the GENERATE body's `model` field (`Model.Ref`).
                model: wiring.model_id.clone(),
                authorization: wiring.authorization,
                variant: deps.variant.clone(),
                ..Default::default()
            },
            deps.transport.clone(),
        ));
        log_provider_ready(
            &wiring.base_url,
            wiring.host_version.as_deref(),
            &wiring.provider_id,
        );
        *write_or_recover(&self.state.cache) = Some(provider.clone());
        Ok(provider)
    }

    /// The startup probe: resolve once, returning the reason on failure.
    ///
    /// A failure is NOT cached, so the first capture retries resolution.
    pub fn warm(&self) -> Option<&'static str> {
        self.resolve().err().map(|error| error.reason())
    }

    /// The cached provider, or a fresh resolution, in `ProviderError` shape.
    ///
    /// The structured capture lane resolves on use exactly as `complete` does,
    /// and needs the same `Config` mapping a failed resolution produces.
    pub(crate) fn resolved(&self) -> Result<Arc<OpenCodeHostProvider>, ProviderError> {
        self.resolve().map_err(Self::resolve_error)
    }

    /// Drop the cached origin: the next use re-resolves `service.json`.
    ///
    /// The structured lane calls this on ANY error — a foreign process on a
    /// recycled port answers `401`/`404`/junk, never `Network`, and caching
    /// that origin pins a stranger for the engine's life.
    pub(crate) fn invalidate(&self) {
        *write_or_recover(&self.state.cache) = None;
    }

    /// The provider error shape of a resolution failure.
    fn resolve_error(error: HostWiringError) -> ProviderError {
        ProviderError::new(ProviderErrorKind::Config, error.reason()).with_provider(PROVIDER_NAME)
    }
}

impl AiProvider for LazyHostProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let state = self.state.clone();
        match self.resolve() {
            Ok(inner) => Box::pin(inner.complete(request).map(move |chunk| match chunk {
                Ok(chunk) => Ok(chunk),
                Err(error) => {
                    // ANY inner error invalidates the origin (see module docs).
                    *write_or_recover(&state.cache) = None;
                    Err(error)
                }
            })),
            Err(error) => Box::pin(mem_episodic::futures::stream::iter(vec![Err(
                Self::resolve_error(error),
            )])),
        }
    }

    fn embed<'life0, 'async_trait>(
        &'life0 self,
        request: EmbedRequest,
    ) -> BoxFuture<'async_trait, Result<Vec<Vec<f32>>, ProviderError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            match self.resolve() {
                Ok(inner) => inner.embed(request).await,
                Err(error) => Err(Self::resolve_error(error)),
            }
        })
    }

    fn list_models<'life0, 'async_trait>(
        &'life0 self,
    ) -> BoxFuture<'async_trait, Result<Vec<ModelInfo>, ProviderError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            match self.resolve() {
                Ok(inner) => inner.list_models().await,
                Err(error) => Err(Self::resolve_error(error)),
            }
        })
    }

    fn count_tokens<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        model: &'life1 str,
        text: &'life2 str,
    ) -> BoxFuture<'async_trait, Result<u32, ProviderError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            match self.resolve() {
                Ok(inner) => inner.count_tokens(model, text).await,
                Err(error) => Err(Self::resolve_error(error)),
            }
        })
    }
}

/// `Auto-capture provider ready` — one line per successful resolution.
///
/// `baseUrl` is omitted when empty (a preset manual provider fills its own
/// endpoint), `hostVersion` when the version is unreadable.
pub(crate) fn log_provider_ready(base_url: &str, host_version: Option<&str>, provider_id: &str) {
    log_fields(
        AUTO_CAPTURE_PROVIDER_READY,
        &[
            (
                "baseUrl",
                optional_str(Some(base_url).filter(|url| !url.is_empty())),
            ),
            ("hostVersion", optional_str(host_version)),
            ("providerId", Value::String(provider_id.to_owned())),
        ],
    );
}

/// `Auto-capture provider unavailable` — one line per failed resolution.
pub(crate) fn log_provider_unavailable(reason: &str, host_version: Option<&str>) {
    log_fields(
        AUTO_CAPTURE_PROVIDER_UNAVAILABLE,
        &[
            ("reason", Value::String(reason.to_owned())),
            ("hostVersion", optional_str(host_version)),
        ],
    );
}

fn optional_str(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_owned()))
}

/// One log line: `null` fields are OMITTED (the payload declares optionals
/// with `?`, never as an explicit `null`).
fn log_fields(signature: &str, fields: &[(&str, Value)]) {
    let mut map = serde_json::Map::new();
    for (key, value) in fields {
        if !value.is_null() {
            map.insert((*key).to_owned(), value.clone());
        }
    }
    log(signature, Some(&Value::Object(map)));
}
