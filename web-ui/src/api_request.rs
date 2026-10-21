//! The single fetch seam.
//!
//! The seam is written directly on `web-sys` so `RequestMode::SameOrigin` and
//! `RequestCredentials::SameOrigin` are explicit rather than a library default.
//! `build_url`, `prepare`, `finish` and `map_transport` are pure; `request<T>`
//! is the one-line adapter that performs the `fetch`.

use serde::de::DeserializeOwned;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, RequestCredentials, RequestInit, RequestMode, Response};

use crate::api::auth::{self, AuthContext};
use crate::api::envelope::{ApiError, parse_envelope};
use crate::paths::encode_component;

/// One API request, as the caller describes it. The seam appends nothing of its
/// own: no scope, no project id, no session id.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestSpec {
    pub method: &'static str,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<serde_json::Value>,
}

impl RequestSpec {
    pub fn new(method: &'static str, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            body: None,
        }
    }

    pub fn get(path: impl Into<String>) -> Self {
        Self::new("GET", path)
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self::new("POST", path)
    }

    pub fn put(path: impl Into<String>) -> Self {
        Self::new("PUT", path)
    }

    pub fn patch(path: impl Into<String>) -> Self {
        Self::new("PATCH", path)
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self::new("DELETE", path)
    }

    pub fn with_query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((key.into(), value.into()));
        self
    }

    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// Build the root-relative request URL.
///
/// Refused: an absolute (`https://…`), protocol-relative (`//…`) or
/// backslash-bearing path. The query is percent-encoded and carries only the
/// caller's pairs.
pub fn build_url(spec: &RequestSpec) -> Result<String, ApiError> {
    validate_path(&spec.path)?;

    let mut url = String::with_capacity(spec.path.len() + 16);
    url.push_str(&spec.path);
    if !spec.query.is_empty() {
        url.push('?');
        for (index, (key, value)) in spec.query.iter().enumerate() {
            if index > 0 {
                url.push('&');
            }
            url.push_str(&encode_component(key));
            url.push('=');
            url.push_str(&encode_component(value));
        }
    }
    Ok(url)
}

fn validate_path(path: &str) -> Result<(), ApiError> {
    let root_relative = path.starts_with('/') && !path.starts_with("//");
    let carries_scheme = path.contains("://");
    let carries_backslash = path.contains('\\');
    let carries_control = path.contains(['\n', '\r', '\t']);

    if !root_relative || carries_scheme || carries_backslash || carries_control {
        return Err(ApiError::Malformed);
    }
    Ok(())
}

/// The pure request shape: method, same-origin mode, same-origin credentials.
///
/// It carries no header and no body value, so it can never carry a credential.
pub fn init_for(spec: &RequestSpec) -> RequestInit {
    let init = RequestInit::new();
    init.set_method(spec.method);
    init.set_mode(RequestMode::SameOrigin);
    init.set_credentials(RequestCredentials::SameOrigin);
    init
}

/// Build the URL and the full `RequestInit` (headers + JSON body). No network.
pub fn prepare<T>(
    spec: &RequestSpec,
    auth: &AuthContext,
) -> Result<(String, RequestInit), ApiError> {
    let url = build_url(spec)?;
    let pairs = auth::build_headers(spec, auth)?;
    let headers = headers_from(&pairs)?;

    let init = init_for(spec);
    init.set_headers(headers.as_ref());

    if let Some(body) = &spec.body {
        let json = serde_json::to_string(body).map_err(|_| ApiError::Malformed)?;
        init.set_body(&JsValue::from_str(&json));
    }

    Ok((url, init))
}

/// Interpret a fetched response through the typed envelope, threading the
/// status the caller read from `Response::status()`.
pub fn finish<T: DeserializeOwned>(status: u16, bytes: &[u8]) -> Result<T, ApiError> {
    parse_envelope::<T>(status, bytes)
}

/// Map a failed `fetch` promise to the bounded transport error. The JS
/// exception detail is deliberately dropped.
pub fn map_transport(_detail: JsValue) -> ApiError {
    ApiError::Transport
}

/// The `fetch` adapter: `prepare` + `fetch` + `finish`.
///
/// This is the only function in the crate that performs I/O; it is exercised by
/// the Step 9 browser check rather than the node-mode test suite.
pub async fn request<T: DeserializeOwned>(
    spec: &RequestSpec,
    auth: &AuthContext,
) -> Result<T, ApiError> {
    let (url, init) = prepare::<T>(spec, auth)?;

    let window = web_sys::window().ok_or(ApiError::Transport)?;
    let response = JsFuture::from(window.fetch_with_str_and_init(&url, &init))
        .await
        .map_err(map_transport)?;
    let response: Response = response.dyn_into().map_err(map_transport)?;

    let status = response.status();
    let buffer = JsFuture::from(response.array_buffer().map_err(|_| ApiError::Transport)?)
        .await
        .map_err(map_transport)?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();

    finish(status, &bytes)
}

fn headers_from(pairs: &[(String, String)]) -> Result<Headers, ApiError> {
    let headers = Headers::new().map_err(|_| ApiError::Transport)?;
    for (name, value) in pairs {
        headers
            .append(name, value)
            .map_err(|_| ApiError::Transport)?;
    }
    Ok(headers)
}
