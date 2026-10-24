//! Request-lifecycle constants and the panic envelope.
//!
//! Upstream `src/services/request-timeouts.ts`.

use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;

/// The external profile cleanup deadline.
pub const EXTERNAL_PROFILE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(60);

/// The opencode profile cleanup deadline.
pub const OPENCODE_PROFILE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(300);

/// The grace added on top of the slowest cleanup deadline.
const HTTP_TIMEOUT_GRACE: Duration = Duration::from_secs(30);

/// The larger of two durations, in seconds (const-friendly).
const fn max_secs(left: u64, right: u64) -> u64 {
    if left >= right { left } else { right }
}

/// The HTTP idle timeout — always above every cleanup deadline.
pub const HTTP_IDLE_TIMEOUT: Duration = Duration::from_secs(
    max_secs(
        EXTERNAL_PROFILE_CLEANUP_TIMEOUT.as_secs(),
        OPENCODE_PROFILE_CLEANUP_TIMEOUT.as_secs(),
    ) + HTTP_TIMEOUT_GRACE.as_secs(),
);

/// The generic 500 envelope returned when a handler panics (D-3).
pub fn panic_response() -> Response {
    // infallible constructor — this is the panic-recovery middleware and must not be able to panic (ADR-0013 D-3)
    crate::http::response(
        StatusCode::INTERNAL_SERVER_ERROR,
        crate::http::JSON,
        &[],
        Body::from(crate::error::error_envelope("Internal Server Error")),
    )
}
