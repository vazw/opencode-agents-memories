//! Auth middleware for the remote provider plumbing.
//!
//! The outbound request never carries a plaintext secret until the last
//! moment: the caller describes *how* to authenticate ([`AuthStrategy`]) by
//! naming a [`SecretResolver`] reference, and this module resolves it
//! per-call, returning the single `(header, value)` pair to inject.
//!
//! Security invariants:
//! - the resolved secret is never logged, never formatted, never stored on the
//!   provider struct — it lives only in the request's header map for the
//!   duration of the call;
//! - a missing reference (`Ok(None)`) or an unset reference (`None`) yields no
//!   header rather than an error, matching upstream (`if (config.apiKey)`).

use std::sync::Arc;

use crate::provider::error::ProviderError;
use crate::provider::secret_resolver::SecretResolver;

/// How a remote provider authenticates an outbound request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AuthStrategy {
    /// No auth header at all.
    #[default]
    None,
    /// `Authorization: Bearer <secret>` (OpenAI-compatible providers).
    Bearer {
        /// `SecretResolver` reference; `None` disables the header.
        secret_ref: Option<String>,
    },
    /// An arbitrary header carrying the raw secret (e.g. `x-api-key`).
    ApiKey {
        /// Header name, e.g. `"x-api-key"`.
        header: String,
        /// `SecretResolver` reference; `None` disables the header.
        secret_ref: Option<String>,
    },
}

impl AuthStrategy {
    /// Bearer strategy over a resolver reference.
    pub fn bearer(secret_ref: impl Into<String>) -> Self {
        Self::Bearer {
            secret_ref: Some(secret_ref.into()),
        }
    }

    /// Named-header strategy over a resolver reference.
    pub fn api_key(header: impl Into<String>, secret_ref: impl Into<String>) -> Self {
        Self::ApiKey {
            header: header.into(),
            secret_ref: Some(secret_ref.into()),
        }
    }

    /// The resolver reference this strategy points at, if any.
    pub fn secret_ref(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Bearer { secret_ref } => secret_ref.as_deref(),
            Self::ApiKey { secret_ref, .. } => secret_ref.as_deref(),
        }
    }
}

/// Resolve the auth header for `strategy`.
///
/// Returns `Ok(None)` when no header should be injected (no strategy, no
/// reference, or the reference is unconfigured). The returned value is the
/// plaintext secret and must not be logged by callers.
pub async fn resolve_auth(
    resolver: &Arc<dyn SecretResolver>,
    strategy: &AuthStrategy,
) -> Result<Option<(String, String)>, ProviderError> {
    let Some(reference) = strategy.secret_ref() else {
        return Ok(None);
    };
    let Some(secret) = resolver.resolve(reference).await? else {
        return Ok(None);
    };
    let header = match strategy {
        AuthStrategy::None => return Ok(None),
        AuthStrategy::Bearer { .. } => "authorization".to_owned(),
        AuthStrategy::ApiKey { header, .. } => header.to_ascii_lowercase(),
    };
    let value = match strategy {
        AuthStrategy::Bearer { .. } => format!("Bearer {secret}"),
        _ => secret,
    };
    Ok(Some((header, value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::secret_resolver::StaticSecretResolver;

    #[tokio::test]
    async fn bearer_resolves_from_resolver() {
        let resolver: Arc<dyn SecretResolver> =
            Arc::new(StaticSecretResolver::new().with_secret("env:OPENAI_API_KEY", "sk-test"));
        let header = resolve_auth(&resolver, &AuthStrategy::bearer("env:OPENAI_API_KEY"))
            .await
            .expect("resolves");
        assert_eq!(
            header,
            Some(("authorization".to_owned(), "Bearer sk-test".to_owned()))
        );
    }

    #[tokio::test]
    async fn api_key_header_is_lowercased_and_raw() {
        let resolver: Arc<dyn SecretResolver> =
            Arc::new(StaticSecretResolver::new().with_secret("env:ANTHROPIC_API_KEY", "sk-ant"));
        let header = resolve_auth(
            &resolver,
            &AuthStrategy::api_key("x-api-key", "env:ANTHROPIC_API_KEY"),
        )
        .await
        .expect("resolves");
        assert_eq!(header, Some(("x-api-key".to_owned(), "sk-ant".to_owned())));
    }

    #[tokio::test]
    async fn unconfigured_reference_yields_no_header() {
        let resolver: Arc<dyn SecretResolver> = Arc::new(StaticSecretResolver::new());
        assert_eq!(
            resolve_auth(&resolver, &AuthStrategy::bearer("env:MISSING"))
                .await
                .expect("resolves"),
            None
        );
        assert_eq!(
            resolve_auth(&resolver, &AuthStrategy::None)
                .await
                .expect("resolves"),
            None
        );
    }
}
