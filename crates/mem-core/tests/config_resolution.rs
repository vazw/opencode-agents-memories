//! Port of upstream `config-resolution.test.ts` — fs spies / HOME sandboxes
//! replaced by real temp files plus injected `ConfigPaths` and `BTreeMap` env
//! (no process-environment mutation, so parallel tests cannot race).

use std::collections::BTreeMap;
use std::error::Error as _;
use std::path::PathBuf;

use mem_core::config::{
    Config, ConfigPaths, OPENCODE_TIMEOUT_MS_DEFAULT, outer_structured_output_timeout_ms,
    resolve_secret_value,
};
use mem_core::error::ConfigError;
mod common;

type Env = BTreeMap<String, String>;

fn env_of(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// Builds a temp home + temp project dir, writes the requested candidate
/// files, and runs the resolution chain through the public `Config::load`.
fn run(global: Option<&str>, project: Option<&str>, env: &Env) -> Result<Config, ConfigError> {
    let home = common::unique_temp_dir("resolution-home");
    if let Some(content) = global {
        common::write_file(&home, ".config/opencode/opencode-mem.jsonc", content);
    }
    let project_dir = common::unique_temp_dir("resolution-project");
    if let Some(content) = project {
        common::write_file(&project_dir, ".opencode/opencode-mem.jsonc", content);
    }
    let paths = ConfigPaths::for_project(&project_dir, &home);
    Config::load(&paths, env)
}

fn run_ok(global: Option<&str>, project: Option<&str>, env: &Env) -> Config {
    run(global, project, env).expect("resolution must succeed")
}

#[test]
fn test_global_config_used_when_no_project_config() {
    let config = run_ok(
        Some(r#"{"opencodeModel": "global-model"}"#),
        None,
        &Env::new(),
    );
    assert_eq!(config.opencode_model.as_deref(), Some("global-model"));
}

#[test]
fn test_project_config_overrides_global_config() {
    let config = run_ok(
        Some(r#"{"opencodeProvider": "anthropic", "opencodeModel": "global-model"}"#),
        Some(r#"{"opencodeProvider": "openai", "opencodeModel": "project-model"}"#),
        &Env::new(),
    );
    assert_eq!(config.opencode_provider.as_deref(), Some("openai"));
    assert_eq!(config.opencode_model.as_deref(), Some("project-model"));
}

#[test]
fn test_shallow_merge_project_adds_and_global_preserved() {
    let config = run_ok(
        Some(r#"{"opencodeModel": "claude-haiku", "autoCaptureEnabled": false}"#),
        Some(r#"{"opencodeProvider": "anthropic"}"#),
        &Env::new(),
    );
    assert_eq!(config.opencode_provider.as_deref(), Some("anthropic"));
    assert_eq!(config.opencode_model.as_deref(), Some("claude-haiku"));
    assert!(!config.auto_capture_enabled);
}

#[test]
fn test_invalid_shape_error_names_key_path() {
    let error = run(Some(r#"{"webServerPort": "eighty"}"#), None, &Env::new())
        .expect_err("wrong-typed known key must be rejected");

    let ConfigError::InvalidShape { path, .. } = &error else {
        panic!("expected InvalidShape, got: {error:?}")
    };
    assert!(
        path.contains("webServerPort"),
        "path must name the offending key: {path}"
    );
    assert!(
        error.to_string().contains("webServerPort"),
        "display must name the key: {error}"
    );
    // The #[source] chain carries the underlying serde error.
    let serde_error = error
        .source()
        .and_then(|e| e.downcast_ref::<serde_json::Error>())
        .expect("InvalidShape must chain to the serde error");
    assert!(serde_error.to_string().contains("eighty"), "{serde_error}");
}

#[test]
fn test_jsonc_string_values_preserved_through_loading() {
    let global = r#"{
        // fixture comments tolerated
        "storagePath": "/tmp/cache,]v2",
        "memoryModel": "local-model",
        "memoryApiUrl": "https://example.invalid/v1",
        "memoryApiKey": "fixture-only-key",
        "memoryExtraParams": {"stop": [",}", ",]",],},
    }"#;
    let config = run_ok(Some(global), None, &Env::new());
    assert_eq!(config.storage_path, PathBuf::from("/tmp/cache,]v2"));
    assert_eq!(config.memory_api_key.as_deref(), Some("fixture-only-key"));
    // Extra params survive loading verbatim (provider round-trip is W2.1).
    let extra = config.memory_extra_params.expect("params preserved");
    assert_eq!(extra, serde_json::json!({ "stop": [",}", ",]"] }));
}

#[test]
fn test_auto_cleanup_policy_stays_global() {
    let config = run_ok(
        Some(
            r#"{"opencodeModel": "global-model", "autoCleanupEnabled": false, "autoCleanupRetentionDays": 90}"#,
        ),
        Some(
            r#"{"opencodeModel": "project-model", "autoCleanupEnabled": true, "autoCleanupRetentionDays": 0}"#,
        ),
        &Env::new(),
    );
    assert_eq!(config.opencode_model.as_deref(), Some("project-model"));
    assert!(!config.auto_cleanup_enabled);
    assert_eq!(config.auto_cleanup_retention_days, 90);
}

#[test]
fn test_safe_cleanup_defaults_when_only_project_cleanup_settings() {
    let config = run_ok(
        None,
        Some(r#"{"autoCleanupEnabled": true, "autoCleanupRetentionDays": -1}"#),
        &Env::new(),
    );
    assert!(config.auto_cleanup_enabled);
    assert_eq!(config.auto_cleanup_retention_days, 30);
}

#[test]
fn test_project_embedding_transport_settings_rejected() {
    let env = env_of(&[("OPENAI_API_KEY", "ambient-secret")]);
    let error = run(
        Some(r#"{"embeddingModel": "global-model"}"#),
        Some(r#"{"embeddingApiUrl": "https://attacker.example/v1"}"#),
        &env,
    )
    .expect_err("project remote-provider field must be rejected");

    let ConfigError::ProjectRemoteProviderFields { fields, .. } = &error else {
        panic!("expected ProjectRemoteProviderFields, got: {error:?}")
    };
    assert_eq!(fields, &vec!["embeddingApiUrl".to_owned()]);
    // Rejected before any secret resolution: the error never leaks secrets and
    // the ambient env var could not have been inherited.
    assert!(
        !error.to_string().contains("ambient-secret"),
        "must not leak secret values: {error}"
    );
}

#[test]
fn test_project_memory_provider_settings_rejected() {
    let error = run(
        Some(r#"{"memoryApiKey": "global-secret"}"#),
        Some(r#"{"memoryProvider": "orcarouter", "memoryApiUrl": "https://attacker.example/v1"}"#),
        &Env::new(),
    )
    .expect_err("project memory provider settings must be rejected");
    let ConfigError::ProjectRemoteProviderFields { fields, .. } = &error else {
        panic!("expected ProjectRemoteProviderFields, got: {error:?}")
    };
    assert_eq!(
        fields,
        &vec!["memoryProvider".to_owned(), "memoryApiUrl".to_owned()]
    );
}

#[test]
fn test_global_remote_providers_and_project_overrides_work() {
    let env = env_of(&[
        ("TEST_EMBEDDING_KEY", "global-embedding-secret"),
        ("TEST_MEMORY_KEY", "global-memory-secret"),
    ]);
    let config = run_ok(
        Some(
            r#"{
            "embeddingApiUrl": "https://trusted-embeddings.example/v1",
            "embeddingApiKey": "env://TEST_EMBEDDING_KEY",
            "memoryProvider": "openai-chat",
            "memoryApiUrl": "https://trusted-memory.example/v1",
            "memoryApiKey": "env://TEST_MEMORY_KEY"
        }"#,
        ),
        Some(r#"{"opencodeModel": "project-model"}"#),
        &env,
    );
    assert_eq!(config.opencode_model.as_deref(), Some("project-model"));
    assert_eq!(
        config.embedding_api_url.as_deref(),
        Some("https://trusted-embeddings.example/v1")
    );
    assert_eq!(
        config.embedding_api_key.as_deref(),
        Some("global-embedding-secret")
    );
    assert_eq!(
        config.memory_api_url.as_deref(),
        Some("https://trusted-memory.example/v1")
    );
    assert_eq!(
        config.memory_api_key.as_deref(),
        Some("global-memory-secret")
    );
}

#[test]
fn test_config_memory_api_key_beats_atlas_env_fallback() {
    let env = env_of(&[("ATLASCLOUD_API_KEY", "atlas-env-key")]);
    let config = run_ok(
        Some(r#"{"memoryProvider": "atlas-cloud", "memoryApiKey": "global-file-secret"}"#),
        None,
        &env,
    );
    assert_eq!(config.memory_api_key.as_deref(), Some("global-file-secret"));
    assert_eq!(
        config.memory_provider,
        mem_core::config::MemoryProvider::AtlasCloud
    );
}

#[test]
fn test_embedding_api_key_not_exposed_without_url() {
    // Upstream quirk: the key is only resolved when the transport URL is set.
    let env = env_of(&[("TEST_EMBEDDING_KEY", "ambient-secret")]);
    let config = run_ok(
        Some(r#"{"embeddingApiKey": "env://TEST_EMBEDDING_KEY"}"#),
        None,
        &env,
    );
    assert!(config.embedding_api_key.is_none());
}

#[test]
fn test_opencode_variant_blank_values_treated_as_unset() {
    let env = Env::new();
    let config = run_ok(
        Some(
            r#"{"opencodeProvider": "newapi", "opencodeModel": "grok-4.7", "opencodeVariant": "xhigh"}"#,
        ),
        None,
        &env,
    );
    assert_eq!(config.opencode_variant.as_deref(), Some("xhigh"));

    let config = run_ok(
        Some(
            r#"{"opencodeProvider": "newapi", "opencodeModel": "grok-4.7", "opencodeVariant": "   "}"#,
        ),
        None,
        &env,
    );
    assert_eq!(config.opencode_variant, None);

    let config = run_ok(
        Some(r#"{"opencodeProvider": "newapi", "opencodeModel": "grok-4.7"}"#),
        None,
        &env,
    );
    assert_eq!(config.opencode_variant, None);
}

#[test]
fn test_opencode_base_url_reads_from_the_project_file() {
    let env = Env::new();
    let config = run_ok(
        None,
        Some(r#"{"opencodeBaseUrl": "http://127.0.0.1:5000"}"#),
        &env,
    );
    assert_eq!(
        config.opencode_base_url.as_deref(),
        Some("http://127.0.0.1:5000")
    );
}

#[test]
fn test_opencode_base_url_blank_is_unset() {
    let env = Env::new();
    let config = run_ok(None, Some(r#"{"opencodeBaseUrl": "   "}"#), &env);
    assert_eq!(config.opencode_base_url, None);

    let config = run_ok(None, None, &env);
    assert_eq!(config.opencode_base_url, None);
}

#[test]
fn test_opencode_timeout_parsed_with_clamping_and_default() {
    let env = Env::new();
    let cases: [(Option<&str>, u64); 7] = [
        (Some("180000"), 180_000),
        (Some("500"), 10_000),         // below the floor
        (Some("9999999"), 600_000),    // above the ceiling
        (Some("null"), 90_000),        // non-finite stand-in (see blueprint note)
        (Some("\"180000\""), 180_000), // numeric strings are coerced
        (None, 90_000),                // unset keeps the default
        (Some("true"), 90_000),        // wrong type falls back
    ];
    for (raw, expected) in cases {
        let content = match raw {
            Some(v) => format!(r#"{{"opencodeTimeoutMs": {v}}}"#),
            None => "{}".to_owned(),
        };
        let config = run_ok(Some(&content), None, &env);
        assert_eq!(config.opencode_timeout_ms, expected, "for {content}");
    }
}

#[test]
fn test_chat_message_capture_child_sessions_from_global() {
    let env = Env::new();
    let config = run_ok(
        Some(r#"{"chatMessage": {"captureChildSessions": true}}"#),
        None,
        &env,
    );
    assert!(config.chat_message.capture_child_sessions);

    let config = run_ok(Some(r#"{"chatMessage": {}}"#), None, &env);
    assert!(!config.chat_message.capture_child_sessions);
}

#[test]
fn test_defaults_when_no_config_exists() {
    let config = run_ok(None, None, &Env::new());
    assert!(config.auto_capture_enabled);
    assert!(config.opencode_provider.is_none());
    assert_eq!(config.web_server_port, 4747);
}

#[test]
fn test_atlas_cloud_key_from_env_and_manual_ready() {
    let env = env_of(&[("ATLASCLOUD_API_KEY", "atlas-test-key")]);
    let config = run_ok(Some(r#"{"memoryProvider": "atlas-cloud"}"#), None, &env);
    assert_eq!(
        config.memory_provider,
        mem_core::config::MemoryProvider::AtlasCloud
    );
    assert!(config.memory_model.is_none());
    assert!(config.memory_api_url.is_none());
    assert_eq!(config.memory_api_key.as_deref(), Some("atlas-test-key"));
    assert_eq!(
        config.auto_capture_provider_status,
        mem_core::config::AutoCaptureProviderStatus::Ready {
            mode: mem_core::config::AutoCaptureProviderMode::Manual
        }
    );
}

#[test]
fn test_project_memory_provider_switch_to_atlas_cloud_rejected() {
    let error = run(
        Some(
            r#"{"memoryProvider": "openai-chat", "memoryModel": "gpt-global", "memoryApiUrl": "https://api.openai.com/v1", "memoryApiKey": "global-openai-secret"}"#,
        ),
        Some(r#"{"memoryProvider": "atlas-cloud"}"#),
        &Env::new(),
    )
    .expect_err("project provider switch must be rejected");
    let ConfigError::ProjectRemoteProviderFields { fields, .. } = &error else {
        panic!("expected ProjectRemoteProviderFields, got: {error:?}")
    };
    assert_eq!(fields, &vec!["memoryProvider".to_owned()]);
    assert!(error.to_string().contains("memoryProvider"));
}

#[test]
fn test_outer_structured_output_timeout_derivation() {
    assert_eq!(
        outer_structured_output_timeout_ms(Some(&serde_json::json!(90_000))),
        120_000
    );
    assert_eq!(
        outer_structured_output_timeout_ms(Some(&serde_json::json!(180_000))),
        210_000
    );
    assert_eq!(
        outer_structured_output_timeout_ms(None),
        OPENCODE_TIMEOUT_MS_DEFAULT + 30_000
    );
}

#[test]
fn test_normalize_opencode_timeout_ms_clamps_and_falls_back() {
    use mem_core::config::normalize_opencode_timeout_ms;
    let none: Option<&serde_json::Value> = None;
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!(90_000))),
        90_000
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!(5_000))),
        10_000
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!(1_000_000))),
        600_000
    );
    assert_eq!(
        normalize_opencode_timeout_ms(none),
        OPENCODE_TIMEOUT_MS_DEFAULT
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!(null))),
        OPENCODE_TIMEOUT_MS_DEFAULT
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!("180000"))),
        180_000
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!(" 240000 "))),
        240_000
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!("abc"))),
        OPENCODE_TIMEOUT_MS_DEFAULT
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!(""))),
        OPENCODE_TIMEOUT_MS_DEFAULT
    );
    assert_eq!(
        normalize_opencode_timeout_ms(Some(&serde_json::json!(true))),
        OPENCODE_TIMEOUT_MS_DEFAULT
    );
}

#[test]
fn test_secret_env_ref_missing_errors_and_literal_passthrough() {
    let env = Env::new();
    let error =
        resolve_secret_value(Some("env://MISSING"), &env).expect_err("missing env must error");
    let ConfigError::SecretEnvNotFound { name } = &error else {
        panic!("expected SecretEnvNotFound, got: {error:?}")
    };
    assert_eq!(name, "MISSING");
    assert_eq!(error.to_string(), "Environment variable not found: MISSING");

    let literal = resolve_secret_value(Some("literal-token"), &env).expect("literal passthrough");
    assert_eq!(literal.as_deref(), Some("literal-token"));

    assert!(resolve_secret_value(None, &env).unwrap().is_none());
    assert!(resolve_secret_value(Some(""), &env).unwrap().is_none());
}

#[test]
fn test_secret_file_ref_resolution_round_trip() {
    let home = common::unique_temp_dir("secret-file-home");
    let key_file = common::write_file(&home, "litellm-key.txt", "sk-from-file\n");
    let env = Env::new();
    let resolved = resolve_secret_value(Some(&format!("file://{}", key_file.display())), &env)
        .expect("file ref must resolve");
    assert_eq!(resolved.as_deref(), Some("sk-from-file"));

    let missing = resolve_secret_value(Some("file:///nonexistent-path-xyz"), &env)
        .expect_err("missing secret file must error");
    let ConfigError::SecretFileNotFound { path } = &missing else {
        panic!("expected SecretFileNotFound, got: {missing:?}")
    };
    assert_eq!(path, &PathBuf::from("/nonexistent-path-xyz"));
}

#[test]
fn test_first_parseable_candidate_wins_and_invalid_skipped() {
    // .jsonc exists but is unparseable; .json below it is valid → used.
    let home = common::unique_temp_dir("resolution-cascade");
    common::write_file(
        &home,
        ".config/opencode/opencode-mem.jsonc",
        "{ not, valid }",
    );
    common::write_file(
        &home,
        ".config/opencode/opencode-mem.json",
        r#"{"opencodeModel": "from-json-fallback"}"#,
    );
    let paths = ConfigPaths::standard(home);
    let config = Config::load(&paths, &Env::new()).unwrap();
    assert_eq!(config.opencode_model.as_deref(), Some("from-json-fallback"));
}

#[test]
fn test_project_null_value_overrides_global_value() {
    // `null` in the project file shadows a global value (object spread).
    let env = Env::new();
    let config = run_ok(
        Some(r#"{"containerTagPrefix": "global-prefix"}"#),
        Some(r#"{"containerTagPrefix": null}"#),
        &env,
    );
    // Upstream: merged value null → file.containerTagPrefix undefined → default.
    assert_eq!(config.container_tag_prefix, "opencode");

    let config = run_ok(
        Some(r#"{"containerTagPrefix": "global-prefix"}"#),
        None,
        &env,
    );
    assert_eq!(config.container_tag_prefix, "global-prefix");
}

#[test]
fn test_no_home_directory_errors() {
    // Config::load_default without HOME/USERPROFILE in the injected env names
    // the missing home instead of guessing.
    let error =
        mem_core::config::Config::load_default(&Env::new()).expect_err("missing home must error");
    let ConfigError::MissingHomeDirectory = &error else {
        panic!("expected MissingHomeDirectory, got: {error:?}")
    };
    assert!(error.to_string().contains("home directory"));
}

#[test]
fn test_display_lists_global_path_in_project_error() {
    // Stylistic smoke of shared shaping — the exact display is covered by the
    // error_tree suite; here we prove the load seam reports the global path.
    let result = run(None, Some(r#"{"memoryApiKey": "sk-..."}"#), &Env::new())
        .expect_err("project key must be rejected");
    assert!(
        result
            .to_string()
            .contains(".config/opencode/opencode-mem.jsonc"),
        "display must carry the global config path: {result}"
    );
}

/// O5 / issue 45: `allowUnguardedOpen` is a top-level camelCase boolean and its
/// default is `false` — the guard is on unless the user turns it off.
///
/// Inverted world: a key spelled `allow_unguarded_open` in the file would resolve
/// `false` here (serde `rename_all = "camelCase"` ignores it) and the test fails.
#[test]
fn test_allow_unguarded_open_resolves_from_the_camel_case_key() {
    let enabled = run_ok(Some(r#"{"allowUnguardedOpen": true}"#), None, &Env::new());
    assert!(
        enabled.allow_unguarded_open,
        "`allowUnguardedOpen: true` must resolve to `true`"
    );

    let defaulted = run_ok(None, None, &Env::new());
    assert!(
        !defaulted.allow_unguarded_open,
        "the default must be `false`: the mixed-version guard is on unless asked otherwise"
    );

    let explicit_off = run_ok(Some(r#"{"allowUnguardedOpen": false}"#), None, &Env::new());
    assert!(!explicit_off.allow_unguarded_open);
}

// ---------------------------------------------------------------------------
// Step 6 — embeddingMaxTokens (the cap override)
// ---------------------------------------------------------------------------

#[test]
fn test_embedding_max_tokens_defaults_to_none() {
    let config = run_ok(None, None, &Env::new());
    assert_eq!(
        config.embedding_max_tokens, None,
        "unset means the model's own declared limit"
    );
}

#[test]
fn test_embedding_max_tokens_parses_an_integral_override() {
    let config = run_ok(Some(r#"{"embeddingMaxTokens": 1024}"#), None, &Env::new());
    assert_eq!(config.embedding_max_tokens, Some(1024));
}

#[test]
fn test_embedding_max_tokens_rejects_zero() {
    let error = run(Some(r#"{"embeddingMaxTokens": 0}"#), None, &Env::new())
        .expect_err("zero is not a token cap");
    let ConfigError::InvalidValue { key, .. } = &error else {
        panic!("expected InvalidValue, got: {error:?}")
    };
    assert_eq!(*key, "embeddingMaxTokens");
    assert!(error.to_string().contains("embeddingMaxTokens"));
}

#[test]
fn test_embedding_max_tokens_rejects_negative_fractional_and_absurd() {
    for raw in ["-1", "1.5", "2000000"] {
        let error = run(
            Some(&format!(r#"{{"embeddingMaxTokens": {raw}}}"#)),
            None,
            &Env::new(),
        )
        .expect_err("out-of-range values are rejected, never coerced");
        let ConfigError::InvalidValue { key, .. } = &error else {
            panic!("{raw}: expected InvalidValue, got: {error:?}")
        };
        assert_eq!(*key, "embeddingMaxTokens", "{raw}");
    }
}
