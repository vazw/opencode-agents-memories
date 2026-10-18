//! Config loading: paths, composition (defaults < global file < project file
//! < environment-derived values), typed validation, provider status, and
//! placeholder helpers — the shared kernel seam for later waves.

mod normalize;
mod schema;

pub use normalize::{
    DEFAULT_INJECTION_MARKERS, OPENCODE_TIMEOUT_MS_DEFAULT, OPENCODE_TIMEOUT_MS_MAX,
    OPENCODE_TIMEOUT_MS_MIN, OPENCODE_TIMEOUT_OUTER_MARGIN_MS, QWEN3_EMBEDDING_QUERY_PREFIX,
    embedding_dimensions_for_model, embedding_model_preset, expand_path,
    normalize_auto_capture_max_context_bytes, normalize_auto_cleanup_retention_days,
    normalize_embedding_dimensions, normalize_embedding_dtype,
    normalize_episodic_consolidation_age_days, normalize_injection_markers,
    normalize_memory_temperature, normalize_opencode_timeout_ms, normalize_opencode_variant,
    normalize_user_profile_auto_cleanup_interval, outer_structured_output_timeout_ms,
    render_json_value, render_number, resolve_secret_value,
};
pub use schema::{
    AutoCaptureProviderMode, AutoCaptureProviderSettings, AutoCaptureProviderStatus, ChatMessage,
    Compaction, Config, DEFAULTS, DatabaseEncryptionCipher, Defaults, EmbeddingModelPreset,
    EmbeddingPooling, Episodic, FileChatMessageConfig, FileCompactionConfig, FileConfig,
    FileEpisodicConfig, FileMemoryConfig, InjectOn, LOCAL_ONNX_DTYPES, LocalOnnxDtype,
    MemoryDefaultScope, MemoryProvider, MemoryScope, MemoryTemperature,
};

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::env::{EnvSource, home_dir};
use crate::error::ConfigError;

// -------------------------------------------------------------------------
// JSONC parse seam
// -------------------------------------------------------------------------

/// Strict-JSON-plus-extras parse options: comments and trailing commas are
/// the only tolerated extensions (upstream `stripJsoncComments` +
/// `JSON.parse`).
pub static JSONC_OPTIONS: jsonc_parser::ParseOptions = jsonc_parser::ParseOptions {
    allow_comments: true,
    allow_trailing_commas: true,
    allow_loose_object_property_names: false,
    allow_missing_commas: false,
    allow_single_quoted_strings: false,
    allow_hexadecimal_numbers: false,
    allow_unary_plus_numbers: false,
    allow_bare_decimal_point_numbers: false,
    allow_non_finite_numbers: false,
    allow_extended_string_escapes: false,
};

/// Parses `text` as JSONC into a [`serde_json::Value`].
///
/// Empty/whitespace-only input deserializes as `Value::Null` (jsonc-parser
/// semantics), matching upstream where an unparsable config yields no usable
/// object. Malformed input is a [`ConfigError::Jsonc`] naming the parser
/// error.
pub fn parse_jsonc(text: &str) -> Result<Value, ConfigError> {
    jsonc_parser::parse_to_serde_value::<Value>(text, &JSONC_OPTIONS)
        .map_err(|source| ConfigError::Jsonc { source })
}

// -------------------------------------------------------------------------
// Config paths
// -------------------------------------------------------------------------

/// Candidate config file paths for the global side and the project side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub home: PathBuf,
    pub global_files: Vec<PathBuf>,
    pub project_files: Vec<PathBuf>,
}

impl ConfigPaths {
    /// Global candidates only: `<home>/.config/opencode/opencode-mem.jsonc`
    /// then `.json`.
    pub fn standard(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let config_dir = home.join(".config").join("opencode");
        Self {
            home,
            global_files: vec![
                config_dir.join("opencode-mem.jsonc"),
                config_dir.join("opencode-mem.json"),
            ],
            project_files: Vec::new(),
        }
    }

    /// Adds the project candidates `<project>/.opencode/opencode-mem.jsonc`
    /// then `.json`, mirroring upstream `initConfig`.
    pub fn for_project(project_dir: impl AsRef<Path>, home: impl Into<PathBuf>) -> Self {
        let mut paths = Self::standard(home);
        let project_config_dir = project_dir.as_ref().join(".opencode");
        paths.project_files = vec![
            project_config_dir.join("opencode-mem.jsonc"),
            project_config_dir.join("opencode-mem.json"),
        ];
        paths
    }
}

// -------------------------------------------------------------------------
// File candidate loading (first parseable wins)
// -------------------------------------------------------------------------

/// Reads the first candidate that exists, reads, and parses to a JSON object.
///
/// Upstream parity: unreadable/invalid/non-object candidates are silently
/// skipped — the `tracing::warn!` here is observability only. Files are
/// decoded lossily (Node's replacement decoding behavior).
fn read_side_files(files: &[PathBuf]) -> Map<String, Value> {
    for path in files {
        if !path.exists() {
            continue;
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error, "skipping unreadable config file");
                continue;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let parsed = match parse_jsonc(&text) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error, "skipping invalid config file");
                continue;
            }
        };
        match parsed {
            Value::Object(map) => return map,
            other => {
                tracing::warn!(path = %path.display(), kind = render_kind(&other), "skipping non-object config file");
            }
        }
    }
    Map::new()
}

fn render_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// -------------------------------------------------------------------------
// Composition policy
// -------------------------------------------------------------------------

/// Global-only remote-provider fields: forbidden in project files by key
/// PRESENCE (a present key with `null` still counts), so a project file can
/// never trigger inheritance or hijack of global provider transport/keys.
const GLOBAL_ONLY_REMOTE_PROVIDER_FIELDS: [&str; 5] = [
    "embeddingApiUrl",
    "embeddingApiKey",
    "memoryProvider",
    "memoryApiUrl",
    "memoryApiKey",
];

/// Campaign-level policy keys project files may carry but whose values
/// never take effect — cleanup stays a global/defaults decision.
const PROJECT_DROPPED_FIELDS: [&str; 2] = ["autoCleanupEnabled", "autoCleanupRetentionDays"];

// -------------------------------------------------------------------------
// Build + load API
// -------------------------------------------------------------------------

impl Config {
    /// Layers the candidate files: defaults < global file < project file, then
    /// environment-derived values during build.
    pub fn load(paths: &ConfigPaths, env: &dyn EnvSource) -> Result<Self, ConfigError> {
        let global_map = read_side_files(&paths.global_files);
        let project_map = read_side_files(&paths.project_files);

        let present: Vec<String> = GLOBAL_ONLY_REMOTE_PROVIDER_FIELDS
            .iter()
            .filter(|field| project_map.contains_key(**field))
            .map(|field| (*field).to_owned())
            .collect();
        if !present.is_empty() {
            return Err(ConfigError::ProjectRemoteProviderFields {
                fields: present,
                global_config_path: paths.global_files[0].clone(),
            });
        }

        let mut merged = global_map;
        let mut project = project_map;
        for key in PROJECT_DROPPED_FIELDS {
            project.remove(key);
        }
        for (key, value) in project {
            merged.insert(key, value);
        }

        let file_config: FileConfig = serde_path_to_error::deserialize(Value::Object(merged))
            .map_err(|error| ConfigError::InvalidShape {
                path: error.path().to_string(),
                source: error.into_inner(),
            })?;

        Self::build(file_config, env, &paths.home)
    }

    /// Loads from the standard global paths derived from the env home.
    pub fn load_default(env: &dyn EnvSource) -> Result<Self, ConfigError> {
        let home = home_dir(env).ok_or(ConfigError::MissingHomeDirectory)?;
        Self::load(&ConfigPaths::standard(home), env)
    }

    /// Loads with the project-side candidates of `project_dir` added.
    pub fn load_for_project(
        project_dir: impl AsRef<Path>,
        env: &dyn EnvSource,
    ) -> Result<Self, ConfigError> {
        let home = home_dir(env).ok_or(ConfigError::MissingHomeDirectory)?;
        Self::load(&ConfigPaths::for_project(project_dir, home), env)
    }

    /// Mirrors upstream `buildConfig` field for field; copy every default
    /// from the [DEFAULTS] table rather than inventing one.
    fn build(file: FileConfig, env: &dyn EnvSource, home: &Path) -> Result<Self, ConfigError> {
        use normalize::*;

        let memory_provider = file
            .memory_provider
            .unwrap_or(schema::MemoryProvider::OpenAiChat);
        let resolved_memory_api_key = resolve_secret_value(file.memory_api_key.as_deref(), env)?;
        let memory_api_key = resolved_memory_api_key.or_else(|| {
            if memory_provider == schema::MemoryProvider::AtlasCloud {
                env.get("ATLASCLOUD_API_KEY")
            } else {
                None
            }
        });

        let embedding_model = file
            .embedding_model
            .unwrap_or_else(|| DEFAULTS.embedding_model.to_owned());
        let preset = embedding_model_preset(&embedding_model);
        let embedding_dimensions = normalize_embedding_dimensions(
            file.embedding_dimensions
                .unwrap_or_else(|| embedding_dimensions_for_model(&embedding_model) as f64),
        )?;
        let embedding_pooling = file.embedding_pooling.unwrap_or_else(|| {
            preset
                .as_ref()
                .map(|preset| preset.pooling)
                .unwrap_or(DEFAULTS.embedding_pooling)
        });
        let embedding_query_prefix = file.embedding_query_prefix.or_else(|| {
            preset
                .as_ref()
                .and_then(|preset| preset.query_prefix.clone())
        });
        let embedding_document_prefix = file.embedding_document_prefix.or_else(|| {
            preset
                .as_ref()
                .and_then(|preset| preset.document_prefix.clone())
        });
        let embedding_dtype = normalize_embedding_dtype(file.embedding_dtype.as_ref())?;
        let auto_capture_max_context_bytes = normalize_auto_capture_max_context_bytes(
            file.auto_capture_max_context_bytes
                .unwrap_or(DEFAULTS.auto_capture_max_context_bytes),
        )?;
        let user_profile_auto_cleanup_interval = normalize_user_profile_auto_cleanup_interval(
            file.user_profile_auto_cleanup_interval
                .unwrap_or(DEFAULTS.user_profile_auto_cleanup_interval),
        )?;
        let episodic_consolidation_age_days = normalize_episodic_consolidation_age_days(
            file.episodic
                .as_ref()
                .and_then(|episodic| episodic.consolidation_age_days)
                .unwrap_or(DEFAULTS.episodic_consolidation_age_days),
        )?;

        // Only when the transport URL is set: resolve the file key ?? env
        // `OPENAI_API_KEY` (an explicit nullish chain, so "" blocks the env
        // fallback); without a URL the key is never exposed.
        let embedding_api_key = if file.embedding_api_url.is_some() {
            let raw_ref = match file.embedding_api_key.clone() {
                Some(raw) => Some(raw),
                None => env.get("OPENAI_API_KEY"),
            };
            resolve_secret_value(raw_ref.as_deref(), env)?
        } else {
            None
        };

        let storage_path = match file.storage_path.as_deref() {
            Some(path) => expand_path(path, home),
            None => expand_path(DEFAULTS.storage_path, home),
        };

        let web_server_auth_password =
            resolve_secret_value(file.web_server_auth_password.as_deref(), env)?;
        let web_server_api_token = match file.web_server_api_token.as_deref() {
            Some(raw) if !raw.is_empty() => resolve_secret_value(Some(raw), env)?,
            _ => None,
        };

        let auto_capture_provider_settings = AutoCaptureProviderSettings {
            opencode_provider: file.opencode_provider.clone(),
            opencode_model: file.opencode_model.clone(),
            memory_provider: Some(memory_provider),
            memory_model: file.memory_model.clone(),
            memory_api_url: file.memory_api_url.clone(),
            memory_api_key: memory_api_key.clone(),
        };
        let auto_capture_provider_status =
            get_auto_capture_provider_status(&auto_capture_provider_settings);

        let injection_markers_raw = file
            .chat_message
            .as_ref()
            .and_then(|chat| chat.injection_markers.as_ref());
        let chat_message = schema::ChatMessage {
            enabled: file
                .chat_message
                .as_ref()
                .and_then(|chat| chat.enabled)
                .unwrap_or(DEFAULTS.chat_message_enabled),
            max_memories: file
                .chat_message
                .as_ref()
                .and_then(|chat| chat.max_memories)
                .unwrap_or(DEFAULTS.chat_message_max_memories),
            exclude_current_session: file
                .chat_message
                .as_ref()
                .and_then(|chat| chat.exclude_current_session)
                .unwrap_or(DEFAULTS.chat_message_exclude_current_session),
            max_age_days: file
                .chat_message
                .as_ref()
                .and_then(|chat| chat.max_age_days),
            inject_on: file
                .chat_message
                .as_ref()
                .and_then(|chat| chat.inject_on)
                .unwrap_or(DEFAULTS.chat_message_inject_on),
            filter_injected_prompts: file
                .chat_message
                .as_ref()
                .and_then(|chat| chat.filter_injected_prompts)
                .unwrap_or(DEFAULTS.chat_message_filter_injected_prompts),
            injection_markers: normalize_injection_markers(injection_markers_raw)?,
            capture_child_sessions: file
                .chat_message
                .as_ref()
                .and_then(|chat| chat.capture_child_sessions)
                .unwrap_or(DEFAULTS.chat_message_capture_child_sessions),
        };

        let compaction = match file.compaction.as_ref() {
            Some(compaction) => schema::Compaction {
                enabled: compaction.enabled.unwrap_or(DEFAULTS.compaction_enabled),
                memory_limit: compaction
                    .memory_limit
                    .unwrap_or(DEFAULTS.compaction_memory_limit),
            },
            None => schema::Compaction {
                enabled: DEFAULTS.compaction_enabled,
                memory_limit: DEFAULTS.compaction_memory_limit,
            },
        };

        let memory = schema::MemoryScope {
            default_scope: file
                .memory
                .as_ref()
                .and_then(|memory| memory.default_scope)
                .unwrap_or(DEFAULTS.memory_default_scope),
        };

        Ok(Self {
            storage_path,
            user_email_override: file.user_email_override,
            user_name_override: file.user_name_override,
            embedding_model,
            embedding_dimensions,
            embedding_use_task_prefixes: file
                .embedding_use_task_prefixes
                .unwrap_or(DEFAULTS.embedding_use_task_prefixes),
            embedding_pooling,
            embedding_query_prefix,
            embedding_document_prefix,
            embedding_dtype,
            embedding_api_url: file.embedding_api_url,
            embedding_api_key,
            similarity_threshold: file
                .similarity_threshold
                .unwrap_or(DEFAULTS.similarity_threshold),
            max_memories: file.max_memories.unwrap_or(DEFAULTS.max_memories),
            max_profile_items: file.max_profile_items.unwrap_or(DEFAULTS.max_profile_items),
            inject_profile: file.inject_profile.unwrap_or(DEFAULTS.inject_profile),
            container_tag_prefix: file
                .container_tag_prefix
                .unwrap_or_else(|| DEFAULTS.container_tag_prefix.to_owned()),
            auto_capture_enabled: file
                .auto_capture_enabled
                .unwrap_or(DEFAULTS.auto_capture_enabled),
            auto_capture_max_iterations: file
                .auto_capture_max_iterations
                .unwrap_or(DEFAULTS.auto_capture_max_iterations),
            auto_capture_iteration_timeout: file
                .auto_capture_iteration_timeout
                .unwrap_or(DEFAULTS.auto_capture_iteration_timeout),
            auto_capture_max_retries: file
                .auto_capture_max_retries
                .unwrap_or(DEFAULTS.auto_capture_max_retries),
            auto_capture_max_context_bytes,
            auto_capture_language: file.auto_capture_language,
            memory_provider,
            memory_model: file.memory_model,
            memory_api_url: file.memory_api_url,
            memory_api_key,
            memory_temperature: normalize_memory_temperature(file.memory_temperature.as_ref())?,
            force_tool_choice: file.force_tool_choice,
            memory_extra_params: file.memory_extra_params,
            opencode_provider: file.opencode_provider,
            opencode_model: file.opencode_model,
            opencode_variant: normalize_opencode_variant(file.opencode_variant.as_ref()),
            opencode_timeout_ms: normalize_opencode_timeout_ms(file.opencode_timeout_ms.as_ref()),
            auto_capture_provider_status,
            ai_session_retention_days: file
                .ai_session_retention_days
                .unwrap_or(DEFAULTS.ai_session_retention_days),
            web_server_enabled: file
                .web_server_enabled
                .unwrap_or(DEFAULTS.web_server_enabled),
            web_server_port: file.web_server_port.unwrap_or(DEFAULTS.web_server_port),
            web_server_host: file
                .web_server_host
                .unwrap_or_else(|| DEFAULTS.web_server_host.to_owned()),
            web_server_auth_password,
            web_server_auth_username: file.web_server_auth_username,
            web_server_api_token,
            max_vectors_per_shard: file
                .max_vectors_per_shard
                .unwrap_or(DEFAULTS.max_vectors_per_shard),
            auto_cleanup_enabled: file
                .auto_cleanup_enabled
                .unwrap_or(DEFAULTS.auto_cleanup_enabled),
            auto_cleanup_retention_days: normalize_auto_cleanup_retention_days(
                file.auto_cleanup_retention_days
                    .unwrap_or(DEFAULTS.auto_cleanup_retention_days),
            )?,
            deduplication_enabled: file
                .deduplication_enabled
                .unwrap_or(DEFAULTS.deduplication_enabled),
            deduplication_similarity_threshold: file
                .deduplication_similarity_threshold
                .unwrap_or(DEFAULTS.deduplication_similarity_threshold),
            user_profile_analysis_interval: file
                .user_profile_analysis_interval
                .unwrap_or(DEFAULTS.user_profile_analysis_interval),
            user_profile_max_context_bytes: file
                .user_profile_max_context_bytes
                .unwrap_or(DEFAULTS.user_profile_max_context_bytes),
            user_profile_display_preferences: file
                .user_profile_display_preferences
                .unwrap_or(DEFAULTS.user_profile_display_preferences),
            user_profile_display_patterns: file
                .user_profile_display_patterns
                .unwrap_or(DEFAULTS.user_profile_display_patterns),
            user_profile_display_workflows: file
                .user_profile_display_workflows
                .unwrap_or(DEFAULTS.user_profile_display_workflows),
            user_profile_stale_days: file
                .user_profile_stale_days
                .unwrap_or(DEFAULTS.user_profile_stale_days),
            user_profile_inject_preferences: file
                .user_profile_inject_preferences
                .unwrap_or(DEFAULTS.user_profile_inject_preferences),
            user_profile_inject_patterns: file
                .user_profile_inject_patterns
                .unwrap_or(DEFAULTS.user_profile_inject_patterns),
            user_profile_inject_workflows: file
                .user_profile_inject_workflows
                .unwrap_or(DEFAULTS.user_profile_inject_workflows),
            user_profile_confidence_decay_days: file
                .user_profile_confidence_decay_days
                .unwrap_or(DEFAULTS.user_profile_confidence_decay_days),
            user_profile_changelog_retention_count: file
                .user_profile_changelog_retention_count
                .unwrap_or(DEFAULTS.user_profile_changelog_retention_count),
            user_profile_embedding_threshold_same_cat: file
                .user_profile_embedding_threshold_same_cat
                .unwrap_or(DEFAULTS.user_profile_embedding_threshold_same_cat),
            user_profile_embedding_threshold_same_cat_weak: file
                .user_profile_embedding_threshold_same_cat_weak
                .unwrap_or(DEFAULTS.user_profile_embedding_threshold_same_cat_weak),
            user_profile_embedding_threshold_cross_cat: file
                .user_profile_embedding_threshold_cross_cat
                .unwrap_or(DEFAULTS.user_profile_embedding_threshold_cross_cat),
            user_profile_embedding_threshold_cross_cat_weak: file
                .user_profile_embedding_threshold_cross_cat_weak
                .unwrap_or(DEFAULTS.user_profile_embedding_threshold_cross_cat_weak),
            user_profile_centroid_drift_threshold: file
                .user_profile_centroid_drift_threshold
                .unwrap_or(DEFAULTS.user_profile_centroid_drift_threshold),
            user_profile_embedding_min_description_length: file
                .user_profile_embedding_min_description_length
                .unwrap_or(DEFAULTS.user_profile_embedding_min_description_length),
            user_profile_min_evidence_for_retention: file
                .user_profile_min_evidence_for_retention
                .unwrap_or(DEFAULTS.user_profile_min_evidence_for_retention),
            user_profile_auto_cleanup_enabled: file
                .user_profile_auto_cleanup_enabled
                .unwrap_or(DEFAULTS.user_profile_auto_cleanup_enabled),
            user_profile_auto_cleanup_interval,
            user_profile_validation_enabled: file
                .user_profile_validation_enabled
                .unwrap_or(DEFAULTS.user_profile_validation_enabled),
            show_auto_capture_toasts: file
                .show_auto_capture_toasts
                .unwrap_or(DEFAULTS.show_auto_capture_toasts),
            show_user_profile_toasts: file
                .show_user_profile_toasts
                .unwrap_or(DEFAULTS.show_user_profile_toasts),
            show_error_toasts: file.show_error_toasts.unwrap_or(DEFAULTS.show_error_toasts),
            database_encryption_enabled: file
                .database_encryption_enabled
                .unwrap_or(DEFAULTS.database_encryption_enabled),
            database_encryption_key: file.database_encryption_key,
            database_encryption_cipher: file
                .database_encryption_cipher
                .unwrap_or(DEFAULTS.database_encryption_cipher),
            auto_update: file.auto_update.unwrap_or(DEFAULTS.auto_update),
            memory,
            compaction,
            chat_message,
            episodic: schema::Episodic {
                consolidation_age_days: episodic_consolidation_age_days,
            },
        })
    }
}

// -------------------------------------------------------------------------
// Provider status / placeholders / is_configured
// -------------------------------------------------------------------------

/// Placeholder-looking key prefixes/values (upstream `GENERIC_API_KEYS` +
/// template placeholders), compared lowercase after trimming.
const PLACEHOLDER_API_KEYS: [&str; 6] = [
    "sk-...",
    "sk-ant-...",
    "gsk_...",
    "your-api-key",
    "your api key",
    "replace-with-your-api-key",
];

/// True for template placeholder keys; `None`/empty/real-looking keys are not
/// placeholders.
pub fn is_placeholder_api_key(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let trimmed = value.trim().to_lowercase();
    if trimmed.is_empty() {
        return false;
    }
    PLACEHOLDER_API_KEYS
        .iter()
        .any(|placeholder| *placeholder == trimmed)
}

/// `hasValue`: non-empty after trimming.
fn has_value(value: &Option<String>) -> bool {
    value
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

/// Port of upstream `getAutoCaptureProviderStatus`: opencode branch first,
/// exact issue strings in order, and preset providers (`orcarouter` /
/// `atlas-cloud`) requiring only an API key for the manual fallback path.
pub fn get_auto_capture_provider_status(
    settings: &AutoCaptureProviderSettings,
) -> AutoCaptureProviderStatus {
    let has_opencode_provider = has_value(&settings.opencode_provider);
    let has_opencode_model = has_value(&settings.opencode_model);
    if has_opencode_provider && has_opencode_model {
        return AutoCaptureProviderStatus::Ready {
            mode: AutoCaptureProviderMode::OpenCode,
        };
    }

    let mut issues: Vec<String> = Vec::new();
    if !has_opencode_provider {
        issues.push("opencodeProvider is not configured".to_owned());
    }
    if !has_opencode_model {
        issues.push("opencodeModel is not configured".to_owned());
    }

    let has_memory_model = has_value(&settings.memory_model);
    let has_memory_api_url = has_value(&settings.memory_api_url);
    let has_memory_api_key = has_value(&settings.memory_api_key);
    let has_placeholder_memory_key = is_placeholder_api_key(settings.memory_api_key.as_deref());

    // Preset providers fill endpoint/model themselves: only the key is
    // required for the manual fallback path.
    let is_preset_provider = matches!(
        settings.memory_provider,
        Some(schema::MemoryProvider::OrcaRouter) | Some(schema::MemoryProvider::AtlasCloud)
    );
    if is_preset_provider {
        if !has_memory_api_key {
            issues.push("memoryApiKey is not configured".to_owned());
        }
        if has_placeholder_memory_key {
            issues.push("memoryApiKey contains a placeholder value".to_owned());
        }
        if has_memory_api_key && !has_placeholder_memory_key {
            return AutoCaptureProviderStatus::Ready {
                mode: AutoCaptureProviderMode::Manual,
            };
        }
        return AutoCaptureProviderStatus::NotReady { issues };
    }

    if !has_memory_model {
        issues.push("memoryModel is not configured".to_owned());
    }
    if !has_memory_api_url {
        issues.push("memoryApiUrl is not configured".to_owned());
    }
    if !has_memory_api_key {
        issues.push("memoryApiKey is not configured".to_owned());
    }
    if has_placeholder_memory_key {
        issues.push("memoryApiKey contains a placeholder value".to_owned());
    }

    if has_memory_model && has_memory_api_url && has_memory_api_key && !has_placeholder_memory_key {
        AutoCaptureProviderStatus::Ready {
            mode: AutoCaptureProviderMode::Manual,
        }
    } else {
        AutoCaptureProviderStatus::NotReady { issues }
    }
}

/// True when auto-capture has a usable provider for its structured-output
/// calls (upstream `hasAutoCaptureProviderConfig`).
pub fn has_auto_capture_provider_config(settings: &AutoCaptureProviderSettings) -> bool {
    matches!(
        get_auto_capture_provider_status(settings),
        AutoCaptureProviderStatus::Ready { .. }
    )
}

/// Host-contract stub mirroring upstream — always true.
pub fn is_configured() -> bool {
    true
}
