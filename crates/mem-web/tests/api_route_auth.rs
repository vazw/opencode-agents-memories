//! The auth seam over the pinned W6.2 REST surface.
//!
//! Step 2 lands the two D-11 catalog cases; Step 14 adds the walk over every
//! [`mem_web::api::ROUTE_TABLE`] row: the API-token gate, the D-8 Host gate,
//! the Basic-auth gate, the H1 reachability clause and the credential-leak
//! check.
//!
//! D-11 (the user's ruling): the two OpenCode catalog paths are **public on a
//! loopback bind** — OpenCode's skill-catalog fetch sends no
//! `x-opencode-mem-token` — and **require the token on any other bind**.
//!
//! **Walks.** The token walk covers the **34 `/api/*` rows** only
//! (`/api/health` is token-exempt, the two catalog rows are public on a
//! loopback bind and keep their dedicated D-11 cases). The Host, Basic-auth,
//! reachability and credential walks cover the other **36** rows. Every row
//! gets a **freshly built router and a freshly seeded fake** — the walk's
//! destructive rows (`DELETE /api/memories/mem_probe_1`,
//! `DELETE /api/skills/ship-it`) therefore cannot consume the rows that later
//! depend on the same seeds.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::fakes::{MEMORY_ID, PROMPT_ID, SKILL_ID, USER_ID, fake_services};
use common::{RawResponse, api_request, app_state_with_api, oneshot_response};
use mem_web::api::{ROUTE_TABLE, RouteExpectation, RouteSpec};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// The API token every case configures.
const API_TOKEN: &str = "test-token";
/// The local token configured on the state (never echoed either).
const LOCAL_TOKEN: &str = "local-token-value";
/// The Basic password used by the Basic-auth walk.
const BASIC_PASSWORD: &str = "shhh-basic-password";
/// The static-shell marker (confined to the shell, never an API body).
const SHELL_MARKER: &str = "__OPENCODE_MEM_TOKEN__";
/// A `Host` value the D-8 allowlist must refuse (DNS rebinding shape).
const FOREIGN_HOST: &str = "rebind.attacker.example:4747";
/// The loopback bind host (the allowlist is active; catalog is public).
const LOOPBACK: &str = "127.0.0.1";
/// The wildcard bind host (the allowlist is disabled; catalog is token-gated).
const WILDCARD: &str = "0.0.0.0";

/// A seeded router on an explicit bind host (Basic auth disabled).
fn router_on(bind_host: &str) -> axum::Router {
    let (backend, services) = fake_services();
    backend.seed_pending_cleanup();
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        Some(LOCAL_TOKEN),
        PathBuf::from("/tmp/unused-dist"),
        services,
        USER_ID,
        bind_host,
    );
    build_router(Arc::new(state))
}

/// A seeded router with Basic auth enabled.
fn basic_router() -> axum::Router {
    let (backend, services) = fake_services();
    backend.seed_pending_cleanup();
    let state = app_state_with_api(
        WebAuth::new(Some(BASIC_PASSWORD), Some("user"), "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        services,
        USER_ID,
        LOOPBACK,
    );
    build_router(Arc::new(state))
}

/// The walk's canonical request for one table row.
struct Canonical {
    method: &'static str,
    path: String,
    body: Option<&'static str>,
}

/// Builds the route's canonical request (blueprint Step 14's recipe table).
///
/// `POST /api/skills` uses the fresh id `probe-skill` (the seed keeps
/// `ship-it` intact for GET/PUT/DELETE and the catalog file); every other row
/// uses its documented seed.
fn canonical(row: &RouteSpec) -> Canonical {
    let (method, path, body): (&'static str, String, Option<&'static str>) =
        match (row.method, row.path) {
            ("GET", "/api/search") => ("GET", "/api/search?q=probe".to_string(), None),
            ("GET", "/api/user-profile/changelog") => (
                "GET",
                "/api/user-profile/changelog?profileId=probe".to_string(),
                None,
            ),
            ("GET", "/api/user-profile/snapshot") => (
                "GET",
                "/api/user-profile/snapshot?chlogId=probe".to_string(),
                None,
            ),
            ("POST", "/api/memories") => (
                "POST",
                "/api/memories".to_string(),
                Some(r#"{"content":"probe","containerTag":"mem_project_c1c2c3d4e5f67890"}"#),
            ),
            ("PUT", "/api/memories/{id}") => (
                "PUT",
                format!("/api/memories/{MEMORY_ID}"),
                Some(r#"{"content":"probe"}"#),
            ),
            ("DELETE", "/api/memories/{id}") => {
                ("DELETE", format!("/api/memories/{MEMORY_ID}"), None)
            }
            ("POST", "/api/memories/{id}/pin") => {
                ("POST", format!("/api/memories/{MEMORY_ID}/pin"), None)
            }
            ("POST", "/api/memories/{id}/unpin") => {
                ("POST", format!("/api/memories/{MEMORY_ID}/unpin"), None)
            }
            ("POST", "/api/memories/bulk-delete") => (
                "POST",
                "/api/memories/bulk-delete".to_string(),
                Some(r#"{"ids":["mem_probe_1"]}"#),
            ),
            ("PUT", "/api/memories/bulk-delete") => (
                "PUT",
                "/api/memories/bulk-delete".to_string(),
                Some(r#"{"content":"probe"}"#),
            ),
            ("DELETE", "/api/prompts/{id}") => {
                ("DELETE", format!("/api/prompts/{PROMPT_ID}"), None)
            }
            ("POST", "/api/prompts/bulk-delete") => (
                "POST",
                "/api/prompts/bulk-delete".to_string(),
                Some(r#"{"ids":["mem_probe_1"]}"#),
            ),
            ("POST", "/api/user-profile/ai-cleanup/apply")
            | ("PATCH", "/api/user-profile/item") => (
                row.method,
                row.path.to_string(),
                Some(r#"{"type":"preferences","index":0,"action":"edit"}"#),
            ),
            ("POST", "/api/migration/run") => (
                "POST",
                "/api/migration/run".to_string(),
                Some(r#"{"strategy":"fresh-start"}"#),
            ),
            ("POST", "/api/migration/tags/run-batch") => (
                "POST",
                "/api/migration/tags/run-batch".to_string(),
                Some(r#"{"batchSize":5}"#),
            ),
            ("POST", "/api/skills") => (
                "POST",
                "/api/skills".to_string(),
                Some(r###"{"id":"probe-skill","name":"Ship It","content":"## Ship"}"###),
            ),
            ("PUT", "/api/skills/{id}") => (
                "PUT",
                format!("/api/skills/{SKILL_ID}"),
                Some(r###"{"id":"ship-it","name":"Ship It","content":"## Ship"}"###),
            ),
            ("GET", "/api/skills/{id}") => ("GET", format!("/api/skills/{SKILL_ID}"), None),
            ("DELETE", "/api/skills/{id}") => ("DELETE", format!("/api/skills/{SKILL_ID}"), None),
            ("GET", "/skills/{id}/{file}") => {
                ("GET", format!("/skills/{SKILL_ID}/{SKILL_ID}.md"), None)
            }
            ("GET", "/api/projects/{project_id}/sessions") => (
                "GET",
                "/api/projects/probe-project/sessions".to_string(),
                None,
            ),
            ("DELETE", "/api/projects/{project_id}/sessions/{session_id}") => (
                "DELETE",
                "/api/projects/probe-project/sessions/probe-session".to_string(),
                None,
            ),
            _ => (row.method, row.path.to_string(), None),
        };
    Canonical { method, path, body }
}

/// The rows the token walk covers: every `/api/*` row but `/api/health`.
fn token_gated_rows() -> impl Iterator<Item = &'static RouteSpec> {
    ROUTE_TABLE
        .iter()
        .filter(|row| row.path.starts_with("/api/") && row.path != "/api/health")
}

/// Every row but `/api/health` (the Host / Basic / reachability walks).
fn non_health_rows() -> impl Iterator<Item = &'static RouteSpec> {
    ROUTE_TABLE.iter().filter(|row| row.path != "/api/health")
}

/// A hand-built request carrying an explicit `Host` and the API token.
async fn request_with_host(router: axum::Router, canonical: &Canonical, host: &str) -> RawResponse {
    let mut builder = axum::http::Request::builder()
        .method(canonical.method)
        .uri(&canonical.path)
        .header("host", host)
        .header(mem_web::AUTH_HEADER, API_TOKEN);
    let body = match canonical.body {
        Some(payload) => {
            builder = builder.header("content-type", "application/json");
            axum::body::Body::from(payload.to_owned())
        }
        None => axum::body::Body::empty(),
    };
    let request = builder
        .body(body)
        .expect("the walk's request is a valid HTTP request");
    oneshot_response(router, request).await
}

/// Asserts a body carries none of the configured credentials.
fn assert_no_credentials(tag: &str, response: &RawResponse) {
    let body = response.body_string();
    for secret in [API_TOKEN, LOCAL_TOKEN, BASIC_PASSWORD, SHELL_MARKER] {
        assert!(
            !body.contains(secret),
            "{tag} leaks a credential ({secret:?}): {body}"
        );
    }
    for header in ["set-cookie", "authorization"] {
        assert!(
            response.header(header).is_none(),
            "{tag} answers a {header} header"
        );
    }
}

/// The H1 reachability clause: not 404/405, and the documented answer.
fn assert_reachable(row: &RouteSpec, response: &RawResponse) {
    let body = response.body_string();
    let tag = format!("{} {}", row.method, row.path);
    assert!(
        response.status != 404 && response.status != 405,
        "{tag} is shadowed or mis-methoded: {} {body}",
        response.status
    );

    // The two catalog paths answer outside the envelope.
    if row.path == "/skills/index.json" {
        assert_eq!(response.status, 200, "{tag}: {body}");
        assert!(
            body.starts_with(r#"{"skills":"#),
            "{tag} answers the bare catalog object: {body}"
        );
        return;
    }
    if row.path == "/skills/{id}/{file}" {
        assert_eq!(response.status, 200, "{tag}: {body}");
        assert!(
            body.contains("## Ship"),
            "{tag} serves the seeded skill's markdown: {body}"
        );
        return;
    }

    match row.expectation {
        RouteExpectation::Stub => {
            assert_eq!(response.status, 501, "{tag}: {body}");
            assert!(
                body.contains(r#""success":false"#),
                "{tag} uses the envelope: {body}"
            );
            let expected = match row.path {
                "/api/cleanup" => "cleanup is not available in this build",
                "/api/deduplicate" => "deduplicate is not available in this build",
                _ => "Session history endpoints land with W6.3",
            };
            assert!(body.contains(expected), "{tag}: {body}");
        }
        RouteExpectation::BusinessError => {
            // `PUT /api/memories/bulk-delete` — upstream's 200 + `success:false`.
            assert_eq!(response.status, 200, "{tag}: {body}");
            assert!(
                body.contains(r#""success":false"#),
                "{tag} uses the envelope: {body}"
            );
            assert!(body.contains("Memory not found"), "{tag}: {body}");
        }
        RouteExpectation::Success | RouteExpectation::ClientError => {
            assert_eq!(response.status, 200, "{tag}: {body}");
            assert!(
                body.contains(r#""success":true"#),
                "{tag} answers the success envelope: {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_every_new_route_requires_the_api_token() {
    let mut walked = 0_usize;
    for row in token_gated_rows() {
        walked += 1;
        let request = canonical(row);
        let response = api_request(
            router_on(LOOPBACK),
            request.method,
            &request.path,
            None,
            request.body,
        )
        .await;
        assert_eq!(
            response.status,
            401,
            "{} {} without a token answered {}: {}",
            row.method,
            row.path,
            response.status,
            response.body_string()
        );
        let body = response.body_string();
        assert!(
            body.contains(r#""success":false"#) && body.contains("Unauthorized"),
            "{} {} answers the 401 envelope: {body}",
            row.method,
            row.path
        );
        assert_no_credentials(&format!("{} {}", row.method, row.path), &response);
    }
    // 37 rows − `/api/health` − the 2 catalog rows = 34 token-gated rows.
    assert_eq!(walked, 34, "the token walk covers every /api/* row");
}

#[tokio::test]
async fn test_every_new_route_is_refused_for_a_foreign_host_header() {
    let mut walked = 0_usize;
    for row in non_health_rows() {
        walked += 1;
        let request = canonical(row);
        let response = request_with_host(router_on(LOOPBACK), &request, FOREIGN_HOST).await;
        assert_eq!(
            response.status,
            403,
            "{} {} with a foreign Host answered {}: {}",
            row.method,
            row.path,
            response.status,
            response.body_string()
        );
        let body = response.body_string();
        assert!(
            body.contains(r#""success":false"#),
            "{} {} answers the D-8 refusal envelope: {body}",
            row.method,
            row.path
        );
        assert_no_credentials(&format!("{} {}", row.method, row.path), &response);
    }
    assert_eq!(walked, 36, "the Host walk covers every non-health row");
}

#[tokio::test]
async fn test_every_new_route_is_gated_by_basic_auth() {
    let mut walked = 0_usize;
    for row in non_health_rows() {
        walked += 1;
        let request = canonical(row);
        let response = api_request(
            basic_router(),
            request.method,
            &request.path,
            Some(API_TOKEN),
            request.body,
        )
        .await;
        assert_eq!(
            response.status,
            401,
            "{} {} without Basic credentials answered {}: {}",
            row.method,
            row.path,
            response.status,
            response.body_string()
        );
        let challenge = response.header("www-authenticate").unwrap_or_default();
        assert!(
            challenge.starts_with("Basic"),
            "{} {} answers the Basic challenge: {challenge:?}",
            row.method,
            row.path
        );
        assert_no_credentials(&format!("{} {}", row.method, row.path), &response);
    }
    assert_eq!(walked, 36, "the Basic walk covers every non-health row");
}

#[tokio::test]
async fn test_every_new_route_answers_a_reachable_response() {
    let mut walked = 0_usize;
    for row in non_health_rows() {
        walked += 1;
        // A fresh router + a freshly seeded fake per row: the destructive rows
        // (`DELETE /api/memories/mem_probe_1`, `DELETE /api/skills/ship-it`)
        // cannot consume the seeds that the later rows depend on.
        let request = canonical(row);
        let response = api_request(
            router_on(LOOPBACK),
            request.method,
            &request.path,
            Some(API_TOKEN),
            request.body,
        )
        .await;
        assert_reachable(row, &response);
        assert_no_credentials(&format!("{} {}", row.method, row.path), &response);
    }
    assert_eq!(
        walked, 36,
        "the reachability walk covers every non-health row"
    );
}

#[tokio::test]
async fn test_no_api_response_body_carries_a_credential() {
    // Both the refused (no token) and the accepted (canonical) answers of
    // every row are inspected.
    for row in non_health_rows() {
        let request = canonical(row);
        let refused = api_request(
            router_on(LOOPBACK),
            request.method,
            &request.path,
            None,
            request.body,
        )
        .await;
        let tag = format!("{} {} (refused)", row.method, row.path);
        assert_no_credentials(&tag, &refused);

        let accepted = api_request(
            router_on(LOOPBACK),
            request.method,
            &request.path,
            Some(API_TOKEN),
            request.body,
        )
        .await;
        let tag = format!("{} {} (accepted)", row.method, row.path);
        assert_no_credentials(&tag, &accepted);
    }
}

// ---------------------------------------------------------------------------
// D-11 — the catalog posture (Step 2's cases, unchanged)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_catalog_paths_are_public_on_a_loopback_bind() {
    let router = router_on(LOOPBACK);

    // `GET /skills/index.json` — the bare catalog object, no token.
    let index = api_request(router.clone(), "GET", "/skills/index.json", None, None).await;
    assert_eq!(index.status, 200, "the loopback catalog is public");
    let body = index.body_string();
    assert!(
        body.starts_with(r#"{"skills":"#),
        "the catalog answers the bare object, not the envelope: {body}"
    );
    assert!(
        body.contains(SKILL_ID),
        "the seeded skill is listed: {body}"
    );
    assert!(
        index
            .header("content-type")
            .unwrap_or_default()
            .contains("application/json"),
        "the catalog index is JSON"
    );

    // `GET /skills/{id}/{file}` — the raw markdown, no token.
    let file = api_request(
        router,
        "GET",
        &format!("/skills/{SKILL_ID}/{SKILL_ID}.md"),
        None,
        None,
    )
    .await;
    assert_eq!(file.status, 200, "the loopback catalog file is public");
    assert!(
        file.header("content-type")
            .unwrap_or_default()
            .contains("text/markdown"),
        "the catalog file is markdown"
    );
    assert!(
        file.body_string().contains("## Ship"),
        "the seeded skill's markdown is served: {}",
        file.body_string()
    );
}

#[tokio::test]
async fn test_the_catalog_paths_require_the_token_on_a_non_loopback_bind() {
    // `0.0.0.0` is the wildcard bind: `HostAllowlist::for_bind_host` disables
    // the allowlist (the same value `assert_network_auth` treats as a network
    // exposure), so the token rule is the only gate left.
    let router = router_on(WILDCARD);

    for path in [
        "/skills/index.json".to_string(),
        format!("/skills/{SKILL_ID}/{SKILL_ID}.md"),
    ] {
        let refused = api_request(router.clone(), "GET", &path, None, None).await;
        assert_eq!(
            refused.status, 401,
            "a non-loopback catalog request without a token is refused: {path}"
        );
        assert!(
            refused.body_string().contains(r#""success":false"#),
            "the refusal uses the envelope: {}",
            refused.body_string()
        );

        let allowed = api_request(router.clone(), "GET", &path, Some(API_TOKEN), None).await;
        assert_eq!(
            allowed.status, 200,
            "the token unlocks the catalog path: {path}"
        );
    }
}
