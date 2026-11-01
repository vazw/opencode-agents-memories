//! `ProviderError` — the crate-wide provider error seam.
//!
//! Every failure of an [`crate::provider::AiProvider`] is normalised onto this
//! single type so no provider-specific error type can leak above the trait
//! boundary (see `test_no_provider_leakage`). The shape is
//! `{ kind, provider, source }` plus an optional HTTP `status`, and — for
//! failures decoded from a host's declared error body — three machine-readable
//! fields: the closed class token ([`wire_reason`]), a bounded operator
//! `detail`, and the host's `retryAfterMs` hint.

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
    /// The model's reply carried no structured value under any of the three
    /// recovery shapes — a named, retryable model-output miss, not a wire
    /// contract break.
    ModelOutput,
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
            Self::ModelOutput => "model-output",
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

/// The closed class tokens a decoded host failure can carry.
///
/// A token is the ONLY input a caller may branch retryability on: messages are
/// fixed and generic, and `detail` is an operator payload that no decision may
/// parse. [`ALL`] lets an exhaustive table test pin the set.
pub mod wire_reason {
    /// The request body did not match the route's declared schema.
    pub const INVALID_REQUEST: &str = "invalid-request";
    /// The host rejected the credential.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The configured provider id is not known to the host.
    pub const PROVIDER_NOT_FOUND: &str = "provider-not-found";
    /// The configured model id is not known to the host.
    pub const MODEL_NOT_FOUND: &str = "model-not-found";
    /// The configured location (working directory) is not known to the host.
    pub const LOCATION_NOT_FOUND: &str = "location-not-found";
    /// The provider does not support the requested capability for this model.
    pub const MODEL_UNSUPPORTED: &str = "model-unsupported";
    /// The upstream provider failed transiently (502, retryable reason).
    pub const PROVIDER_REQUEST_RETRYABLE: &str = "provider-request-retryable";
    /// The upstream provider failed terminally (502, terminal reason).
    pub const PROVIDER_REQUEST_TERMINAL: &str = "provider-request-terminal";
    /// The host is temporarily unable to serve the request.
    pub const SERVICE_UNAVAILABLE: &str = "service-unavailable";
    /// The route is gone: a 404 whose body carries no decodable tag.
    pub const ROUTE_MOVED: &str = "route-moved";
    /// A decodable error tag this build does not know.
    pub const UNRECOGNISED: &str = "unrecognised";

    /// Every token, in declaration order.
    pub const ALL: [&str; 11] = [
        INVALID_REQUEST,
        UNAUTHORIZED,
        PROVIDER_NOT_FOUND,
        MODEL_NOT_FOUND,
        LOCATION_NOT_FOUND,
        MODEL_UNSUPPORTED,
        PROVIDER_REQUEST_RETRYABLE,
        PROVIDER_REQUEST_TERMINAL,
        SERVICE_UNAVAILABLE,
        ROUTE_MOVED,
        UNRECOGNISED,
    ];
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
    /// The closed class token, when the failure was decoded from a declared
    /// error body ([`wire_reason`]).
    pub wire_reason: Option<&'static str>,
    /// The host's `retryAfterMs` hint, when it sent one.
    pub retry_after_ms: Option<u64>,
    /// Enumerated operator sub-values built at decode time (never the host's
    /// `message`, never a reply), each bounded to 120 chars.
    ///
    /// Boxed string: this error type is returned all over the crate, and the
    /// three new machine-readable fields would otherwise push it over clippy's
    /// `result_large_err` threshold.
    pub detail: Option<Box<str>>,
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
            wire_reason: None,
            retry_after_ms: None,
            detail: None,
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

    /// Attach the closed failure class token.
    pub fn with_wire_reason(mut self, wire_reason: &'static str) -> Self {
        self.wire_reason = Some(wire_reason);
        self
    }

    /// Attach the host's retry-after hint (milliseconds).
    pub fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    /// Attach the enumerated operator detail.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into().into_boxed_str());
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

    /// Retryable when the failure is transient: 429, 5xx, network, timeout, or
    /// a model-output miss (the model may well produce a parseable reply next
    /// time).
    pub fn is_retryable(&self) -> bool {
        match self.kind {
            ProviderErrorKind::Network
            | ProviderErrorKind::Timeout
            | ProviderErrorKind::RateLimited
            | ProviderErrorKind::ModelOutput => true,
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
