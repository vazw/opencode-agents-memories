//! Tokenizer loading + single-sequence encoding (`tokenizers` 1.0.0-rc.2).
//!
//! Limits of this release (documented, not faked): the reader is read-only,
//! accepts canonical `tokenizer.json` v2.0 only, and has no truncation or
//! padding of its own — a legacy file must be canonicalized first, which is
//! why [`load_tokenizer`] names the path and says so in the error.
//!
//! `encode_text` performs a single-sequence encode (the `.wait()` on the
//! encode handle happens inside the caller's blocking closure, never on the
//! async runtime). It defaults to `EncodeOptions::default()`, which adds the
//! post-processor's special tokens. Because the fixture declares no padding,
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
/// post-processor).
pub fn encode_text(
    tokenizer: &PipelineTokenizer,
    text: &str,
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

    Ok(TokenizedInput {
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
    })
}
