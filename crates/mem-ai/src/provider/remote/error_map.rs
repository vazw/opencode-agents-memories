//! Unified HTTP status → [`ProviderError`] mapping.
//!
//! Every adapter routes non-2xx responses through here so the classification
//! is uniform: 401/403 → [`ProviderErrorKind::Auth`], 429 →
//! [`ProviderErrorKind::RateLimited`], everything else →
//! [`ProviderErrorKind::HttpStatus`]. Providers may narrow the mapping with a
//! [`StatusOverrides`] table (e.g. a gateway that signals rate limiting with a
//! non-429 status) without forking the default classification.
//!
//! Response bodies are **never** echoed verbatim: only a bounded, JSON-parsed
//! `error.message`/`message` (provider-authored, not user prompt data) is
//! surfaced, and it is truncated. This mirrors the OpenCode host provider's
//! redaction policy and keeps secrets/prompts out of logs.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::provider::error::{ProviderError, ProviderErrorKind};

/// Maximum length of a provider-authored error message echoed back.
const MAX_MESSAGE_CHARS: usize = 200;

/// Provider-specific status overrides for [`map_status_with`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusOverrides {
    entries: BTreeMap<u16, ProviderErrorKind>,
}

impl StatusOverrides {
    /// An empty override table (default classification everywhere).
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) the classification for `status`.
    pub fn with(mut self, status: u16, kind: ProviderErrorKind) -> Self {
        self.entries.insert(status, kind);
        self
    }

    /// The override for `status`, if any.
    pub fn kind_for(&self, status: u16) -> Option<ProviderErrorKind> {
        self.entries.get(&status).copied()
    }

    /// Whether the table holds no overrides.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Default classification of an HTTP status.
pub fn default_kind(status: u16) -> ProviderErrorKind {
    match status {
        401 | 403 => ProviderErrorKind::Auth,
        429 => ProviderErrorKind::RateLimited,
        _ => ProviderErrorKind::HttpStatus,
    }
}

/// Map a non-2xx response onto the unified error with default classification.
pub fn map_status(provider: &str, status: u16, body: &str) -> ProviderError {
    map_status_with(provider, status, body, &StatusOverrides::new())
}

/// Map a non-2xx response, honouring provider `overrides` first.
pub fn map_status_with(
    provider: &str,
    status: u16,
    body: &str,
    overrides: &StatusOverrides,
) -> ProviderError {
    let kind = overrides
        .kind_for(status)
        .unwrap_or_else(|| default_kind(status));
    let mut message = format!("HTTP {status}");
    if let Some(detail) = safe_error_message(body) {
        message.push_str(": ");
        message.push_str(&detail);
    }
    ProviderError::new(kind, message)
        .with_provider(provider)
        .with_status(status)
}

/// Extract a bounded, provider-authored error message from a JSON body.
///
/// Only known error fields are read, and only when they are strings; the raw
/// body is never returned. Returns `None` for non-JSON/empty bodies.
fn safe_error_message(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    let candidate = value
        .get("error")
        .and_then(|error| match error {
            Value::String(message) => Some(message.as_str()),
            Value::Object(object) => object.get("message").and_then(Value::as_str),
            _ => None,
        })
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| value.get("msg").and_then(Value::as_str))?;
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }
    Some(truncate(candidate, MAX_MESSAGE_CHARS))
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let mut out: String = value.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::error::ProviderErrorKind;

    #[test]
    fn default_classification_covers_auth_and_rate_limit() {
        assert_eq!(default_kind(401), ProviderErrorKind::Auth);
        assert_eq!(default_kind(403), ProviderErrorKind::Auth);
        assert_eq!(default_kind(429), ProviderErrorKind::RateLimited);
        assert_eq!(default_kind(500), ProviderErrorKind::HttpStatus);
        assert_eq!(default_kind(404), ProviderErrorKind::HttpStatus);
    }

    #[test]
    fn overrides_take_precedence() {
        let overrides = StatusOverrides::new().with(400, ProviderErrorKind::RateLimited);
        let error = map_status_with("p", 400, "", &overrides);
        assert_eq!(error.kind, ProviderErrorKind::RateLimited);
        assert_eq!(error.status, Some(400));
        // Untouched statuses keep the default.
        assert_eq!(
            map_status_with("p", 401, "", &overrides).kind,
            ProviderErrorKind::Auth
        );
    }

    #[test]
    fn message_is_json_parsed_bounded_and_never_raw() {
        let error = map_status(
            "p",
            400,
            r#"{"error":{"message":"bad temperature value","prompt":"secret prompt"}}"#,
        );
        assert!(error.message.contains("bad temperature value"));
        assert!(!error.message.contains("secret prompt"));

        let long = "x".repeat(500);
        let body = format!(r#"{{"message":"{long}"}}"#);
        let error = map_status("p", 500, &body);
        assert!(error.message.chars().count() < 500);
        assert!(error.message.ends_with('…'));

        // Non-JSON body: no echo at all.
        let error = map_status("p", 502, "<html>upstream failed</html>");
        assert_eq!(error.message, "HTTP 502");
    }

    #[test]
    fn error_carries_provider_and_status() {
        let error = map_status("anthropic", 429, "");
        assert_eq!(error.provider, "anthropic");
        assert_eq!(error.status, Some(429));
        assert!(error.is_retryable());
    }
}
