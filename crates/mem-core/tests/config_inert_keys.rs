//! Tests for inert configuration key detection in Config::load.

use std::collections::BTreeMap;

use mem_core::config::{Config, ConfigPaths};
mod common;

type Env = BTreeMap<String, String>;

fn load_config(jsonc: &str) -> Config {
    let home = common::unique_temp_dir("inert-keys-home");
    common::write_file(&home, ".config/opencode/opencode-mem.jsonc", jsonc);
    let paths = ConfigPaths::standard(home);
    Config::load(&paths, &Env::new()).expect("load must succeed")
}

#[test]
fn test_inert_keys_extracted_when_set_in_jsonc() {
    let config = load_config(r#"{"similarityThreshold": 0.8, "maxProfileItems": 5}"#);
    assert_eq!(
        config.inert_keys_present,
        vec![
            "maxProfileItems".to_string(),
            "similarityThreshold".to_string(),
        ]
    );
}

#[test]
fn test_nested_memory_default_scope_detected() {
    let config = load_config(r#"{"memory": {"defaultScope": "all-projects"}}"#);
    assert_eq!(config.inert_keys_present, vec!["defaultScope".to_string()]);
}

#[test]
fn test_no_inert_keys_when_only_live_keys_set() {
    let config = load_config(r#"{"embeddingModel": "Xenova/nomic-embed-text-v1"}"#);
    assert!(
        config.inert_keys_present.is_empty(),
        "expected no inert keys, got: {:?}",
        config.inert_keys_present
    );
}

#[test]
fn test_snake_case_inert_keys_detected() {
    let config = load_config(r#"{"similarity_threshold": 0.8, "auto_capture_max_iterations": 10}"#);
    assert_eq!(
        config.inert_keys_present,
        vec![
            "auto_capture_max_iterations".to_string(),
            "similarity_threshold".to_string(),
        ]
    );
}

#[test]
fn test_clean_empty_config_has_empty_inert_keys() {
    let config = load_config("{}");
    assert!(config.inert_keys_present.is_empty());
}

#[test]
fn test_nested_snake_case_memory_default_scope_detected() {
    let config = load_config(r#"{"memory": {"default_scope": "project"}}"#);
    assert_eq!(config.inert_keys_present, vec!["default_scope".to_string()]);
}
