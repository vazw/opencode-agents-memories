//! `ProviderRegistry` — the provider factory.
//!
//! Holds typed [`ProviderConfig`] values, resolves them to `Arc<dyn AiProvider>`
//! through registered factories, and owns the secret resolver it was injected
//! with. `register_builtin` wires the OpenCode host provider plus the seven
//! W2.2 remote adapters, without touching callers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::provider::error::ProviderError;
use crate::provider::opencode_host::{
    OpenCodeHostConfig, OpenCodeHostProvider, ReqwestHostTransport,
};
use crate::provider::provider_trait::AiProvider;
use crate::provider::remote::anthropic::{self as anthropic, AnthropicProvider};
use crate::provider::remote::atlas_cloud::{self as atlas_cloud, AtlasCloudProvider};
use crate::provider::remote::gemini::{self as gemini, GeminiProvider};
use crate::provider::remote::minimax::{self as minimax, MinimaxProvider};
use crate::provider::remote::openai_chat::{self as openai_chat, OpenAiChatProvider};
use crate::provider::remote::openai_responses::{
    self as openai_responses, OpenAiResponsesProvider,
};
use crate::provider::remote::orcarouter::{self as orcarouter, OrcaRouterProvider};
use crate::provider::remote::plumbing::{HttpTransport, RemoteProviderConfig, ReqwestTransport};
use crate::provider::secret_resolver::SecretResolver;

/// Constructor signature shared by every remote adapter: the already-validated
/// config plus the injected secret resolver, yielding a boxed provider.
///
/// A boxed closure (not a `fn` pointer) so the HTTP transport can be captured:
/// [`ProviderRegistry::register_builtin`] captures `reqwest`, and
/// [`ProviderRegistry::register_builtin_with`] captures whatever the caller
/// injects.
type RemoteBuild = Arc<
    dyn Fn(
            RemoteProviderConfig,
            Arc<dyn SecretResolver>,
        ) -> Result<Arc<dyn AiProvider>, ProviderError>
        + Send
        + Sync,
>;

/// Discriminant for a registered provider kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProviderKind {
    /// The OpenCode host SDK / HTTP provider.
    OpenCodeHost,
    /// Anthropic Messages API.
    Anthropic,
    /// OpenAI Chat Completions API.
    OpenAIChat,
    /// OpenAI Responses API.
    OpenAIResponses,
    /// Google Gemini `generateContent` API.
    Gemini,
    /// MiniMax (Anthropic Messages-compatible).
    Minimax,
    /// Atlas Cloud (OpenAI-compatible).
    AtlasCloud,
    /// OrcaRouter (OpenAI-compatible gateway).
    OrcaRouter,
}

impl ProviderKind {
    /// Stable, config-facing label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenCodeHost => "opencode-host",
            Self::Anthropic => anthropic::PROVIDER_NAME,
            Self::OpenAIChat => openai_chat::PROVIDER_NAME,
            Self::OpenAIResponses => openai_responses::PROVIDER_NAME,
            Self::Gemini => gemini::PROVIDER_NAME,
            Self::Minimax => minimax::PROVIDER_NAME,
            Self::AtlasCloud => atlas_cloud::PROVIDER_NAME,
            Self::OrcaRouter => orcarouter::PROVIDER_NAME,
        }
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed provider configuration. One variant per provider kind so the factory
/// can resolve a config without stringly-typed lookups.
///
/// The seven remote adapters share [`RemoteProviderConfig`] because their
/// configuration surface is identical; only the defaults differ, and those are
/// applied by each adapter.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderConfig {
    /// OpenCode host provider.
    OpenCodeHost(OpenCodeHostConfig),
    /// Anthropic Messages provider.
    Anthropic(RemoteProviderConfig),
    /// OpenAI chat-completion provider.
    OpenAIChat(RemoteProviderConfig),
    /// OpenAI responses provider.
    OpenAIResponses(RemoteProviderConfig),
    /// Google Gemini provider.
    Gemini(RemoteProviderConfig),
    /// MiniMax provider.
    Minimax(RemoteProviderConfig),
    /// Atlas Cloud provider.
    AtlasCloud(RemoteProviderConfig),
    /// OrcaRouter provider.
    OrcaRouter(RemoteProviderConfig),
}

impl ProviderConfig {
    /// The provider kind this config targets.
    pub fn kind(&self) -> ProviderKind {
        match self {
            Self::OpenCodeHost(_) => ProviderKind::OpenCodeHost,
            Self::Anthropic(_) => ProviderKind::Anthropic,
            Self::OpenAIChat(_) => ProviderKind::OpenAIChat,
            Self::OpenAIResponses(_) => ProviderKind::OpenAIResponses,
            Self::Gemini(_) => ProviderKind::Gemini,
            Self::Minimax(_) => ProviderKind::Minimax,
            Self::AtlasCloud(_) => ProviderKind::AtlasCloud,
            Self::OrcaRouter(_) => ProviderKind::OrcaRouter,
        }
    }

    /// The model id the config targets.
    pub fn model(&self) -> &str {
        match self {
            Self::OpenCodeHost(config) => &config.model,
            Self::Anthropic(config)
            | Self::OpenAIChat(config)
            | Self::OpenAIResponses(config)
            | Self::Gemini(config)
            | Self::Minimax(config)
            | Self::AtlasCloud(config)
            | Self::OrcaRouter(config) => &config.model,
        }
    }

    /// The base URL the config targets.
    pub fn base_url(&self) -> &str {
        match self {
            Self::OpenCodeHost(config) => &config.base_url,
            Self::Anthropic(config)
            | Self::OpenAIChat(config)
            | Self::OpenAIResponses(config)
            | Self::Gemini(config)
            | Self::Minimax(config)
            | Self::AtlasCloud(config)
            | Self::OrcaRouter(config) => &config.base_url,
        }
    }

    /// The request budget the config imposes.
    pub fn timeout(&self) -> Duration {
        match self {
            Self::OpenCodeHost(config) => config.timeout,
            Self::Anthropic(config)
            | Self::OpenAIChat(config)
            | Self::OpenAIResponses(config)
            | Self::Gemini(config)
            | Self::Minimax(config)
            | Self::AtlasCloud(config)
            | Self::OrcaRouter(config) => config.timeout,
        }
    }
}

/// A registered factory: config in, trait-object provider out.
type ProviderFactory =
    Arc<dyn Fn(&ProviderConfig) -> Result<Arc<dyn AiProvider>, ProviderError> + Send + Sync>;

/// The provider factory + injection seam.
pub struct ProviderRegistry {
    resolver: Arc<dyn SecretResolver>,
    factories: HashMap<ProviderKind, ProviderFactory>,
}

impl ProviderRegistry {
    /// Build a registry with an injected [`SecretResolver`].
    ///
    /// The resolver is never constructed here — it is supplied by the caller
    /// (W2.3 wires the file-backed one).
    pub fn new(resolver: Arc<dyn SecretResolver>) -> Self {
        Self {
            resolver,
            factories: HashMap::new(),
        }
    }

    /// The injected resolver.
    pub fn resolver(&self) -> &Arc<dyn SecretResolver> {
        &self.resolver
    }

    /// The provider kind used when no explicit config is supplied.
    pub fn default_provider(&self) -> ProviderKind {
        ProviderKind::OpenCodeHost
    }

    /// Register an explicit factory for `kind` (used by W2.2/provider tests).
    pub fn register(&mut self, kind: ProviderKind, factory: ProviderFactory) -> &mut Self {
        self.factories.insert(kind, factory);
        self
    }

    /// Register the built-in providers over the production `reqwest` transport.
    pub fn register_builtin(&mut self) -> &mut Self {
        self.register_builtin_with(Arc::new(ReqwestTransport::new()))
    }

    /// Register the built-in providers over an INJECTED HTTP transport.
    ///
    /// The seam exists so a caller can drive the real factories (real configs,
    /// real adapters) with a recording transport; production always uses
    /// [`Self::register_builtin`]. The `opencode-host` provider keeps its own
    /// [`ReqwestHostTransport`].
    pub fn register_builtin_with(&mut self, transport: Arc<dyn HttpTransport>) -> &mut Self {
        self.register(
            ProviderKind::OpenCodeHost,
            Arc::new(|config| {
                let ProviderConfig::OpenCodeHost(host) = config else {
                    return Err(ProviderError::config(
                        "opencode-host factory received a foreign config",
                    )
                    .with_provider("registry"));
                };
                host.validate()?;
                let provider =
                    OpenCodeHostProvider::new(host.clone(), Arc::new(ReqwestHostTransport::new()));
                Ok(Arc::new(provider) as Arc<dyn AiProvider>)
            }),
        );

        let resolver = self.resolver.clone();
        // One handle per factory: each `move` closure consumes its own clone.
        let remote_transport = transport.clone();

        let transport = remote_transport.clone();
        self.register(
            ProviderKind::Anthropic,
            Self::remote_factory(
                resolver.clone(),
                ProviderKind::Anthropic,
                anthropic::PROVIDER_NAME,
                false,
                Arc::new(move |config, resolver| {
                    Ok(
                        Arc::new(AnthropicProvider::new(config, resolver, transport.clone()))
                            as Arc<dyn AiProvider>,
                    )
                }),
            ),
        );
        let transport = remote_transport.clone();
        self.register(
            ProviderKind::OpenAIChat,
            Self::remote_factory(
                resolver.clone(),
                ProviderKind::OpenAIChat,
                openai_chat::PROVIDER_NAME,
                false,
                Arc::new(move |config, resolver| {
                    Ok(
                        Arc::new(OpenAiChatProvider::new(config, resolver, transport.clone()))
                            as Arc<dyn AiProvider>,
                    )
                }),
            ),
        );
        let transport = remote_transport.clone();
        self.register(
            ProviderKind::OpenAIResponses,
            Self::remote_factory(
                resolver.clone(),
                ProviderKind::OpenAIResponses,
                openai_responses::PROVIDER_NAME,
                false,
                Arc::new(move |config, resolver| {
                    Ok(Arc::new(OpenAiResponsesProvider::new(
                        config,
                        resolver,
                        transport.clone(),
                    )) as Arc<dyn AiProvider>)
                }),
            ),
        );
        let transport = remote_transport.clone();
        self.register(
            ProviderKind::Gemini,
            Self::remote_factory(
                resolver.clone(),
                ProviderKind::Gemini,
                gemini::PROVIDER_NAME,
                false,
                Arc::new(move |config, resolver| {
                    Ok(
                        Arc::new(GeminiProvider::new(config, resolver, transport.clone()))
                            as Arc<dyn AiProvider>,
                    )
                }),
            ),
        );
        let transport = remote_transport.clone();
        self.register(
            ProviderKind::Minimax,
            Self::remote_factory(
                resolver.clone(),
                ProviderKind::Minimax,
                minimax::PROVIDER_NAME,
                true,
                Arc::new(move |config, resolver| {
                    Ok(
                        Arc::new(MinimaxProvider::new(config, resolver, transport.clone()))
                            as Arc<dyn AiProvider>,
                    )
                }),
            ),
        );
        let transport = remote_transport.clone();
        self.register(
            ProviderKind::AtlasCloud,
            Self::remote_factory(
                resolver.clone(),
                ProviderKind::AtlasCloud,
                atlas_cloud::PROVIDER_NAME,
                false,
                Arc::new(move |config, resolver| {
                    Ok(
                        Arc::new(AtlasCloudProvider::new(config, resolver, transport.clone()))
                            as Arc<dyn AiProvider>,
                    )
                }),
            ),
        );
        let transport = remote_transport.clone();
        self.register(
            ProviderKind::OrcaRouter,
            Self::remote_factory(
                resolver.clone(),
                ProviderKind::OrcaRouter,
                orcarouter::PROVIDER_NAME,
                false,
                Arc::new(move |config, resolver| {
                    Ok(
                        Arc::new(OrcaRouterProvider::new(config, resolver, transport.clone()))
                            as Arc<dyn AiProvider>,
                    )
                }),
            ),
        );

        self
    }

    /// Build a remote-adapter factory.
    ///
    /// `require_base_url` is true for adapters with no default endpoint
    /// (MiniMax); the config is validated before the provider is constructed.
    fn remote_factory(
        resolver: Arc<dyn SecretResolver>,
        kind: ProviderKind,
        provider_name: &'static str,
        require_base_url: bool,
        build: RemoteBuild,
    ) -> ProviderFactory {
        Arc::new(move |config| {
            let remote = match config {
                ProviderConfig::Anthropic(config) if kind == ProviderKind::Anthropic => config,
                ProviderConfig::OpenAIChat(config) if kind == ProviderKind::OpenAIChat => config,
                ProviderConfig::OpenAIResponses(config)
                    if kind == ProviderKind::OpenAIResponses =>
                {
                    config
                }
                ProviderConfig::Gemini(config) if kind == ProviderKind::Gemini => config,
                ProviderConfig::Minimax(config) if kind == ProviderKind::Minimax => config,
                ProviderConfig::AtlasCloud(config) if kind == ProviderKind::AtlasCloud => config,
                ProviderConfig::OrcaRouter(config) if kind == ProviderKind::OrcaRouter => config,
                _ => {
                    return Err(ProviderError::config(format!(
                        "{provider_name} factory received a foreign config"
                    ))
                    .with_provider("registry"));
                }
            };
            remote.validate(provider_name, require_base_url)?;
            (build)(remote.clone(), resolver.clone())
        })
    }

    /// Resolve one config through its registered factory.
    pub fn create(&self, config: &ProviderConfig) -> Result<Arc<dyn AiProvider>, ProviderError> {
        let kind = config.kind();
        let factory = self.factories.get(&kind).ok_or_else(|| {
            ProviderError::config(format!("no factory registered for provider kind {kind}"))
                .with_provider("registry")
        })?;
        factory(config)
    }

    /// Resolve the first config that produces a provider (fallback chain).
    ///
    /// Returns the last factory error when every entry fails, and a
    /// configuration error when the chain is empty.
    pub fn resolve_with_fallback(
        &self,
        configs: &[ProviderConfig],
    ) -> Result<Arc<dyn AiProvider>, ProviderError> {
        let mut last_error = None;
        for config in configs {
            match self.create(config) {
                Ok(provider) => return Ok(provider),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            ProviderError::config("no provider configs supplied").with_provider("registry")
        }))
    }
}
