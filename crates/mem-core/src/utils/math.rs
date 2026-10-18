//! Port of upstream `src/utils/math.ts` (v2.31.3 @ 906038e).
//!
//! Cosine similarity and L2 normalization shared by upstream
//! `deduplication-service.ts` (f32 embedding vectors) and
//! `user-profile-manager.ts` (f64 JSON-stored centroids — roadmap
//! W3.2/W3.3). All accumulation is f64 for JS-number parity — the f32
//! variant widens each element with `f64::from` and never accumulates in
//! f32.

/// Computes the cosine similarity of two f32 vectors.
///
/// Returns exactly `0.0` (never NaN) on length mismatch or when either
/// vector has zero magnitude (including empty inputs).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    cosine_similarity_impl(
        a.iter().map(|&v| f64::from(v)),
        a.len(),
        b.iter().map(|&v| f64::from(v)),
        b.len(),
    )
}

/// Computes the cosine similarity of two f64 vectors (JSON-stored
/// centroids), otherwise identical to the f32 variant.
pub fn cosine_similarity_numbers(a: &[f64], b: &[f64]) -> f64 {
    cosine_similarity_impl(a.iter().copied(), a.len(), b.iter().copied(), b.len())
}

fn cosine_similarity_impl(
    a: impl Iterator<Item = f64>,
    a_len: usize,
    b: impl Iterator<Item = f64>,
    b_len: usize,
) -> f64 {
    if a_len != b_len {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut mag_a = 0.0f64;
    let mut mag_b = 0.0f64;

    for (av, bv) in a.zip(b) {
        dot += av * bv;
        mag_a += av * av;
        mag_b += bv * bv;
    }

    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (mag_a.sqrt() * mag_b.sqrt())
}

/// Scales a vector to unit Euclidean length.
///
/// Zero-norm inputs (including the empty vector) are returned unchanged —
/// never NaN, never a division by zero.
pub fn l2_normalize(vec: &[f64]) -> Vec<f64> {
    let norm = vec.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm == 0.0 {
        return vec.to_vec();
    }
    vec.iter().map(|v| v / norm).collect()
}
