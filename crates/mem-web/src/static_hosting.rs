//! Static hosting of the `web-ui/dist` SPA bundle (roadmap §0b D5, ADR-0003).
//!
//! Upstream `web-server.ts` `serveStaticFile` (L838-895) and the SPA
//! fallback of `handleRequest` (L789-793).
//!
//! The path-traversal defense is delegated to `tower_http`'s `ServeDir` (it
//! rejects `..` and encoded separators); the SPA fallback never serves a
//! path that contains `..`.

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use tower::ServiceExt;
use tower_http::services::fs::ServeDir;

use crate::server::AppState;

/// The marker the injected script defines on `window`.
const TOKEN_MARKER: &str = "__OPENCODE_MEM_TOKEN__";

/// The immutable cache policy for hashed assets and fonts.
const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The one-day cache policy for images and icons.
const IMAGE_CACHE_CONTROL: &str = "public, max-age=86400";

/// The cache policy for everything else (including the shell).
const NO_CACHE: &str = "no-cache";

/// The font extensions that get the immutable policy.
const FONT_EXTENSIONS: [&str; 3] = ["woff", "woff2", "ttf"];

/// The image extensions that get the one-day policy.
const IMAGE_EXTENSIONS: [&str; 5] = ["ico", "png", "jpg", "jpeg", "svg"];

/// The body of the "no such file" answer.
const FILE_NOT_FOUND_BODY: &str = "File not found";

/// The body of the generic "no such route" answer.
const NOT_FOUND_BODY: &str = "Not Found";

/// Injects the local-token script into the shell's `<head>`.
///
/// The token is JSON-encoded and every `<` is escaped as `<`, so a token
/// can never break out of the script element. Without a `</head>` in the
/// document the HTML is returned unchanged.
pub fn inject_token_script(html: &str, token: Option<&str>) -> String {
    let Some(token) = token else {
        return html.to_owned();
    };
    let encoded = serde_json::to_string(token).expect("a string is always serializable");
    let escaped = encoded.replace('<', "\\u003c");
    let script = format!("<script>window.{TOKEN_MARKER}={escaped}</script>");
    match html.find("</head>") {
        Some(index) => {
            let mut out = String::with_capacity(html.len() + script.len());
            out.push_str(&html[..index]);
            out.push_str(&script);
            out.push_str(&html[index..]);
            out
        }
        None => html.to_owned(),
    }
}

/// The `Cache-Control` value for a served relative path.
pub fn cache_control_for(relative_path: &str) -> &'static str {
    if relative_path.starts_with("assets/") {
        return IMMUTABLE_CACHE_CONTROL;
    }
    let extension = relative_path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension {
        Some(extension) if FONT_EXTENSIONS.contains(&extension.as_str()) => IMMUTABLE_CACHE_CONTROL,
        Some(extension) if IMAGE_EXTENSIONS.contains(&extension.as_str()) => IMAGE_CACHE_CONTROL,
        _ => NO_CACHE,
    }
}

/// `GET /` and `GET /index.html` — the SPA shell.
pub(crate) async fn index_handler(State(state): State<Arc<AppState>>) -> Response {
    index_shell(&state).await
}

/// The static fallback: serve a file, or the shell for client-routed paths.
pub(crate) async fn static_fallback(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Response {
    // Upstream serves static files on GET only; every other method answers
    // 404 (not 405), so the route structure is not leaked.
    if request.method() != Method::GET {
        return not_found_response();
    }
    let path = request.uri().path().to_owned();
    // The SPA fallback never applies to the API surface.
    if path.starts_with("/api/") {
        return not_found_response();
    }

    let serve_dir = ServeDir::new(&state.dist_dir).append_index_html_on_directories(false);
    let response = match serve_dir.oneshot(request).await {
        Ok(response) => response,
        Err(_) => return not_found_response(),
    };
    if response.status() != StatusCode::NOT_FOUND {
        let relative = path.trim_start_matches('/');
        let (parts, body) = response.into_parts();
        let mut response = axum::response::Response::from_parts(parts, axum::body::Body::new(body));
        if let Ok(value) = HeaderValue::from_str(cache_control_for(relative)) {
            response.headers_mut().insert(header::CACHE_CONTROL, value);
        }
        return response;
    }

    if is_spa_eligible(&path) {
        return index_shell(&state).await;
    }
    not_found_response()
}

/// The 404 answer for a path that does not exist.
pub(crate) fn not_found_response() -> Response {
    text_response(StatusCode::NOT_FOUND, NOT_FOUND_BODY)
}

/// The 404 answer for a missing file.
fn file_not_found_response() -> Response {
    text_response(StatusCode::NOT_FOUND, FILE_NOT_FOUND_BODY)
}

/// The handler used for the 405-to-404 mapping and for the explicit `HEAD`
/// routes (upstream answers `HEAD` with 404, not 200).
pub(crate) async fn not_found_handler() -> Response {
    not_found_response()
}

/// Builds a plain-text response.
fn text_response(status: StatusCode, body: &str) -> Response {
    axum::http::Response::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(Body::from(body.to_owned()))
        .expect("the text response is always a valid HTTP response")
}

/// Reads the shell from the dist directory (never cached).
async fn index_shell(state: &AppState) -> Response {
    match read_index_html(&state.dist_dir).await {
        Some(html) => {
            let token = state.local_token.token();
            let body = inject_token_script(&html, token.as_deref());
            axum::http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html")
                .header("Cache-Control", NO_CACHE)
                .body(Body::from(body))
                .expect("the shell response is always a valid HTTP response")
        }
        None => file_not_found_response(),
    }
}

/// Reads `<dist>/index.html` off the runtime thread.
async fn read_index_html(dist_dir: &Path) -> Option<String> {
    let path = dist_dir.join("index.html");
    tokio::task::spawn_blocking(move || std::fs::read_to_string(path).ok())
        .await
        .ok()
        .flatten()
}

/// Whether a missing path is a client route (and may fall back to the shell).
///
/// The last path segment must not contain a `.` — otherwise a missing asset
/// would be masked by the shell and hashed-asset loading would break
/// silently — and the path must not be the root. A path containing `..` is
/// never eligible.
fn is_spa_eligible(path: &str) -> bool {
    if path == "/" || path.contains("..") {
        return false;
    }
    let last_segment = path.rsplit('/').next().unwrap_or_default();
    !last_segment.contains('.')
}
