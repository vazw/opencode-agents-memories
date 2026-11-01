//! Provider config + registry contract (upstream `ai-provider-config.test.ts`).
//!
//! Covers typed config resolution, timeout plumbing, connected-provider
//! refresh delegation, and the registry fallback chain.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mem_ai::provider::{
    AiProvider, OpenCodeHostConfig, OpenCodeHostProvider, ProviderConfig, ProviderErrorKind,
    ProviderKind, ProviderRegistry, ReqwestHostTransport, StaticSecretResolver,
};

fn registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new(Arc::new(StaticSecretResolver::new()));
    registry.register_builtin();
    registry
}

fn host_config(model: &str) -> OpenCodeHostConfig {
    // Explicit: `Default::default()` is deliberately NOT dialable (empty base
    // URL), so every test names the endpoint it dials.
    OpenCodeHostConfig {
        model: model.to_owned(),
        base_url: "http://127.0.0.1:4096".to_owned(),
        ..OpenCodeHostConfig::default()
    }
}

#[test]
fn test_provider_config_resolution() {
    let config = ProviderConfig::OpenCodeHost(host_config("gpt-4o-mini"));

    assert_eq!(config.kind(), ProviderKind::OpenCodeHost);
    assert_eq!(config.kind().as_str(), "opencode-host");
    assert_eq!(config.model(), "gpt-4o-mini");
    assert_eq!(config.base_url(), "http://127.0.0.1:4096");

    let registry = registry();
    assert_eq!(registry.default_provider(), ProviderKind::OpenCodeHost);

    // The factory yields the provider erased behind the trait seam.
    let provider: Arc<dyn AiProvider> = registry.create(&config).expect("builtin factory resolves");
    assert_eq!(provider.name(), "opencode-host");
}

#[test]
fn test_provider_config_timeout() {
    let config = OpenCodeHostConfig {
        timeout: Duration::from_millis(1234),
        ..OpenCodeHostConfig::default()
    };
    let wrapped = ProviderConfig::OpenCodeHost(config.clone());
    assert_eq!(wrapped.timeout(), Duration::from_millis(1234));

    // The host provider surfaces the configured timeout it enforces.
    let transport = Arc::new(ReqwestHostTransport::new());
    let provider = OpenCodeHostProvider::new(config, transport);
    assert_eq!(provider.timeout(), Duration::from_millis(1234));

    // Defaults mirror the upstream hard ceiling (90s).
    assert_eq!(
        OpenCodeHostConfig::default().timeout,
        Duration::from_millis(90_000)
    );
}

#[tokio::test]
async fn test_connected_provider_refresh() {
    let transport = Arc::new(ReqwestHostTransport::new());
    let provider = Arc::new(OpenCodeHostProvider::new(
        OpenCodeHostConfig {
            base_url: "http://127.0.0.1:4096".to_owned(),
            ..OpenCodeHostConfig::default()
        },
        transport,
    ));

    provider.set_connected_providers(vec!["anthropic".to_owned(), "github-copilot".to_owned()]);
    assert!(provider.is_provider_connected("anthropic"));
    assert!(provider.is_provider_connected("github-copilot"));
    assert!(!provider.is_provider_connected("openai"));

    // A refresher registers late-arriving providers; refresh happens on miss
    // only, never on hit.
    let refresh_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&refresh_calls);
    let weak = Arc::downgrade(&provider);
    provider.set_connected_providers_refresher(Arc::new(move || {
        let calls = Arc::clone(&calls);
        let weak = weak.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if let Some(provider) = weak.upgrade() {
                provider.set_connected_providers(vec![
                    "anthropic".to_owned(),
                    "github-copilot".to_owned(),
                    "newapi".to_owned(),
                ]);
            }
        })
    }));

    assert!(provider.ensure_provider_connected("anthropic").await);
    assert_eq!(refresh_calls.load(Ordering::SeqCst), 0);

    assert!(provider.ensure_provider_connected("newapi").await);
    assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn test_fallback_chain() {
    let registry = registry();

    // A broken config (empty base URL) fails the factory; the chain then
    // succeeds on the next entry.
    let broken = ProviderConfig::OpenCodeHost(OpenCodeHostConfig {
        base_url: String::new(),
        ..host_config("broken-model")
    });
    let good = ProviderConfig::OpenCodeHost(host_config("good-model"));

    let provider = registry
        .resolve_with_fallback(&[broken, good])
        .expect("falls through to the valid config");
    assert_eq!(provider.name(), "opencode-host");

    // A single valid entry resolves directly.
    let single = registry
        .resolve_with_fallback(&[ProviderConfig::OpenCodeHost(host_config("only"))])
        .expect("single valid config resolves");
    assert_eq!(single.name(), "opencode-host");

    // An empty chain is a configuration error.
    let err = match registry.resolve_with_fallback(&[]) {
        Ok(_) => panic!("empty chain must fail"),
        Err(error) => error,
    };
    assert_eq!(err.kind, ProviderErrorKind::Config);
}
