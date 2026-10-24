//! The upstream `ApiResponse<T>` envelope, built by hand.
//!
//! Upstream `web-server.ts` `jsonResponse` wraps a handler result:
//!
//! - `{"success":true,"data":…}` when the handler carries data;
//! - `{"success":true}` when it does not — the `data` key is **omitted**,
//!   never `null` (upstream's `JSON.stringify` drops `undefined`);
//! - `{"success":false,"error":"…"}`.
//!
//! `success` is always the first key, byte-stable. `serde_json::Map` sorts
//! its keys, so the envelope is assembled directly (the same house pattern as
//! `crate::error::error_envelope` / `crate::health::health_envelope`).
//!
//! A DTO whose `Serialize` fails must produce the D-9 `500` — never a panic
//! (ADR-0013 D-3): the serialization `Result` is matched, the cause goes to
//! the log, and the *response* carries the fixed `Internal Server Error` text.

use axum::body::Body;
use axum::http::{Response, StatusCode};

use crate::http;

/// The fixed message of an internal failure (D-9).
pub const INTERNAL_ERROR: &str = "Internal Server Error";

/// The fixed `success:false` envelope emitted when even the error message
/// cannot be serialized. A `&'static str` literal, so it cannot fail itself.
const FIXED_ERROR_BODY: &str = r#"{"success":false,"error":"Internal Server Error"}"#;

/// `{"success":true,"data":<serialized>}`, or `None` when the DTO cannot be
/// serialized (the caller answers the D-9 `500`).
// success body: a private Option carrier; the only caller `ok` maps None to the D-9 500 (ADR-0013 D-3)
fn success_body<T: serde::Serialize + ?Sized>(data: &T) -> Option<String> {
    match serde_json::to_string(data) {
        Ok(serialized) => Some(format!(r#"{{"success":true,"data":{serialized}}}"#)),
        Err(error) => {
            tracing::error!(error = %error, "response DTO failed to serialize");
            None
        }
    }
}

/// `{"success":true}` — the `data` key is omitted.
pub fn success_empty() -> String {
    r#"{"success":true}"#.to_string()
}

/// `{"success":false,"error":"<escaped>"}`.
///
/// On a serialization failure the fixed envelope is emitted and the cause is
/// logged (the response never carries a panic, and never an unescaped body).
pub fn error(message: &str) -> String {
    // error envelope: on failure log the cause and fall back to the fixed body (ADR-0013 D-3)
    match serde_json::to_string(message) {
        Ok(escaped) => format!(r#"{{"success":false,"error":{escaped}}}"#),
        Err(error) => {
            tracing::error!(error = %error, "error message failed to serialize");
            FIXED_ERROR_BODY.to_string()
        }
    }
}

/// Builds a JSON response with the given status.
pub fn json(status: StatusCode, body: String) -> Response<Body> {
    // Response::new + status_mut + headers_mut().insert — every step total (ADR-0013 D-3)
    http::response(status, http::JSON, &[], Body::from(body))
}

/// Builds a raw (non-envelope) JSON response with the upstream charset.
pub fn raw_json(status: StatusCode, body: String) -> Response<Body> {
    // the same infallible constructor; the content type is a const HeaderValue (ADR-0013 D-3)
    http::response(status, http::JSON_UTF8, &[], Body::from(body))
}

/// Builds a text response with an explicit content type.
pub fn text(
    status: StatusCode,
    content_type: axum::http::HeaderValue,
    body: String,
) -> Response<Body> {
    // the same constructor; `content_type` is a HeaderValue, so the 3 api/skills.rs call sites are compile-checked (ADR-0013 D-3)
    http::response(status, content_type, &[], Body::from(body))
}

/// `{"success":<flag>,"data":…}` — upstream's `handleRunMigration` answers
/// `success: result.success` while still carrying `data`.
///
/// `None` when the DTO cannot be serialized; the handler maps it to the
/// D-9 `500` (F11: before this, the flag form could never answer 500).
pub fn success_with_flag(flag: bool, data: &impl serde::Serialize) -> Option<String> {
    // the flag form can now answer a real 500: api/maintenance.rs maps None to internal_error() (ADR-0013 D-3 / F11)
    match serde_json::to_string(data) {
        Ok(serialized) => Some(format!(r#"{{"success":{flag},"data":{serialized}}}"#)),
        Err(error) => {
            tracing::error!(error = %error, "migration DTO failed to serialize");
            None
        }
    }
}

/// A `200` success envelope carrying `data`, or the D-9 `500` when the DTO
/// cannot be serialized.
pub fn ok<T: serde::Serialize + ?Sized>(data: &T) -> Response<Body> {
    match success_body(data) {
        Some(body) => json(StatusCode::OK, body),
        None => internal_error(),
    }
}

/// A `200` success envelope with the `data` key omitted.
pub fn ok_empty() -> Response<Body> {
    json(StatusCode::OK, success_empty())
}

/// A `200` upstream business error (`success:false`, HTTP 200).
pub fn business_error(message: &str) -> Response<Body> {
    json(StatusCode::OK, error(message))
}

/// A `400` client error.
pub fn bad_request(message: &str) -> Response<Body> {
    json(StatusCode::BAD_REQUEST, error(message))
}

/// The fixed `500` answer (D-9 — the detail goes to the log, never here).
pub fn internal_error() -> Response<Body> {
    json(StatusCode::INTERNAL_SERVER_ERROR, error(INTERNAL_ERROR))
}

/// A `501` documented stub.
pub fn not_implemented(message: &str) -> Response<Body> {
    json(StatusCode::NOT_IMPLEMENTED, error(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DTO whose `Serialize` always fails, so the 500 path is reachable.
    struct FailingSerialize;

    impl serde::Serialize for FailingSerialize {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "intentional serialization failure",
            ))
        }
    }

    #[test]
    fn test_failing_serialize_yields_internal_error_envelope() {
        assert!(success_body(&FailingSerialize).is_none());
        let response = ok(&FailingSerialize);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
