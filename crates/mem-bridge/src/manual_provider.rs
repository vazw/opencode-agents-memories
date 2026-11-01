//! The documented **manual** auto-capture provider.
//!
//! `opencode-mem.jsonc` documents a manual fallback: `memoryProvider`,
//! `memoryModel`, `memoryApiUrl`, `memoryApiKey` (a literal, `env://…` or
//! `file://…` reference resolved at config load). Every piece existed —
//! `MemoryProvider`, `ProviderConfig`, `ProviderRegistry`, the seven adapters —
//! but nothing built a provider from those four fields, so the engine always
//! dialled the OpenCode host. This module is that wiring and nothing more.
//!
//! Precedence (owner ruling 2026-11-01, ADR-0028 as amended): the OpenCode host
//! is the primary route when the operator names it (`opencodeProvider` or
//! `opencodeModel`); with the host unnamed, a COMPLETE manual block is the
//! fallback, and a block that is partially filled is CLAIMED and therefore
//! fails closed with the named [`REASON_UNUSABLE`] — never silently replaced by
//! another provider.
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
use url::Url;

use mem_core::config::{Config, MemoryProvider, is_placeholder_api_key};
use mem_core::logging::log;

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
    // Mandate 5 (ADR-0028 D-28-6): a cleartext non-loopback endpoint carries
    // the bearer key in the open. Reported once, by name, and NOT refused —
    // a private-LAN proxy is the owner's working configuration; the long-lived
    // fix (TLS, or a loopback tunnel) is theirs to make.
    if let Some(api_url) = api_url.filter(|url| is_cleartext_non_loopback(url)) {
        log_insecure_endpoint(api_url);
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

/// True when `url` moves a bearer credential over CLEARTEXT to a NON-loopback
/// host (mandate 5, ADR-0028 D-28-6).
///
/// `http://` on loopback stays allowed: that is the documented local-provider
/// case (Ollama-style endpoints on `127.0.0.1`/`localhost`/`[::1]`), where the
/// credential never leaves the machine. `https://` is always allowed, and an
/// unparsable value is not this predicate's business — the registry rejects it
/// on its own terms.
pub fn is_cleartext_non_loopback(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url.trim()) else {
        return false;
    };
    if parsed.scheme() != "http" {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Ipv4(address)) => !address.is_loopback(),
        Some(url::Host::Ipv6(address)) => !address.is_loopback(),
        Some(url::Host::Domain(domain)) => !domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// The endpoint as it may be LOGGED: userinfo stripped, everything else kept.
///
/// F6: `memoryApiUrl` is operator input and an embedded `user:pass@` would be
/// written to the sink verbatim. The host arm logs a resolved ORIGIN only; this
/// keeps the path (which is the diagnosable part, e.g. `/v1`) and drops the
/// credential. A value that does not parse is stripped textually rather than
/// trusted.
pub fn sanitized_base_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match Url::parse(trimmed) {
        Ok(mut parsed) => {
            if !parsed.username().is_empty() || parsed.password().is_some() {
                // Errors here mean "cannot be a base URL", which cannot happen
                // for an already-parsed `http(s)` URL; the `let _` keeps the
                // redaction unconditional.
                let _ = parsed.set_username("");
                let _ = parsed.set_password(None);
            }
            parsed.to_string()
        }
        Err(_) => strip_userinfo_textually(trimmed),
    }
}

/// The unparsable-value fallback: drop everything before the last `@` of the
/// authority so a malformed URL cannot leak a credential into the sink.
fn strip_userinfo_textually(raw: &str) -> String {
    let authority_end = raw.find('/').unwrap_or(raw.len());
    let authority = &raw[..authority_end];
    let Some((_, host)) = authority.rsplit_once('@') else {
        return raw.to_owned();
    };
    match authority.split_once("://") {
        Some((scheme, _)) => format!("{scheme}://{host}{}", &raw[authority_end..]),
        None => format!("{host}{}", &raw[authority_end..]),
    }
}

/// `Auto-capture provider insecure endpoint` — the mandate-5 report (D-28-6).
///
/// Payload: the sanitized endpoint (userinfo stripped), the scheme, and the
/// remediation hint. The resolved key is never an input here.
fn log_insecure_endpoint(url: &str) {
    let mut map = serde_json::Map::new();
    map.insert(
        "baseUrl".to_owned(),
        serde_json::Value::String(sanitized_base_url(url)),
    );
    map.insert(
        "scheme".to_owned(),
        serde_json::Value::String("http".to_owned()),
    );
    map.insert(
        "hint".to_owned(),
        serde_json::Value::String(
            "the API key crosses this endpoint in cleartext; use https:// (or a loopback tunnel)"
                .to_owned(),
        ),
    );
    log(
        mem_core::logging::signatures::AUTO_CAPTURE_PROVIDER_INSECURE_ENDPOINT,
        Some(&serde_json::Value::Object(map)),
    );
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

/// True when the operator claimed the manual block.
///
/// Signals, in the order the config layer knows them: `memory_provider_claimed`
/// (the `memoryProvider` key's PRESENCE — the resolved value always exists, so
/// it cannot answer this; rework F2), or any of the three values the config's
/// own comment calls "all 3 lines".
fn is_claimed(config: &Config) -> bool {
    config.memory_provider_claimed
        || non_blank(config.memory_model.as_deref()).is_some()
        || non_blank(config.memory_api_url.as_deref()).is_some()
        || non_blank(config.memory_api_key.as_deref()).is_some()
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
