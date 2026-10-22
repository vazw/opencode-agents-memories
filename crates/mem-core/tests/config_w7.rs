//! W7.1 — the config surface for the sound-memory flags and the promotion
//! knobs ([CONFIG CONTRACT]): both flags default OFF, the knobs are pinned,
//! out-of-range values are rejected naming their camelCase key, unknown keys
//! are tolerated, and every existing key keeps its meaning.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use mem_core::config::{Config, ConfigPaths};
use mem_core::error::ConfigError;

type Env = BTreeMap<String, String>;

fn load_with(label: &str, jsonc: &str) -> Result<Config, ConfigError> {
    let home = common::unique_temp_dir(label);
    if !jsonc.is_empty() {
        common::write_file(&home, ".config/opencode/opencode-mem.jsonc", jsonc);
    }
    let paths = ConfigPaths::standard(home);
    Config::load(&paths, &Env::new())
}

fn assert_shape_error(result: Result<Config, ConfigError>, label: &str) {
    match result {
        Err(ConfigError::InvalidShape { .. }) => {}
        other => panic!("{label}: expected a shape error, got {other:?}"),
    }
}

#[test]
fn test_w7_flags_default_off() {
    let config = load_with("w7-flags-default", "").expect("defaults load");
    assert!(!config.feature_flags.sound_memory);
    assert!(!config.feature_flags.promotion_pipeline);
}

#[test]
fn test_w7_knobs_defaults_are_pinned() {
    let config = load_with("w7-knobs-default", "").expect("defaults load");
    assert!(!config.sound_memory.raw_context_required);
    assert_eq!(config.promotion.ttl_fallback_days, 7);
}

#[test]
fn test_w7_section_overrides_from_jsonc() {
    let config = load_with(
        "w7-overrides",
        r#"{
            // comments and trailing commas are part of the JSONC surface
            "featureFlags": { "soundMemory": true, "promotionPipeline": true },
            "soundMemory": { "rawContextRequired": true },
            "promotion": { "ttlFallbackDays": 30 },
        }"#,
    )
    .expect("overrides load");
    assert!(config.feature_flags.sound_memory);
    assert!(config.feature_flags.promotion_pipeline);
    assert!(config.sound_memory.raw_context_required);
    assert_eq!(config.promotion.ttl_fallback_days, 30);
}

#[test]
fn test_w7_ttl_fallback_days_rejects_out_of_range() {
    for (label, value) in [
        ("zero", "0"),
        ("negative", "-1"),
        ("fractional", "1.5"),
        ("above-range", "3651"),
    ] {
        let result = load_with(
            &format!("w7-ttl-{label}"),
            &format!(r#"{{ "promotion": {{ "ttlFallbackDays": {value} }} }}"#),
        );
        match result {
            Err(ConfigError::InvalidValueExpected { key, .. }) => {
                assert_eq!(
                    key, "promotion.ttlFallbackDays",
                    "{label} must name the camelCase key"
                );
            }
            other => panic!("{label}: expected InvalidValueExpected, got {other:?}"),
        }
    }

    // A non-number is a SHAPE failure, not a range failure.
    let shape = load_with(
        "w7-ttl-bool",
        r#"{ "featureFlags": { "soundMemory": true }, "promotion": { "ttlFallbackDays": true } }"#,
    );
    assert_shape_error(shape, "bool ttl");

    // The bounds themselves are accepted.
    for (label, value) in [("low", 1), ("high", 3650)] {
        let config = load_with(
            &format!("w7-ttl-ok-{label}"),
            &format!(r#"{{ "promotion": {{ "ttlFallbackDays": {value} }} }}"#),
        )
        .unwrap_or_else(|error| panic!("{label} must be accepted: {error}"));
        assert_eq!(config.promotion.ttl_fallback_days, value);
    }

    // A non-boolean flag is a shape failure.
    let flag = load_with(
        "w7-flag-shape",
        r#"{ "featureFlags": { "soundMemory": "yes" } }"#,
    );
    assert_shape_error(flag, "string flag");
}

#[test]
fn test_w7_unknown_keys_are_tolerated() {
    let config = load_with(
        "w7-unknown",
        r#"{
            "featureFlags": { "soundMemory": true, "hotColdTiering": true, "vaultTier": true },
            "multiAgent": { "isolationMode": "strict", "agentRoles": ["main"] },
            "hotCold": { "enabled": true },
            "soundMemory": { "rawContextRequired": false, "segmentIsolation": true },
            "promotion": { "ttlFallbackDays": 7, "factsToProfileRecurrenceThreshold": 3 },
            "aKeyThatDoesNotExist": { "nested": [1, 2, 3] }
        }"#,
    )
    .expect("unknown keys must be tolerated");
    assert!(config.feature_flags.sound_memory);
    assert!(!config.feature_flags.promotion_pipeline);
    assert!(!config.sound_memory.raw_context_required);
    assert_eq!(config.promotion.ttl_fallback_days, 7);
}

#[test]
fn test_w7_existing_keys_keep_their_defaults() {
    let config = load_with("w7-existing-defaults", "").expect("defaults load");
    assert_eq!(config.embedding_model, "Xenova/nomic-embed-text-v1");
    assert_eq!(config.embedding_dimensions, 768);
    assert_eq!(config.max_memories, 10);
    assert_eq!(config.episodic.consolidation_age_days, 30);
    assert!(config.auto_capture_enabled);
    assert_eq!(config.web_server_port, 4747);
    assert_eq!(config.storage_path, {
        // The storage path is still derived from the passed home's default.
        let path = Path::new(&config.storage_path);
        assert!(path.ends_with(".opencode-mem/data"));
        path.to_path_buf()
    });

    // An override still lands on the existing key, unaffected by the new
    // sections.
    let config = load_with(
        "w7-existing-override",
        r#"{ "featureFlags": { "soundMemory": true }, "maxMemories": 3, "episodic": { "consolidationAgeDays": 12 } }"#,
    )
    .expect("override loads");
    assert_eq!(config.max_memories, 3);
    assert_eq!(config.episodic.consolidation_age_days, 12);
}
