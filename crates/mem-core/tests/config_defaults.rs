//! Port of upstream `config.test.ts` "CONFIG defaults" cluster plus the new
//! episodic threshold. Every test loads from an empty temp home with an
//! injected `BTreeMap` env (never the process environment).

use std::collections::BTreeMap;
use std::path::Path;

use mem_core::config::{
    AutoCaptureProviderSettings, Config, ConfigPaths, DEFAULT_INJECTION_MARKERS,
    get_auto_capture_provider_status, normalize_auto_capture_max_context_bytes,
    normalize_auto_cleanup_retention_days,
};
use mem_core::config::{EmbeddingPooling, InjectOn, MemoryDefaultScope};
use mem_core::error::ConfigError;
mod common;

type Env = BTreeMap<String, String>;

fn default_config(label: &str) -> Config {
    let home = common::unique_temp_dir(label);
    let paths = ConfigPaths::standard(home);
    Config::load(&paths, &Env::new()).expect("defaults must load from an empty home without errors")
}

fn settings_from(config: &Config) -> AutoCaptureProviderSettings {
    AutoCaptureProviderSettings {
        opencode_provider: config.opencode_provider.clone(),
        opencode_model: config.opencode_model.clone(),
        memory_provider: Some(config.memory_provider),
        memory_model: config.memory_model.clone(),
        memory_api_url: config.memory_api_url.clone(),
        memory_api_key: config.memory_api_key.clone(),
    }
}

#[test]
fn test_storage_path_defaults_to_home_data_dir() {
    let home = common::unique_temp_dir("defaults-storagepath");
    let paths = ConfigPaths::standard(home.clone());
    let config = Config::load(&paths, &Env::new()).unwrap();
    let expected = home.join(".opencode-mem").join("data");
    assert_eq!(config.storage_path, expected);
    assert!(!config.storage_path.as_os_str().is_empty());
}

#[test]
fn test_default_embedding_model_is_nomic() {
    let config = default_config("defaults-model");
    assert_eq!(config.embedding_model, "Xenova/nomic-embed-text-v1");
}

#[test]
fn test_default_embedding_use_task_prefixes_false() {
    let config = default_config("defaults-taskprefixes");
    assert!(!config.embedding_use_task_prefixes);
}

#[test]
fn test_default_embedding_pooling_is_mean() {
    let config = default_config("defaults-pooling");
    assert_eq!(config.embedding_pooling, EmbeddingPooling::Mean);
}

#[test]
fn test_default_embedding_dtype_unset() {
    let config = default_config("defaults-dtype");
    assert!(config.embedding_dtype.is_none());
}

#[test]
fn test_default_embedding_dimensions_768() {
    let config = default_config("defaults-dims");
    assert_eq!(config.embedding_dimensions, 768);
    assert!(config.embedding_dimensions > 0);
}

#[test]
fn test_similarity_threshold_between_zero_and_one() {
    let config = default_config("defaults-similarity");
    assert!((0.0..=1.0).contains(&config.similarity_threshold));
    assert_eq!(config.similarity_threshold, 0.6);
}

#[test]
fn test_positive_max_memories_and_max_profile_items() {
    let config = default_config("defaults-maxmemories");
    assert!(config.max_memories > 0);
    assert_eq!(config.max_memories, 10);
    assert!(config.max_profile_items > 0);
    assert_eq!(config.max_profile_items, 5);
}

#[test]
fn test_web_server_defaults() {
    let config = default_config("defaults-webserver");
    assert!(config.web_server_enabled);
    assert_eq!(config.web_server_port, 4747);
    assert_eq!(config.web_server_host, "127.0.0.1");
}

#[test]
fn test_max_vectors_per_shard_positive() {
    let config = default_config("defaults-shard");
    assert!(config.max_vectors_per_shard > 0);
    assert_eq!(config.max_vectors_per_shard, 50000);
}

#[test]
fn test_compaction_defaults() {
    let config = default_config("defaults-compaction");
    assert!(config.compaction.enabled);
    assert_eq!(config.compaction.memory_limit, 10);
}

#[test]
fn test_chat_message_defaults() {
    let config = default_config("defaults-chatmessage");
    let chat = &config.chat_message;
    assert!(chat.enabled);
    assert_eq!(chat.max_memories, 3);
    assert!(chat.exclude_current_session);
    assert_eq!(chat.inject_on, InjectOn::First);
    assert!(chat.filter_injected_prompts);
    assert!(!chat.capture_child_sessions);
    assert!(chat.max_age_days.is_none());
    let expected: Vec<String> = DEFAULT_INJECTION_MARKERS
        .iter()
        .map(|m| (*m).to_owned())
        .collect();
    assert_eq!(chat.injection_markers, expected);
}

#[test]
fn test_default_injection_markers_match_upstream_list() {
    let expected = [
        "<system-reminder>",
        "<!-- OMO_INTERNAL_INITIATOR -->",
        "<!-- OMO_INTERNAL_NOREPLY -->",
        "[SYSTEM DIRECTIVE: OH-MY-OPENCODE",
        "[Agent Usage Reminder]",
        "[Category+Skill Reminder]",
        "<team_mode_status",
        "<auto-slash-command>",
    ];
    assert_eq!(DEFAULT_INJECTION_MARKERS.len(), expected.len());
    for (actual, expected) in DEFAULT_INJECTION_MARKERS.iter().zip(expected) {
        assert_eq!(*actual, expected);
    }
}

#[test]
fn test_boolean_toggle_defaults() {
    let config = default_config("defaults-booleans");
    assert!(config.auto_capture_enabled);
    assert!(config.inject_profile);
    assert!(config.web_server_enabled);
    assert!(config.auto_cleanup_enabled);
    assert!(config.deduplication_enabled);
}

#[test]
fn test_default_auto_capture_context_budget_and_misc() {
    let config = default_config("defaults-misc");
    assert_eq!(config.auto_capture_max_context_bytes, 131072);
    assert_eq!(config.auto_capture_max_iterations, 5);
    assert_eq!(config.auto_capture_iteration_timeout, 30000);
    assert_eq!(config.auto_capture_max_retries, 3);
    assert_eq!(config.ai_session_retention_days, 7);
    assert_eq!(config.container_tag_prefix, "opencode");
    assert!(config.deduplication_similarity_threshold >= 0.0);
    assert!(config.deduplication_similarity_threshold <= 1.0);
}

#[test]
fn test_default_memory_scope_is_project() {
    let config = default_config("defaults-scope");
    assert_eq!(config.memory.default_scope, MemoryDefaultScope::Project);
}

#[test]
fn test_user_profile_defaults() {
    let config = default_config("defaults-userprofile");
    assert_eq!(config.user_profile_analysis_interval, 10);
    assert_eq!(config.user_profile_max_context_bytes, 32768);
    assert_eq!(config.user_profile_display_preferences, 20);
    assert_eq!(config.user_profile_display_patterns, 15);
    assert_eq!(config.user_profile_display_workflows, 10);
    assert_eq!(config.user_profile_stale_days, 2);
    assert_eq!(config.user_profile_inject_preferences, 5);
    assert_eq!(config.user_profile_inject_patterns, 5);
    assert_eq!(config.user_profile_inject_workflows, 3);
    assert_eq!(config.user_profile_confidence_decay_days, 30);
    assert_eq!(config.user_profile_changelog_retention_count, 5);
    assert_eq!(config.user_profile_embedding_threshold_same_cat, 0.8);
    assert_eq!(config.user_profile_embedding_threshold_same_cat_weak, 0.5);
    assert_eq!(config.user_profile_embedding_threshold_cross_cat, 0.9);
    assert_eq!(config.user_profile_embedding_threshold_cross_cat_weak, 0.8);
    assert_eq!(config.user_profile_centroid_drift_threshold, 0.65);
    assert_eq!(config.user_profile_embedding_min_description_length, 5);
    assert_eq!(config.user_profile_min_evidence_for_retention, 3);
    assert!(config.user_profile_auto_cleanup_enabled);
    assert_eq!(config.user_profile_auto_cleanup_interval, 100);
    assert!(!config.user_profile_validation_enabled);
}

#[test]
fn test_toast_defaults_true() {
    let config = default_config("defaults-toasts");
    assert!(config.show_auto_capture_toasts);
    assert!(config.show_user_profile_toasts);
    assert!(config.show_error_toasts);
}

#[test]
fn test_database_and_auto_update_defaults() {
    let config = default_config("defaults-db");
    assert!(!config.database_encryption_enabled);
    assert!(config.database_encryption_key.is_none());
    assert!(config.auto_update);
}

#[test]
fn test_default_episodic_consolidation_age_days_is_30() {
    let config = default_config("defaults-episodic");
    assert_eq!(config.episodic.consolidation_age_days, 30);
}

#[test]
fn test_consolidation_age_days_validated_as_positive_integer() {
    // New-tier threshold lives in config so tuning needs no recompile: values
    // must be integers >= 1 and the error must name the key.
    for raw in [0.0, -1.0, 1.5] {
        let home = common::unique_temp_dir("defaults-episodic-bad");
        common::write_file(
            &home,
            ".config/opencode/opencode-mem.jsonc",
            &format!("{{\"episodic\": {{\"consolidationAgeDays\": {raw}}}}}"),
        );
        let paths = ConfigPaths::standard(home);
        let error = Config::load(&paths, &Env::new()).expect_err("must be rejected");
        let ConfigError::InvalidValue { key, .. } = &error else {
            panic!("expected InvalidValue, got: {error:?}")
        };
        assert_eq!(*key, "episodic.consolidationAgeDays");
        assert!(error.to_string().contains("episodic.consolidationAgeDays"));
    }
    let home = common::unique_temp_dir("defaults-episodic-bad-none");
    common::write_file(
        &home,
        ".config/opencode/opencode-mem.jsonc",
        "{\"episodic\": {\"consolidationAgeDays\": null}}",
    );
    let paths = ConfigPaths::standard(home);
    let config = Config::load(&paths, &Env::new()).unwrap();
    assert_eq!(config.episodic.consolidation_age_days, 30);
}

#[test]
fn test_auto_capture_provider_status_matches_helper() {
    let config = default_config("defaults-status");
    let settings = settings_from(&config);
    assert_eq!(
        config.auto_capture_provider_status,
        get_auto_capture_provider_status(&settings)
    );
}

#[test]
fn test_normalize_auto_capture_max_context_bytes_rejects_unsafe_budgets() {
    for value in [-1.0, 1024.0, 16_777_217.0] {
        let error = normalize_auto_capture_max_context_bytes(value)
            .expect_err("unsafe budget must be rejected");
        let ConfigError::InvalidValue { key, .. } = &error else {
            panic!("expected InvalidValue, got: {error:?}")
        };
        assert_eq!(*key, "autoCaptureMaxContextBytes");
    }
    assert_eq!(
        normalize_auto_capture_max_context_bytes(131_072.0).unwrap(),
        131_072
    );
    assert_eq!(
        normalize_auto_capture_max_context_bytes(16_777_216.0).unwrap(),
        16_777_216
    );
    assert_eq!(
        normalize_auto_capture_max_context_bytes(16_384.0).unwrap(),
        16_384
    );
}

#[test]
fn test_normalize_auto_cleanup_retention_days_rejects_unsafe_periods() {
    for value in [0.0, -1.0, 1.5, f64::INFINITY, f64::NAN] {
        let error = normalize_auto_cleanup_retention_days(value)
            .expect_err("unsafe retention must be rejected");
        let ConfigError::InvalidValue { key, .. } = &error else {
            panic!("expected InvalidValue, got: {error:?}")
        };
        assert_eq!(*key, "autoCleanupRetentionDays");
    }
    assert_eq!(normalize_auto_cleanup_retention_days(1.0).unwrap(), 1);
    assert_eq!(normalize_auto_cleanup_retention_days(30.0).unwrap(), 30);
}

#[test]
fn test_config_paths_project_files_extend_global_files() {
    let home = common::unique_temp_dir("defaults-paths");
    let paths = ConfigPaths::standard(home.clone());
    let expected_global0 = home
        .join(".config")
        .join("opencode")
        .join("opencode-mem.jsonc");
    assert!(!paths.global_files.is_empty());
    assert_eq!(paths.global_files.first().unwrap(), &expected_global0);
    assert!(paths.project_files.is_empty());

    let project = Path::new("/some/project");
    let extended = ConfigPaths::for_project(project, home.clone());
    assert_eq!(extended.global_files, paths.global_files);
    let expected_project0 = project.join(".opencode").join("opencode-mem.jsonc");
    assert_eq!(extended.project_files.first().unwrap(), &expected_project0);
    // Both extension files keep the parsed-wins candidate order.
    let second = project.join(".opencode").join("opencode-mem.json");
    assert_eq!(extended.project_files.get(1).unwrap(), &second);
}
