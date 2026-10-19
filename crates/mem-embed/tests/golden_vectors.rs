//! Golden-vector parity (blueprint step 13).
//!
//! The upstream suite exercises `Xenova/nomic-embed-text-v1` through
//! transformers.js. This port pins the same three inputs against embeddings
//! recorded by that exact JS stack (onnxruntime-node 1.30.0) — the fixture and
//! its provenance live in `tests/fixtures/nomic_goldens.json` + `README.md`.
//!
//! Two tests:
//! - `test_nomic_goldens_metadata_and_shapes` — always-on. Pins the fixture's
//!   metadata and shape contract (the recording is what proves the numbers;
//!   this is what proves the *file* is the one we think it is).
//! - `test_nomic_embeddings_match_recorded_ts_goldens` — `#[ignore]`d. It needs
//!   the pre-downloaded nomic cache (network wall: `hub: None`, so a cache miss
//!   can never silently download) and therefore stays out of the hermetic
//!   `cargo test`. Run explicitly:
//!
//!   ```text
//!   MEM_EMBED_GOLDEN_CACHE=/tmp/opencode/w14-model-cache \
//!     cargo test -p mem-embed --test golden_vectors -- --ignored --nocapture
//!   ```
//!
//! Tolerance rationale: our runtime is `ort` 2.0.0-rc.14 while the goldens come
//! from onnxruntime-node 1.30.0 — same graph, different operator kernel
//! versions. `cos >= 0.9999` / `max_abs <= 1e-3` is far tighter than the
//! upstream parity surface needs while still catching a real pipeline break
//! (wrong pooling, un-normalized output, wrong dtype file).

use std::path::PathBuf;

use mem_embed::local::{EmbeddingPooling, LocalOnnxDtype};
use mem_embed::{Embedder, LocalLoadOptions, LocalOnnxEmbedder};
use serde::Deserialize;

/// The exact run command, quoted in the unset-env panic and in `README.md`.
const IGNORED_RUN_COMMAND: &str = "MEM_EMBED_GOLDEN_CACHE=/tmp/opencode/w14-model-cache \
    cargo test -p mem-embed --test golden_vectors -- --ignored --nocapture";

/// The model id recorded by the TS script (also the cache subdirectory name).
const GOLDEN_MODEL: &str = "Xenova/nomic-embed-text-v1";
/// nomic-embed-text-v1 hidden size.
const GOLDEN_DIMS: usize = 768;

#[derive(Debug, Deserialize)]
struct Goldens {
    model: String,
    pooling: String,
    normalize: bool,
    dtype: String,
    dims: usize,
    inputs: Vec<String>,
    embeddings: Vec<Vec<f32>>,
}

fn goldens_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nomic_goldens.json")
}

fn load_goldens() -> Goldens {
    let path = goldens_path();
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing or unreadable golden fixture {path:?}: {error}"));
    serde_json::from_str(&contents).expect("nomic_goldens.json must parse")
}

#[test]
fn test_nomic_goldens_metadata_and_shapes() {
    let goldens = load_goldens();

    assert_eq!(goldens.model, GOLDEN_MODEL);
    assert_eq!(goldens.pooling, "mean");
    assert!(goldens.normalize, "goldens must be L2-normalized");
    assert_eq!(goldens.dtype, "fp32");
    assert_eq!(goldens.dims, GOLDEN_DIMS);

    assert!(!goldens.inputs.is_empty(), "no recorded inputs");
    assert_eq!(
        goldens.embeddings.len(),
        goldens.inputs.len(),
        "one embedding per recorded input"
    );

    for (index, embedding) in goldens.embeddings.iter().enumerate() {
        assert_eq!(
            embedding.len(),
            GOLDEN_DIMS,
            "embedding {index} must have {GOLDEN_DIMS} dimensions"
        );
        assert!(
            embedding.iter().all(|value| value.is_finite()),
            "embedding {index} contains a non-finite value"
        );
    }
}

#[tokio::test]
#[ignore = "requires MEM_EMBED_GOLDEN_CACHE pointing at the pre-downloaded nomic cache (network-free run)"]
async fn test_nomic_embeddings_match_recorded_ts_goldens() {
    let cache_root = match std::env::var("MEM_EMBED_GOLDEN_CACHE") {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => panic!(
            "MEM_EMBED_GOLDEN_CACHE is not set. Point it at the pre-downloaded \
             nomic cache and re-run:\n{IGNORED_RUN_COMMAND}"
        ),
    };

    let goldens = load_goldens();
    let embedder = LocalOnnxEmbedder::new(LocalLoadOptions {
        model: GOLDEN_MODEL.to_string(),
        cache_root,
        dtype: Some(LocalOnnxDtype::Fp32),
        pooling: EmbeddingPooling::Mean,
        intra_threads: None,
        // Network wall: a cache miss must fail loudly, never download.
        hub: None,
    });

    let mut min_cosine = f64::INFINITY;
    let mut max_abs_diff = 0.0_f32;

    for (input, expected) in goldens.inputs.iter().zip(&goldens.embeddings) {
        let actual = embedder
            .embed(input)
            .await
            .unwrap_or_else(|error| panic!("embedding {input:?} failed: {error}"));

        assert_eq!(
            actual.len(),
            expected.len(),
            "dimension mismatch for {input:?}"
        );

        let cosine = mem_core::utils::math::cosine_similarity(&actual, expected);
        let max_abs = actual
            .iter()
            .zip(expected)
            .map(|(a, e)| (a - e).abs())
            .fold(0.0_f32, f32::max);

        println!("input={input:?} cosine={cosine:.6} max_abs={max_abs:.3e}");
        assert!(
            cosine >= 0.9999,
            "cosine {cosine:.6} below tolerance for {input:?}"
        );
        assert!(
            max_abs <= 1e-3,
            "max-abs {max_abs:.3e} above tolerance for {input:?}"
        );

        min_cosine = min_cosine.min(cosine);
        max_abs_diff = max_abs_diff.max(max_abs);
    }

    println!("MIN cosine={min_cosine:.6} MAX max_abs={max_abs_diff:.3e}");
}
