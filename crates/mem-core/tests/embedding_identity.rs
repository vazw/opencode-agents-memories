//! The storage embedding identity (issue 43 W2): deterministic, field
//! sensitive, credential-free, and fully mapped from the resolved config.

mod common;

use std::collections::BTreeMap;

use mem_core::config::{Config, ConfigPaths, EmbeddingPooling, LocalOnnxDtype};
use mem_core::embedding::EmbeddingIdentity;

type Env = BTreeMap<String, String>;

/// The resolved defaults from an empty temp home (never the real home).
fn default_config(label: &str) -> Config {
    let home = common::unique_temp_dir(label);
    let paths = ConfigPaths::standard(home);
    Config::load(&paths, &Env::new()).expect("defaults load from an empty home")
}

/// One input changed at a time; every change must move the fingerprint.
#[test]
fn test_fingerprint_is_deterministic_and_field_sensitive() {
    let config = default_config("embedding-identity-sensitive");
    let base = EmbeddingIdentity::from_config(&config);
    let base_fingerprint = base.fingerprint().expect("serializable identity");

    assert_eq!(
        base.fingerprint().expect("repeat"),
        base_fingerprint,
        "the same identity must serialize byte-identically"
    );
    assert_eq!(
        base_fingerprint,
        r#"{"v":1,"model":"Xenova/nomic-embed-text-v1","dims":768,"pooling":"mean","dtype":null,"task_prefixes":false,"query_prefix":null,"document_prefix":null}"#,
        "the default identity is byte-identical to the pre-change literal"
    );

    let variants: Vec<(&str, EmbeddingIdentity)> = vec![
        ("model", {
            let mut identity = base.clone();
            identity.model = "other/model".to_string();
            identity
        }),
        ("dimensions", {
            let mut identity = base.clone();
            identity.dimensions += 1;
            identity
        }),
        ("pooling", {
            let mut identity = base.clone();
            identity.pooling = if identity.pooling == EmbeddingPooling::Mean {
                EmbeddingPooling::Cls
            } else {
                EmbeddingPooling::Mean
            };
            identity
        }),
        ("dtype", {
            let mut identity = base.clone();
            identity.dtype = match identity.dtype {
                Some(_) => None,
                None => Some(LocalOnnxDtype::Fp16),
            };
            identity
        }),
        ("task_prefixes", {
            let mut identity = base.clone();
            identity.use_task_prefixes = !identity.use_task_prefixes;
            identity
        }),
        ("query_prefix", {
            let mut identity = base.clone();
            identity.query_prefix = Some("query: ".to_string());
            identity
        }),
        ("document_prefix", {
            let mut identity = base.clone();
            identity.document_prefix = Some("passage: ".to_string());
            identity
        }),
        ("max_tokens", {
            let mut identity = base.clone();
            identity.max_tokens = Some(1024);
            identity
        }),
    ];

    for (field, variant) in variants {
        let fingerprint = variant.fingerprint().expect("variant serializes");
        assert_ne!(
            fingerprint, base_fingerprint,
            "changing `{field}` must change the fingerprint"
        );
    }
}

#[test]
fn test_identity_moves_when_a_cap_is_configured() {
    let mut config = default_config("embedding-identity-cap");
    let base = EmbeddingIdentity::from_config(&config)
        .fingerprint()
        .expect("base identity serializes");

    config.embedding_max_tokens = Some(1024);
    let capped = EmbeddingIdentity::from_config(&config);
    let fingerprint = capped.fingerprint().expect("capped identity serializes");

    assert_eq!(
        capped.max_tokens,
        Some(1024),
        "the configured cap maps from `Config`"
    );
    assert_ne!(fingerprint, base, "a configured cap must move the identity");
    assert!(
        fingerprint.contains(r#""max_tokens":1024"#),
        "the cap is serialized under its own key: {fingerprint}"
    );
}

/// The fingerprint lands in a plaintext shard that backups copy: neither the
/// API key nor anything derived from the URL may appear in it.
#[test]
fn test_fingerprint_excludes_credentials() {
    let mut config = default_config("embedding-identity-credentials");
    config.embedding_api_url =
        Some("https://embed.example.test/v1/embeddings?token=SECRET-URL-TOKEN".to_string());
    config.embedding_api_key = Some("SECRET-API-KEY".to_string());

    let fingerprint = EmbeddingIdentity::from_config(&config)
        .fingerprint()
        .expect("serializable identity");

    assert!(
        !fingerprint.contains("SECRET-API-KEY"),
        "the API key must never enter the fingerprint: {fingerprint}"
    );
    assert!(
        !fingerprint.contains("SECRET-URL-TOKEN") && !fingerprint.contains("embed.example.test"),
        "the API URL (which can carry a credential) must never enter the fingerprint: {fingerprint}"
    );
}

/// Every identity input maps from `Config`, and each one reaches the
/// fingerprint.
#[test]
fn test_from_config_maps_every_identity_input() {
    let mut config = default_config("embedding-identity-mapping");
    config.embedding_model = "custom/embedding-model".to_string();
    config.embedding_dimensions = 1024;
    config.embedding_pooling = EmbeddingPooling::LastToken;
    config.embedding_dtype = Some(LocalOnnxDtype::Q8);
    config.embedding_use_task_prefixes = true;
    config.embedding_query_prefix = Some("query: ".to_string());
    config.embedding_document_prefix = Some("passage: ".to_string());

    let identity = EmbeddingIdentity::from_config(&config);
    assert_eq!(identity.model, "custom/embedding-model");
    assert_eq!(identity.dimensions, 1024);
    assert_eq!(identity.pooling, EmbeddingPooling::LastToken);
    assert_eq!(identity.dtype, Some(LocalOnnxDtype::Q8));
    assert!(identity.use_task_prefixes);
    assert_eq!(identity.query_prefix.as_deref(), Some("query: "));
    assert_eq!(identity.document_prefix.as_deref(), Some("passage: "));

    let fingerprint = identity.fingerprint().expect("serializable identity");
    assert_eq!(
        fingerprint,
        r#"{"v":1,"model":"custom/embedding-model","dims":1024,"pooling":"last_token","dtype":"q8","task_prefixes":true,"query_prefix":"query: ","document_prefix":"passage: "}"#,
        "the canonical shape is pinned byte for byte"
    );
}
