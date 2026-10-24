//! CORS policy — upstream `src/services/cors.ts`.
//!
//! Upstream only advertises CORS headers on preflight responses; a rejected
//! origin gets a 403 envelope with no `Access-Control-*` header at all.

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response as AxumResponse;

use crate::http;

/// The loopback hosts a browser origin may come from.
const ALLOWED_LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// The methods advertised on a preflight response.
const CORS_ALLOWED_METHODS: &str = "GET, POST, PUT, DELETE, OPTIONS";

/// The request headers advertised on a preflight response.
const CORS_ALLOWED_HEADERS: &str = "Content-Type";

/// The preflight cache lifetime.
const CORS_MAX_AGE: &str = "600";

/// The body of the rejected-origin envelope.
const DISALLOWED_BODY: &str =
    r#"{"success":false,"error":"Cross-origin requests are restricted to loopback origins."}"#;

/// Decides whether a browser origin may talk to the server.
///
/// A missing `Origin` (a non-browser request) is allowed. An origin is
/// allowed when it is an `http`/`https` URL whose host is loopback, or when
/// Basic auth is enabled — upstream opens the gate in that case because the
/// auth challenge is then the real control.
pub fn is_allowed_browser_origin(origin: Option<&str>, http_auth_enabled: bool) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }
    let host = url.host_str().unwrap_or_default();
    if ALLOWED_LOOPBACK_HOSTS.contains(&host) {
        return true;
    }
    http_auth_enabled
}

/// The preflight (`OPTIONS`) response.
pub fn preflight_response(origin: Option<&str>, http_auth_enabled: bool) -> AxumResponse {
    if !is_allowed_browser_origin(origin, http_auth_enabled) {
        return disallowed_response();
    }

    // infallible constructor: the static values are `const HeaderValue`s and the
    // echoed Origin is validated; a failure DENIES, it never echoes (ADR-0013 D-3)
    let mut extra: Vec<(HeaderName, HeaderValue)> = vec![(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static(CORS_MAX_AGE),
    )];
    if let Some(origin) = origin {
        let Ok(allow_origin) = HeaderValue::try_from(origin) else {
            return disallowed_response();
        };
        extra.push((header::ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin));
        extra.push((
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(CORS_ALLOWED_METHODS),
        ));
        extra.push((
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(CORS_ALLOWED_HEADERS),
        ));
        extra.push((header::VARY, HeaderValue::from_static("Origin")));
    }
    http::response_without_content_type(StatusCode::NO_CONTENT, &extra, Body::empty())
}

/// The 403 envelope for a rejected cross-origin request.
pub fn disallowed_response() -> AxumResponse {
    // infallible constructor with the const JSON content type and a fixed body (ADR-0013 D-3)
    http::response(
        StatusCode::FORBIDDEN,
        http::JSON,
        &[],
        Body::from(DISALLOWED_BODY),
    )
}
