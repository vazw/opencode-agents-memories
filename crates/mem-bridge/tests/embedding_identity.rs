//! Issue 177: the effective embedding dtype is resolved once, from the cache
//! the loader uses, and the identity document takes the loader's value.
//!
//! Behavior is asserted through the public seam only:
//! `mem_bridge::embedding_identity::resolve_effective_embedding(&Config)`.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use mem_bridge::embedding_identity::resolve_effective_embedding;
use mem_core::config::{Config, LocalOnnxDtype};

fn load_config(root: &Path, home: &Path) -> Config {
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.to_string_lossy().to_string())]
        .into_iter()
        .collect();
    let mut config = Config::load(&mem_core::config::ConfigPaths::standard(home), &env)
        .expect("the default config loads");
    config.storage_path = root.to_path_buf();
    config.embedding_dimensions = 8;
    config
}

/// `<storagePath>/.cache/<model>/onnx/model_quantized.onnx`, empty, and no
/// `config.json` — the absence is what sends `resolve_local_onnx_dtype` to the
/// file probe, which then finds the q8 weights.
fn write_q8_cache(config: &Config) {
    let onnx = config
        .storage_path
        .join(".cache")
        .join(&config.embedding_model)
        .join("onnx");
    std::fs::create_dir_all(&onnx).expect("create the onnx cache dir");
    std::fs::write(onnx.join("model_quantized.onnx"), b"").expect("write the empty q8 weights");
}

/// `resolve_effective_embedding` must feed the identity from the loader's
/// resolution: unset dtype + q8-only cache means q8 loads AND q8 is stamped.
#[test]
fn identity_dtype_resolves_from_cache() {
    let root = common::TestDir::new("identity-dtype-cache");
    let home = common::TestDir::new("identity-dtype-cache-home");
    let mut config = load_config(&root, &home);
    config.embedding_dtype = None;
    write_q8_cache(&config);

    let (options, identity) = resolve_effective_embedding(&config);

    assert_eq!(
        identity.dtype,
        Some(LocalOnnxDtype::Q8),
        "the identity must stamp the dtype the loader resolves, not the unset config value"
    );
    assert_eq!(
        options.dtype,
        Some(LocalOnnxDtype::Q8),
        "the loader options must carry the cache-resolved dtype"
    );
    let fingerprint = identity.fingerprint().expect("identity serializes");
    assert!(
        fingerprint.contains("\"dtype\":\"q8\""),
        "the document must name the loaded dtype: {fingerprint}"
    );
}

/// `Auto` is reachable config and the loader treats it exactly as unset
/// (`crates/mem-embed/src/local/mod.rs:171-176`): both arms must resolve from
/// the cache, including an empty cache's `None`.
#[test]
fn identity_dtype_resolves_auto_from_cache() {
    // Arm 1: `Auto` + q8-only cache ⇒ both halves q8.
    let root = common::TestDir::new("identity-dtype-auto-q8");
    let home = common::TestDir::new("identity-dtype-auto-q8-home");
    let mut config = load_config(&root, &home);
    config.embedding_dtype = Some(LocalOnnxDtype::Auto);
    write_q8_cache(&config);

    let (options, identity) = resolve_effective_embedding(&config);
    assert_eq!(
        identity.dtype,
        Some(LocalOnnxDtype::Q8),
        "Auto must resolve to the cached dtype, not be stamped as 'auto'"
    );
    assert_eq!(options.dtype, Some(LocalOnnxDtype::Q8));
    let fingerprint = identity.fingerprint().expect("identity serializes");
    assert!(
        fingerprint.contains("\"dtype\":\"q8\""),
        "the document must name the loaded dtype: {fingerprint}"
    );

    // Arm 2: `Auto` + a cache root that holds no `config.json` and no
    // `model*.onnx` ⇒ the probe resolves nothing; both halves stay `None`.
    let root = common::TestDir::new("identity-dtype-auto-empty");
    let home = common::TestDir::new("identity-dtype-auto-empty-home");
    let mut config = load_config(&root, &home);
    config.embedding_dtype = Some(LocalOnnxDtype::Auto);
    std::fs::create_dir_all(
        config
            .storage_path
            .join(".cache")
            .join(&config.embedding_model),
    )
    .expect("create the model cache dir");

    let (options, identity) = resolve_effective_embedding(&config);
    assert_eq!(
        identity.dtype, None,
        "an empty cache resolves no dtype; the document must not say 'auto'"
    );
    assert_eq!(options.dtype, None);
    let fingerprint = identity.fingerprint().expect("identity serializes");
    assert!(
        fingerprint.contains("\"dtype\":null"),
        "the document must render the unresolved dtype as null: {fingerprint}"
    );
}

/// The regression guard for the wiring: an explicit concrete dtype wins over
/// the cache, and a remote embedder never probes the cache (it loads no local
/// weights; the identity keeps the configured value).
#[test]
fn identity_dtype_respects_explicit_and_remote() {
    // Arm 1: explicit Fp32 over a q8-only cache ⇒ both halves Fp32.
    let root = common::TestDir::new("identity-dtype-explicit");
    let home = common::TestDir::new("identity-dtype-explicit-home");
    let mut config = load_config(&root, &home);
    config.embedding_dtype = Some(LocalOnnxDtype::Fp32);
    write_q8_cache(&config);

    let (options, identity) = resolve_effective_embedding(&config);
    assert_eq!(
        options.dtype,
        Some(LocalOnnxDtype::Fp32),
        "an explicit dtype must not be overridden by the cache probe"
    );
    assert_eq!(
        identity.dtype,
        Some(LocalOnnxDtype::Fp32),
        "the identity echoes the explicit dtype verbatim"
    );

    // Arm 2: remote config + unset dtype ⇒ both halves None; no cache probe.
    let root = common::TestDir::new("identity-dtype-remote");
    let home = common::TestDir::new("identity-dtype-remote-home");
    let mut config = load_config(&root, &home);
    config.embedding_dtype = None;
    config.embedding_api_url = Some("https://example.invalid/v1/embeddings".to_string());
    config.embedding_api_key = Some("test-key".to_string());
    write_q8_cache(&config);

    let (options, identity) = resolve_effective_embedding(&config);
    assert_eq!(
        identity.dtype, None,
        "a remote embedder loads no local weights; the identity keeps the configured None"
    );
    assert_eq!(
        options.dtype, None,
        "a remote embedder must not receive a cache-resolved local dtype"
    );
}
