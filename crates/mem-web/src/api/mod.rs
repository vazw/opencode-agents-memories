//! The REST surface: the pinned route table and the router that registers it.
//!
//! Upstream `web-server.ts` dispatches 33 API method+path entries; the pinned
//! table holds **39 rows** (37 of them `/api/*`, 2 OpenCode catalog paths).
//! `/api/health` is row 1 and is registered by
//! [`crate::server::build_router`] (W6.1 shipped it); [`api_router`] makes 31
//! `.route(…)` calls in this file (`grep -c '^\s*\.route('
//! crates/mem-web/src/api/mod.rs` = 31), plus the two route-layer `DELETE`
//! branches on the static `bulk-delete` paths (upstream's `"Invalid ID"`
//! branch — see the static-shadowing note below).
//!
//! **axum 0.8 syntax:** every parameter is `{name}`, never `:name` —
//! `Router::route` panics on a `:`-prefixed segment (`validate_v07_paths`), so
//! a colon is a build-time failure in every test.
//!
//! **Static shadowing (B2):** matchit 0.8.4 gives a static segment priority
//! over a dynamic one **for every method**. `/api/memories/bulk-delete`
//! therefore shadows `/api/memories/{id}` for `DELETE` and `PUT` as well as
//! `POST`, and the route-layer `200 {"success":false,"error":"Invalid ID"}`
//! must be registered on the static path (otherwise the request falls to the
//! method-not-allowed fallback and answers a plain-text 404). matchit
//! backtracks, so `/api/memories/bulk-delete/pin` still reaches `{id}/pin`.

// The handlers answer a client error by building the whole `Response` and
// returning it (the house pattern of the W6.1 gate); boxing every `Response`
// would obscure that seam for no runtime gain.
#![allow(clippy::result_large_err)]

pub mod client;
pub mod deduplication;
pub mod dto;
pub mod envelope;
pub mod guard;
pub mod maintenance;
pub mod memories;
pub mod milestones;
pub mod ports;
pub mod profile;
pub mod prompts;
pub mod sessions;
pub mod skills;
pub mod stats;
pub mod store_adapter;
pub mod tags;

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use serde::de::DeserializeOwned;

use crate::server::AppState;
pub use envelope::INTERNAL_ERROR;

/// The API request-body cap (G1, D-21). There was no pre-existing cap to
/// preserve: `TimeoutLayer` bounds time, not bytes.
pub const MAX_API_BODY_BYTES: usize = 1024 * 1024;

/// The documented answer class of a table row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteExpectation {
    /// `2xx` with `success:true`.
    Success,
    /// `200` with `success:false` (an upstream handler-level business error).
    BusinessError,
    /// `4xx` (the skills routes only, upstream's explicit status map).
    ClientError,
    /// The documented `501`: a stub handler that answers without consulting
    /// its port (`/api/cleanup`), a real handler whose port is unavailable in
    /// this build (the session-history rows), or a real handler whose port
    /// answers `Unavailable` only in the wiring that drives the case (the
    /// contract-suite fake, or the D-38 no-composed-stores degrade).
    Stub,
}

/// One row of the pinned route table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSpec {
    pub method: &'static str,
    pub path: &'static str,
    pub upstream_line: &'static str,
    pub module: &'static str,
    pub expectation: RouteExpectation,
}

/// The pinned REST surface (39 rows): upstream's rows plus the port's own
/// additions.
///
/// Both the contract test and the auth walk iterate this constant.
pub const ROUTE_TABLE: [RouteSpec; 39] = [
    RouteSpec {
        method: "GET",
        path: "/api/health",
        upstream_line: ":482",
        module: "(W6.1, already shipped)",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/tags",
        upstream_line: ":509",
        module: "api/tags.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/memories",
        upstream_line: ":514",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/memories",
        upstream_line: ":524",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "DELETE",
        path: "/api/memories/{id}",
        upstream_line: ":530",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "PUT",
        path: "/api/memories/{id}",
        upstream_line: ":541",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/memories/bulk-delete",
        upstream_line: ":551",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "PUT",
        path: "/api/memories/bulk-delete",
        upstream_line: ":541",
        module: "api/memories.rs",
        expectation: RouteExpectation::BusinessError,
    },
    RouteSpec {
        method: "GET",
        path: "/api/search",
        upstream_line: ":558",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/stats",
        upstream_line: ":573",
        module: "api/stats.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/memories/{id}/pin",
        upstream_line: ":578",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/memories/{id}/unpin",
        upstream_line: ":587",
        module: "api/memories.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/cleanup",
        upstream_line: ":596",
        module: "api/maintenance.rs",
        expectation: RouteExpectation::Stub,
    },
    RouteSpec {
        method: "POST",
        path: "/api/deduplicate",
        upstream_line: ":601",
        module: "api/maintenance.rs",
        // The handler is real and the composition root injects the
        // deduplication port (issue 54). The 501 survives in two wirings: the
        // contract suite's `FakeBackend`, and the D-38 no-composed-stores
        // degrade (the `RootWebLauncher` `None` arm, reached when composition
        // fails and the web port still binds). It is not a leftover stub.
        expectation: RouteExpectation::Stub,
    },
    RouteSpec {
        method: "GET",
        path: "/api/migration/detect",
        upstream_line: ":606",
        module: "api/maintenance.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/migration/tags/detect",
        upstream_line: ":611",
        module: "api/maintenance.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/migration/tags/run-batch",
        upstream_line: ":616",
        module: "api/maintenance.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/migration/tags/progress",
        upstream_line: ":623",
        module: "api/maintenance.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/migration/run",
        upstream_line: ":628",
        module: "api/maintenance.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "DELETE",
        path: "/api/prompts/{id}",
        upstream_line: ":638",
        module: "api/prompts.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/prompts/bulk-delete",
        upstream_line: ":649",
        module: "api/prompts.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/user-profile",
        upstream_line: ":656",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/user-profile/changelog",
        upstream_line: ":662",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/user-profile/snapshot",
        upstream_line: ":672",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/user-profile/refresh",
        upstream_line: ":681",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/user-profile/ai-cleanup",
        upstream_line: ":688",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/user-profile/ai-cleanup/apply",
        upstream_line: ":700",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "PATCH",
        path: "/api/user-profile/item",
        upstream_line: ":707",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/skills",
        upstream_line: ":713",
        module: "api/skills.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "POST",
        path: "/api/skills",
        upstream_line: ":720",
        module: "api/skills.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/skills/{id}",
        upstream_line: ":726",
        module: "api/skills.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "PUT",
        path: "/api/skills/{id}",
        upstream_line: ":735",
        module: "api/skills.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "DELETE",
        path: "/api/skills/{id}",
        upstream_line: ":745",
        module: "api/skills.rs",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/skills/index.json",
        upstream_line: ":755",
        module: "api/skills.rs (catalog)",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/skills/{id}/{file}",
        upstream_line: ":767",
        module: "api/skills.rs (catalog)",
        expectation: RouteExpectation::Success,
    },
    RouteSpec {
        method: "GET",
        path: "/api/projects/{project_id}/sessions",
        upstream_line: "— (new)",
        module: "api/sessions.rs",
        expectation: RouteExpectation::Stub,
    },
    RouteSpec {
        method: "DELETE",
        path: "/api/projects/{project_id}/sessions/{session_id}",
        upstream_line: "— (new)",
        module: "api/sessions.rs",
        expectation: RouteExpectation::Stub,
    },
    RouteSpec {
        method: "GET",
        path: "/api/projects/{project_id}/milestones",
        upstream_line: "— (new, W6.6)",
        module: "api/milestones.rs",
        expectation: RouteExpectation::Success,
    },
    // Issue 48.14 — port-only (47.1-B2′): no upstream counterpart. Appended
    // LAST so the shared-router envelope walk is not perturbed by the fake
    // profile being deleted before the other profile rows are probed.
    RouteSpec {
        method: "DELETE",
        path: "/api/user-profile",
        upstream_line: "n/a — port-only (47.1-B2′, issue 48)",
        module: "api/profile.rs",
        expectation: RouteExpectation::Success,
    },
];

/// Whether the path is one of the two OpenCode catalog paths (D-11).
pub fn is_catalog_path(path: &str) -> bool {
    path == "/skills/index.json" || (path.starts_with("/skills/") && path.ends_with(".md"))
}

/// Builds the API router (the 38 non-health table paths + the two static
/// `DELETE` route-layer branches).
///
/// The 1 MiB body cap is layered here, so it applies to every API method and
/// is inherited by the merged router.
pub fn api_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/tags", get(tags::list_tags))
        .route(
            "/api/memories",
            get(memories::list_memories).post(memories::add_memory),
        )
        .route(
            "/api/memories/{id}",
            delete(memories::delete_memory).put(memories::update_memory),
        )
        // B2: the static path shadows `{id}` for every method.
        .route(
            "/api/memories/bulk-delete",
            post(memories::bulk_delete_memories)
                .delete(reject_bulk_delete_id)
                .put(memories::update_bulk_delete_path),
        )
        .route("/api/search", get(memories::search))
        .route("/api/stats", get(stats::stats))
        .route("/api/memories/{id}/pin", post(memories::pin_memory))
        .route("/api/memories/{id}/unpin", post(memories::unpin_memory))
        .route("/api/cleanup", post(maintenance::cleanup))
        .route("/api/deduplicate", post(maintenance::deduplicate))
        .route("/api/migration/detect", get(maintenance::detect_migration))
        .route(
            "/api/migration/tags/detect",
            get(maintenance::detect_tag_migration),
        )
        .route(
            "/api/migration/tags/run-batch",
            post(maintenance::run_tag_migration_batch),
        )
        .route(
            "/api/migration/tags/progress",
            get(maintenance::tag_migration_progress),
        )
        .route("/api/migration/run", post(maintenance::run_migration))
        .route("/api/prompts/{id}", delete(prompts::delete_prompt))
        .route(
            "/api/prompts/bulk-delete",
            post(prompts::bulk_delete_prompts).delete(reject_bulk_delete_id),
        )
        .route(
            "/api/user-profile",
            get(profile::get_user_profile).delete(profile::delete_user_profile),
        )
        .route(
            "/api/user-profile/changelog",
            get(profile::get_profile_changelog),
        )
        .route(
            "/api/user-profile/snapshot",
            get(profile::get_profile_snapshot),
        )
        .route("/api/user-profile/refresh", post(profile::refresh_profile))
        .route("/api/user-profile/ai-cleanup", post(profile::ai_cleanup))
        .route(
            "/api/user-profile/ai-cleanup/apply",
            post(profile::apply_cleanup),
        )
        .route(
            "/api/user-profile/item",
            patch(profile::update_profile_item),
        )
        .route(
            "/api/skills",
            get(skills::list_skills).post(skills::create_skill),
        )
        .route(
            "/api/skills/{id}",
            get(skills::get_skill)
                .put(skills::update_skill)
                .delete(skills::delete_skill),
        )
        .route("/skills/index.json", get(skills::catalog_index))
        .route("/skills/{id}/{file}", get(skills::catalog_file))
        .route(
            "/api/projects/{project_id}/sessions",
            get(sessions::list_sessions),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}",
            delete(sessions::delete_session),
        )
        .route(
            "/api/projects/{project_id}/milestones",
            get(milestones::list_milestones),
        )
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
}

/// The route-layer `"Invalid ID"` branch (B2).
///
/// Upstream `web-server.ts:530-539` / `:638-647` answer
/// `200 {"success":false,"error":"Invalid ID"}` for the static `bulk-delete`
/// path; with matchit's static-over-dynamic priority the request never reaches
/// `{id}`, so the branch must be registered on the static path itself.
pub async fn reject_bulk_delete_id() -> Response {
    envelope::business_error("Invalid ID")
}

/// The marker of the scaffolding answer every slice replaced during this wave.
///
/// No route answers it any more: Step 15's
/// `api_endpoint_contract::test_no_route_answers_the_placeholder_envelope`
/// walks the whole table and asserts the marker is absent from every body.
pub const PLACEHOLDER_MARKER: &str = "is not implemented yet";

/// Collects every value of a repeated query key (`?tag=a&tag=b`).
pub fn query_values(query: &[(String, String)], key: &str) -> Vec<String> {
    query
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .collect()
}

/// The first value of a query key.
pub fn query_first<'a>(query: &'a [(String, String)], key: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Parses `page`/`pageSize` under D-12: `page >= 1`, `1 <= pageSize <= 200`.
pub fn parse_pagination(query: &[(String, String)]) -> Result<(usize, usize), Response> {
    let page = parse_positive(query_first(query, "page"), 1)?;
    let page_size = parse_positive(query_first(query, "pageSize"), 20)?;
    if page_size > 200 {
        return Err(envelope::bad_request("Invalid pagination parameters"));
    }
    Ok((page, page_size))
}

/// Parses one pagination value, refusing anything but a plain positive
/// integer (upstream `parseInt` silently produced `NaN`).
fn parse_positive(raw: Option<&str>, default: usize) -> Result<usize, Response> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|parsed| *parsed >= 1)
            .ok_or_else(|| envelope::bad_request("Invalid pagination parameters")),
    }
}

/// Parses a required JSON body. A malformed body is a fixed `400` (never an
/// echo of the body).
pub fn json_body_required<T: DeserializeOwned>(bytes: &Bytes) -> Result<T, Response> {
    serde_json::from_slice(bytes).map_err(|_| envelope::bad_request("Invalid JSON body"))
}

/// Parses an optional JSON body: an empty or malformed body is the default
/// (upstream's `req.json().catch(() => ({}))`).
pub fn json_body_or_default<T: DeserializeOwned + Default>(bytes: &Bytes) -> T {
    if bytes.is_empty() {
        return T::default();
    }
    serde_json::from_slice(bytes).unwrap_or_default()
}

/// The fixed message of the absent-service class (issue 61): the `501` body of
/// a route whose port is not wired in this build. The `PortError`'s own
/// Display is a log-level description and is never echoed.
pub const UNAVAILABLE_MESSAGE: &str = "the endpoint is not available in this build";

/// The answer for a port failure. The detail goes to the log and never into the
/// response (D-9); the absent-service class answers the documented `501` stub
/// envelope instead of a `500` (issue 61).
pub fn port_failure(endpoint: &'static str, error: ports::PortError) -> Response {
    tracing::error!(endpoint, error = %error, "api handler failed");
    match error {
        // The service is not wired in this build: the same class the explicit
        // stubs answer, so a client can tell "not supported here" from
        // "broken here" without reading the body.
        ports::PortError::Unavailable => envelope::not_implemented(UNAVAILABLE_MESSAGE),
        // W3: the typed model refusal is answered 409 with its own Display
        // (the operator remedy rides in the message).
        ports::PortError::ModelMismatch(report) => envelope::conflict(&report.to_string()),
        // MEASURED status quo (2026-10-27), not an endorsement: `NotFound` and
        // `Invalid` are pre-empted by every handler that documents 404/400/200
        // (sessions, skills, maintenance, profile), and on the generic path
        // they keep the D-9 500. Redesigning them is not issue 61's.
        ports::PortError::NotFound | ports::PortError::Invalid(_) | ports::PortError::Store(_) => {
            envelope::internal_error()
        }
    }
}

/// A `Query` alias for the repeated-key pairs used by every GET handler.
pub type QueryPairs = Query<Vec<(String, String)>>;

/// The status used by the explicit stubs and by the absent-service class
/// through [`port_failure`]; the tests bind the builder's literal to this
/// constant.
pub const STUB_STATUS: StatusCode = StatusCode::NOT_IMPLEMENTED;
