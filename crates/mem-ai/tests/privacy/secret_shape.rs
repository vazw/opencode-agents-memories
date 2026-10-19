//! Port of `tests/secret-shape` contracts: `SecretSource`, `SecretString`,
//! `validate_secret_shape`, and the W2.1 `SecretResolver` conformance of
//! [`FileSecretResolver`] (AC5, AC6, D1, D3, D7).
//!
//! Hermetic: env access goes through an injected `EnvSource`, so no test
//! mutates the process environment.

#[path = "common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use mem_ai::privacy::secret::{
    FileSecretResolver, SecretSource, SecretString, validate_secret_shape,
};
use mem_ai::provider::{ProviderRegistry, SecretResolver};

fn assert_resolver<T: SecretResolver>() {}

fn env_with(pairs: &[(&str, &str)]) -> Arc<dyn mem_core::env::EnvSource + Send + Sync> {
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        map.insert((*key).to_owned(), (*value).to_owned());
    }
    Arc::new(map)
}

// ---------------------------------------------------------------------------
// AC5 — the W2.1 trait is implemented, not redefined, and the resolver is
// injectable.
// ---------------------------------------------------------------------------

#[test]
fn test_file_secret_resolver_implements_secret_resolver_trait() {
    assert_resolver::<FileSecretResolver>();
    let _boxed: Arc<dyn SecretResolver> = Arc::new(FileSecretResolver::new());
}

#[test]
fn test_file_secret_resolver_injects_into_registry() {
    let registry = ProviderRegistry::new(Arc::new(FileSecretResolver::new()));
    let _resolver: &Arc<dyn SecretResolver> = registry.resolver();
}

#[tokio::test]
async fn test_env_reference_missing_is_none() {
    // D3: the W2.1 trait defines `Ok(None)` as "simply not configured", so a
    // missing variable is not an error.
    let resolver = FileSecretResolver::new().with_env(env_with(&[]));
    assert_eq!(
        resolver.resolve("env://OPENAI_API_KEY").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn test_env_reference_resolves_from_injected_env() {
    let resolver =
        FileSecretResolver::new().with_env(env_with(&[("OPENAI_API_KEY", "sk-test-0123456789")]));
    assert_eq!(
        resolver.resolve("env://OPENAI_API_KEY").await.unwrap(),
        Some("sk-test-0123456789".to_owned())
    );
}

#[test]
fn test_secret_source_parses_env_reference() {
    assert_eq!(
        SecretSource::parse("env://OPENAI_API_KEY"),
        Some(SecretSource::Env {
            var: "OPENAI_API_KEY".to_owned()
        })
    );
}

#[test]
fn test_secret_source_parses_file_reference() {
    assert_eq!(
        SecretSource::parse("file:///tmp/secret"),
        Some(SecretSource::File {
            path: std::path::PathBuf::from("/tmp/secret")
        })
    );
}

#[test]
fn test_secret_source_literal_is_none() {
    // A bare value is a literal, not a reference.
    assert_eq!(SecretSource::parse("sk-literal-value"), None);
    assert_eq!(SecretSource::parse(""), None);
    assert_eq!(SecretSource::parse("vault://x"), None);
}

#[test]
fn test_secret_source_reference_round_trips() {
    for reference in ["env://OPENAI_API_KEY", "file:///tmp/secret"] {
        let parsed = SecretSource::parse(reference).expect("reference should parse");
        assert_eq!(parsed.reference(), reference);
    }
}

// ---------------------------------------------------------------------------
// AC6 — shape validation (D7) and `SecretString` hygiene (D1).
// ---------------------------------------------------------------------------

#[test]
fn test_valid_literal_secret_is_accepted() {
    assert!(validate_secret_shape("sk-test-0123456789").is_ok());
}

#[test]
fn test_known_vendor_prefixes_are_accepted() {
    // D7: shape validation is malformed-content rejection, never a vendor
    // allow-list — but every known prefix must still pass.
    let accepted = [
        "sk-proj-abcdefghijklmnop",
        "sk-ant-api03-abcdefghijklmnop",
        "gsk_abcdefghijklmnop",
        "AIzaSyD-abcdefghijklmnop",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "YWJjZGVmZ2hpamtsbW5vcA==",
        "atlas-key-without-known-prefix",
    ];
    for secret in accepted {
        assert!(
            validate_secret_shape(secret).is_ok(),
            "expected {secret} to be accepted"
        );
    }
}

#[test]
fn test_placeholder_secret_is_rejected() {
    for placeholder in ["your-api-key", "replace-with-your-api-key", "sk-ant-..."] {
        let error = validate_secret_shape(placeholder).unwrap_err();
        assert_eq!(error.kind, mem_ai::provider::ProviderErrorKind::Config);
    }
}

#[test]
fn test_empty_secret_is_rejected() {
    assert!(validate_secret_shape("").is_err());
}

#[test]
fn test_whitespace_only_secret_is_rejected() {
    assert!(validate_secret_shape("   \n\t ").is_err());
}

#[test]
fn test_interior_whitespace_is_rejected() {
    assert!(validate_secret_shape("sk-test 0123456789").is_err());
    assert!(validate_secret_shape("sk-test\t0123456789").is_err());
}

#[test]
fn test_control_characters_are_rejected() {
    assert!(validate_secret_shape("sk-test\u{0}0123456789").is_err());
    assert!(validate_secret_shape("sk-test\u{7f}0123456789").is_err());
}

#[test]
fn test_json_blob_is_rejected() {
    assert!(validate_secret_shape("{\"apiKey\":\"sk-test-0123456789\"}").is_err());
    assert!(validate_secret_shape("[\"sk-test-0123456789\"]").is_err());
    assert!(validate_secret_shape("\"sk-test-0123456789\"").is_err());
}

#[test]
fn test_too_short_secret_is_rejected() {
    assert!(validate_secret_shape("short").is_err());
    assert!(validate_secret_shape("1234567").is_err());
    assert!(validate_secret_shape("12345678").is_ok());
}

#[test]
fn test_secret_string_debug_redacts_value() {
    let secret = SecretString::new("sk-test-0123456789");
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains("sk-test-0123456789"));
    assert_eq!(rendered, "SecretString(***)");
}

#[test]
fn test_secret_string_expose_and_zeroize_clears_buffer() {
    let mut secret = SecretString::new("sk-test-0123456789");
    assert_eq!(secret.expose_secret(), "sk-test-0123456789");
    secret.zeroize();
    assert!(secret.expose_secret().is_empty());
}

#[test]
fn test_shape_error_never_echoes_the_value() {
    let value = "sk-test 0123456789";
    let error = validate_secret_shape(value).unwrap_err();
    assert!(!error.message.contains("0123456789"));
    assert!(!error.to_string().contains("0123456789"));
}
