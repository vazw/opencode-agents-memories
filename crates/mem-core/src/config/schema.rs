//! Raw config schema, defaults table, public enums, and the resolved
//! [`Config`] structs.
//!
//! `FileConfig` transcribes the upstream `OpenCodeMemConfig` interface field
//! for field (`src/config.ts`); every field is optional and unknown keys stay
//! tolerated (no `deny_unknown_fields` — forward compatibility). Numeric
//! values normalized by keys are typed `f64` in the raw schema so
//! out-of-range/negative/fractional values reach the key-naming normalizers.

use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Public enums
// ---------------------------------------------------------------------------

/// Local pooling strategy for transformer embedding models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingPooling {
    Mean,
    Cls,
    LastToken,
}

impl EmbeddingPooling {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mean => "mean",
            Self::Cls => "cls",
            Self::LastToken => "last_token",
        }
    }
}

impl<'de> Deserialize<'de> for EmbeddingPooling {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "mean" => Ok(Self::Mean),
            "cls" => Ok(Self::Cls),
            "last_token" => Ok(Self::LastToken),
            other => Err(serde::de::Error::custom(
                crate::error::ConfigError::InvalidValueExpected {
                    key: "embeddingPooling",
                    value: other.to_owned(),
                    expected: "\"mean\" | \"cls\" | \"last_token\"",
                },
            )),
        }
    }
}

/// Dtypes accepted by the local transformers.js ONNX pipeline
/// (upstream `LOCAL_ONNX_DTYPES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOnnxDtype {
    Auto,
    Fp32,
    Fp16,
    Q8,
    Int8,
    Uint8,
    Q4,
    Bnb4,
    Q4F16,
    Q2,
    Q2F16,
    Q1,
    Q1F16,
}

pub const LOCAL_ONNX_DTYPES: [LocalOnnxDtype; 13] = [
    LocalOnnxDtype::Auto,
    LocalOnnxDtype::Fp32,
    LocalOnnxDtype::Fp16,
    LocalOnnxDtype::Q8,
    LocalOnnxDtype::Int8,
    LocalOnnxDtype::Uint8,
    LocalOnnxDtype::Q4,
    LocalOnnxDtype::Bnb4,
    LocalOnnxDtype::Q4F16,
    LocalOnnxDtype::Q2,
    LocalOnnxDtype::Q2F16,
    LocalOnnxDtype::Q1,
    LocalOnnxDtype::Q1F16,
];

/// The rendered `expected` list for dtype errors (must equal
/// `LOCAL_ONNX_DTYPES.map(as_str).join(", ")`).
pub const LOCAL_ONNX_DTYPES_EXPECTED: &str =
    "auto, fp32, fp16, q8, int8, uint8, q4, bnb4, q4f16, q2, q2f16, q1, q1f16";

impl LocalOnnxDtype {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fp32 => "fp32",
            Self::Fp16 => "fp16",
            Self::Q8 => "q8",
            Self::Int8 => "int8",
            Self::Uint8 => "uint8",
            Self::Q4 => "q4",
            Self::Bnb4 => "bnb4",
            Self::Q4F16 => "q4f16",
            Self::Q2 => "q2",
            Self::Q2F16 => "q2f16",
            Self::Q1 => "q1",
            Self::Q1F16 => "q1f16",
        }
    }

    /// Canonical member of the 13-value list (exact, case-sensitive).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Option<Self> {
        LOCAL_ONNX_DTYPES
            .iter()
            .find(|candidate| candidate.as_str() == value)
            .copied()
    }
}

/// Remote memory provider type (upstream string union).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProvider {
    #[doc(hidden)]
    AtlasCloud,
    OpenAiChat,
    OpenAiResponses,
    Anthropic,
    Minimax,
    OrcaRouter,
}

impl MemoryProvider {
    /// Exact upstream wire value (note `orcarouter` has no separator).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AtlasCloud => "atlas-cloud",
            Self::OpenAiChat => "openai-chat",
            Self::OpenAiResponses => "openai-responses",
            Self::Anthropic => "anthropic",
            Self::Minimax => "minimax",
            Self::OrcaRouter => "orcarouter",
        }
    }
}

impl std::str::FromStr for MemoryProvider {
    type Err = crate::error::ConfigError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::from_wire(s) {
            Some(provider) => Ok(provider),
            None => Err(crate::error::ConfigError::InvalidValue {
                key: "memoryProvider",
                value: s.to_owned(),
            }),
        }
    }
}

impl MemoryProvider {
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "atlas-cloud" => Some(Self::AtlasCloud),
            "openai-chat" => Some(Self::OpenAiChat),
            "openai-responses" => Some(Self::OpenAiResponses),
            "anthropic" => Some(Self::Anthropic),
            "minimax" => Some(Self::Minimax),
            "orcarouter" => Some(Self::OrcaRouter),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for MemoryProvider {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_wire(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown memoryProvider: {raw}")))
    }
}

/// Default scope for memory list/search queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryDefaultScope {
    Project,
    AllProjects,
}

impl MemoryDefaultScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::AllProjects => "all-projects",
        }
    }
}

impl<'de> Deserialize<'de> for MemoryDefaultScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "project" => Ok(Self::Project),
            "all-projects" => Ok(Self::AllProjects),
            other => Err(serde::de::Error::custom(format!(
                "unknown memory.defaultScope: {other}"
            ))),
        }
    }
}

/// When chat memory injection happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectOn {
    First,
    Always,
}

impl InjectOn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Always => "always",
        }
    }
}

impl<'de> Deserialize<'de> for InjectOn {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "first" => Ok(Self::First),
            "always" => Ok(Self::Always),
            other => Err(serde::de::Error::custom(format!(
                "unknown chatMessage.injectOn: {other}"
            ))),
        }
    }
}

/// Cipher for libSQL/Turso database encryption (upstream 7-variant union).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseEncryptionCipher {
    Aes128Gcm,
    Aes256Gcm,
    Aegis256,
    Aegis256X2,
    Aegis128L,
    Aegis128X2,
    Aegis128X4,
}

impl DatabaseEncryptionCipher {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aes128Gcm => "aes128gcm",
            Self::Aes256Gcm => "aes256gcm",
            Self::Aegis256 => "aegis256",
            Self::Aegis256X2 => "aegis256x2",
            Self::Aegis128L => "aegis128l",
            Self::Aegis128X2 => "aegis128x2",
            Self::Aegis128X4 => "aegis128x4",
        }
    }
}

impl<'de> Deserialize<'de> for DatabaseEncryptionCipher {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "aes128gcm" => Ok(Self::Aes128Gcm),
            "aes256gcm" => Ok(Self::Aes256Gcm),
            "aegis256" => Ok(Self::Aegis256),
            "aegis256x2" => Ok(Self::Aegis256X2),
            "aegis128l" => Ok(Self::Aegis128L),
            "aegis128x2" => Ok(Self::Aegis128X2),
            "aegis128x4" => Ok(Self::Aegis128X4),
            other => Err(serde::de::Error::custom(format!(
                "unknown databaseEncryptionCipher: {other}"
            ))),
        }
    }
}

/// `memoryTemperature`: a number, or `false` to omit the parameter entirely
/// for reasoning models that reject it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoryTemperature {
    Value(f64),
    Disabled,
}

/// Embedding model preset (pooling + task prefixes by model family).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingModelPreset {
    pub pooling: EmbeddingPooling,
    /// When present (including `""`), the default query/document prefix.
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
}

// ---------------------------------------------------------------------------
// Auto-capture provider status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCaptureProviderMode {
    OpenCode,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoCaptureProviderStatus {
    Ready { mode: AutoCaptureProviderMode },
    NotReady { issues: Vec<String> },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoCaptureProviderSettings {
    pub opencode_provider: Option<String>,
    pub opencode_model: Option<String>,
    pub memory_provider: Option<MemoryProvider>,
    pub memory_model: Option<String>,
    pub memory_api_url: Option<String>,
    pub memory_api_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Raw file schema (transcription of upstream OpenCodeMemConfig)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileConfig {
    pub storage_path: Option<String>,
    pub user_email_override: Option<String>,
    pub user_name_override: Option<String>,
    pub memory: Option<FileMemoryConfig>,
    pub embedding_model: Option<String>,
    pub embedding_dimensions: Option<f64>,
    pub embedding_api_url: Option<String>,
    pub embedding_api_key: Option<String>,
    pub embedding_use_task_prefixes: Option<bool>,
    pub embedding_pooling: Option<EmbeddingPooling>,
    pub embedding_query_prefix: Option<String>,
    pub embedding_document_prefix: Option<String>,
    pub embedding_dtype: Option<Value>,
    pub embedding_max_tokens: Option<f64>,
    pub similarity_threshold: Option<f64>,
    pub max_memories: Option<u32>,
    /// Upstream's `maxProfileItems`, parsed for compatibility and
    /// **deliberately unread**: no consumer exists, and the injection caps come
    /// from the three `userProfileInject*` knobs (issue 44.9). Do not wire it
    /// to a cap — that is the whole point of the AC.
    pub max_profile_items: Option<u32>,
    pub inject_profile: Option<bool>,
    pub container_tag_prefix: Option<String>,
    pub auto_capture_enabled: Option<bool>,
    pub auto_capture_max_iterations: Option<u32>,
    pub auto_capture_iteration_timeout: Option<u64>,
    pub auto_capture_max_retries: Option<u32>,
    pub auto_capture_max_context_bytes: Option<f64>,
    pub auto_capture_language: Option<String>,
    pub memory_provider: Option<MemoryProvider>,
    pub memory_model: Option<String>,
    pub memory_api_url: Option<String>,
    pub memory_api_key: Option<String>,
    pub memory_temperature: Option<Value>,
    pub force_tool_choice: Option<bool>,
    pub memory_extra_params: Option<Value>,
    pub opencode_provider: Option<String>,
    pub opencode_model: Option<String>,
    pub opencode_variant: Option<Value>,
    /// Operator override for the host's `{origin}/api` base. When set it wins
    /// over `~/.local/state/opencode/service.json`'s origin, and the service
    /// file is then consulted for credentials ONLY.
    pub opencode_base_url: Option<String>,
    pub opencode_timeout_ms: Option<Value>,
    pub ai_session_retention_days: Option<u32>,
    pub web_server_enabled: Option<bool>,
    pub web_server_port: Option<u16>,
    pub web_server_host: Option<String>,
    pub web_server_auth_password: Option<String>,
    pub web_server_auth_username: Option<String>,
    pub web_server_api_token: Option<String>,
    pub max_vectors_per_shard: Option<u32>,
    pub auto_cleanup_enabled: Option<bool>,
    pub auto_cleanup_retention_days: Option<f64>,
    pub deduplication_enabled: Option<bool>,
    pub deduplication_similarity_threshold: Option<f64>,
    pub user_profile_analysis_interval: Option<u32>,
    pub user_profile_max_context_bytes: Option<u32>,
    pub user_profile_display_preferences: Option<u32>,
    pub user_profile_display_patterns: Option<u32>,
    pub user_profile_display_workflows: Option<u32>,
    pub user_profile_stale_days: Option<u32>,
    pub user_profile_inject_preferences: Option<u32>,
    pub user_profile_inject_patterns: Option<u32>,
    pub user_profile_inject_workflows: Option<u32>,
    pub user_profile_confidence_decay_days: Option<u32>,
    pub user_profile_changelog_retention_count: Option<u32>,
    pub user_profile_embedding_threshold_same_cat: Option<f64>,
    pub user_profile_embedding_threshold_same_cat_weak: Option<f64>,
    pub user_profile_embedding_threshold_cross_cat: Option<f64>,
    pub user_profile_embedding_threshold_cross_cat_weak: Option<f64>,
    pub user_profile_centroid_drift_threshold: Option<f64>,
    pub user_profile_embedding_min_description_length: Option<u32>,
    pub user_profile_min_evidence_for_retention: Option<u32>,
    pub user_profile_auto_cleanup_enabled: Option<bool>,
    pub user_profile_auto_cleanup_interval: Option<f64>,
    pub user_profile_validation_enabled: Option<bool>,
    pub show_auto_capture_toasts: Option<bool>,
    pub show_user_profile_toasts: Option<bool>,
    pub show_error_toasts: Option<bool>,
    pub database_encryption_key: Option<String>,
    pub database_encryption_enabled: Option<bool>,
    pub database_encryption_cipher: Option<DatabaseEncryptionCipher>,
    /// Top-level switch that disables the mixed-version open guard (issue 45 O4/O5).
    pub allow_unguarded_open: Option<bool>,
    pub auto_update: Option<bool>,
    pub compaction: Option<FileCompactionConfig>,
    pub chat_message: Option<FileChatMessageConfig>,
    pub episodic: Option<FileEpisodicConfig>,
    pub feature_flags: Option<FileFeatureFlags>,
    pub sound_memory: Option<FileSoundMemory>,
    pub promotion: Option<FilePromotion>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMemoryConfig {
    pub default_scope: Option<MemoryDefaultScope>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCompactionConfig {
    pub enabled: Option<bool>,
    pub memory_limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChatMessageConfig {
    pub enabled: Option<bool>,
    pub max_memories: Option<u32>,
    pub exclude_current_session: Option<bool>,
    pub max_age_days: Option<u32>,
    pub inject_on: Option<InjectOn>,
    pub filter_injected_prompts: Option<bool>,
    /// Kept loose: validated/merged by the config normalizer.
    pub injection_markers: Option<Value>,
    pub capture_child_sessions: Option<bool>,
}

/// New-tier threshold (roadmap R10): consolidation age lives in config so
/// tuning needs no recompile.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEpisodicConfig {
    pub consolidation_age_days: Option<f64>,
}

/// W7.1 — the feature-flag section. Only the two flags with a consumer in
/// this wave are read; every other flag mentioned in the draft stays absent
/// (dead config is a defect, not forward compatibility).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFeatureFlags {
    pub sound_memory: Option<bool>,
    pub promotion_pipeline: Option<bool>,
}

/// W7.1 — the sound-memory section.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSoundMemory {
    pub raw_context_required: Option<bool>,
}

/// W7.1 — the promotion section. `ttlFallbackDays` arrives as `f64` so
/// out-of-range/fractional values reach the key-naming normalizer instead of
/// becoming a serde shape error.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePromotion {
    pub ttl_fallback_days: Option<f64>,
}

// ---------------------------------------------------------------------------
// Defaults table (transcribed from upstream DEFAULTS)
// ---------------------------------------------------------------------------

/// Static transcription of the upstream `DEFAULTS` table. The example jsonc
/// is a cross-check only; this table wins.
pub struct Defaults {
    pub storage_path: &'static str,
    pub embedding_model: &'static str,
    pub embedding_dimensions: u32,
    pub embedding_use_task_prefixes: bool,
    pub embedding_pooling: EmbeddingPooling,
    pub similarity_threshold: f64,
    pub max_memories: u32,
    /// Upstream's `maxProfileItems` default, copied verbatim into
    /// `Config::max_profile_items`. It is deliberately unread everywhere: the
    /// injection caps are the three `userProfileInject*` knobs (issue 44.9).
    pub max_profile_items: u32,
    pub inject_profile: bool,
    pub container_tag_prefix: &'static str,
    pub auto_capture_enabled: bool,
    pub auto_capture_max_iterations: u32,
    pub auto_capture_iteration_timeout: u64,
    pub auto_capture_max_retries: u32,
    pub auto_capture_max_context_bytes: f64,
    pub ai_session_retention_days: u32,
    pub web_server_enabled: bool,
    pub web_server_port: u16,
    pub web_server_host: &'static str,
    pub max_vectors_per_shard: u32,
    pub auto_cleanup_enabled: bool,
    pub auto_cleanup_retention_days: f64,
    pub deduplication_enabled: bool,
    pub deduplication_similarity_threshold: f64,
    pub user_profile_analysis_interval: u32,
    pub user_profile_max_context_bytes: u32,
    pub user_profile_display_preferences: u32,
    pub user_profile_display_patterns: u32,
    pub user_profile_display_workflows: u32,
    pub user_profile_stale_days: u32,
    pub user_profile_inject_preferences: u32,
    pub user_profile_inject_patterns: u32,
    pub user_profile_inject_workflows: u32,
    pub user_profile_confidence_decay_days: u32,
    pub user_profile_changelog_retention_count: u32,
    pub user_profile_embedding_threshold_same_cat: f64,
    pub user_profile_embedding_threshold_same_cat_weak: f64,
    pub user_profile_embedding_threshold_cross_cat: f64,
    pub user_profile_embedding_threshold_cross_cat_weak: f64,
    pub user_profile_centroid_drift_threshold: f64,
    pub user_profile_embedding_min_description_length: u32,
    pub user_profile_min_evidence_for_retention: u32,
    pub user_profile_auto_cleanup_enabled: bool,
    pub user_profile_auto_cleanup_interval: f64,
    pub user_profile_validation_enabled: bool,
    pub show_auto_capture_toasts: bool,
    pub show_user_profile_toasts: bool,
    pub show_error_toasts: bool,
    pub database_encryption_enabled: bool,
    pub database_encryption_cipher: DatabaseEncryptionCipher,
    pub allow_unguarded_open: bool,
    pub auto_update: bool,
    pub memory_default_scope: MemoryDefaultScope,
    pub compaction_enabled: bool,
    pub compaction_memory_limit: u32,
    pub chat_message_enabled: bool,
    pub chat_message_max_memories: u32,
    pub chat_message_exclude_current_session: bool,
    pub chat_message_inject_on: InjectOn,
    pub chat_message_filter_injected_prompts: bool,
    pub chat_message_capture_child_sessions: bool,
    pub episodic_consolidation_age_days: f64,
    pub feature_flags_sound_memory: bool,
    pub feature_flags_promotion_pipeline: bool,
    pub sound_memory_raw_context_required: bool,
    pub promotion_ttl_fallback_days: u32,
}

pub static DEFAULTS: Defaults = Defaults {
    storage_path: "~/.opencode-mem/data",
    embedding_model: "Xenova/nomic-embed-text-v1",
    embedding_dimensions: 768,
    embedding_use_task_prefixes: false,
    embedding_pooling: EmbeddingPooling::Mean,
    similarity_threshold: 0.6,
    max_memories: 10,
    max_profile_items: 5,
    inject_profile: true,
    container_tag_prefix: "opencode",
    auto_capture_enabled: true,
    auto_capture_max_iterations: 5,
    auto_capture_iteration_timeout: 30_000,
    auto_capture_max_retries: 3,
    auto_capture_max_context_bytes: 131_072.0,
    ai_session_retention_days: 7,
    web_server_enabled: true,
    web_server_port: 4747,
    web_server_host: "127.0.0.1",
    max_vectors_per_shard: 50_000,
    auto_cleanup_enabled: true,
    auto_cleanup_retention_days: 30.0,
    deduplication_enabled: true,
    deduplication_similarity_threshold: 0.9,
    user_profile_analysis_interval: 10,
    user_profile_max_context_bytes: 32_768,
    user_profile_display_preferences: 20,
    user_profile_display_patterns: 15,
    user_profile_display_workflows: 10,
    user_profile_stale_days: 2,
    user_profile_inject_preferences: 5,
    user_profile_inject_patterns: 5,
    user_profile_inject_workflows: 3,
    user_profile_confidence_decay_days: 30,
    user_profile_changelog_retention_count: 5,
    user_profile_embedding_threshold_same_cat: 0.8,
    user_profile_embedding_threshold_same_cat_weak: 0.5,
    user_profile_embedding_threshold_cross_cat: 0.9,
    user_profile_embedding_threshold_cross_cat_weak: 0.8,
    user_profile_centroid_drift_threshold: 0.65,
    user_profile_embedding_min_description_length: 5,
    user_profile_min_evidence_for_retention: 3,
    user_profile_auto_cleanup_enabled: true,
    user_profile_auto_cleanup_interval: 100.0,
    user_profile_validation_enabled: false,
    show_auto_capture_toasts: true,
    show_user_profile_toasts: true,
    show_error_toasts: true,
    database_encryption_enabled: false,
    database_encryption_cipher: DatabaseEncryptionCipher::Aes256Gcm,
    allow_unguarded_open: false,
    auto_update: true,
    memory_default_scope: MemoryDefaultScope::Project,
    compaction_enabled: true,
    compaction_memory_limit: 10,
    chat_message_enabled: true,
    chat_message_max_memories: 3,
    chat_message_exclude_current_session: true,
    chat_message_inject_on: InjectOn::First,
    chat_message_filter_injected_prompts: true,
    chat_message_capture_child_sessions: false,
    episodic_consolidation_age_days: 30.0,
    feature_flags_sound_memory: false,
    feature_flags_promotion_pipeline: false,
    sound_memory_raw_context_required: false,
    promotion_ttl_fallback_days: 7,
};

// ---------------------------------------------------------------------------
// Resolved config structs
// ---------------------------------------------------------------------------

/// The resolved, validated configuration every later wave consumes.
///
/// Value-typed by design: no process-global mutable `CONFIG` like upstream
/// had — a resolved instance is passed around explicitly.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub storage_path: std::path::PathBuf,
    pub user_email_override: Option<String>,
    pub user_name_override: Option<String>,

    pub embedding_model: String,
    pub embedding_dimensions: u32,
    pub embedding_use_task_prefixes: bool,
    pub embedding_pooling: EmbeddingPooling,
    pub embedding_query_prefix: Option<String>,
    pub embedding_document_prefix: Option<String>,
    pub embedding_dtype: Option<LocalOnnxDtype>,
    /// The operator's token-cap override (`None` = use the model's declared
    /// position limit).
    pub embedding_max_tokens: Option<u32>,
    pub embedding_api_url: Option<String>,
    pub embedding_api_key: Option<String>,

    pub similarity_threshold: f64,
    pub max_memories: u32,
    /// Upstream's `maxProfileItems`, resolved for compatibility and
    /// **deliberately unread**: it never reaches the renderer, whose caps are
    /// the three `userProfileInject*` knobs (issue 44.9).
    pub max_profile_items: u32,
    pub inject_profile: bool,
    pub container_tag_prefix: String,

    pub auto_capture_enabled: bool,
    pub auto_capture_max_iterations: u32,
    pub auto_capture_iteration_timeout: u64,
    pub auto_capture_max_retries: u32,
    pub auto_capture_max_context_bytes: u32,
    pub auto_capture_language: Option<String>,

    pub memory_provider: MemoryProvider,
    pub memory_model: Option<String>,
    pub memory_api_url: Option<String>,
    pub memory_api_key: Option<String>,
    pub memory_temperature: Option<MemoryTemperature>,
    pub force_tool_choice: Option<bool>,
    pub memory_extra_params: Option<Value>,

    pub opencode_provider: Option<String>,
    pub opencode_model: Option<String>,
    pub opencode_variant: Option<String>,
    /// Resolved `opencodeBaseUrl`: a non-blank loopback `http(s)` base the
    /// engine dials instead of the discovered origin (`None` = discover).
    pub opencode_base_url: Option<String>,
    pub opencode_timeout_ms: u64,
    pub auto_capture_provider_status: AutoCaptureProviderStatus,

    pub ai_session_retention_days: u32,

    pub web_server_enabled: bool,
    pub web_server_port: u16,
    pub web_server_host: String,
    pub web_server_auth_password: Option<String>,
    pub web_server_auth_username: Option<String>,
    pub web_server_api_token: Option<String>,

    pub max_vectors_per_shard: u32,
    pub auto_cleanup_enabled: bool,
    pub auto_cleanup_retention_days: u32,
    pub deduplication_enabled: bool,
    pub deduplication_similarity_threshold: f64,

    pub user_profile_analysis_interval: u32,
    pub user_profile_max_context_bytes: u32,
    pub user_profile_display_preferences: u32,
    pub user_profile_display_patterns: u32,
    pub user_profile_display_workflows: u32,
    pub user_profile_stale_days: u32,
    pub user_profile_inject_preferences: u32,
    pub user_profile_inject_patterns: u32,
    pub user_profile_inject_workflows: u32,
    pub user_profile_confidence_decay_days: u32,
    pub user_profile_changelog_retention_count: u32,
    pub user_profile_embedding_threshold_same_cat: f64,
    pub user_profile_embedding_threshold_same_cat_weak: f64,
    pub user_profile_embedding_threshold_cross_cat: f64,
    pub user_profile_embedding_threshold_cross_cat_weak: f64,
    pub user_profile_centroid_drift_threshold: f64,
    pub user_profile_embedding_min_description_length: u32,
    pub user_profile_min_evidence_for_retention: u32,
    pub user_profile_auto_cleanup_enabled: bool,
    pub user_profile_auto_cleanup_interval: u32,
    pub user_profile_validation_enabled: bool,

    pub show_auto_capture_toasts: bool,
    pub show_user_profile_toasts: bool,
    pub show_error_toasts: bool,

    pub database_encryption_enabled: bool,
    /// Kept raw (env:// / file:// / hex); resolved lazily by the encryption wave.
    pub database_encryption_key: Option<String>,
    pub database_encryption_cipher: DatabaseEncryptionCipher,

    /// Disables the mixed-version open guard (issue 45 O4/O5). `false` by default:
    /// the guard is on unless the user turns it off. The env var
    /// `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` wins over this key when both are set.
    pub allow_unguarded_open: bool,

    pub auto_update: bool,

    pub memory: MemoryScope,
    pub compaction: Compaction,
    pub chat_message: ChatMessage,
    pub episodic: Episodic,
    pub feature_flags: FeatureFlags,
    pub sound_memory: SoundMemory,
    pub promotion: Promotion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryScope {
    pub default_scope: MemoryDefaultScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub enabled: bool,
    pub memory_limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub enabled: bool,
    pub max_memories: u32,
    pub exclude_current_session: bool,
    pub max_age_days: Option<u32>,
    pub inject_on: InjectOn,
    pub filter_injected_prompts: bool,
    pub injection_markers: Vec<String>,
    pub capture_child_sessions: bool,
}

/// New-tier episodic memory settings (upstream has no equivalent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Episodic {
    pub consolidation_age_days: u32,
}

/// W7.1 — the two feature flags this wave consumes.
///
/// Both default OFF, and OFF provably writes nothing (no retention rows, no
/// `facts.db`). A flag gates BEHAVIOUR only; schema migrations are never
/// flag-gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureFlags {
    /// Capture persists raw context + segments (`featureFlags.soundMemory`).
    pub sound_memory: bool,
    /// Composition wires the production cross-tier task
    /// (`featureFlags.promotionPipeline`).
    pub promotion_pipeline: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            sound_memory: DEFAULTS.feature_flags_sound_memory,
            promotion_pipeline: DEFAULTS.feature_flags_promotion_pipeline,
        }
    }
}

/// W7.1 — the sound-memory section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundMemory {
    /// A capture payload with no raw text is dead-lettered instead of
    /// falling back to the transcript.
    pub raw_context_required: bool,
}

impl Default for SoundMemory {
    fn default() -> Self {
        Self {
            raw_context_required: DEFAULTS.sound_memory_raw_context_required,
        }
    }
}

/// W7.1 — the promotion section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Promotion {
    /// The Q7 TTL anchor: a session is due when its last write is older
    /// than this many days (default 7).
    pub ttl_fallback_days: u32,
}

impl Default for Promotion {
    fn default() -> Self {
        Self {
            ttl_fallback_days: DEFAULTS.promotion_ttl_fallback_days,
        }
    }
}
