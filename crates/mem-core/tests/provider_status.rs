//! Port of upstream `config.test.ts` provider-status / placeholder /
//! isConfigured clusters against the public helper API.

use mem_core::config::{
    AutoCaptureProviderMode, AutoCaptureProviderSettings, AutoCaptureProviderStatus,
    MemoryProvider, get_auto_capture_provider_status, has_auto_capture_provider_config,
    is_configured, is_placeholder_api_key, resolve_secret_value,
};
use mem_core::error::ConfigError;
use std::collections::BTreeMap;

fn settings(
    opencode_provider: Option<&str>,
    opencode_model: Option<&str>,
    memory_provider: Option<MemoryProvider>,
    memory_model: Option<&str>,
    memory_api_url: Option<&str>,
    memory_api_key: Option<&str>,
) -> AutoCaptureProviderSettings {
    AutoCaptureProviderSettings {
        opencode_provider: opencode_provider.map(str::to_owned),
        opencode_model: opencode_model.map(str::to_owned),
        memory_provider,
        memory_model: memory_model.map(str::to_owned),
        memory_api_url: memory_api_url.map(str::to_owned),
        memory_api_key: memory_api_key.map(str::to_owned),
    }
}

#[test]
fn test_template_manual_api_placeholders_not_usable_auto_capture_config() {
    // The config template ships `memoryApiKey: "sk-..."` as a placeholder —
    // model+url set, placeholder key, no opencode provider → NOT ready.
    let config = settings(
        None,
        None,
        None,
        Some("gpt-4o-mini"),
        Some("https://api.openai.com/v1"),
        Some("sk-..."),
    );
    assert!(!has_auto_capture_provider_config(&config));
    let issues = match get_auto_capture_provider_status(&config) {
        AutoCaptureProviderStatus::NotReady { issues } => issues,
        ready => panic!("expected not-ready, got {ready:?}"),
    };
    assert!(issues.contains(&"memoryApiKey contains a placeholder value".to_owned()));
}

#[test]
fn test_opencode_provider_settings_usable_auto_capture_config() {
    let config = settings(
        Some("anthropic"),
        Some("claude-haiku-4-5-20251001"),
        None,
        None,
        None,
        None,
    );
    assert!(has_auto_capture_provider_config(&config));
    assert_eq!(
        get_auto_capture_provider_status(&config),
        AutoCaptureProviderStatus::Ready {
            mode: AutoCaptureProviderMode::OpenCode
        }
    );
}

#[test]
fn test_manual_provider_ready_when_model_url_key_configured() {
    let config = settings(
        None,
        None,
        None,
        Some("local-model"),
        Some("http://127.0.0.1:11434/v1"),
        Some("local-api-key"),
    );
    assert!(has_auto_capture_provider_config(&config));
    assert_eq!(
        get_auto_capture_provider_status(&config),
        AutoCaptureProviderStatus::Ready {
            mode: AutoCaptureProviderMode::Manual
        }
    );
}

#[test]
fn test_missing_memory_api_key_reported_with_expected_issues() {
    let config = settings(
        None,
        None,
        None,
        Some("gpt-4o-mini"),
        Some("https://api.openai.com/v1"),
        None,
    );
    let issues = match get_auto_capture_provider_status(&config) {
        AutoCaptureProviderStatus::NotReady { issues } => issues,
        ready => panic!("expected not-ready, got {ready:?}"),
    };
    assert_eq!(
        issues,
        vec![
            "opencodeProvider is not configured".to_owned(),
            "opencodeModel is not configured".to_owned(),
            "memoryApiKey is not configured".to_owned(),
        ]
    );
}

#[test]
fn test_each_missing_manual_provider_field_reported_independently() {
    let config = settings(None, None, None, None, None, Some("sk-..."));
    let issues = match get_auto_capture_provider_status(&config) {
        AutoCaptureProviderStatus::NotReady { issues } => issues,
        ready => panic!("expected not-ready, got {ready:?}"),
    };
    assert_eq!(
        issues,
        vec![
            "opencodeProvider is not configured".to_owned(),
            "opencodeModel is not configured".to_owned(),
            "memoryModel is not configured".to_owned(),
            "memoryApiUrl is not configured".to_owned(),
            "memoryApiKey contains a placeholder value".to_owned(),
        ]
    );
}

#[test]
fn test_preset_providers_ready_on_key_alone() {
    // orcarouter / atlas-cloud fill endpoint+model themselves: an API key is
    // the only thing required for the manual fallback path.
    for provider in [MemoryProvider::OrcaRouter, MemoryProvider::AtlasCloud] {
        let config = settings(None, None, Some(provider), None, None, Some("real-key"));
        assert_eq!(
            get_auto_capture_provider_status(&config),
            AutoCaptureProviderStatus::Ready {
                mode: AutoCaptureProviderMode::Manual
            },
            "provider {provider:?}"
        );
    }
}

#[test]
fn test_placeholder_api_key_detection() {
    assert!(is_placeholder_api_key(Some("sk-...")));
    assert!(is_placeholder_api_key(Some("sk-ant-...")));
    assert!(is_placeholder_api_key(Some("gsk_...")));
    assert!(is_placeholder_api_key(Some("  SK-...  ")));
    assert!(is_placeholder_api_key(Some("Your-API-Key")));
    assert!(!is_placeholder_api_key(Some("sk-test-realish")));
    assert!(!is_placeholder_api_key(None));
    assert!(!is_placeholder_api_key(Some("")));
}

#[test]
fn test_is_configured_returns_true() {
    assert!(is_configured());
}

#[test]
fn test_missing_atlas_env_variable_is_secret_error() {
    // EnvSource injection keeps this hermetic: an injected empty map means the
    // `env://` lookup does not see any variable.
    let env: BTreeMap<String, String> = BTreeMap::new();
    let error = resolve_secret_value(Some("env://ATLASCLOUD_API_KEY"), &env)
        .expect_err("missing env secret must error");
    match &error {
        ConfigError::SecretEnvNotFound { name } => {
            assert_eq!(name, "ATLASCLOUD_API_KEY");
        }
        other => panic!("wrong variant: {other:?}"),
    }
    assert!(error.to_string().contains("ATLASCLOUD_API_KEY"));
    assert!(!error.to_string().contains("atlas-test-key"));
}
