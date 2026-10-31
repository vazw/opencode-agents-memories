//! Tokenizer loading + single-sequence encoding (`tokenizers` 1.0.0-rc.2).
//!
//! Limits of this release (documented, not faked): the reader is read-only,
//! accepts canonical `tokenizer.json` v2.0 only, and has no truncation or
//! padding of its own (truncation is applied by [`encode_text`]) — a legacy
//! file must be canonicalized first, which is why [`load_tokenizer`] names the
//! path and says so in the error.
//!
//! `encode_text` performs a single-sequence encode and caps the result at the
//! caller's token limit ([`truncate_tokenized`]). On the per-embed path the
//! `.wait()` on the encode handle happens inside the caller's blocking
//! closure, never on the async runtime; the one exception is the one-time
//! special-token probe inside `LocalOnnxEmbedder::load`, which is itself
//! already a blocking path (`std::fs::read` of the weights + `Session::load`).
//! It defaults to `EncodeOptions::default()`, which adds the post-processor's
//! special tokens. Because the fixture declares no padding,
//! `attention_mask`/`type_ids` are typically `None` (upstream transformers.js
//! synthesizes an all-ones mask for a single unpadded sequence; the session
//! does the same when the model declares an `attention_mask` input).

use std::path::Path;

use tokenizers::pipeline::{EncodeOptions, PipelineTokenizer};

use crate::error::EmbedError;

/// One encoded sequence: token ids plus the optional mask/type-id arrays the
/// tokenizer produced (`None` when the encoding has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenizedInput {
    pub ids: Vec<i64>,
    pub attention_mask: Option<Vec<i64>>,
    pub type_ids: Option<Vec<i64>>,
    /// How many ids [`truncate_tokenized`] removed from this input
    /// (`0` = nothing was dropped).
    pub dropped_tokens: usize,
}

/// Cap `input` at `max_tokens` ids, keeping the head and the tail.
///
/// Contract: `max_tokens` is floored at 1, so the function is total and can
/// never widen an input. `head = max_tokens / 2`, `tail = max_tokens - head`;
/// the result is `ids[..head] ++ ids[len - tail..]`, so the first and last ids
/// (the template's boundary special tokens) always survive. The optional mask
/// and type-id arrays are sliced with the same two ranges and stay the same
/// length as `ids`. `dropped_tokens = old_len - new_len` (`0` when
/// `old_len <= max_tokens`), and a second pass at the same cap is a no-op.
///
/// `max_tokens == 2` is the documented degenerate pair (`[CLS][SEP]`, no
/// content token). Configuration reaches it only through a model that declares
/// exactly `specials + 2` positions: the loader refuses a declaration below
/// that, and refuses a configured cap below `specials + 1`, so no configured
/// value can produce it. It stays valid and mask-consistent if reached.
pub fn truncate_tokenized(input: TokenizedInput, max_tokens: usize) -> TokenizedInput {
    let max_tokens = max_tokens.max(1);
    let len = input.ids.len();
    if len <= max_tokens {
        return input;
    }
    let head = max_tokens / 2;
    let tail = max_tokens - head;
    let tail_start = len - tail;
    let slice = |values: Vec<i64>| -> Vec<i64> {
        let mut kept = Vec::with_capacity(max_tokens);
        kept.extend_from_slice(&values[..head]);
        kept.extend_from_slice(&values[tail_start..]);
        kept
    };
    TokenizedInput {
        ids: slice(input.ids),
        attention_mask: input.attention_mask.map(slice),
        type_ids: input.type_ids.map(slice),
        dropped_tokens: len - max_tokens,
    }
}

/// Read a canonical `tokenizer.json` v2.0.
pub fn load_tokenizer(path: &Path) -> Result<PipelineTokenizer, EmbedError> {
    tokenizers::from_json_file(path).map_err(|error| EmbedError::TokenizerLoad {
        path: path.to_path_buf(),
        message: format!(
            "{error} (this reader accepts canonical `tokenizer.json` v2.0 only; \
             canonicalize a legacy file first, e.g. `tk_convert::canonicalize_file`)"
        ),
    })
}

/// Encode one text into token ids (adding special tokens per the tokenizer's
/// post-processor), then cap the sequence at `max_tokens` ids with
/// [`truncate_tokenized`]. `usize::MAX` is the documented "unbounded" sentinel.
pub fn encode_text(
    tokenizer: &PipelineTokenizer,
    text: &str,
    max_tokens: usize,
) -> Result<TokenizedInput, EmbedError> {
    let encodings = tokenizer
        .encode(text, &EncodeOptions::default())
        .wait()
        .map_err(|error| EmbedError::Tokenize {
            message: error.to_string(),
        })?;
    let encoding = encodings
        .into_iter()
        .next()
        .ok_or_else(|| EmbedError::Tokenize {
            message: "tokenizer returned no encoding for the input".to_string(),
        })?;

    let encoded = TokenizedInput {
        ids: encoding
            .ids()
            .iter()
            .map(|token| i64::from(token.id()))
            .collect(),
        attention_mask: encoding
            .attention_mask()
            .map(|mask| mask.iter().map(|value| i64::from(*value)).collect()),
        type_ids: encoding
            .type_ids()
            .map(|ids| ids.iter().map(|value| i64::from(*value)).collect()),
        dropped_tokens: 0,
    };
    // A tokenizer that returns a differently-sized mask/type array is a
    // contract violation: let the tensor shape error surface, never "repair"
    // it silently (the slice arithmetic below assumes the arrays are aligned).
    if let Some(mask) = encoded.attention_mask.as_deref() {
        debug_assert_eq!(
            mask.len(),
            encoded.ids.len(),
            "attention_mask length must match ids"
        );
    }
    if let Some(type_ids) = encoded.type_ids.as_deref() {
        debug_assert_eq!(
            type_ids.len(),
            encoded.ids.len(),
            "type_ids length must match ids"
        );
    }
    Ok(truncate_tokenized(encoded, max_tokens))
}
