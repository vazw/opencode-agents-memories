//! Infallible HTTP response construction (ADR-0013 D-3).
//!
//! `axum::http::response::Builder::body(..)` returns a `Result` only because a
//! builder may carry a malformed status or header. Here the status is a
//! `StatusCode` (already validated by the type) and every header value is a
//! `const` [`HeaderValue`] built with the `const fn`
//! [`HeaderValue::from_static`], so an invalid literal is a **compile** error
//! and the construction is total by signature: no `expect`, no `unwrap`, no
//! reachable panic (D-3 elimination).
//!
//! A runtime header value (the CORS `Origin` echo) never goes through this
//! module's constants: the caller validates it with `HeaderValue::try_from`
//! and decides what a refusal means.

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Response, StatusCode, header};

/// `application/json` (the API envelope content type).
pub(crate) const JSON: HeaderValue = HeaderValue::from_static("application/json");

/// `application/json; charset=utf-8` (the raw-JSON content type).
pub(crate) const JSON_UTF8: HeaderValue =
    HeaderValue::from_static("application/json; charset=utf-8");

/// `text/plain`.
pub(crate) const TEXT_PLAIN: HeaderValue = HeaderValue::from_static("text/plain");

/// `text/plain; charset=utf-8` (the Basic-auth challenge).
pub(crate) const TEXT_PLAIN_UTF8: HeaderValue =
    HeaderValue::from_static("text/plain; charset=utf-8");

/// `text/markdown; charset=utf-8` (`GET /api/skills/{id}/content`).
pub(crate) const TEXT_MARKDOWN_UTF8: HeaderValue =
    HeaderValue::from_static("text/markdown; charset=utf-8");

/// `text/html` (the SPA shell).
pub(crate) const HTML: HeaderValue = HeaderValue::from_static("text/html");

/// `no-store` — never cache an authenticated or refusal answer.
pub(crate) const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

/// `no-cache` — always revalidate the shell.
pub(crate) const NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");

/// Builds a response with a `Content-Type` and any extra headers.
///
/// Total: `Response::new` + `status_mut` + `headers_mut().insert` cannot fail.
pub(crate) fn response(
    status: StatusCode,
    content_type: HeaderValue,
    extra: &[(HeaderName, HeaderValue)],
    body: Body,
) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    for (name, value) in extra {
        response.headers_mut().insert(name.clone(), value.clone());
    }
    response
}

/// Builds a response with **no** `Content-Type` header (a `204` preflight
/// answer carries none upstream).
pub(crate) fn response_without_content_type(
    status: StatusCode,
    extra: &[(HeaderName, HeaderValue)],
    body: Body,
) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    for (name, value) in extra {
        response.headers_mut().insert(name.clone(), value.clone());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    #[test]
    fn test_response_sets_status_and_content_type() {
        let built = response(StatusCode::NO_CONTENT, JSON, &[], Body::empty());
        assert_eq!(built.status(), StatusCode::NO_CONTENT);
        assert_eq!(built.headers().get(header::CONTENT_TYPE), Some(&JSON));

        let extra = [(header::CACHE_CONTROL, NO_STORE)];
        let forbidden = response(StatusCode::FORBIDDEN, JSON, &extra, Body::from("x"));
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            forbidden.headers().get(header::CACHE_CONTROL),
            Some(&NO_STORE)
        );
    }
}
