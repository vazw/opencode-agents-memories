//! Tokenizer loading + single-sequence encoding against the committed
//! `tests/fixtures/tiny_tokenizer.json` (blueprint step 4).
//!
//! The fixture is a canonical v2.0 WordLevel tokenizer: added tokens
//! `[UNK]=0` / `[CLS]=1` / `[SEP]=2`, Whitespace pre-tokenizer,
//! `TemplateProcessing` `[CLS] A [SEP]`, vocab `hello=3 world=4 foo=5 bar=6
//! baz=7`. The tokenizers rc.2 reader is read-only and accepts canonical v2.0
//! files only.

mod common;

use mem_embed::local::tokenize::{encode_text, load_tokenizer};

#[test]
fn test_tokenizer_loads_committed_fixture_and_encodes() {
    let tokenizer = load_tokenizer(&common::fixture_path("tiny_tokenizer.json")).expect("load");
    let encoded = encode_text(&tokenizer, "hello world", usize::MAX).expect("encode");
    assert_eq!(encoded.ids, vec![1, 3, 4, 2]);
    // The fixture declares no padding, so no attention mask / type ids exist.
    assert_eq!(encoded.attention_mask, None);
    assert_eq!(encoded.type_ids, None);
}

#[test]
fn test_tokenizer_maps_unknown_words_to_unk() {
    let tokenizer = load_tokenizer(&common::fixture_path("tiny_tokenizer.json")).expect("load");
    let encoded = encode_text(&tokenizer, "hello nope", usize::MAX).expect("encode");
    assert_eq!(encoded.ids, vec![1, 3, 0, 2]);
}
