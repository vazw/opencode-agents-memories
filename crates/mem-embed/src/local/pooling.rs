//! Pooling over token embeddings (the "fastembed recipe": mean / cls /
//! last_token, then L2 normalization) — the parity surface with upstream
//! transformers.js `pipe(text, { pooling, normalize: true })`.
//!
//! Re-exports [`mem_core::config::EmbeddingPooling`], the value-typed
//! `EmbeddingServiceOptions::pooling` member.

use crate::error::EmbedError;

pub use mem_core::config::EmbeddingPooling;

/// Pool a `[1, seq, hidden]` f32 output tensor into one `hidden`-sized,
/// L2-normalized embedding.
///
/// Contract:
/// - `shape` must be exactly `[1, seq, hidden]` with `seq > 0`, `hidden > 0`
///   and `data.len() == seq * hidden`; anything else is
///   [`EmbedError::TensorShape`].
/// - `attention_mask`, when present, must have exactly `seq` entries
///   (non-zero = "keep"); a length mismatch is [`EmbedError::TensorShape`].
/// - `Mean` averages the kept positions (an all-ones mask is equivalent to no
///   mask; an all-zero mask yields the zero vector instead of dividing by
///   zero).
/// - `Cls` takes position 0 (the classification token), ignoring the mask.
/// - `LastToken` takes the last kept position, falling back to `seq - 1` when
///   the mask is absent or entirely zero.
/// - The result is L2-normalized with f32 accumulation (upstream parity; a
///   zero vector stays zero).
pub fn pool_token_embeddings(
    data: &[f32],
    shape: &[i64],
    pooling: EmbeddingPooling,
    attention_mask: Option<&[i64]>,
) -> Result<Vec<f32>, EmbedError> {
    let [batch, seq, hidden] = shape else {
        return Err(tensor_shape(format!(
            "expected a 3-D [batch, seq, hidden] tensor, got {shape:?}"
        )));
    };
    if *batch != 1 {
        return Err(tensor_shape(format!(
            "expected a single-sequence batch, got batch = {batch}"
        )));
    }
    if *seq <= 0 || *hidden <= 0 {
        return Err(tensor_shape(format!(
            "expected positive seq/hidden dimensions, got {shape:?}"
        )));
    }
    let seq = *seq as usize;
    let hidden = *hidden as usize;
    if data.len() != seq * hidden {
        return Err(tensor_shape(format!(
            "data length {} disagrees with shape {shape:?}",
            data.len()
        )));
    }
    if let Some(mask) = attention_mask
        && mask.len() != seq
    {
        return Err(tensor_shape(format!(
            "attention mask length {} disagrees with seq {seq}",
            mask.len()
        )));
    }

    let row = |index: usize| &data[index * hidden..(index + 1) * hidden];
    let kept = |index: usize| attention_mask.is_none_or(|mask| mask[index] != 0);

    let pooled: Vec<f32> = match pooling {
        EmbeddingPooling::Mean => {
            let mut accumulator = vec![0.0f32; hidden];
            let mut count = 0usize;
            for index in 0..seq {
                if !kept(index) {
                    continue;
                }
                for (slot, value) in accumulator.iter_mut().zip(row(index)) {
                    *slot += *value;
                }
                count += 1;
            }
            if count == 0 {
                accumulator
            } else {
                accumulator
                    .iter()
                    .map(|value| value / count as f32)
                    .collect()
            }
        }
        EmbeddingPooling::Cls => row(0).to_vec(),
        EmbeddingPooling::LastToken => {
            let index = (0..seq).rev().find(|index| kept(*index)).unwrap_or(seq - 1);
            row(index).to_vec()
        }
    };

    Ok(l2_normalize_f32(&pooled))
}

/// L2-normalize with f32 accumulation (upstream transformers.js semantics).
///
/// A zero vector stays zero; a non-finite norm (already-infinite inputs)
/// leaves the vector unchanged rather than propagating NaNs.
pub fn l2_normalize_f32(values: &[f32]) -> Vec<f32> {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return values.to_vec();
    }
    values.iter().map(|value| value / norm).collect()
}

fn tensor_shape(message: String) -> EmbedError {
    EmbedError::TensorShape { message }
}
