//! The auth seam.
//!
//! Credential-free localhost is the primary posture, so no login or prompt
//! ships. The seam supports exactly two things:
//!
//! - the local token `mem-web` injects per response into
//!   `window.__OPENCODE_MEM_TOKEN__` (read into memory, never persisted);
//! - an optional Basic credential for an auth-configured server.
//!
//! It never sends `Bearer` and it never invents scope. The token is read
//! through the `MarkerReader` seam so the logic is asserted in node mode; the
//! real `js_sys::Reflect::get` binding is the thin adapter below.

use base64::Engine as _;
use wasm_bindgen::JsValue;

use crate::api::envelope::ApiError;
use crate::api_request::RequestSpec;

/// The exact marker `mem-web`'s `inject_token_script` writes. A name, not a
/// credential: the value is read at runtime and never compiled in.
pub const TOKEN_MARKER: &str = "__OPENCODE_MEM_TOKEN__";

/// The header carrying the local token.
pub const TOKEN_HEADER: &str = "x-opencode-mem-token";

/// The seam over "where does the marker live".
pub trait MarkerReader {
    fn marker(&self) -> Option<String>;
}

/// The browser adapter: `Reflect::get(window, "__OPENCODE_MEM_TOKEN__")`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowMarkerReader;

impl MarkerReader for WindowMarkerReader {
    fn marker(&self) -> Option<String> {
        let window = web_sys::window()?;
        let value = js_sys::Reflect::get(&window, &JsValue::from_str(TOKEN_MARKER)).ok()?;
        value.as_string()
    }
}

/// Read the injected marker through an arbitrary reader; an empty marker reads
/// as absent.
pub fn read_marker_token_with(reader: &dyn MarkerReader) -> Option<String> {
    reader.marker().filter(|token| !token.is_empty())
}

/// Read the injected marker from the browser window.
pub fn read_marker_token() -> Option<String> {
    read_marker_token_with(&WindowMarkerReader)
}

/// A Basic credential for an auth-configured server. Forwarded only when the
/// caller supplies it; it is never read from storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicCredential {
    pub user: String,
    pub password: String,
}

impl BasicCredential {
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            password: password.into(),
        }
    }

    /// `Basic <base64(user:password)>`.
    pub fn authorization_value(&self) -> String {
        let raw = format!("{}:{}", self.user, self.password);
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
        format!("Basic {encoded}")
    }
}

/// Everything the seam may attach to a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthContext {
    pub local_token: Option<String>,
    pub basic: Option<BasicCredential>,
}

impl AuthContext {
    /// The default credential-free context.
    pub fn anonymous() -> Self {
        Self::default()
    }
}

/// Build the request headers.
///
/// `accept` is always present; the token header only when a token is present;
/// `content-type` only when the spec has a body; `authorization` only when a
/// credential was supplied.
pub fn build_headers(
    spec: &RequestSpec,
    auth: &AuthContext,
) -> Result<Vec<(String, String)>, ApiError> {
    let mut headers = Vec::with_capacity(4);
    headers.push(("accept".to_string(), "application/json".to_string()));

    if let Some(token) = auth
        .local_token
        .as_deref()
        .filter(|token| !token.is_empty())
    {
        headers.push((TOKEN_HEADER.to_string(), token.to_string()));
    }

    if spec.body.is_some() {
        headers.push(("content-type".to_string(), "application/json".to_string()));
    }

    if let Some(basic) = &auth.basic {
        headers.push(("authorization".to_string(), basic.authorization_value()));
    }

    Ok(headers)
}
