//! New coverage — no upstream test exists (upstream `src/utils/math.ts` is
//! untested upstream). These tests pin the behavior of the port of
//! `cosineSimilarity` / `cosineSimilarityNumbers` / `l2Normalize`: f64
//! accumulation (JS-number parity), zero-length/zero-magnitude guards
//! returning exactly `0.0` (never NaN), and zero vectors returned unchanged.
//!
//! Consumers upstream: `deduplication-service.ts` (f32 vectors) and
//! `user-profile-manager.ts` (f64 centroids — roadmap W3.2/W3.3).

const EPS: f64 = 1e-9;

#[test]
fn test_cosine_similarity_identical_vectors_is_one() {
    let a = [1.0f32, 2.0, 3.0];
    assert!((mem_core::utils::math::cosine_similarity(&a, &a) - 1.0).abs() < EPS);
}

#[test]
fn test_cosine_similarity_orthogonal_vectors_is_zero() {
    let result = mem_core::utils::math::cosine_similarity(&[1.0f32, 0.0], &[0.0f32, 1.0]);
    assert_eq!(result, 0.0);
}

#[test]
fn test_cosine_similarity_opposite_vectors_is_negative_one() {
    let result = mem_core::utils::math::cosine_similarity(&[1.0f32, 2.0], &[-1.0f32, -2.0]);
    assert!((result + 1.0).abs() < EPS);
}

#[test]
fn test_cosine_similarity_known_value() {
    let result = mem_core::utils::math::cosine_similarity(&[1.0f32, 2.0, 3.0], &[4.0f32, 5.0, 6.0]);
    let expected = 32.0 / (14.0f64.sqrt() * 77.0f64.sqrt());
    assert!((result - expected).abs() < EPS);
}

#[test]
fn test_cosine_similarity_length_mismatch_is_zero() {
    let result = mem_core::utils::math::cosine_similarity(&[1.0f32, 2.0, 3.0], &[1.0f32, 2.0]);
    assert_eq!(result, 0.0);
}

#[test]
fn test_cosine_similarity_zero_magnitude_is_zero() {
    let mixed = mem_core::utils::math::cosine_similarity(&[0.0f32, 0.0], &[1.0f32, 1.0]);
    assert_eq!(mixed, 0.0);
    let both_zero = mem_core::utils::math::cosine_similarity(&[0.0f32, 0.0], &[0.0f32, 0.0]);
    assert_eq!(both_zero, 0.0);
    assert!(!mixed.is_nan() && !both_zero.is_nan());
}

#[test]
fn test_cosine_similarity_empty_inputs_are_zero() {
    assert_eq!(mem_core::utils::math::cosine_similarity(&[], &[]), 0.0);
}

#[test]
fn test_cosine_similarity_numbers_matches_f32_variant() {
    // Dyadic values: exactly representable in f32, so the two variants must
    // agree within the epsilon.
    let a_f32 = [0.5f32, -1.25, 3.0];
    let b_f32 = [2.0f32, 0.25, -4.0];
    let from_f32 = mem_core::utils::math::cosine_similarity(&a_f32, &b_f32);
    let from_numbers = mem_core::utils::math::cosine_similarity_numbers(
        &[0.5f64, -1.25, 3.0],
        &[2.0f64, 0.25, -4.0],
    );
    assert!((from_f32 - from_numbers).abs() < EPS);
    // numbers variant identical-vector case ≈ 1.0.
    let ident = mem_core::utils::math::cosine_similarity_numbers(
        &[0.5f64, -1.25, 3.0],
        &[0.5f64, -1.25, 3.0],
    );
    assert!((ident - 1.0).abs() < EPS);
}

#[test]
fn test_l2_normalize_scales_to_unit_length() {
    let normalized = mem_core::utils::math::l2_normalize(&[3.0f64, 4.0]);
    assert_eq!(normalized, vec![0.6, 0.8]);
    let norm: f64 = normalized.iter().map(|v| v * v).sum::<f64>().sqrt();
    assert!((norm - 1.0).abs() < EPS);
}

#[test]
fn test_l2_normalize_zero_vector_returns_input_unchanged() {
    let input = [0.0f64, 0.0];
    let normalized = mem_core::utils::math::l2_normalize(&input);
    assert_eq!(normalized, vec![0.0, 0.0]);
    assert!(!normalized.iter().any(|v| v.is_nan()));
    assert_eq!(mem_core::utils::math::l2_normalize(&[0.0f64]), vec![0.0]);
}

#[test]
fn test_l2_normalize_empty_is_empty() {
    assert!(mem_core::utils::math::l2_normalize(&[]).is_empty());
}

#[test]
fn test_l2_normalize_negative_components_preserve_direction() {
    assert_eq!(
        mem_core::utils::math::l2_normalize(&[-3.0f64, 4.0]),
        vec![-0.6, 0.8]
    );
}
