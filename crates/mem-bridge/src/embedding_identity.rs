//! The one resolution of the *effective* embedding identity (issue 177).
//!
//! The store identity used to be built from the configured dtype while the
//! local loader resolved the effective one from the model cache (issue 177):
//! a config without `embeddingDtype`, or with `"auto"`, stamped a dtype the
//! loader never loads. [`resolve_effective_embedding`] resolves the dtype once
//! and feeds both halves — the loader options and the identity document — so
//! the document can never name a dtype the runtime does not load.

use mem_core::config::Config;
use mem_core::embedding::EmbeddingIdentity;
use mem_embed::local::LocalLoadOptions;

use crate::composition::local_load_options_from_config;

/// The loader options and the embedding identity, resolved together.
///
/// The identity is derived from the SAME options the embedder is built from,
/// so composition, re-embed and consolidation cannot diverge.
pub fn resolve_effective_embedding(config: &Config) -> (LocalLoadOptions, EmbeddingIdentity) {
    let mut options = local_load_options_from_config(config);
    // A remote embedder never loads local weights: its identity keeps the
    // configured dtype (upstream behaviour) and the cache probe is skipped.
    let remote = config.embedding_api_url.is_some() && config.embedding_api_key.is_some();
    if !remote {
        // The loader treats `Auto` exactly as unset (`crates/mem-embed/src/local/mod.rs:171-176`):
        // both arms run the same cache probe, so both must resolve here too.
        // Anything else is an explicit choice and wins.
        options.dtype = match options.dtype {
            Some(mem_core::config::LocalOnnxDtype::Auto) | None => {
                mem_embed::resolve_local_onnx_dtype(&options.model, &options.cache_root)
            }
            explicit => explicit,
        };
    }
    let mut identity = EmbeddingIdentity::from_config(config);
    // The loader options are the authority for the effective dtype: the
    // document may never name a dtype the runtime does not load. With an
    // explicit dtype this is the config value (unchanged); unset/auto it is
    // the cache-resolved dtype (or `None`), which is what the loader loads.
    identity.dtype = options.dtype;
    (options, identity)
}
