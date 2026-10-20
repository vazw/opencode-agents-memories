//! The ordered request gate.
//!
//! The order is upstream `web-server.ts` `handleRequest` plus the D-8 Host
//! allowlist as check 0:
//!
//! 0. D-8 Host allowlist — before the Origin gate, before the OPTIONS
//!    short-circuit, before Basic auth, before every route and the fallback,
//!    therefore before the local token can be embedded in any response body.
//! 1. Origin gate.
//! 2. `OPTIONS` preflight short-circuit.
//! 3. Basic auth.
//! 4. API token / local token for `/api/*` (except `/api/health`).
//! 5. Next.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;

use crate::AUTH_HEADER;
use crate::api_auth::{authorize_api_request, local_token_matches};
use crate::cors::{disallowed_response, is_allowed_browser_origin, preflight_response};
use crate::error::error_envelope;
use crate::host_allowlist::host_refusal_response;
use crate::server::AppState;

/// The request gate middleware.
pub(crate) async fn gate(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    // 0. D-8 — the Host allowlist runs FIRST. In a DNS-rebinding attack the
    //    browser sends `Host: <attacker-domain>:4747`; because the rebinding
    //    makes the attacker's page same-origin with the server, the CORS gate
    //    cannot stop it — this check can, and it runs before any response
    //    body can carry the local token.
    let host_header = request
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok());
    if !state.host_allowlist.allows(host_header) {
        return host_refusal_response();
    }

    let path = request.uri().path().to_owned();
    let method = request.method().clone();

    // 1. Origin gate. A malformed (non-UTF-8) Origin is refused.
    let origin = match request.headers().get("origin") {
        None => None,
        Some(value) => match value.to_str() {
            Ok(origin) => Some(origin.to_owned()),
            Err(_) => return disallowed_response(),
        },
    };
    let auth_enabled = state.basic_auth.is_enabled();
    if !is_allowed_browser_origin(origin.as_deref(), auth_enabled) {
        return disallowed_response();
    }

    // 2. Preflight short-circuit — before auth, like upstream.
    if method == Method::OPTIONS {
        return preflight_response(origin.as_deref(), auth_enabled);
    }

    // 3. Basic auth.
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    if !state.basic_auth.check(authorization, &path) {
        return state.basic_auth.challenge();
    }

    // 4. The API surface needs the local token (or the configured API
    //    token). `/api/health` is exempt — the liveness probe must work
    //    without credentials.
    if path.starts_with("/api/") && path != "/api/health" {
        let provided_local = request
            .headers()
            .get(AUTH_HEADER)
            .and_then(|value| value.to_str().ok());
        let local_ok = local_token_matches(provided_local, state.local_token.token().as_deref());
        let api_ok = match state.api_token.as_deref().filter(|token| !token.is_empty()) {
            Some(token) => authorize_api_request(authorization, provided_local, Some(token)),
            None => false,
        };
        if !(local_ok || api_ok) {
            return unauthorized_response();
        }
    }

    // 5. Next.
    next.run(request).await
}

/// The 401 answer for an unauthorized API request.
fn unauthorized_response() -> Response {
    axum::http::Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .body(Body::from(error_envelope("Unauthorized")))
        .expect("the unauthorized response is always a valid HTTP response")
}
