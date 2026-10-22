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

use axum::body::Body;
use axum::http::{Response, StatusCode};

/// The fixed message of an internal failure (D-9).
pub const INTERNAL_ERROR: &str = "Internal Server Error";

/// `{"success":true,"data":<serialized>}`.
pub fn success<T: serde::Serialize + ?Sized>(data: &T) -> String {
    let serialized = serde_json::to_string(data).expect("a response DTO is always serializable");
    format!(r#"{{"success":true,"data":{serialized}}}"#)
}

/// `{"success":true}` — the `data` key is omitted.
pub fn success_empty() -> String {
    r#"{"success":true}"#.to_string()
}

/// `{"success":false,"error":"<escaped>"}`.
pub fn error(message: &str) -> String {
    let escaped = serde_json::to_string(message).expect("a string is always serializable");
    format!(r#"{{"success":false,"error":{escaped}}}"#)
}

/// Builds a JSON response with the given status.
pub fn json(status: StatusCode, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .expect("the response is always a valid HTTP response")
}

/// Builds a raw (non-envelope) JSON response with the upstream charset.
pub fn raw_json(status: StatusCode, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(Body::from(body))
        .expect("the response is always a valid HTTP response")
}

/// Builds a text response with an explicit content type.
pub fn text(status: StatusCode, content_type: &str, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .body(Body::from(body))
        .expect("the response is always a valid HTTP response")
}

/// `{"success":<flag>,"data":…}` — upstream's `handleRunMigration` answers
/// `success: result.success` while still carrying `data`.
pub fn success_with_flag(flag: bool, data: &impl serde::Serialize) -> String {
    let serialized = serde_json::to_string(data).expect("a response DTO is always serializable");
    format!(r#"{{"success":{flag},"data":{serialized}}}"#)
}

/// A `200` success envelope carrying `data`.
pub fn ok<T: serde::Serialize + ?Sized>(data: &T) -> Response<Body> {
    json(StatusCode::OK, success(data))
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
