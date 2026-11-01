//! The documented **manual** auto-capture provider.
//!
//! `opencode-mem.jsonc` documents a manual fallback: `memoryProvider`,
//! `memoryModel`, `memoryApiUrl`, `memoryApiKey` (a literal, `env://…` or
//! `file://…` reference resolved at config load). Every piece existed —
//! `MemoryProvider`, `ProviderConfig`, `ProviderRegistry`, the seven adapters —
//! but nothing built a provider from those four fields, so the engine always
//! dialled the OpenCode host. This module is that wiring and nothing more.
//!
//! Precedence (owner ruling 2026-11-01, ADR-0028): a COMPLETE manual block wins;
//! otherwise the OpenCode host provider applies. A block that is partially
//! filled is CLAIMED and therefore fails closed with the named
//! [`REASON_UNUSABLE`] — never silently replaced by another provider.
//!
//! The resolved key is seated in a [`StaticSecretResolver`] under
//! [`MEMORY_API_KEY_REF`] and reaches the adapter as an `Authorization: Bearer`
//! header resolved per call; it is never stored on the provider struct, never
//! logged, and never formatted.

use std::sync::Arc;

use mem_ai::provider::{
    AiProvider, HttpTransport, ProviderConfig, ProviderRegistry, RemoteProviderConfig,
    ReqwestTransport, StaticSecretResolver,
};

use mem_core::config::{Config, MemoryProvider, is_placeholder_api_key};

/// The synthetic secret reference the config's resolved key is seated under.
pub const MEMORY_API_KEY_REF: &str = "memoryApiKey";

/// The one named reason for a claimed-but-unusable manual block.
///
/// It covers three inputs: a required field is missing, the key is an obvious
/// placeholder, or the registry rejected the config. They share a string on
/// purpose — the log line names the layer, and the operator fixes config.
pub const REASON_UNUSABLE: &str = "memory_provider_unusable";

/// Build the manual provider over `transport` (the test seam).
///
/// `Ok(None)` = the manual block is not configured (no manual field set at
/// all); `Err(REASON_UNUSABLE)` = the block is claimed but cannot be built.
pub fn build_manual_provider(
    config: &Config,
    transport: Arc<dyn HttpTransport>,
) -> Result<Option<Arc<dyn AiProvider>>, &'static str> {
    if !is_claimed(config) {
        return Ok(None);
    }
    let model = non_blank(config.memory_model.as_deref());
    let api_url = non_blank(config.memory_api_url.as_deref());
    let api_key = non_blank(config.memory_api_key.as_deref());
    let is_preset = matches!(
        config.memory_provider,
        MemoryProvider::AtlasCloud | MemoryProvider::OrcaRouter
    );
    // Preset providers fill endpoint and model themselves; every other adapter
    // needs both (upstream `getAutoCaptureProviderStatus`).
    let usable = if is_preset {
        api_key.is_some()
    } else {
        model.is_some() && api_url.is_some() && api_key.is_some()
    };
    let Some(api_key) = api_key else {
        return Err(REASON_UNUSABLE);
    };
    if !usable || is_placeholder_api_key(config.memory_api_key.as_deref()) {
        return Err(REASON_UNUSABLE);
    }

    let resolver = Arc::new(StaticSecretResolver::new().with_secret(MEMORY_API_KEY_REF, api_key));
    let mut remote = RemoteProviderConfig::new(model.unwrap_or_default());
    if let Some(api_url) = api_url {
        remote.base_url = api_url.to_owned();
    }
    remote.api_key_ref = Some(MEMORY_API_KEY_REF.to_owned());
    if let Some(force_tool_choice) = config.force_tool_choice {
        remote.force_tool_choice = force_tool_choice;
    }
    let provider_config = provider_config(config.memory_provider, remote);

    let mut registry = ProviderRegistry::new(resolver);
    registry.register_builtin_with(transport);
    registry
        .create(&provider_config)
        .map(Some)
        .map_err(|_| REASON_UNUSABLE)
}

/// The production entry point: the manual provider over `reqwest`, or `None`
/// when the block is unconfigured.
pub fn manual_capture_provider(
    config: &Config,
) -> Result<Option<Arc<dyn AiProvider>>, &'static str> {
    build_manual_provider(config, Arc::new(ReqwestTransport::new()))
}

/// The typed provider config for one `memoryProvider` value.
pub fn provider_config(provider: MemoryProvider, remote: RemoteProviderConfig) -> ProviderConfig {
    match provider {
        MemoryProvider::OpenAiChat => ProviderConfig::OpenAIChat(remote),
        MemoryProvider::OpenAiResponses => ProviderConfig::OpenAIResponses(remote),
        MemoryProvider::Anthropic => ProviderConfig::Anthropic(remote),
        MemoryProvider::Minimax => ProviderConfig::Minimax(remote),
        MemoryProvider::AtlasCloud => ProviderConfig::AtlasCloud(remote),
        MemoryProvider::OrcaRouter => ProviderConfig::OrcaRouter(remote),
    }
}

/// True when the operator filled in at least one manual field.
///
/// `memoryProvider` alone cannot say this: it is not optional in the resolved
/// config (it defaults to `openai-chat`), so the three values the config's own
/// comment calls "all 3 lines" are the signal.
fn is_claimed(config: &Config) -> bool {
    non_blank(config.memory_model.as_deref()).is_some()
        || non_blank(config.memory_api_url.as_deref()).is_some()
        || non_blank(config.memory_api_key.as_deref()).is_some()
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
