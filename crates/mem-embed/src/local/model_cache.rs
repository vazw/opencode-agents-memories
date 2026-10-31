//! Model-cache layout: which ONNX weight variant is already cached for a
//! model, and whether `config.json` declares one.
//!
//! Port of `readDeclaredDtype` / `detectCachedDtype` / `resolveLocalOnnxDtype`
//! from upstream `src/services/embedding.ts`, including the pinned precedence:
//!
//! 1. a declared **string** dtype in `transformers.js_config.dtype` wins;
//! 2. a declared **per-file object** map disables the probe entirely (passing
//!    `dtype` would override the per-file config transformers.js resolves);
//! 3. otherwise the first cached `onnx/model{suffix}.onnx` in
//!    [`ONNX_DTYPE_VARIANTS`] order decides.
//!
//! Divergences from upstream (recorded): an unknown declared string falls
//! through to the cache probe instead of being forwarded and failing later,
//! and the result is an `Option` (the `"auto"` member is never produced).

use std::path::{Path, PathBuf};

pub use mem_core::config::LocalOnnxDtype;

use crate::error::EmbedError;

/// ONNX weight variants in descending precision, mirroring transformers.js'
/// `DEFAULT_DTYPE_SUFFIX_MAPPING` (`(dtype, file-name suffix)`; fp32 has no
/// suffix). The order is part of the contract: the probe returns the first hit.
pub const ONNX_DTYPE_VARIANTS: [(&str, &str); 12] = [
    ("fp32", ""),
    ("fp16", "_fp16"),
    ("q8", "_quantized"),
    ("int8", "_int8"),
    ("uint8", "_uint8"),
    ("q4", "_q4"),
    ("q4f16", "_q4f16"),
    ("bnb4", "_bnb4"),
    ("q2", "_q2"),
    ("q2f16", "_q2f16"),
    ("q1", "_q1"),
    ("q1f16", "_q1f16"),
];

/// Concrete weight files, keyed by dtype (the name each dtype maps to under
/// `{model}/onnx/`).
const WEIGHTS_FILES: [(LocalOnnxDtype, &str); 12] = [
    (LocalOnnxDtype::Fp32, "onnx/model.onnx"),
    (LocalOnnxDtype::Fp16, "onnx/model_fp16.onnx"),
    (LocalOnnxDtype::Q8, "onnx/model_quantized.onnx"),
    (LocalOnnxDtype::Int8, "onnx/model_int8.onnx"),
    (LocalOnnxDtype::Uint8, "onnx/model_uint8.onnx"),
    (LocalOnnxDtype::Q4, "onnx/model_q4.onnx"),
    (LocalOnnxDtype::Q4F16, "onnx/model_q4f16.onnx"),
    (LocalOnnxDtype::Bnb4, "onnx/model_bnb4.onnx"),
    (LocalOnnxDtype::Q2, "onnx/model_q2.onnx"),
    (LocalOnnxDtype::Q2F16, "onnx/model_q2f16.onnx"),
    (LocalOnnxDtype::Q1, "onnx/model_q1.onnx"),
    (LocalOnnxDtype::Q1F16, "onnx/model_q1f16.onnx"),
];

/// `{cacheRoot}/{model}` — the transformers.js cache directory for a model id.
pub fn model_cache_root(model: &str, cache_root: &Path) -> PathBuf {
    cache_root.join(model)
}

/// What `config.json` declares about the dtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclaredDtype {
    /// A known concrete dtype string (outranks the cache probe).
    Declared(LocalOnnxDtype),
    /// A per-file `{ model: "q8", ... }` map — never override it.
    PerFileObject,
    /// Missing, unreadable, malformed, blank or of another JSON type.
    Absent,
}

/// Read `transformers.js_config.dtype` from the cached `config.json`.
fn read_declared_dtype(model: &str, cache_root: &Path) -> DeclaredDtype {
    let Ok(text) = std::fs::read_to_string(model_cache_root(model, cache_root).join("config.json"))
    else {
        return DeclaredDtype::Absent;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
        return DeclaredDtype::Absent;
    };
    let Some(declared) = parsed
        .get("transformers.js_config")
        .and_then(|config| config.get("dtype"))
    else {
        return DeclaredDtype::Absent;
    };
    match declared {
        serde_json::Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return DeclaredDtype::Absent;
            }
            // Unknown strings fall through to the probe (recorded divergence).
            match LocalOnnxDtype::from_str(trimmed) {
                Some(dtype) => DeclaredDtype::Declared(dtype),
                None => DeclaredDtype::Absent,
            }
        }
        // Note: arrays are excluded, exactly like upstream's
        // `declared && typeof declared === "object" && !Array.isArray(declared)`.
        serde_json::Value::Object(_) => DeclaredDtype::PerFileObject,
        _ => DeclaredDtype::Absent,
    }
}

/// The dtype whose ONNX weights are already cached, per the pinned variant
/// order (so a quantized-only repo is not asked for fp32 weights).
fn detect_cached_dtype(model: &str, cache_root: &Path) -> Option<LocalOnnxDtype> {
    let onnx_dir = model_cache_root(model, cache_root).join("onnx");
    ONNX_DTYPE_VARIANTS
        .iter()
        .find_map(|(dtype, suffix)| {
            let candidate = onnx_dir.join(format!("model{suffix}.onnx"));
            candidate.exists().then_some(*dtype)
        })
        .and_then(LocalOnnxDtype::from_str)
}

/// Resolve the concrete dtype for a cached model, or `None` to leave dtype
/// selection to the loader's default (`Auto` is never produced).
pub fn resolve_local_onnx_dtype(model: &str, cache_root: &Path) -> Option<LocalOnnxDtype> {
    match read_declared_dtype(model, cache_root) {
        DeclaredDtype::Declared(dtype) => Some(dtype),
        DeclaredDtype::PerFileObject => None,
        DeclaredDtype::Absent => detect_cached_dtype(model, cache_root),
    }
}

/// Read the token position limit the cached `config.json` declares.
///
/// Collects every present, positive-integer value among the root
/// `max_position_embeddings` / `n_positions` keys and the same keys under
/// `text_config`, and returns the **minimum** of those present (`None` when the
/// file is missing/unparseable or declares none). A present but malformed value
/// (string, float, zero, negative) is treated as absent and reported through
/// `mem_core::logging::log` as it is read: each call reads the file once and
/// reports each malformed declaration it encounters on that read, so no
/// once-mechanism is implied or needed.
///
/// Callers must validate the model id first (`local::validate_model_id`) —
/// that is the path-traversal wall for this read.
pub fn read_declared_position_limit(model: &str, cache_root: &Path) -> Option<usize> {
    let Ok(text) = std::fs::read_to_string(model_cache_root(model, cache_root).join("config.json"))
    else {
        return None;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
        return None;
    };
    let mut limits = Vec::new();
    for container in [Some(&parsed), parsed.get("text_config")]
        .into_iter()
        .flatten()
    {
        for key in ["max_position_embeddings", "n_positions"] {
            if let Some(limit) = declared_position_value(model, key, container) {
                limits.push(limit);
            }
        }
    }
    limits.into_iter().min()
}

/// One declaration: a positive integer, or `None` (absent or malformed —
/// malformed is reported through the logging facade, never an error).
fn declared_position_value(model: &str, key: &str, container: &serde_json::Value) -> Option<usize> {
    let value = container.get(key)?;
    match value.as_u64() {
        Some(limit) if limit > 0 => usize::try_from(limit).ok(),
        _ => {
            mem_core::logging::log(
                "embedding.position_limit_ignored",
                Some(&serde_json::json!({
                    "model": model,
                    "key": key,
                    "value": value,
                })),
            );
            None
        }
    }
}

/// Resolve the token cap this build feeds the model, from the declared limit,
/// the tokenizer's special-token count and the optional configured override.
///
/// `Ok(None)` means unbounded. The override is never clamped: it is refused
/// loudly when it exceeds a declared limit. A cap that cannot keep both
/// boundary special tokens is refused at load on **both** paths — a configured
/// override below `specials + 1` (a configured cap must keep both specials plus
/// a content token, so a configured 2 is refused and `cap == 2` stays
/// model-only), and a declaration below `specials + 2` when no override is set.
/// Every configuration-reachable cap therefore keeps both boundary special
/// tokens, and a configured one keeps a content token too, so no configuration
/// yields a `[SEP]`-only vector. Otherwise the cap is the override, or
/// `declared - specials`.
///
/// The signature carries no model id (this is a pure policy function); the
/// refusals name their source (`config.json` / `embeddingMaxTokens`) and both
/// numbers.
pub fn effective_max_tokens(
    declared: Option<usize>,
    special_tokens: usize,
    configured: Option<u32>,
) -> Result<Option<usize>, EmbedError> {
    if let Some(configured) = configured
        && (configured as usize) < special_tokens + 1
    {
        return Err(EmbedError::ModelLoad {
            model: "embeddingMaxTokens".to_string(),
            message: format!(
                "{}; raise embeddingMaxTokens to at least {}, or unset it",
                cap_refusal(
                    "configured",
                    configured as usize,
                    &format!(
                        "both boundary special tokens plus a content token (needs at least {} tokens)",
                        special_tokens + 1
                    )
                ),
                special_tokens + 1
            ),
        });
    }
    match (declared, configured) {
        (Some(model_limit), Some(configured)) => {
            let configured_usize = configured as usize;
            if configured_usize > model_limit {
                return Err(EmbedError::MaxTokensExceedsModelLimit {
                    configured,
                    model_limit,
                });
            }
            Ok(Some(configured_usize))
        }
        (None, Some(configured)) => Ok(Some(configured as usize)),
        (Some(model_limit), None) => {
            if model_limit < special_tokens + 2 {
                return Err(EmbedError::ModelLoad {
                    model: "config.json".to_string(),
                    message: format!(
                        "the cached config.json declares {model_limit} token positions, so {}; \
                         provision a model with a larger position limit",
                        cap_refusal(
                            "derived",
                            model_limit.saturating_sub(special_tokens),
                            &format!(
                                "both boundary special tokens (needs at least {} positions)",
                                special_tokens + 2
                            )
                        )
                    ),
                });
            }
            Ok(Some(model_limit - special_tokens))
        }
        (None, None) => Ok(None),
    }
}

/// The sentence both refusals share: the resolved cap would be `cap` token(s),
/// and that cannot keep what this lane needs. Each arm supplies its own
/// requirement — the configured arm needs both boundary special tokens **plus a
/// content token** (an operator can always pick that, so a configured cap of 2
/// is refused), the derived arm needs both boundary special tokens only, which
/// is why `cap == 2` stays reachable by a model declaring exactly
/// `specials + 2` positions. The arithmetic differs; the rule does not.
fn cap_refusal(kind: &str, cap: usize, requirement: &str) -> String {
    format!("the {kind} cap would be {cap} token(s), which cannot keep {requirement}")
}

/// The cache-relative weights file for a dtype, or `None` for
/// [`LocalOnnxDtype::Auto`] (which is resolution input, not a concrete file).
pub fn weights_file_name(dtype: LocalOnnxDtype) -> Option<&'static str> {
    WEIGHTS_FILES
        .iter()
        .find_map(|(candidate, file)| (*candidate == dtype).then_some(*file))
}
