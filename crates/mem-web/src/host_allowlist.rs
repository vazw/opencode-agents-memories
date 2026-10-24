//! D-8 — the Host-header allowlist that refuses DNS-rebinding traffic.
//!
//! In a DNS-rebinding attack the attacker's page rebinds its own domain to
//! `127.0.0.1`; the browser then sends `Host: <attacker-domain>:4747` to the
//! local server. Because the rebinding makes the page *same-origin* with the
//! server, the CORS gate cannot stop it — the Host allowlist is the control
//! that does. The check runs first in the request gate, before any response
//! body can carry the local token.
//!
//! The comparison is name-based, not address-based: the port is parsed for
//! well-formedness only and never compared to the bound port (a reverse proxy
//! may front the server on another port).

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response as AxumResponse;

/// The loopback names that are always allowed.
const LOOPBACK_NAMES: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// The bind hosts that disable the allowlist entirely.
const WILDCARD_HOSTS: [&str; 2] = ["0.0.0.0", "::"];

/// The body of the refusal envelope.
const REFUSAL_BODY: &str = r#"{"success":false,"error":"Host header is not allowed."}"#;

/// The names a request's `Host` header may carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAllowlist {
    enforced: bool,
    names: Vec<String>,
}

impl HostAllowlist {
    /// Builds the allowlist for the host the server binds to.
    ///
    /// A wildcard bind (`0.0.0.0` / `::`) disables the allowlist: the
    /// operator has explicitly opted into network exposure, where
    /// `assert_network_auth` already demands an API token or a Basic
    /// password, and no name-based allowlist can discriminate such a bind.
    /// Otherwise the loopback names plus the configured host are allowed.
    pub fn for_bind_host(host: &str) -> Self {
        let normalized = normalize_host(host);
        if WILDCARD_HOSTS.contains(&normalized.as_str()) {
            return HostAllowlist {
                enforced: false,
                names: Vec::new(),
            };
        }

        let mut names: Vec<String> = Vec::with_capacity(LOOPBACK_NAMES.len() + 1);
        for name in LOOPBACK_NAMES {
            names.push(name.to_owned());
        }
        if !names.contains(&normalized) {
            names.push(normalized);
        }
        HostAllowlist {
            enforced: true,
            names,
        }
    }

    /// Whether the allowlist is enforced (`false` only for a wildcard bind).
    pub fn is_enforced(&self) -> bool {
        self.enforced
    }

    /// The gate predicate over the raw `Host` header value.
    ///
    /// A missing, empty or malformed `Host` is refused — never silently
    /// allowed.
    pub fn allows(&self, host_header: Option<&str>) -> bool {
        if !self.enforced {
            return true;
        }
        let Some(host_header) = host_header else {
            return false;
        };
        let Some(host) = parse_host_header(host_header) else {
            return false;
        };
        self.names.contains(&host)
    }

    /// The normalized allowed names.
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

/// The fixed 403 envelope for a refused `Host`.
///
/// It never carries the token, the `__OPENCODE_MEM_TOKEN__` marker, or an
/// echo of the offending (attacker-controlled) `Host` value.
pub fn host_refusal_response() -> AxumResponse {
    // infallible constructor; the refusal body is a const &str (ADR-0013 D-3)
    crate::http::response(
        StatusCode::FORBIDDEN,
        crate::http::JSON,
        &[(axum::http::header::CACHE_CONTROL, crate::http::NO_STORE)],
        Body::from(REFUSAL_BODY),
    )
}

/// Normalizes a host name: trimmed, ASCII-lowercased, IPv6 brackets stripped.
fn normalize_host(host: &str) -> String {
    let trimmed = host.trim();
    let without_brackets = match (trimmed.starts_with('['), trimmed.ends_with(']')) {
        (true, true) if trimmed.len() >= 2 => &trimmed[1..trimmed.len() - 1],
        _ => trimmed,
    };
    without_brackets.to_ascii_lowercase()
}

/// Parses a `Host` header value into its normalized host name.
///
/// Returns `None` when the value is empty or malformed. The port is parsed
/// for well-formedness only and is never compared to the bound port.
fn parse_host_header(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if value.starts_with('[') {
        // A bracketed IPv6 literal: the host ends at the closing bracket.
        let end = value.find(']')?;
        let host = &value[1..end];
        if host.is_empty() {
            return None;
        }
        let remainder = &value[end + 1..];
        if !remainder.is_empty() {
            let port = remainder.strip_prefix(':')?;
            if !is_well_formed_port(port) {
                return None;
            }
        }
        return Some(normalize_host(host));
    }

    match value.rfind(':') {
        Some(index) => {
            let prefix = &value[..index];
            if prefix.contains(':') {
                // A bare IPv6 literal such as `::1`: the whole value is the
                // host, not a host/port pair.
                Some(normalize_host(value))
            } else {
                let port = &value[index + 1..];
                if prefix.is_empty() || !is_well_formed_port(port) {
                    return None;
                }
                Some(normalize_host(prefix))
            }
        }
        None => Some(normalize_host(value)),
    }
}

/// Whether the port part is 1-5 ASCII digits.
fn is_well_formed_port(port: &str) -> bool {
    !port.is_empty() && port.len() <= 5 && port.bytes().all(|byte| byte.is_ascii_digit())
}
