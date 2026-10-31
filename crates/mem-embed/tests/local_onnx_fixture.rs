//! Local ONNX embedder end-to-end (blueprint step 5): generated fixture bytes
//! through the real ort session, dtype/weights selection, mask synthesis,
//! hub-fetch fallback and the model-id path wall.
//!
//! No network, no committed ONNX binary: `common::onnx_fixture` generates the
//! model at test time and the committed `tiny_tokenizer.json` provides the
//! ids. Expected vectors are computed here from the fixture formula (mean
//! pooling over the token rows, then L2 normalization) — never from the
//! implementation under test.

mod common;

use std::sync::{Arc, Mutex};

use mem_core::config::{EmbeddingPooling, LocalOnnxDtype};
use mem_embed::Embedder;
use mem_embed::error::EmbedError;
use mem_embed::hub::HubTransport;
use mem_embed::local::{LocalLoadOptions, LocalOnnxEmbedder};

use common::onnx_fixture;

const MODEL: &str = "org/tiny-embed";

/// Token ids for `"hello world"` under the committed fixture tokenizer
/// (`[CLS] hello world [SEP]`).
const HELLO_WORLD_IDS: [i64; 4] = [1, 3, 4, 2];
/// Token ids for `"foo bar baz"` (one position longer: dynamic `seq`).
const FOO_BAR_BAZ_IDS: [i64; 5] = [1, 5, 6, 7, 2];

/// Populate `{cache}/{MODEL}` with one weight file, the tokenizer fixture and
/// a minimal `config.json`.
fn install(cache: &common::TempDir, weights_file: &str, scale: f32, with_attention_mask: bool) {
    let model_dir = cache.path().join(MODEL);
    onnx_fixture::write_tiny_model(&model_dir, weights_file, scale, with_attention_mask);
    std::fs::copy(
        onnx_fixture::tiny_tokenizer_path(),
        model_dir.join("tokenizer.json"),
    )
    .expect("copy committed tokenizer fixture");
    common::write_file(&model_dir, "config.json", r#"{"model_type":"tiny"}"#);
}

fn embedder_for(
    cache: &common::TempDir,
    model: &str,
    dtype: Option<LocalOnnxDtype>,
    hub: Option<Arc<dyn HubTransport>>,
) -> LocalOnnxEmbedder {
    LocalOnnxEmbedder::new(LocalLoadOptions {
        model: model.to_string(),
        cache_root: cache.path().to_path_buf(),
        dtype,
        pooling: EmbeddingPooling::Mean,
        intra_threads: Some(1),
        hub,
        max_tokens: None,
    })
}

fn embedder(
    cache: &common::TempDir,
    dtype: Option<LocalOnnxDtype>,
    hub: Option<Arc<dyn HubTransport>>,
) -> LocalOnnxEmbedder {
    embedder_for(cache, MODEL, dtype, hub)
}

/// Expected embedding: mean of the fixture rows for `ids`, L2-normalized.
fn expected_mean_normalized(ids: &[i64], scale: f32) -> Vec<f32> {
    let rows: Vec<Vec<f32>> = ids
        .iter()
        .map(|id| onnx_fixture::fixture_row(*id, scale))
        .collect();
    let mut mean = [0.0f32; onnx_fixture::FIXTURE_HIDDEN];
    for row in &rows {
        for (accumulator, value) in mean.iter_mut().zip(row) {
            *accumulator += value / rows.len() as f32;
        }
    }
    let norm = mean.iter().map(|value| value * value).sum::<f32>().sqrt();
    mean.iter().map(|value| value / norm).collect()
}

fn assert_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len(), "length mismatch");
    for (index, (got, want)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (got - want).abs() <= 1e-5,
            "component {index}: expected {want}, got {got} ({actual:?} vs {expected:?})"
        );
    }
}

fn unit_norm(vector: &[f32]) -> f32 {
    vector.iter().map(|value| value * value).sum::<f32>().sqrt()
}

#[tokio::test]
async fn test_fixture_model_embeds_int64_tokens_through_gather() {
    let cache = common::temp_dir("onnx-fixture-baseline");
    install(&cache, "onnx/model.onnx", 1.0, false);

    let vector = embedder(&cache, None, None)
        .embed("hello world")
        .await
        .expect("embed through the generated Gather model");

    assert_eq!(vector.len(), onnx_fixture::FIXTURE_HIDDEN);
    assert_close(&vector, &expected_mean_normalized(&HELLO_WORLD_IDS, 1.0));
}

#[tokio::test]
async fn test_fixture_model_accepts_dynamic_sequence_lengths() {
    let cache = common::temp_dir("onnx-fixture-dynamic-seq");
    install(&cache, "onnx/model.onnx", 1.0, false);
    let embedder = embedder(&cache, None, None);

    let short = embedder.embed("hello world").await.expect("4-token input");
    let long = embedder.embed("foo bar baz").await.expect("5-token input");

    assert_close(&short, &expected_mean_normalized(&HELLO_WORLD_IDS, 1.0));
    assert_close(&long, &expected_mean_normalized(&FOO_BAR_BAZ_IDS, 1.0));
    assert_ne!(short, long);
}

#[tokio::test]
async fn test_fixture_model_with_attention_mask_variant_feeds_mask() {
    let plain = common::temp_dir("onnx-fixture-no-mask");
    install(&plain, "onnx/model.onnx", 1.0, false);
    let masked = common::temp_dir("onnx-fixture-with-mask");
    install(&masked, "onnx/model.onnx", 1.0, true);

    // The mask-declaring variant requires an `attention_mask` feed: the run
    // below only succeeds because the session synthesizes the all-ones mask
    // transformers.js uses for a single unpadded sequence.
    let from_masked = embedder(&masked, None, None)
        .embed("hello world")
        .await
        .expect("mask-declaring variant loads and runs");
    let from_plain = embedder(&plain, None, None)
        .embed("hello world")
        .await
        .expect("baseline variant");

    assert_eq!(from_masked, from_plain);
    assert_close(
        &from_masked,
        &expected_mean_normalized(&HELLO_WORLD_IDS, 1.0),
    );
}

#[tokio::test]
async fn test_fixture_model_with_token_type_ids_variant_feeds_declared_type_ids() {
    let cache = common::temp_dir("onnx-fixture-token-type-ids");
    let model_dir = cache.path().join(MODEL);
    onnx_fixture::write_tiny_model_with_token_type_ids(&model_dir, "onnx/model.onnx", 1.0);
    std::fs::copy(
        onnx_fixture::tiny_tokenizer_path(),
        model_dir.join("tokenizer.json"),
    )
    .expect("copy committed tokenizer fixture");
    common::write_file(&model_dir, "config.json", r#"{"model_type":"tiny"}"#);

    // Regression: the real nomic graph declares `token_type_ids` (the
    // BERT-family name), not `type_ids`. The run below only succeeds because
    // the session feeds the declared type-id input with the all-zeros sequence
    // transformers.js uses — before that alias was handled, loading the real
    // model failed with `Missing Input: token_type_ids`.
    let vector = embedder(&cache, None, None)
        .embed("hello world")
        .await
        .expect("token_type_ids-declaring variant loads and runs");

    assert_close(&vector, &expected_mean_normalized(&HELLO_WORLD_IDS, 1.0));
}

#[tokio::test]
async fn test_fixture_embedder_l2_normalizes_output() {
    let cache = common::temp_dir("onnx-fixture-normalized");
    install(&cache, "onnx/model.onnx", 1.0, false);

    let vector = embedder(&cache, None, None)
        .embed("hello world")
        .await
        .expect("embed");

    assert!((unit_norm(&vector) - 1.0).abs() <= 1e-5, "norm {vector:?}");
}

#[tokio::test]
async fn test_loader_uses_cached_quantized_weights_when_dtype_unset() {
    let cache = common::temp_dir("onnx-fixture-quantized-cache");
    // Only the quantized weights exist, and they carry a *negative* scale:
    // a uniform scale cancels under L2 normalization, so only the sign can
    // prove which file's bytes were read.
    install(&cache, "onnx/model_quantized.onnx", -1.0, false);

    let vector = embedder(&cache, None, None)
        .embed("hello world")
        .await
        .expect("probe finds the cached quantized weights");

    assert_close(&vector, &expected_mean_normalized(&HELLO_WORLD_IDS, -1.0));
    // Explicitly: the negation of the fp32(-less) expectation — the fp32 file
    // does not exist and could not have produced this.
    let negated: Vec<f32> = expected_mean_normalized(&HELLO_WORLD_IDS, 1.0)
        .iter()
        .map(|value| -value)
        .collect();
    assert_close(&vector, &negated);
}

#[tokio::test]
async fn test_loader_honours_explicit_dtype_over_cache_probe() {
    let cache = common::temp_dir("onnx-fixture-explicit-dtype");
    install(&cache, "onnx/model.onnx", 1.0, false);
    install(&cache, "onnx/model_fp16.onnx", -2.0, false);

    let vector = embedder(&cache, Some(LocalOnnxDtype::Fp16), None)
        .embed("hello world")
        .await
        .expect("explicit dtype wins over the probe");

    // The fp16 file's negative scale shows up, the fp32 file's does not.
    assert_close(&vector, &expected_mean_normalized(&HELLO_WORLD_IDS, -2.0));
}

#[tokio::test]
async fn test_loader_reports_missing_files_without_hub() {
    let cache = common::temp_dir("onnx-fixture-missing");
    std::fs::create_dir_all(cache.path().join(MODEL)).expect("empty model dir");

    let error = embedder(&cache, None, None)
        .embed("hello world")
        .await
        .expect_err("nothing cached and no hub to fetch from");

    assert!(
        matches!(error, EmbedError::ModelFilesMissing { .. }),
        "{error:?}"
    );
    let display = error.to_string();
    assert!(display.contains(MODEL), "{display}");
    assert!(display.contains("onnx/model.onnx"), "{display}");
}

/// Fake hub serving exactly the three files a bare cache needs.
#[derive(Debug)]
struct ModelHub {
    calls: Arc<Mutex<Vec<String>>>,
    scale: f32,
}

impl HubTransport for ModelHub {
    fn get(&self, url: &str) -> mem_embed::BoxFuture<'_, Result<Vec<u8>, EmbedError>> {
        let url = url.to_string();
        Box::pin(async move {
            self.calls.lock().expect("calls lock").push(url.to_string());
            if url.ends_with("onnx/model.onnx") {
                Ok(onnx_fixture::tiny_model_bytes(self.scale, false))
            } else if url.ends_with("tokenizer.json") {
                Ok(onnx_fixture::tiny_tokenizer_bytes())
            } else if url.ends_with("config.json") {
                Ok(b"{}".to_vec())
            } else {
                Err(EmbedError::HubHttp {
                    url: url.to_string(),
                    status: 404,
                })
            }
        })
    }
}

#[tokio::test]
async fn test_loader_fetches_missing_files_via_hub_transport() {
    let cache = common::temp_dir("onnx-fixture-hub-fetch");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let hub = ModelHub {
        calls: calls.clone(),
        scale: 1.0,
    };
    let hub: Arc<dyn HubTransport> = Arc::new(hub);

    let vector = embedder(&cache, None, Some(hub))
        .embed("hello world")
        .await
        .expect("fetch the missing files, then load");

    assert_close(&vector, &expected_mean_normalized(&HELLO_WORLD_IDS, 1.0));
    let urls = calls.lock().expect("calls lock").clone();
    assert_eq!(urls.len(), 3, "{urls:?}");
    assert!(urls.iter().any(|url| url.contains("tokenizer.json")));
    assert!(urls.iter().any(|url| url.contains("onnx/model.onnx")));
    assert!(cache.path().join(MODEL).join("onnx/model.onnx").exists());
    assert!(cache.path().join(MODEL).join("tokenizer.json").exists());
    assert!(cache.path().join(MODEL).join("config.json").exists());
}

#[tokio::test]
async fn test_loader_rejects_model_id_traversal() {
    let cache = common::temp_dir("onnx-fixture-traversal");
    let embedder = embedder_for(&cache, "../escape", None, None);

    let error = embedder
        .embed("hello world")
        .await
        .expect_err("a traversal model id must never reach the filesystem");
    assert!(
        matches!(error, EmbedError::InvalidModelId { .. }),
        "{error:?}"
    );

    // The failure is sticky: warmup reports the same wall without retrying.
    let warmup = embedder.warmup().await.expect_err("warmup reports it too");
    assert!(
        matches!(warmup, EmbedError::InvalidModelId { .. }),
        "{warmup:?}"
    );
}

#[tokio::test]
async fn test_loader_reports_missing_weights_for_explicit_dtype() {
    let cache = common::temp_dir("onnx-fixture-missing-fp16");
    install(&cache, "onnx/model.onnx", 1.0, false);

    let error = embedder(&cache, Some(LocalOnnxDtype::Fp16), None)
        .embed("hello world")
        .await
        .expect_err("the explicitly requested weights are absent");

    assert!(
        matches!(error, EmbedError::ModelFilesMissing { .. }),
        "{error:?}"
    );
    let display = error.to_string();
    assert!(display.contains("onnx/model_fp16.onnx"), "{display}");
    assert!(display.contains(MODEL), "{display}");
}
