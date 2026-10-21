//! The typed REST envelope and the bounded errors.
//!
//! `ApiError` is created here (Step 5); `ApiEnvelope` + `parse_envelope` land in
//! Step 7. Every error path is bounded: the fixed messages carry no exception
//! detail, and the server's own `error` field is truncated to 200 characters.
//! The raw response body is never echoed, logged or rendered.

use serde::de::DeserializeOwned;

/// The bound on a server-supplied error message.
pub const MAX_ERROR_MESSAGE_CHARS: usize = 200;

/// Upstream's `ApiResultSchema` shape: the one envelope every endpoint uses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ApiEnvelope<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub message: Option<String>,
}

/// The bounded API error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The network call itself failed. Carries a fixed message, never the JS
    /// exception detail.
    Transport,
    /// The server answered with a non-success envelope or a 4xx/5xx status.
    Server { status: u16, message: String },
    /// The response could not be decoded (or a request could not be built).
    /// Carries a fixed message, never the body's bytes.
    Malformed,
}

impl ApiError {
    /// Build a server error, bounding the message to 200 characters.
    pub fn server(status: u16, message: impl Into<String>) -> Self {
        Self::Server {
            status,
            message: bound_message(&message.into()),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport => f.write_str("network request failed"),
            Self::Server { status, message } => write!(f, "server error {status}: {message}"),
            Self::Malformed => f.write_str("malformed request or response body"),
        }
    }
}

impl std::error::Error for ApiError {}

/// Truncate a server message to [`MAX_ERROR_MESSAGE_CHARS`] characters.
pub fn bound_message(message: &str) -> String {
    message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

/// Parse the typed envelope, threading the caller-supplied status.
///
/// - a 2xx success envelope yields the typed data;
/// - a failure envelope or a non-2xx status yields a bounded
///   [`ApiError::Server`] carrying the server's own `error`/`message`;
/// - an unparseable body (or a success envelope with no data) yields
///   [`ApiError::Malformed`] without echoing a single body byte.
pub fn parse_envelope<T: DeserializeOwned>(status: u16, bytes: &[u8]) -> Result<T, ApiError> {
    let envelope: ApiEnvelope<T> =
        serde_json::from_slice(bytes).map_err(|_| ApiError::Malformed)?;

    if (200..300).contains(&status) && envelope.success {
        return envelope.data.ok_or(ApiError::Malformed);
    }

    let message = envelope
        .error
        .or(envelope.message)
        .unwrap_or_else(|| "request failed".to_string());
    Err(ApiError::server(status, message))
}
