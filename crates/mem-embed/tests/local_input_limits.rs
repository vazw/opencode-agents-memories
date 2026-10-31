//! Input-limit behaviour of the local embedding lane.
//!
//! Step 2 pins the pure head+tail truncation; later steps add the encode cap,
//! the model-derived limit, the override and the embedder hop. Every test
//! observes a public seam only.

mod common;

use std::collections::BTreeMap;

use mem_core::config::{Config, ConfigPaths, EmbeddingPooling};
use mem_embed::Embedder;
use mem_embed::error::EmbedError;
use mem_embed::local::model_cache::{effective_max_tokens, read_declared_position_limit};
use mem_embed::local::tokenize::{TokenizedInput, encode_text, load_tokenizer, truncate_tokenized};
use mem_embed::local::{LocalLoadOptions, LocalOnnxEmbedder};
use mem_embed::service::EmbeddingServiceOptions;

/// A token-only input (the fixture tokenizer declares no mask/type ids).
fn input(ids: Vec<i64>) -> TokenizedInput {
    TokenizedInput {
        ids,
        attention_mask: None,
        type_ids: None,
        dropped_tokens: 0,
    }
}

#[test]
fn test_truncation_keeps_head_and_tail_and_drops_the_middle() {
    let result = truncate_tokenized(input((1..=10).collect()), 6);

    assert_eq!(
        result.ids,
        vec![1, 2, 3, 8, 9, 10],
        "head = cap/2 and tail = cap - head: the middle is what is dropped"
    );
    assert_eq!(result.dropped_tokens, 4);
}

#[test]
fn test_truncation_keeps_mask_and_type_ids_aligned_with_ids() {
    // Distinct values make the two slice ranges observable, not just lengths.
    let mask: Vec<i64> = (1..=20).map(|value| value * 10).collect();
    let types: Vec<i64> = (1..=20).map(|value| value + 100).collect();
    let result = truncate_tokenized(
        TokenizedInput {
            ids: (1..=20).collect(),
            attention_mask: Some(mask),
            type_ids: Some(types),
            dropped_tokens: 0,
        },
        8,
    );

    assert_eq!(result.ids, vec![1, 2, 3, 4, 17, 18, 19, 20]);
    assert_eq!(
        result.attention_mask.expect("mask survives"),
        vec![10, 20, 30, 40, 170, 180, 190, 200]
    );
    assert_eq!(
        result.type_ids.expect("type ids survive"),
        vec![101, 102, 103, 104, 117, 118, 119, 120]
    );
    assert_eq!(result.dropped_tokens, 12);
}

#[test]
fn test_truncation_is_a_noop_and_idempotent_at_the_cap() {
    let ids: Vec<i64> = (1..=8).collect();
    let once = truncate_tokenized(input(ids.clone()), 8);

    assert_eq!(once.ids, ids, "an input of exactly cap ids is untouched");
    assert_eq!(once.dropped_tokens, 0, "the boundary is `>`, never `>=`");

    let twice = truncate_tokenized(once.clone(), 8);
    assert_eq!(twice.ids, once.ids, "idempotent at the same cap");
    assert_eq!(twice.dropped_tokens, 0);
}

#[test]
fn test_truncation_floors_a_zero_cap_at_one_token() {
    let result = truncate_tokenized(input((1..=20).collect()), 0);

    assert_eq!(
        result.ids,
        vec![20],
        "the floor keeps the TAIL, so the last id survives"
    );
    assert_eq!(result.dropped_tokens, 19);
}

#[test]
fn test_truncation_cap_two_keeps_both_specials() {
    let result = truncate_tokenized(input(vec![1, 3, 4, 2]), 2);

    assert_eq!(
        result.ids,
        vec![1, 2],
        "the documented degenerate pair: [CLS][SEP]"
    );
    assert_eq!(result.dropped_tokens, 2);
}

/// The failing input itself: 2,000 words plus the template's two specials
/// (Step 1 measured `PRE_CHANGE_IDS=2002`).
fn failing_input() -> String {
    let text = "hello ".repeat(2_000);
    assert_eq!(text.chars().count(), 12_000);
    text
}

#[test]
fn test_12000_char_input_is_capped_to_the_model_limit() {
    let tokenizer = load_tokenizer(&common::fixture_path("tiny_tokenizer.json")).expect("load");
    let encoded = encode_text(&tokenizer, &failing_input(), 8).expect("encode");

    assert_eq!(encoded.ids.len(), 8, "the encode seam itself caps the ids");
    assert_eq!(
        encoded.dropped_tokens,
        2_002 - 8,
        "the Step 1 measurement (2,002 ids) is the number this assertion uses"
    );
}

#[test]
fn test_12000_char_input_keeps_special_tokens_at_both_ends() {
    let tokenizer = load_tokenizer(&common::fixture_path("tiny_tokenizer.json")).expect("load");
    let encoded = encode_text(&tokenizer, &failing_input(), 8).expect("encode");

    assert_eq!(encoded.ids[0], 1, "`[CLS]` survives at the head");
    assert_eq!(encoded.ids[7], 2, "`[SEP]` survives at the tail");
    if let Some(mask) = encoded.attention_mask.as_ref() {
        assert_eq!(mask.len(), encoded.ids.len(), "the mask stays aligned");
    }
}

// ---------------------------------------------------------------------------
// Step 4 — the position limit comes from the cached `config.json`
// ---------------------------------------------------------------------------

const MODEL: &str = "org/model";

/// Install a `config.json` for `MODEL` under a fresh cache root.
fn cache_with_config(label: &str, config: &str) -> common::TempDir {
    let cache = common::temp_dir(label);
    common::write_file(&cache.path().join(MODEL), "config.json", config);
    cache
}

#[test]
fn test_declared_limit_reads_n_positions() {
    // The real nomic shape: the limit lives under `n_positions`, and
    // `max_position_embeddings` is absent.
    let cache = cache_with_config(
        "input-limits-n-positions",
        r#"{"model_type":"nomic_bert","n_embd":768,"n_positions":8192}"#,
    );

    assert_eq!(
        read_declared_position_limit(MODEL, cache.path()),
        Some(8192)
    );
}

#[test]
fn test_declared_limit_ignores_the_embedding_width() {
    let cache = cache_with_config(
        "input-limits-embedding-width",
        r#"{"model_type":"nomic_bert","n_embd":768,"n_positions":8192}"#,
    );

    assert_eq!(
        read_declared_position_limit(MODEL, cache.path()),
        Some(8192),
        "`n_embd` (768) is the hidden width, never a position count"
    );
}

#[test]
fn test_declared_limit_reads_max_position_embeddings() {
    let cache = cache_with_config(
        "input-limits-max-position",
        r#"{"model_type":"xlm-roberta","max_position_embeddings":514}"#,
    );

    assert_eq!(read_declared_position_limit(MODEL, cache.path()), Some(514));
}

#[test]
fn test_declared_limit_takes_the_smallest_declaration() {
    let cache = cache_with_config(
        "input-limits-smallest",
        r#"{"max_position_embeddings":8192,"n_positions":514}"#,
    );

    assert_eq!(
        read_declared_position_limit(MODEL, cache.path()),
        Some(514),
        "the minimum of the present declarations is the binding one"
    );
}

#[test]
fn test_declared_limit_reads_nested_text_config() {
    let cache = cache_with_config(
        "input-limits-text-config",
        r#"{"model_type":"multimodal","text_config":{"max_position_embeddings":1024}}"#,
    );

    assert_eq!(
        read_declared_position_limit(MODEL, cache.path()),
        Some(1024)
    );
}

#[test]
fn test_declared_limit_is_none_when_absent_or_malformed() {
    let absent = cache_with_config("input-limits-absent", r#"{"model_type":"tiny"}"#);
    assert_eq!(read_declared_position_limit(MODEL, absent.path()), None);

    let malformed = cache_with_config(
        "input-limits-malformed",
        r#"{"max_position_embeddings":"512","n_positions":0}"#,
    );
    assert_eq!(
        read_declared_position_limit(MODEL, malformed.path()),
        None,
        "a string and a zero are not position limits"
    );

    let negative = cache_with_config("input-limits-negative", r#"{"max_position_embeddings":-1}"#);
    assert_eq!(read_declared_position_limit(MODEL, negative.path()), None);

    let fractional = cache_with_config(
        "input-limits-fractional",
        r#"{"max_position_embeddings":512.5}"#,
    );
    assert_eq!(read_declared_position_limit(MODEL, fractional.path()), None);

    let missing_file = common::temp_dir("input-limits-missing-file");
    assert_eq!(
        read_declared_position_limit(MODEL, missing_file.path()),
        None
    );
}

// ---------------------------------------------------------------------------
// Step 5 — the effective cap, the override and the refusals
// ---------------------------------------------------------------------------

#[test]
fn test_effective_cap_subtracts_special_tokens() {
    assert_eq!(
        effective_max_tokens(Some(514), 2, None).expect("e5 declaration"),
        Some(512)
    );
    assert_eq!(
        effective_max_tokens(Some(8192), 2, None).expect("nomic declaration"),
        Some(8190)
    );
}

#[test]
fn test_effective_cap_refuses_an_override_above_the_limit() {
    let error = effective_max_tokens(Some(514), 2, Some(40_000))
        .expect_err("an override above the declared limit is refused, never clamped");

    assert!(
        matches!(
            error,
            EmbedError::MaxTokensExceedsModelLimit {
                configured: 40_000,
                model_limit: 514
            }
        ),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("at most 514"),
        "the message must name the remedy: {error}"
    );
}

#[test]
fn test_effective_cap_refuses_a_limit_that_cannot_hold_its_specials() {
    let error =
        effective_max_tokens(Some(3), 2, None).expect_err("declared == specials + 1 is refused");

    let EmbedError::ModelLoad { message, .. } = &error else {
        panic!("expected ModelLoad, got {error:?}")
    };
    assert!(
        message.contains("the cached config.json declares 3 token positions"),
        "the message names the declaration: {message}"
    );
    assert!(
        message.contains(
            "the derived cap would be 1 token(s), which cannot keep both boundary special tokens \
             (needs at least 4 positions)"
        ),
        "the derived-cap framing states the binding requirement, not specials + 1: {message}"
    );
    assert!(
        !message.contains("needs at least 3"),
        "the old off-by-one advice (the value it refuses) must be gone: {message}"
    );

    assert_eq!(
        effective_max_tokens(Some(4), 2, None).expect("specials + 2 is the degenerate pair"),
        Some(2),
        "cap == 2 stays reachable, but only through a model declaring exactly specials + 2 positions"
    );
}

#[test]
fn test_effective_cap_is_unbounded_without_a_declaration() {
    assert_eq!(
        effective_max_tokens(None, 2, None).expect("no declaration"),
        None,
        "an unknown-limit model is unbounded by design, not a default constant"
    );
    assert_eq!(
        effective_max_tokens(None, 2, Some(1024)).expect("override is the only bound"),
        Some(1024)
    );
}

#[test]
fn test_effective_cap_honours_an_override_under_the_limit() {
    assert_eq!(
        effective_max_tokens(Some(8192), 2, Some(1024)).expect("override under the limit"),
        Some(1024)
    );
}

#[test]
fn test_effective_cap_refuses_an_override_that_cannot_hold_its_specials() {
    for configured in [1, 2] {
        let error = effective_max_tokens(Some(8192), 2, Some(configured))
            .expect_err("a configured cap below specials + 1 is refused at load");
        let EmbedError::ModelLoad { message, .. } = &error else {
            panic!("expected ModelLoad for embeddingMaxTokens {configured}, got {error:?}")
        };
        assert!(
            message.contains(&format!(
                "the configured cap would be {configured} token(s), which cannot keep both \
                 boundary special tokens plus a content token (needs at least 3 tokens)"
            )),
            "the configured cap is refused with the same derived-cap rule: {message}"
        );
        assert!(
            message.contains("raise embeddingMaxTokens to at least 3"),
            "the remedy is named: {message}"
        );
    }

    assert_eq!(
        effective_max_tokens(Some(8192), 2, Some(3)).expect("specials + 1 is the floor"),
        Some(3),
        "the floor itself is accepted, so the refusal is not off by one"
    );
}

#[test]
fn test_effective_cap_refuses_an_override_under_an_undeclared_limit() {
    for configured in [1, 2] {
        let error = effective_max_tokens(None, 2, Some(configured)).expect_err(
            "an unknown-limit model cannot be configured into a specials-only cap either",
        );
        let EmbedError::ModelLoad { message, .. } = &error else {
            panic!("expected ModelLoad for embeddingMaxTokens {configured}, got {error:?}")
        };
        assert!(
            message.contains(&format!(
                "the configured cap would be {configured} token(s), which cannot keep both \
                 boundary special tokens plus a content token (needs at least 3 tokens)"
            )),
            "one rule, no declaration or not: {message}"
        );
    }

    assert_eq!(
        effective_max_tokens(None, 2, Some(3)).expect("the floor is accepted"),
        Some(3),
        "above the floor the override stays the only bound"
    );
}

// ---------------------------------------------------------------------------
// Step 7 — the cap wired through the embedder
// ---------------------------------------------------------------------------

/// Install the generated ONNX fixture + committed tokenizer under `MODEL`,
/// with the given `config.json` body.
fn fixture_cache(label: &str, config: &str) -> common::TempDir {
    let cache = common::temp_dir(label);
    let model_dir = cache.path().join(MODEL);
    common::onnx_fixture::write_tiny_model(&model_dir, "onnx/model.onnx", 1.0, false);
    std::fs::copy(
        common::onnx_fixture::tiny_tokenizer_path(),
        model_dir.join("tokenizer.json"),
    )
    .expect("copy the committed tokenizer fixture");
    common::write_file(&model_dir, "config.json", config);
    cache
}

fn fixture_embedder(cache: &common::TempDir, max_tokens: Option<u32>) -> LocalOnnxEmbedder {
    LocalOnnxEmbedder::new(LocalLoadOptions {
        model: MODEL.to_string(),
        cache_root: cache.path().to_path_buf(),
        dtype: None,
        pooling: EmbeddingPooling::Mean,
        intra_threads: Some(1),
        hub: None,
        max_tokens,
    })
}

/// Expected embedding: mean of the fixture rows for `ids`, L2-normalized.
fn expected_mean_normalized(ids: &[i64]) -> Vec<f32> {
    let rows: Vec<Vec<f32>> = ids
        .iter()
        .map(|id| common::onnx_fixture::fixture_row(*id, 1.0))
        .collect();
    let mut mean = [0.0f32; common::onnx_fixture::FIXTURE_HIDDEN];
    for row in &rows {
        for (accumulator, value) in mean.iter_mut().zip(row) {
            *accumulator += value / rows.len() as f32;
        }
    }
    let norm = mean.iter().map(|value| value * value).sum::<f32>().sqrt();
    mean.iter().map(|value| value / norm).collect()
}

fn unit_norm(vector: &[f32]) -> f32 {
    vector.iter().map(|value| value * value).sum::<f32>().sqrt()
}

#[tokio::test]
async fn test_configured_cap_overrides_the_declared_limit() {
    let cache = fixture_cache(
        "input-limits-override",
        r#"{"model_type":"tiny","max_position_embeddings":64}"#,
    );
    let embedder = fixture_embedder(&cache, Some(6));

    embedder.embed(&failing_input()).await.expect("embed");

    let stats = embedder
        .truncation_stats()
        .expect("the local lane reports truncation counters");
    assert_eq!(
        stats.cap_tokens,
        Some(6),
        "the configured cap wins over the declared 64 (mutant: 62)"
    );
    assert_eq!(stats.truncated_inputs, 1);
    assert_eq!(
        stats.dropped_tokens,
        2_002 - 6,
        "ignoring the override would drop 2_002 - 62"
    );
}

#[tokio::test]
async fn test_configured_cap_that_cannot_hold_its_specials_fails_the_load() {
    // Declares 64 positions, so only the configured cap can refuse this model.
    let cache = fixture_cache(
        "input-limits-configured-floor",
        r#"{"model_type":"tiny","max_position_embeddings":64}"#,
    );
    let embedder = fixture_embedder(&cache, Some(2));

    let error = embedder
        .warmup()
        .await
        .expect_err("a specials-only configured cap must refuse the load, not warm up");
    assert!(
        matches!(error, EmbedError::ModelLoad { .. }),
        "the refusal is a load error (the service gate turns it into WarmupFailed): {error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("the configured cap would be 2 token(s)"),
        "the load failure carries the configured-cap framing: {error}"
    );
}

#[tokio::test]
async fn test_over_long_input_still_produces_a_usable_embedding() {
    // Declares 8 positions, so the derived cap is 8 - 2 specials = 6.
    let cache = fixture_cache(
        "input-limits-declared-8",
        r#"{"model_type":"tiny","max_position_embeddings":8}"#,
    );
    let embedder = fixture_embedder(&cache, None);

    let vector = embedder.embed(&failing_input()).await.expect("embed");

    // cap 6 = head 3 + tail 3 over the 2,002 ids: [CLS] 3 3 3 3 [SEP].
    let expected = expected_mean_normalized(&[1, 3, 3, 3, 3, 2]);
    assert_eq!(vector.len(), expected.len());
    for (index, (got, want)) in vector.iter().zip(&expected).enumerate() {
        assert!(
            (got - want).abs() <= 1e-5,
            "component {index}: expected {want}, got {got}"
        );
    }
    assert!(
        (unit_norm(&vector) - 1.0).abs() <= 1e-5,
        "the output is L2-normalized"
    );

    let stats = embedder.truncation_stats().expect("local stats");
    assert_eq!(stats.cap_tokens, Some(6));
    assert_eq!(stats.truncated_inputs, 1);
    assert_eq!(stats.dropped_tokens, 2_002 - 6);
}

#[tokio::test]
async fn test_model_without_a_declared_limit_embeds_the_full_input() {
    let cache = fixture_cache("input-limits-no-declaration", r#"{"model_type":"tiny"}"#);
    let embedder = fixture_embedder(&cache, None);

    embedder.embed(&failing_input()).await.expect("embed");

    let stats = embedder.truncation_stats().expect("local stats");
    assert_eq!(
        stats.cap_tokens, None,
        "no declaration and no override means unbounded, not a default constant"
    );
    assert_eq!(stats.truncated_inputs, 0);
    assert_eq!(stats.dropped_tokens, 0);
}

#[test]
fn test_service_options_carry_the_configured_cap() {
    let home = common::temp_dir("input-limits-service-cap-home");
    common::write_file(
        home.path(),
        ".config/opencode/opencode-mem.jsonc",
        r#"{"embeddingMaxTokens":1024}"#,
    );
    let paths = ConfigPaths::standard(home.path().to_path_buf());
    let env: BTreeMap<String, String> = BTreeMap::new();
    let config = Config::load(&paths, &env).expect("config loads");

    let options = EmbeddingServiceOptions::from_config(&config);

    assert_eq!(
        options.max_tokens,
        Some(1024),
        "the service options carry the configured cap"
    );
    assert_eq!(
        options.local_load_options().max_tokens,
        Some(1024),
        "the service-to-loader hop carries it too (mutant: None)"
    );
}
