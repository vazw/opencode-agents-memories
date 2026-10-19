//! Pooling + L2 normalization parity surface (blueprint step 4).
//!
//! Upstream hands pooling to transformers.js
//! (`pipe(text, { pooling, normalize: true })`); the Rust port implements the
//! fastembed recipe directly: mean (attention-mask weighted) / cls (first
//! position) / last_token (last unmasked position), then L2 normalization.
//! These tests pin the observable vectors, not the implementation.

use mem_embed::local::pooling::{EmbeddingPooling, l2_normalize_f32, pool_token_embeddings};

fn assert_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len(), "length mismatch");
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() <= 1e-6,
            "component {index}: expected {e}, got {a} ({actual:?} vs {expected:?})"
        );
    }
}

/// Unit-length form of `values` (what pooling returns after normalization).
fn unit(values: &[f32]) -> Vec<f32> {
    let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    values.iter().map(|v| v / norm).collect()
}

fn tensor_shape_error(error: mem_embed::EmbedError) -> bool {
    matches!(error, mem_embed::EmbedError::TensorShape { .. })
}

#[test]
fn test_mean_pooling_averages_all_positions_without_mask() {
    let data = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // rows [1,2] [3,4] [5,6]
    let pooled = pool_token_embeddings(&data, &[1, 3, 2], EmbeddingPooling::Mean, None)
        .expect("mean pooling");
    // mean = [3, 4] → normalized [0.6, 0.8]
    assert_close(&pooled, &unit(&[3.0, 4.0]));
    assert_close(&pooled, &[0.6, 0.8]);
}

#[test]
fn test_mean_pooling_excludes_masked_positions() {
    let data = [1.0, 2.0, 100.0, 100.0, -50.0, -50.0, 7.0, 9.0];
    let mask = [1i64, 1, 0, 0];
    let pooled = pool_token_embeddings(&data, &[1, 4, 2], EmbeddingPooling::Mean, Some(&mask))
        .expect("mean");
    // Only rows 0 and 1 count → mean = [50.5, 51.0]
    assert_close(&pooled, &unit(&[50.5, 51.0]));

    // An all-ones mask is equivalent to no mask.
    let ones = [1i64, 1, 1, 1];
    let no_mask =
        pool_token_embeddings(&data, &[1, 4, 2], EmbeddingPooling::Mean, None).expect("mean");
    let all_ones = pool_token_embeddings(&data, &[1, 4, 2], EmbeddingPooling::Mean, Some(&ones))
        .expect("mean");
    assert_close(&all_ones, &no_mask);
}

#[test]
fn test_cls_pooling_takes_first_token() {
    let data = [1.0, 0.0, 0.0, 9.0, 0.0, 0.0];
    let pooled =
        pool_token_embeddings(&data, &[1, 3, 2], EmbeddingPooling::Cls, None).expect("cls");
    assert_close(&pooled, &[1.0, 0.0]);

    // cls ignores the mask (the first position is the classification token).
    let mask = [0i64, 1, 1];
    let masked =
        pool_token_embeddings(&data, &[1, 3, 2], EmbeddingPooling::Cls, Some(&mask)).expect("cls");
    assert_close(&masked, &[1.0, 0.0]);
}

#[test]
fn test_last_token_pooling_takes_last_unmasked_position() {
    // rows: [0.6, 0.8] [0.0, 1.0] [1.0, 0.0]
    let data = [0.6, 0.8, 0.0, 1.0, 1.0, 0.0];
    // No mask → the final position.
    let pooled =
        pool_token_embeddings(&data, &[1, 3, 2], EmbeddingPooling::LastToken, None).expect("last");
    assert_close(&pooled, &[1.0, 0.0]);

    // Masked → the last position whose mask is non-zero (here: index 1).
    let mask = [1i64, 1, 0];
    let masked = pool_token_embeddings(&data, &[1, 3, 2], EmbeddingPooling::LastToken, Some(&mask))
        .expect("last");
    assert_close(&masked, &[0.0, 1.0]);

    // An all-zero mask falls back to the final position (never panics).
    let zero_mask = [0i64, 0, 0];
    let fallback = pool_token_embeddings(
        &data,
        &[1, 3, 2],
        EmbeddingPooling::LastToken,
        Some(&zero_mask),
    )
    .expect("last");
    assert_close(&fallback, &[1.0, 0.0]);
}

#[test]
fn test_l2_normalize_produces_unit_length_vector() {
    let normalized = l2_normalize_f32(&[3.0, 4.0]);
    for (got, want) in normalized.iter().zip([0.6f32, 0.8]) {
        assert!((got - want).abs() <= 1e-6, "got {normalized:?}");
    }
    let norm = normalized.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() <= 1e-6, "norm {norm}");
}

#[test]
fn test_l2_normalize_keeps_zero_vector_unchanged() {
    assert_eq!(l2_normalize_f32(&[0.0, 0.0, 0.0]), vec![0.0, 0.0, 0.0]);
}

#[test]
fn test_pooling_rejects_shape_or_mask_mismatch() {
    let data = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

    // Not a 3-D [batch, seq, hidden] tensor.
    assert!(tensor_shape_error(
        pool_token_embeddings(&data, &[3, 2], EmbeddingPooling::Mean, None).unwrap_err()
    ));
    // Batch must be single-sequence.
    assert!(tensor_shape_error(
        pool_token_embeddings(&data, &[2, 3, 1], EmbeddingPooling::Mean, None).unwrap_err()
    ));
    // Empty sequence / hidden dimension.
    assert!(tensor_shape_error(
        pool_token_embeddings(&[], &[1, 0, 2], EmbeddingPooling::Mean, None).unwrap_err()
    ));
    assert!(tensor_shape_error(
        pool_token_embeddings(&data, &[1, 3, 0], EmbeddingPooling::Mean, None).unwrap_err()
    ));
    // data length disagrees with seq × hidden.
    assert!(tensor_shape_error(
        pool_token_embeddings(&data, &[1, 3, 4], EmbeddingPooling::Mean, None).unwrap_err()
    ));
    // Mask length disagrees with seq.
    assert!(tensor_shape_error(
        pool_token_embeddings(&data, &[1, 3, 2], EmbeddingPooling::Mean, Some(&[1, 1]))
            .unwrap_err()
    ));
}
