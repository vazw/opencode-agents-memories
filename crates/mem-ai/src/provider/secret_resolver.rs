//! `SecretResolver` — the secret-lookup seam.
//!
//! Defined here so W2.1 ships independently: the registry receives a
//! `Arc<dyn SecretResolver>` by injection and never constructs one. W2.3 adds
//! the file-backed implementation ([`crate::privacy::secret::FileSecretResolver`],
//! re-exported as [`crate::provider::FileSecretResolver`]) with strict
//! permission checks; this module provides the trait plus an in-memory
//! implementation for tests.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::provider::error::ProviderError;

/// Resolves a secret reference (e.g. `"env:OPENAI_API_KEY"` or a vault key)
/// to its plaintext value.
///
/// Implementations are injected into [`crate::provider::ProviderRegistry`];
/// the registry never builds one itself.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    /// Resolve `reference`.
    ///
    /// Returns `Ok(None)` when the reference is simply not configured, and
    /// `Err(_)` only for a genuine resolution failure (unreadable file, bad
    /// permissions, malformed reference).
    async fn resolve(&self, reference: &str) -> Result<Option<String>, ProviderError>;
}

/// In-memory resolver: a fixed map of reference → value.
///
/// Used by tests and as the default when no real resolver is wired in. It is
/// never populated from disk, so it cannot leak filesystem secrets.
#[derive(Debug, Default, Clone)]
pub struct StaticSecretResolver {
    secrets: HashMap<String, String>,
}

impl StaticSecretResolver {
    /// An empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed one reference.
    pub fn with_secret(mut self, reference: impl Into<String>, value: impl Into<String>) -> Self {
        self.secrets.insert(reference.into(), value.into());
        self
    }
}

#[async_trait]
impl SecretResolver for StaticSecretResolver {
    async fn resolve(&self, reference: &str) -> Result<Option<String>, ProviderError> {
        Ok(self.secrets.get(reference).cloned())
    }
}
