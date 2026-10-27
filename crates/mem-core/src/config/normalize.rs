//! Config normalizers, model presets, path expansion, markers, and secret
//! resolution — one key-named rule per upstream normalization helper.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::schema::{
    EmbeddingModelPreset, EmbeddingPooling, LOCAL_ONNX_DTYPES_EXPECTED, LocalOnnxDtype,
    MemoryTemperature,
};
use crate::env::{EnvSource, home_dir};
use crate::error::ConfigError;

// -------------------------------------------------------------------------
// Rendering helpers (upstream `${value}` mirroring)
// -------------------------------------------------------------------------

/// Renders `f64` values the way JavaScript stringifies numbers in messages
/// (`Infinity`, `NaN`, integers without a trailing `.0`).
pub fn render_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        }
    } else {
        format!("{value}")
    }
}

/// Renders a JSONC value in `${value}` style for error messages. Never
/// includes secrets — config values by definition are not secret refs here.
pub fn render_json_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            let magnitude = n.as_f64();
            match magnitude {
                Some(f) => render_number(f),
                None => "number".to_owned(),
            }
        }
        Value::Array(items) => items
            .iter()
            .map(render_json_value)
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

// -------------------------------------------------------------------------
// opencode timeout family
// -------------------------------------------------------------------------

pub const OPENCODE_TIMEOUT_MS_MIN: u64 = 10_000;
pub const OPENCODE_TIMEOUT_MS_MAX: u64 = 600_000;
pub const OPENCODE_TIMEOUT_MS_DEFAULT: u64 = 90_000;
/// Margin added to the configured timeout for the outer safety-net races:
/// the inner, more informative provider timeout always fires first.
pub const OPENCODE_TIMEOUT_OUTER_MARGIN_MS: u64 = 30_000;

/// Structured-output prompt timeout: non-number/non-finite values fall back
/// to the default (numeric strings are coerced first), otherwise the value
/// is clamped to `[MIN, MAX]`.
pub fn normalize_opencode_timeout_ms(value: Option<&Value>) -> u64 {
    let numeric = match value {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return OPENCODE_TIMEOUT_MS_DEFAULT;
            }
            trimmed.parse::<f64>().ok()
        }
        Some(_) => None,
    };
    match numeric {
        Some(hint) if hint.is_finite() => clamp_timeout(hint),
        _ => OPENCODE_TIMEOUT_MS_DEFAULT,
    }
}

fn clamp_timeout(value: f64) -> u64 {
    (value
        .clamp(
            OPENCODE_TIMEOUT_MS_MIN as f64,
            OPENCODE_TIMEOUT_MS_MAX as f64,
        )
        .round()) as u64
}

/// Outer safety-net timeout wrapping one structured-output call: configured
/// timeout plus a fixed margin.
pub fn outer_structured_output_timeout_ms(configured: Option<&Value>) -> u64 {
    normalize_opencode_timeout_ms(configured) + OPENCODE_TIMEOUT_OUTER_MARGIN_MS
}

// -------------------------------------------------------------------------
// Range-validated numeric normals
// -------------------------------------------------------------------------

/// Valid UTF-8 budget for the auto-capture markdown context: integral and in
/// `[16384, 16 * 1024 * 1024]` inclusive (upstream bounds).
pub fn normalize_auto_capture_max_context_bytes(value: f64) -> Result<u32, ConfigError> {
    if value.is_finite() && value.fract() == 0.0 && (16_384.0..=16_777_216.0).contains(&value) {
        Ok(value as u32)
    } else {
        Err(ConfigError::InvalidValue {
            key: "autoCaptureMaxContextBytes",
            value: render_number(value),
        })
    }
}

/// Automatic memory cleanup retention: finite integral `>= 1`.
///
/// Divergence (documented): values above `u32::MAX` are rejected here although
/// upstream JS only rejects non-safe-integers — absurd budgets can never be
/// meaningful retention periods.
pub fn normalize_auto_cleanup_retention_days(value: f64) -> Result<u32, ConfigError> {
    if value.is_finite() && value.fract() == 0.0 && (1.0..=u32::MAX as f64).contains(&value) {
        Ok(value as u32)
    } else {
        Err(ConfigError::InvalidValue {
            key: "autoCleanupRetentionDays",
            value: render_number(value),
        })
    }
}

/// Embedding vector dimension: integral and within `1..=65_536`.
pub fn normalize_embedding_dimensions(value: f64) -> Result<u32, ConfigError> {
    if value.is_finite() && value.fract() == 0.0 && (1.0..=65_536.0).contains(&value) {
        Ok(value as u32)
    } else {
        Err(ConfigError::InvalidValue {
            key: "embeddingDimensions",
            value: render_number(value),
        })
    }
}

/// Prompt count between automatic user-profile AI cleanup runs: finite
/// integral `>= 1`.
pub fn normalize_user_profile_auto_cleanup_interval(value: f64) -> Result<u32, ConfigError> {
    if value.is_finite() && value.fract() == 0.0 && (1.0..=u32::MAX as f64).contains(&value) {
        Ok(value as u32)
    } else {
        Err(ConfigError::InvalidValue {
            key: "userProfileAutoCleanupInterval",
            value: render_number(value),
        })
    }
}

/// New-tier episodic threshold: finite integral `>= 1`.
pub fn normalize_episodic_consolidation_age_days(value: f64) -> Result<u32, ConfigError> {
    if value.is_finite() && value.fract() == 0.0 && (1.0..=u32::MAX as f64).contains(&value) {
        Ok(value as u32)
    } else {
        Err(ConfigError::InvalidValue {
            key: "episodic.consolidationAgeDays",
            value: render_number(value),
        })
    }
}

/// W7.1 — `promotion.ttlFallbackDays`: an integer in `1..=3650`, rejected
/// (never coerced) otherwise. The error names the camelCase JSONC path.
pub fn normalize_ttl_fallback_days(value: f64) -> Result<u32, ConfigError> {
    if value.is_finite() && value.fract() == 0.0 && (1.0..=3650.0).contains(&value) {
        Ok(value as u32)
    } else {
        Err(ConfigError::InvalidValueExpected {
            key: "promotion.ttlFallbackDays",
            value: render_number(value),
            expected: "an integer between 1 and 3650",
        })
    }
}

// -------------------------------------------------------------------------
// Loose raw-value normalizers
// -------------------------------------------------------------------------

/// Optional dtype for the local ONNX pipeline.
pub fn normalize_embedding_dtype(
    value: Option<&Value>,
) -> Result<Option<LocalOnnxDtype>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            match LocalOnnxDtype::from_str(trimmed) {
                Some(dtype) => Ok(Some(dtype)),
                None => Err(ConfigError::InvalidValueExpected {
                    key: "embeddingDtype",
                    value: trimmed.to_owned(),
                    expected: LOCAL_ONNX_DTYPES_EXPECTED,
                }),
            }
        }
        other => Err(ConfigError::InvalidValue {
            key: "embeddingDtype",
            value: render_json_value(other),
        }),
    }
}

/// Blank/whitespace opencodeVariant values are unset so prompt bodies never
/// carry an empty `variant` key.
pub fn normalize_opencode_variant(value: Option<&Value>) -> Option<String> {
    let Some(Value::String(raw)) = value else {
        return None;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// `memoryTemperature`: number → [`MemoryTemperature::Value`], `false` →
/// [`MemoryTemperature::Disabled`], anything else → key-named error.
pub fn normalize_memory_temperature(
    value: Option<&Value>,
) -> Result<Option<MemoryTemperature>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() => Ok(Some(MemoryTemperature::Value(f))),
            _ => Err(ConfigError::InvalidValue {
                key: "memoryTemperature",
                value: render_number(f64::NAN),
            }),
        },
        Value::Bool(false) => Ok(Some(MemoryTemperature::Disabled)),
        other => Err(ConfigError::InvalidValue {
            key: "memoryTemperature",
            value: render_json_value(other),
        }),
    }
}

// -------------------------------------------------------------------------
// Injection markers
// -------------------------------------------------------------------------

/// Markers that identify a text block as machine-injected rather than typed
/// (upstream `injected-prompt-filter.ts`). The merge semantics land here;
/// the filter functions are the injected-prompt-filter port in
/// `crates/mem-ai/src/privacy/injection.rs`.
///
/// Divergence (issue 50, D-26-8): the first eight entries are upstream's list;
/// `<memory_context>` is the port's own marker, appended for the one frame this
/// engine emits. `<user_profile>` is NOT a marker (transitively covered by the
/// frame — a second emitter cannot exist), and neither is `<memory>`, whose
/// closer appears on every scored row and would end the region after the first.
pub const DEFAULT_INJECTION_MARKERS: &[&str] = &[
    "<system-reminder>",
    "<!-- OMO_INTERNAL_INITIATOR -->",
    "<!-- OMO_INTERNAL_NOREPLY -->",
    "[SYSTEM DIRECTIVE: OH-MY-OPENCODE",
    "[Agent Usage Reminder]",
    "[Category+Skill Reminder]",
    "<team_mode_status",
    "<auto-slash-command>",
    "<memory_context>",
];

const MARKERS_KEY: &str = "chatMessage.injectionMarkers";
const MARKERS_ENTRY_KEY: &str = "chatMessage.injectionMarkers entry";

/// User-supplied markers are appended to the built-in marker set (case
/// insensitive among the first), never replacing it — adding one marker
/// cannot silently disable protection against the others.
pub fn normalize_injection_markers(value: Option<&Value>) -> Result<Vec<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(DEFAULT_INJECTION_MARKERS
            .iter()
            .map(|m| (*m).to_owned())
            .collect());
    };
    let Some(entries) = value.as_array() else {
        return Err(ConfigError::InvalidValueExpected {
            key: MARKERS_KEY,
            value: render_json_value(value),
            expected: "an array",
        });
    };

    let mut markers: Vec<String> = DEFAULT_INJECTION_MARKERS
        .iter()
        .map(|m| (*m).to_owned())
        .collect();
    let mut seen: BTreeSet<String> = markers.iter().map(|m| m.to_lowercase()).collect();
    for entry in entries {
        let Some(raw) = entry.as_str() else {
            return Err(ConfigError::InvalidValueExpected {
                key: MARKERS_ENTRY_KEY,
                value: render_json_value(entry),
                expected: "a string",
            });
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_lowercase()) {
            markers.push(trimmed.to_owned());
        }
    }
    Ok(markers)
}

// -------------------------------------------------------------------------
// Model presets + dimension map
// -------------------------------------------------------------------------

pub const QWEN3_EMBEDDING_QUERY_PREFIX: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ";

/// Known local embedding model presets (substring match on `embeddingModel`).
/// Explicit config keys always win over these defaults.
pub fn embedding_model_preset(model: &str) -> Option<EmbeddingModelPreset> {
    let id = model.to_lowercase();
    if id.contains("bge-m3") {
        return Some(EmbeddingModelPreset {
            pooling: EmbeddingPooling::Cls,
            query_prefix: Some(String::new()),
            document_prefix: Some(String::new()),
        });
    }
    if id.contains("multilingual-e5") || id.contains("e5-") {
        return Some(EmbeddingModelPreset {
            pooling: EmbeddingPooling::Mean,
            query_prefix: Some("query: ".to_owned()),
            document_prefix: Some("passage: ".to_owned()),
        });
    }
    if id.contains("qwen3-embedding") {
        return Some(EmbeddingModelPreset {
            pooling: EmbeddingPooling::LastToken,
            query_prefix: Some(QWEN3_EMBEDDING_QUERY_PREFIX.to_owned()),
            document_prefix: Some(String::new()),
        });
    }
    if id.contains("jina-embeddings") {
        return Some(EmbeddingModelPreset {
            pooling: EmbeddingPooling::Mean,
            query_prefix: Some(String::new()),
            document_prefix: Some(String::new()),
        });
    }
    if id.contains("nomic-embed") {
        return Some(EmbeddingModelPreset {
            pooling: EmbeddingPooling::Mean,
            query_prefix: None,
            document_prefix: None,
        });
    }
    None
}

/// Auto-detected vector dimension: exact-case match in the upstream
/// dimension map; any unknown model defaults to 768.
pub fn embedding_dimensions_for_model(model: &str) -> u32 {
    match model {
        // Local Xenova models
        "Xenova/nomic-embed-text-v1" => 768,
        "Xenova/nomic-embed-text-v1-unsupervised" => 768,
        "Xenova/nomic-embed-text-v1-ablated" => 768,
        "Xenova/jina-embeddings-v2-base-en" => 768,
        "Xenova/jina-embeddings-v2-base-zh" => 768,
        "Xenova/jina-embeddings-v2-base-de" => 768,
        "Xenova/jina-embeddings-v2-small-en" => 512,
        "Xenova/all-MiniLM-L6-v2" => 384,
        "Xenova/all-MiniLM-L12-v2" => 384,
        "Xenova/all-mpnet-base-v2" => 768,
        "Xenova/bge-base-en-v1.5" => 768,
        "Xenova/bge-small-en-v1.5" => 384,
        "Xenova/bge-m3" => 1024,
        "Xenova/gte-small" => 384,
        "Xenova/GIST-small-Embedding-v0" => 384,
        "Xenova/text-embedding-ada-002" => 1536,

        // Other local / ONNX community models
        "intfloat/multilingual-e5-large" => 1024,
        "intfloat/multilingual-e5-base" => 768,
        "intfloat/e5-large-v2" => 1024,
        "onnx-community/Qwen3-Embedding-0.6B-ONNX" => 1024,

        // OpenAI API models
        "text-embedding-3-small" => 1536,
        "text-embedding-3-large" => 3072,
        "text-embedding-ada-002" => 1536,

        // Cohere API models
        "embed-english-v3.0" => 1024,
        "embed-multilingual-v3.0" => 1024,
        "embed-english-light-v3.0" => 384,
        "embed-multilingual-light-v3.0" => 384,

        // Google API models
        "text-embedding-004" => 768,
        "text-multilingual-embedding-002" => 768,

        // Voyage AI models
        "voyage-3" => 1024,
        "voyage-3-lite" => 512,
        "voyage-code-3" => 1024,

        _ => 768,
    }
}

// -------------------------------------------------------------------------
// Paths + secrets
// -------------------------------------------------------------------------

/// `~/x` → `home/x`; `~` → `home`; everything else as-is.
pub fn expand_path(path: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if path == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(path)
    }
}

/// Resolves a secret ref: literal / `env://NAME` / `file://path`.
///
/// Errors name the variable or file path, never the resolved value. File
/// permission checks and key auto-creation are later waves' scope.
pub fn resolve_secret_value(
    value: Option<&str>,
    env: &dyn EnvSource,
) -> Result<Option<String>, ConfigError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }

    if let Some(file_ref) = raw.strip_prefix("file://") {
        let home = home_dir(env);
        let needs_home = file_ref.starts_with("~/") || file_ref == "~";
        let expanded = match (&home, needs_home) {
            (Some(home), _) => expand_path(file_ref, home),
            (None, true) => return Err(ConfigError::MissingHomeDirectory),
            (None, false) => PathBuf::from(file_ref),
        };

        if !expanded.exists() {
            return Err(ConfigError::SecretFileNotFound { path: expanded });
        }
        let bytes = std::fs::read(&expanded).map_err(|source| ConfigError::SecretFileRead {
            path: expanded.clone(),
            source,
        })?;
        // Lossy UTF-8 mirrors Node's replacement decoding; the value is
        // trimmed of surrounding whitespace/newlines.
        let content = String::from_utf8_lossy(&bytes).trim().to_owned();
        return Ok(Some(content));
    }

    if let Some(name) = raw.strip_prefix("env://") {
        return match env.get(name) {
            Some(resolved) if !resolved.is_empty() => Ok(Some(resolved)),
            _ => Err(ConfigError::SecretEnvNotFound {
                name: name.to_owned(),
            }),
        };
    }

    Ok(Some(raw.to_owned()))
}
