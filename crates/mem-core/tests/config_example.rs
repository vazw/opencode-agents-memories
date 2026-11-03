//! Guards the shipped config example at the repo root
//! (`opencode-mem.example.jsonc`): every key in it must be a field of
//! `FileConfig`, and every value must resolve to the built-in default, so
//! copying the example verbatim changes nothing.
//!
//! Two independent guards, deliberately not one:
//!
//! 1. `ExampleFile` is a test-local transcription of `FileConfig` with
//!    `deny_unknown_fields` — the production struct must keep tolerating
//!    unknown keys, so the mirror is where strictness lives.
//! 2. The mirror can be taught a key that production does not have. Test 6
//!    therefore asks *production*: it replaces each shipped key's value with an
//!    object, which a real, non-`Value`-typed field rejects.
#![allow(dead_code)] // the mirror exists to be deserialized into, not read

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mem_core::config::{
    Config, ConfigPaths, DatabaseEncryptionCipher, EmbeddingPooling, InjectOn, MemoryDefaultScope,
    MemoryProvider, parse_jsonc,
};
use mem_core::error::ConfigError;
use serde::Deserialize;
use serde_json::Value;

type Env = BTreeMap<String, String>;

/// A transcription of every `FileConfig` field, with the strictness production
/// does not have: an unknown key is an error here, so a misspelled key in the
/// shipped example fails instead of resolving to nothing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExampleFile {
    storage_path: Option<String>,
    user_email_override: Option<String>,
    user_name_override: Option<String>,
    memory: Option<ExampleMemory>,
    embedding_model: Option<String>,
    embedding_dimensions: Option<f64>,
    embedding_api_url: Option<String>,
    embedding_api_key: Option<String>,
    embedding_use_task_prefixes: Option<bool>,
    embedding_pooling: Option<EmbeddingPooling>,
    embedding_query_prefix: Option<String>,
    embedding_document_prefix: Option<String>,
    embedding_dtype: Option<Value>,
    embedding_max_tokens: Option<f64>,
    similarity_threshold: Option<f64>,
    max_memories: Option<u32>,
    max_profile_items: Option<u32>,
    inject_profile: Option<bool>,
    container_tag_prefix: Option<String>,
    auto_capture_enabled: Option<bool>,
    auto_capture_max_iterations: Option<u32>,
    auto_capture_iteration_timeout: Option<u64>,
    auto_capture_max_retries: Option<u32>,
    auto_capture_max_context_bytes: Option<f64>,
    auto_capture_language: Option<String>,
    memory_provider: Option<MemoryProvider>,
    memory_model: Option<String>,
    memory_api_url: Option<String>,
    memory_api_key: Option<String>,
    memory_temperature: Option<Value>,
    force_tool_choice: Option<bool>,
    memory_extra_params: Option<Value>,
    opencode_provider: Option<String>,
    opencode_model: Option<String>,
    opencode_variant: Option<Value>,
    opencode_base_url: Option<String>,
    opencode_timeout_ms: Option<Value>,
    ai_session_retention_days: Option<u32>,
    web_server_enabled: Option<bool>,
    web_server_port: Option<u16>,
    web_server_host: Option<String>,
    web_server_auth_password: Option<String>,
    web_server_auth_username: Option<String>,
    web_server_api_token: Option<String>,
    max_vectors_per_shard: Option<u32>,
    auto_cleanup_enabled: Option<bool>,
    auto_cleanup_retention_days: Option<f64>,
    deduplication_enabled: Option<bool>,
    deduplication_similarity_threshold: Option<f64>,
    user_profile_analysis_interval: Option<u32>,
    user_profile_max_context_bytes: Option<u32>,
    user_profile_display_preferences: Option<u32>,
    user_profile_display_patterns: Option<u32>,
    user_profile_display_workflows: Option<u32>,
    user_profile_stale_days: Option<u32>,
    user_profile_inject_preferences: Option<u32>,
    user_profile_inject_patterns: Option<u32>,
    user_profile_inject_workflows: Option<u32>,
    user_profile_confidence_decay_days: Option<u32>,
    user_profile_changelog_retention_count: Option<u32>,
    user_profile_embedding_threshold_same_cat: Option<f64>,
    user_profile_embedding_threshold_same_cat_weak: Option<f64>,
    user_profile_embedding_threshold_cross_cat: Option<f64>,
    user_profile_embedding_threshold_cross_cat_weak: Option<f64>,
    user_profile_centroid_drift_threshold: Option<f64>,
    user_profile_embedding_min_description_length: Option<u32>,
    user_profile_min_evidence_for_retention: Option<u32>,
    user_profile_auto_cleanup_enabled: Option<bool>,
    user_profile_auto_cleanup_interval: Option<f64>,
    user_profile_validation_enabled: Option<bool>,
    show_auto_capture_toasts: Option<bool>,
    show_user_profile_toasts: Option<bool>,
    show_error_toasts: Option<bool>,
    database_encryption_key: Option<String>,
    database_encryption_enabled: Option<bool>,
    database_encryption_cipher: Option<DatabaseEncryptionCipher>,
    allow_unguarded_open: Option<bool>,
    auto_update: Option<bool>,
    compaction: Option<ExampleCompaction>,
    chat_message: Option<ExampleChatMessage>,
    episodic: Option<ExampleEpisodic>,
    feature_flags: Option<ExampleFeatureFlags>,
    sound_memory: Option<ExampleSoundMemory>,
    promotion: Option<ExamplePromotion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExampleMemory {
    default_scope: Option<MemoryDefaultScope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExampleCompaction {
    enabled: Option<bool>,
    memory_limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExampleChatMessage {
    enabled: Option<bool>,
    max_memories: Option<u32>,
    exclude_current_session: Option<bool>,
    max_age_days: Option<u32>,
    inject_on: Option<InjectOn>,
    filter_injected_prompts: Option<bool>,
    injection_markers: Option<Value>,
    capture_child_sessions: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExampleEpisodic {
    consolidation_age_days: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExampleFeatureFlags {
    sound_memory: Option<bool>,
    promotion_pipeline: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExampleSoundMemory {
    raw_context_required: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExamplePromotion {
    ttl_fallback_days: Option<u32>,
}

fn example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../opencode-mem.example.jsonc")
}

fn example_text() -> String {
    let path = example_path();
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "the shipped example is missing at {}: {error}",
            path.display()
        )
    })
}

fn load_in(home: &Path, jsonc: &str) -> Result<Config, ConfigError> {
    common::write_file(home, ".config/opencode/opencode-mem.jsonc", jsonc);
    Config::load(&ConfigPaths::standard(home), &Env::new())
}

fn load_text(label: &str, jsonc: &str) -> Result<Config, ConfigError> {
    load_in(&common::unique_temp_dir(label), jsonc)
}

/// Every non-object leaf path in the example, e.g. `["chatMessage", "enabled"]`.
fn leaf_paths(value: &Value, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    let Value::Object(map) = value else {
        panic!("the example must be a JSON object at every level");
    };
    for (key, child) in map {
        prefix.push(key.clone());
        match child {
            Value::Object(_) => leaf_paths(child, prefix, out),
            _ => out.push(prefix.clone()),
        }
        prefix.pop();
    }
}

#[test]
fn test_example_parses_with_no_unknown_keys() {
    let text = example_text();
    let parsed = parse_jsonc(&text).expect("the shipped example must be valid JSONC");
    let Value::Object(map) = parsed else {
        panic!("the shipped example must be a JSON object");
    };
    serde_json::from_value::<ExampleFile>(Value::Object(map.clone())).unwrap_or_else(|error| {
        panic!("the example carries a key that is not in FileConfig: {error}")
    });

    assert!(
        map.contains_key("maxProfileItems"),
        "the inert maxProfileItems key must ship"
    );
    assert!(
        map.contains_key("autoUpdate"),
        "the inert autoUpdate key must ship"
    );
    let chat = map
        .get("chatMessage")
        .expect("the example must ship a chatMessage section");
    assert!(
        chat.get("captureChildSessions").is_some(),
        "the live opt-in `chatMessage.captureChildSessions` key must ship (D-3, D-15)"
    );
    assert!(
        map.len() >= 15,
        "the example was emptied: {} top-level keys",
        map.len()
    );
}

#[test]
fn test_example_values_match_the_built_in_defaults() {
    let home = common::unique_temp_dir("defaults");
    let before = Config::load(&ConfigPaths::standard(home.clone()), &Env::new())
        .expect("the no-file load must succeed");
    let after = load_in(&home, &example_text()).expect("the shipped example must load");
    assert_eq!(
        after, before,
        "copying the example must change no resolved value"
    );
}

#[test]
fn test_guard_rejects_a_misspelled_key() {
    let text = example_text();
    let mutated = text.replace("\"maxMemories\":", "\"maxMemores\":");
    assert_ne!(
        mutated, text,
        "the fixture no longer contains \"maxMemories\": — update this mutation"
    );

    let parsed = parse_jsonc(&mutated).expect("the mutated example must still parse");
    let error = serde_json::from_value::<ExampleFile>(parsed)
        .expect_err("the mirror must reject the misspelled key");
    let message = error.to_string();
    assert!(
        message.contains("maxMemores"),
        "the failure must name the misspelled key, got: {message}"
    );
}

#[test]
fn test_guard_rejects_a_non_default_value() {
    let text = example_text();
    let mutated = text.replace(
        "\"autoCaptureMaxRetries\": 3",
        "\"autoCaptureMaxRetries\": 9",
    );
    assert_ne!(
        mutated, text,
        "the fixture no longer contains \"autoCaptureMaxRetries\": 3 — update this mutation"
    );

    let home = common::unique_temp_dir("non-default");
    let before = Config::load(&ConfigPaths::standard(home.clone()), &Env::new())
        .expect("the no-file load must succeed");
    assert_eq!(
        before.auto_capture_max_retries, 3,
        "DEFAULTS moved under this test — update the expectation"
    );
    let after = load_in(&home, &mutated).expect("a well-typed non-default value must still load");
    assert_eq!(
        after.auto_capture_max_retries, 9,
        "the non-default value did not reach the resolved config"
    );
}

#[test]
fn test_guard_rejects_a_wrong_typed_value() {
    let text = example_text();
    let mutated = text.replace("\"maxMemories\": 10", "\"maxMemories\": \"10\"");
    assert_ne!(
        mutated, text,
        "the fixture no longer contains \"maxMemories\": 10 — update this mutation"
    );

    let result = load_text("wrong-type", &mutated);
    assert!(
        matches!(result, Err(ConfigError::InvalidShape { .. })),
        "a string where a number belongs must answer InvalidShape, got: {:?}",
        result.err()
    );
}

#[test]
fn test_every_shipped_key_exists_in_the_production_schema() {
    let text = example_text();
    let parsed = parse_jsonc(&text).expect("the shipped example must parse");
    let mut paths = Vec::new();
    leaf_paths(&parsed, &mut Vec::new(), &mut paths);
    assert_eq!(
        paths.len(),
        35,
        "the example's leaf-key count changed (24 flat + 11 nested = 35) — update the census and the README table"
    );

    let home = common::unique_temp_dir("key-existence");
    for path in &paths {
        let mut mutated = parsed.clone();
        let mut cursor = &mut mutated;
        for segment in path {
            cursor = cursor.get_mut(segment).expect("path came from this value");
        }
        *cursor = serde_json::json!({ "__bogus__": true });

        let rendered = serde_json::to_string(&mutated).expect("a Value always serializes");
        let result = load_in(&home, &rendered);
        assert!(
            matches!(result, Err(ConfigError::InvalidShape { .. })),
            "the example ships `{}`, which production did not reject with InvalidShape. Three \
             ways that happens: (a) it is not a FileConfig field (unknown keys are tolerated by \
             design); (b) it is one of the six `Option<Value>` fields whose normalizer answers a \
             different error instead — memoryTemperature, embeddingDtype and injectionMarkers \
             surface as InvalidValue/InvalidValueExpected; or (c) it is memoryExtraParams, \
             opencodeVariant or opencodeTimeoutMs, which accept the object and the load answers \
             Ok. Each one means the example must not ship this key. Result: {:?}",
            path.join("."),
            result.err(),
        );
    }
}
