//! HTTP Basic auth for the static shell — upstream `src/services/web-auth.ts`.
//!
//! The password and the username are trimmed; a blank password disables the
//! gate entirely. Credentials are compared in constant time.

use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::Response as AxumResponse;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use subtle::ConstantTimeEq;

/// The realm advertised in the `WWW-Authenticate` challenge.
pub const WEB_AUTH_REALM: &str = "OpenCode Memory Explorer";

/// The username used when neither an explicit nor an OS username is usable.
const FALLBACK_USERNAME: &str = "user";

/// The body of the challenge response.
const CHALLENGE_BODY: &str = "Authentication required";

/// The Basic-auth gate for the web server.
#[derive(Debug, Clone)]
pub struct WebAuth {
    enabled: bool,
    username: String,
    expected_username: Vec<u8>,
    expected_password: Vec<u8>,
}

impl WebAuth {
    /// Builds the gate from a password, an explicit username and the
    /// caller's OS-username fallback.
    ///
    /// A blank (or absent) password disables the gate; a blank username falls
    /// back to `os_username`, and a blank `os_username` falls back to
    /// `"user"`.
    pub fn new(password: Option<&str>, username: Option<&str>, os_username: &str) -> WebAuth {
        let password = password.unwrap_or_default().trim().to_owned();
        let enabled = !password.is_empty();

        let explicit = username.unwrap_or_default().trim();
        let effective_username = if explicit.is_empty() {
            let os_user = os_username.trim();
            if os_user.is_empty() {
                FALLBACK_USERNAME.to_owned()
            } else {
                os_user.to_owned()
            }
        } else {
            explicit.to_owned()
        };

        WebAuth {
            enabled,
            expected_username: effective_username.as_bytes().to_vec(),
            expected_password: password.as_bytes().to_vec(),
            username: effective_username,
        }
    }

    /// Whether Basic auth is enforced.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The effective username.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Whether the request may proceed.
    ///
    /// The health path is always exempt (the liveness probe must work
    /// without credentials), and a disabled gate admits everything.
    pub fn check(&self, authorization: Option<&str>, path: &str) -> bool {
        if !self.enabled {
            return true;
        }
        if path == "/api/health" {
            return true;
        }
        let Some(authorization) = authorization else {
            return false;
        };
        let Some((username, password)) = decode_basic_auth(authorization) else {
            return false;
        };

        // Both comparisons are evaluated (no short-circuit on the username)
        // so the comparison cost does not leak which half was wrong.
        let username_ok = bool::from(self.expected_username.ct_eq(username.as_bytes()));
        let password_ok = bool::from(self.expected_password.ct_eq(password.as_bytes()));
        username_ok & password_ok
    }

    /// The 401 challenge response.
    pub fn challenge(&self) -> AxumResponse {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("Content-Type", "text/plain; charset=utf-8")
            .header(
                "WWW-Authenticate",
                format!("Basic realm=\"{WEB_AUTH_REALM}\", charset=\"UTF-8\""),
            )
            .header("Cache-Control", "no-store")
            .body(Body::from(CHALLENGE_BODY))
            .expect("the challenge response is always a valid HTTP response")
    }
}

/// Decodes an `Authorization: Basic …` header into its username and password.
///
/// Returns `None` for a non-Basic scheme, an undecodable payload, a payload
/// that is not valid UTF-8, or a payload without a `:` separator.
fn decode_basic_auth(header: &str) -> Option<(String, String)> {
    let lowered = header.to_ascii_lowercase();
    if !lowered.starts_with("basic ") {
        return None;
    }
    let encoded = header.get(6..)?.trim();
    if encoded.is_empty() {
        return None;
    }
    let decoded = STANDARD.decode(encoded).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let colon = text.find(':')?;
    Some((text[..colon].to_owned(), text[colon + 1..].to_owned()))
}
