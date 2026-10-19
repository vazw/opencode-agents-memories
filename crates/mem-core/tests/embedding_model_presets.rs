//! Port of upstream `embedding-model-presets.test.ts` — the first ten
//! cases mirror it case-for-case:
//!
//! | upstream case                                            | Rust test |
//! |----------------------------------------------------------|-----------|
//! | mean pooling for nomic without forcing task prefixes      | `test_preset_nomic_mean_pooling_without_forced_prefixes` |
//! | cls pooling and empty prefixes for bge-m3                 | `test_preset_bge_m3_cls_with_empty_prefixes` |
//! | e5 query/passage prefixes                                 | `test_preset_e5_query_and_passage_prefixes` |
//! | last_token + instruct query prefix for Qwen3-Embedding    | `test_preset_qwen3_last_token_and_instruct_query_prefix` |
//! | mean pooling and empty prefixes for jina                  | `test_preset_jina_mean_with_empty_prefixes` |
//! | undefined for unknown models                              | `test_preset_unknown_model_returns_none` |
//! | accepts mean, cls, and last_token                         | `test_normalize_pooling_accepts_mean_cls_last_token` |
//! | rejects invalid values                                    | `test_normalize_pooling_rejects_invalid_values_with_offending_key` |
//! | accepts known dtypes and trims whitespace                 | `test_normalize_dtype_accepts_known_values_and_trims_whitespace` |
//! | rejects unknown dtypes                                    | `test_normalize_dtype_rejects_unknown_value` |
//!
//! The eleventh case (`test_resolved_config_applies_preset_defaults_for_qwen3`)
//! is the resolved-config integration the blueprint adds: the preset must
//! survive `Config::load` end-to-end.
//!
//! Upstream's `normalizeEmbeddingPooling` is exposed here through the public
//! deserialize seam (`EmbeddingPooling: Deserialize`); the numeric-rejection
//! half of the upstream case goes through `Config::load` so the error carries
//! the key path (serde_path_to_error), mirroring `toThrow(/embeddingPooling/)`.

use std::collections::BTreeMap;

use mem_core::config::{
    Config, ConfigPaths, EmbeddingPooling, LocalOnnxDtype, embedding_model_preset,
    normalize_embedding_dtype,
};
use serde_json::json;

mod common;

fn preset_of(model: &str) -> Option<mem_core::config::EmbeddingModelPreset> {
    embedding_model_preset(model)
}

#[test]
fn test_preset_nomic_mean_pooling_without_forced_prefixes() {
    let preset = preset_of("Xenova/nomic-embed-text-v1").expect("nomic has a preset");
    assert_eq!(preset.pooling, EmbeddingPooling::Mean);
    // nomic opts into task prefixes only via embeddingUseTaskPrefixes:
    // the preset itself forces no prefixes.
    assert_eq!(preset.query_prefix, None);
    assert_eq!(preset.document_prefix, None);
}

#[test]
fn test_preset_bge_m3_cls_with_empty_prefixes() {
    let preset = preset_of("Xenova/bge-m3").expect("bge-m3 has a preset");
    assert_eq!(preset.pooling, EmbeddingPooling::Cls);
    assert_eq!(preset.query_prefix, Some(String::new()));
    assert_eq!(preset.document_prefix, Some(String::new()));
}

#[test]
fn test_preset_e5_query_and_passage_prefixes() {
    let preset = preset_of("intfloat/multilingual-e5-large").expect("e5 has a preset");
    assert_eq!(preset.pooling, EmbeddingPooling::Mean);
    assert_eq!(preset.query_prefix, Some("query: ".to_owned()));
    assert_eq!(preset.document_prefix, Some("passage: ".to_owned()));

    assert_eq!(
        preset_of("intfloat/e5-large-v2")
            .and_then(|preset| preset.query_prefix)
            .as_deref(),
        Some("query: ")
    );
}

#[test]
fn test_preset_qwen3_last_token_and_instruct_query_prefix() {
    let preset = preset_of("onnx-community/Qwen3-Embedding-0.6B-ONNX").expect("qwen3 has a preset");
    assert_eq!(preset.pooling, EmbeddingPooling::LastToken);
    assert_eq!(preset.document_prefix, Some(String::new()));
    let query = preset.query_prefix.expect("qwen3 query prefix is set");
    assert!(query.contains("Instruct:"), "query prefix: {query}");
    assert!(query.contains("Query: "), "query prefix: {query}");
}

#[test]
fn test_preset_jina_mean_with_empty_prefixes() {
    let preset = preset_of("Xenova/jina-embeddings-v2-base-en").expect("jina has a preset");
    assert_eq!(preset.pooling, EmbeddingPooling::Mean);
    assert_eq!(preset.query_prefix, Some(String::new()));
    assert_eq!(preset.document_prefix, Some(String::new()));
}

#[test]
fn test_preset_unknown_model_returns_none() {
    assert_eq!(preset_of("Xenova/all-MiniLM-L6-v2"), None);
}

#[test]
fn test_normalize_pooling_accepts_mean_cls_last_token() {
    assert_eq!(
        serde_json::from_str::<EmbeddingPooling>("\"mean\"").unwrap(),
        EmbeddingPooling::Mean
    );
    assert_eq!(
        serde_json::from_str::<EmbeddingPooling>("\"cls\"").unwrap(),
        EmbeddingPooling::Cls
    );
    assert_eq!(
        serde_json::from_str::<EmbeddingPooling>("\"last_token\"").unwrap(),
        EmbeddingPooling::LastToken
    );
}

#[test]
fn test_normalize_pooling_rejects_invalid_values_with_offending_key() {
    // String case: the deserialize seam names the offending key.
    let error = serde_json::from_str::<EmbeddingPooling>("\"max\"")
        .expect_err("\"max\" is not a pooling strategy");
    assert!(
        error.to_string().contains("embeddingPooling"),
        "error must name the key: {error}"
    );

    // Numeric case: through the loading seam, mirroring toThrow(/embeddingPooling/).
    let home = common::unique_temp_dir("presets-home");
    common::write_file(
        &home,
        ".config/opencode/opencode-mem.jsonc",
        &json!({ "embeddingPooling": 1 }).to_string(),
    );
    let env = BTreeMap::new();
    let error = Config::load(&ConfigPaths::standard(&home), &env)
        .expect_err("numeric pooling must be rejected");
    assert!(
        error.to_string().contains("embeddingPooling"),
        "display must name the key path: {error}"
    );
}

#[test]
fn test_normalize_dtype_accepts_known_values_and_trims_whitespace() {
    assert_eq!(
        normalize_embedding_dtype(Some(&json!("q8"))).unwrap(),
        Some(LocalOnnxDtype::Q8)
    );
    assert_eq!(
        normalize_embedding_dtype(Some(&json!(" fp32 "))).unwrap(),
        Some(LocalOnnxDtype::Fp32)
    );
    assert_eq!(normalize_embedding_dtype(Some(&json!(""))).unwrap(), None);
    assert_eq!(normalize_embedding_dtype(Some(&json!(null))).unwrap(), None);
    assert_eq!(normalize_embedding_dtype(None).unwrap(), None);
}

#[test]
fn test_normalize_dtype_rejects_unknown_value() {
    let error = normalize_embedding_dtype(Some(&json!("bfloat16")))
        .expect_err("\"bfloat16\" is not a transformers.js dtype");
    assert!(
        error.to_string().contains("embeddingDtype"),
        "error must name the key: {error}"
    );
}

#[test]
fn test_resolved_config_applies_preset_defaults_for_qwen3() {
    let home = common::unique_temp_dir("presets-home");
    common::write_file(
        &home,
        ".config/opencode/opencode-mem.jsonc",
        &json!({ "embeddingModel": "onnx-community/Qwen3-Embedding-0.6B-ONNX" }).to_string(),
    );
    let env = BTreeMap::new();
    let config = Config::load(&ConfigPaths::standard(&home), &env).expect("load must succeed");

    assert_eq!(config.embedding_pooling, EmbeddingPooling::LastToken);
    let query = config
        .embedding_query_prefix
        .expect("qwen3 preset supplies the instruct query prefix");
    assert!(query.contains("Instruct:"), "query prefix: {query}");
    assert!(query.contains("Query: "), "query prefix: {query}");
    assert_eq!(config.embedding_document_prefix, Some(String::new()));
}
