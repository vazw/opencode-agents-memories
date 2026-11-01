//! `ProviderError` — the crate-wide provider error seam.
//!
//! Every failure of an [`crate::provider::AiProvider`] is normalised onto this
//! single type so no provider-specific error type can leak above the trait
//! boundary (see `test_no_provider_leakage`). The shape is
//! `{ kind, provider, source }` plus an optional HTTP `status`.

use std::fmt;

/// Stable classification of a provider failure.
///
/// Kept provider-agnostic on purpose: callers branch on the kind, never on a
/// concrete provider's error enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProviderErrorKind {
    /// The registry/config could not produce a usable provider.
    Config,
    /// Authentication/authorisation was rejected.
    Auth,
    /// The transport failed before a response existed.
    Network,
    /// The request exceeded its configured budget.
    Timeout,
    /// The caller's cancellation token fired.
    Cancelled,
    /// The provider asked us to back off (HTTP 429).
    RateLimited,
    /// The provider returned a non-2xx status.
    HttpStatus,
    /// The response could not be parsed / did not match the wire contract.
    Protocol,
    /// The response was valid JSON but did not carry the fields the wire contract
    /// requires — a SHAPE mismatch, not a parse failure.
    WireShape,
    /// The provider does not implement the requested capability.
    Unsupported,
    /// Anything else.
    Internal,
}

impl ProviderErrorKind {
    /// Stable, log-friendly label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Auth => "auth",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::RateLimited => "rate-limited",
            Self::HttpStatus => "http-status",
            Self::Protocol => "protocol",
            Self::WireShape => "wire-shape",
            Self::Unsupported => "unsupported",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ProviderErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The single error type crossing the provider seam.
#[derive(Debug, thiserror::Error)]
#[error("{provider} {kind}: {message}")]
pub struct ProviderError {
    /// Failure classification.
    pub kind: ProviderErrorKind,
    /// Which provider produced the failure (e.g. `"opencode-host"`,
    /// `"registry"`).
    pub provider: String,
    /// Human-readable, secret-free message.
    pub message: String,
    /// HTTP status when the failure came from a response.
    pub status: Option<u16>,
    /// Underlying cause, when one exists.
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl ProviderError {
    /// Build an error with the given kind + message and the fallback provider
    /// label `"mem-ai"`.
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            provider: "mem-ai".to_owned(),
            message: message.into(),
            status: None,
            source: None,
        }
    }

    /// Configuration failure.
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Config, message)
    }

    /// Cancellation failure.
    pub fn cancelled(provider: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Cancelled, "request cancelled").with_provider(provider)
    }

    /// Timeout failure for the given budget.
    pub fn timeout(provider: impl Into<String>, budget: std::time::Duration) -> Self {
        Self::new(
            ProviderErrorKind::Timeout,
            format!("timed out after {}ms", budget.as_millis()),
        )
        .with_provider(provider)
    }

    /// Network/transport failure.
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Network, message)
    }

    /// Attach (or replace) the provider label.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }

    /// Attach an HTTP status.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Attach an underlying cause.
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Classify an HTTP status into the matching kind.
    pub fn from_status(
        provider: impl Into<String>,
        status: u16,
        message: impl Into<String>,
    ) -> Self {
        let kind = match status {
            401 | 403 => ProviderErrorKind::Auth,
            429 => ProviderErrorKind::RateLimited,
            _ => ProviderErrorKind::HttpStatus,
        };
        Self::new(kind, message)
            .with_provider(provider)
            .with_status(status)
    }

    /// Retryable when the failure is transient: 429, 5xx, network, or timeout.
    pub fn is_retryable(&self) -> bool {
        match self.kind {
            ProviderErrorKind::Network
            | ProviderErrorKind::Timeout
            | ProviderErrorKind::RateLimited => true,
            ProviderErrorKind::HttpStatus => {
                matches!(self.status, Some(429) | Some(500..=599))
            }
            _ => false,
        }
    }

    /// Cancellation is a normal shutdown signal, not a failure to retry.
    pub fn is_cancelled(&self) -> bool {
        self.kind == ProviderErrorKind::Cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn display_names_provider_and_kind() {
        let error = ProviderError::timeout("opencode-host", Duration::from_millis(30));
        assert_eq!(
            error.to_string(),
            "opencode-host timeout: timed out after 30ms"
        );
        assert!(error.is_retryable());
        assert!(!error.is_cancelled());
    }

    #[test]
    fn status_classification_and_retryability() {
        assert_eq!(
            ProviderError::from_status("p", 401, "no").kind,
            ProviderErrorKind::Auth
        );
        assert_eq!(
            ProviderError::from_status("p", 429, "slow").kind,
            ProviderErrorKind::RateLimited
        );
        assert!(ProviderError::from_status("p", 429, "slow").is_retryable());
        assert!(ProviderError::from_status("p", 503, "down").is_retryable());
        assert!(!ProviderError::from_status("p", 404, "gone").is_retryable());
        assert_eq!(
            ProviderError::from_status("p", 404, "gone").status,
            Some(404)
        );
    }

    #[test]
    fn cancellation_is_flagged() {
        let error = ProviderError::cancelled("opencode-host");
        assert!(error.is_cancelled());
        assert!(!error.is_retryable());
    }

    #[test]
    fn source_chain_is_preserved() {
        let cause = std::io::Error::other("boom");
        let error = ProviderError::network("transport").with_source(cause);
        assert!(std::error::Error::source(&error).is_some());
    }
}
