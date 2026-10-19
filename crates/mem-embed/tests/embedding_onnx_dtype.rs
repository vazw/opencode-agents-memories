//! `resolve_local_onnx_dtype` — port of `tests/embedding-onnx-dtype.test.ts`
//! (blueprint step 3). Case-for-case mapping:
//!
//! | upstream case | here |
//! |---|---|
//! | `detects q8 from model_quantized.onnx when no config dtype is declared` | `test_resolve_dtype_detects_q8_from_quantized_weights` |
//! | `prefers declared string dtype over missing fp32 weights` | `test_resolve_dtype_prefers_declared_string_over_cache_probe` |
//! | `does not override per-file object dtype with a cache probe` | `test_resolve_dtype_object_declaration_disables_the_probe` |
//! | `prefers fp32 when both fp32 and q8 weights are cached` | `test_resolve_dtype_prefers_fp32_when_both_cached` |
//! | `returns undefined for an empty cache` | `test_resolve_dtype_returns_none_for_empty_cache` |
//! | `detects q4f16 from model_q4f16.onnx` | `test_resolve_dtype_detects_q4f16` |

mod common;

use std::path::Path;

use common::{TempDir, temp_dir, write_bytes, write_file};
use mem_core::config::LocalOnnxDtype;
use mem_embed::local::resolve_local_onnx_dtype;

/// Model id from the upstream test (`withCache` layout: `{cacheRoot}/{model}`).
const MODEL: &str = "org/quantized-embed";

/// Mirrors upstream `withCache`: a temp cache root holding `{MODEL}/onnx/`,
/// optional `config.json`, and the given weight files as stubs.
fn with_cache(label: &str, config_json: Option<&str>, onnx_files: &[&str]) -> TempDir {
    let guard = temp_dir(label);
    let model_dir = guard.path().join(MODEL);
    if let Some(config) = config_json {
        write_file(&model_dir, "config.json", config);
    }
    for file in onnx_files {
        write_bytes(&model_dir, &format!("onnx/{file}"), b"stub");
    }
    // Upstream always creates `onnx/` even for the empty-cache case.
    std::fs::create_dir_all(model_dir.join("onnx")).expect("create onnx dir");
    guard
}

fn config_with_dtype(dtype_json: &str) -> String {
    format!(r#"{{"model_type":"bert","transformers.js_config":{{"dtype":{dtype_json}}}}}"#)
}

fn resolve(cache_root: &Path) -> Option<LocalOnnxDtype> {
    resolve_local_onnx_dtype(MODEL, cache_root)
}

#[test]
fn test_resolve_dtype_detects_q8_from_quantized_weights() {
    let cache = with_cache("onnx-dtype-q8", None, &["model_quantized.onnx"]);
    assert_eq!(resolve(cache.path()), Some(LocalOnnxDtype::Q8));
}

#[test]
fn test_resolve_dtype_prefers_declared_string_over_cache_probe() {
    let cache = with_cache(
        "onnx-dtype-declared-string",
        Some(&config_with_dtype(r#""q8""#)),
        &["model_quantized.onnx"],
    );
    assert_eq!(resolve(cache.path()), Some(LocalOnnxDtype::Q8));

    // A declared string outranks the probe even when its weights are absent:
    // the declaration is what the caller asked for.
    let declared_fp16 = with_cache(
        "onnx-dtype-declared-without-weights",
        Some(&config_with_dtype(r#""fp16""#)),
        &["model_quantized.onnx"],
    );
    assert_eq!(resolve(declared_fp16.path()), Some(LocalOnnxDtype::Fp16));
}

#[test]
fn test_resolve_dtype_object_declaration_disables_the_probe() {
    let cache = with_cache(
        "onnx-dtype-object",
        Some(&config_with_dtype(r#"{"model":"q8"}"#)),
        &["model_quantized.onnx"],
    );
    assert_eq!(resolve(cache.path()), None);
}

#[test]
fn test_resolve_dtype_prefers_fp32_when_both_cached() {
    let cache = with_cache(
        "onnx-dtype-fp32-first",
        None,
        &["model.onnx", "model_quantized.onnx"],
    );
    assert_eq!(resolve(cache.path()), Some(LocalOnnxDtype::Fp32));
}

#[test]
fn test_resolve_dtype_returns_none_for_empty_cache() {
    let cache = with_cache("onnx-dtype-empty", None, &[]);
    assert_eq!(resolve(cache.path()), None);
}

#[test]
fn test_resolve_dtype_detects_q4f16() {
    let cache = with_cache("onnx-dtype-q4f16", None, &["model_q4f16.onnx"]);
    assert_eq!(resolve(cache.path()), Some(LocalOnnxDtype::Q4F16));
}
