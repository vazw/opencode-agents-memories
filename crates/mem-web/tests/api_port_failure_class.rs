//! Issue 61: the shared `port_failure` helper's failure classes.
//!
//! The helper is the single seam behind the 45 direct `port_failure` calls and
//! the four `skill_failure` sites: an absent service (`PortError::Unavailable`)
//! answers the documented `501` stub envelope, while an internal fault
//! (`PortError::Store`) keeps the fixed `500` with the detail logged and never
//! echoed (D-9). The router-level cases drive a degraded
//! `ApiServices::unavailable()` build, so the class is observed through the
//! shipped router and the auth gate, not only at the helper.
//!
//! Every assertion observes status, content type and the exact body bytes; no
//! case asserts on log output (this tree has no subscriber harness).
#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use mem_web::api::ports::{ApiServices, PortError};
use mem_web::api::{STUB_STATUS, port_failure};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

use common::{RawResponse, api_request, app_state_with_api};

/// The exact `501` body of the absent-service class (issue 61).
const UNAVAILABLE_BODY: &str =
    r#"{"success":false,"error":"the endpoint is not available in this build"}"#;
/// The exact `500` body every other port failure keeps (D-9).
const INTERNAL_BODY: &str = r#"{"success":false,"error":"Internal Server Error"}"#;
/// The API token the fixture's state is built with.
const API_TOKEN: &str = "test-token";
/// A non-existent absolute path: the API cases never read the UI dist.
const MISSING_DIST: &str = "/nonexistent-mem-web-dist";

/// The shipped router over the given port bundle, with the API token set.
fn router_with(api: ApiServices) -> Router {
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from(MISSING_DIST),
        common::throwaway_log_dir(),
        api,
        "probe-user",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

/// One `port_failure` response for `error`, through the public helper.
async fn helper_response(error: PortError) -> RawResponse {
    common::response_to_raw(port_failure("probe", error)).await
}

#[tokio::test]
async fn test_unavailable_answers_501_with_the_documented_stub_envelope() {
    let response = helper_response(PortError::Unavailable).await;

    assert_eq!(
        response.status,
        501,
        "an absent service answered {}: {}",
        response.status,
        response.body_string()
    );
    assert_eq!(
        response.status,
        STUB_STATUS.as_u16(),
        "the answer is the documented stub class"
    );
    assert_eq!(
        response.header("content-type"),
        Some("application/json"),
        "the stub envelope is JSON"
    );
    assert_eq!(
        response.body.as_slice(),
        UNAVAILABLE_BODY.as_bytes(),
        "unexpected stub body: {}",
        response.body_string()
    );
}

#[tokio::test]
async fn test_store_failure_answers_500_without_internal_detail() {
    let detail = "connection refused: /home/probe/shard-7.db";
    let response = helper_response(PortError::Store(detail.to_string())).await;

    assert_eq!(
        response.status,
        500,
        "a store fault answered {}: {}",
        response.status,
        response.body_string()
    );
    assert_eq!(
        response.body.as_slice(),
        INTERNAL_BODY.as_bytes(),
        "a store fault keeps the fixed body: {}",
        response.body_string()
    );
    assert!(
        !response.body_string().contains("shard-7"),
        "no path fragment may be echoed: {}",
        response.body_string()
    );
    assert!(
        !response.body_string().contains(detail),
        "the store detail must never be returned: {}",
        response.body_string()
    );
}

#[tokio::test]
async fn test_the_two_classes_are_distinguishable_by_status_alone() {
    let unavailable = helper_response(PortError::Unavailable).await;
    let store = helper_response(PortError::Store("disk full".to_string())).await;

    assert_ne!(
        unavailable.status, store.status,
        "a client must tell 'not supported here' from 'broken here' by status alone"
    );
    assert_eq!(
        (unavailable.status, store.status),
        (501, 500),
        "absent service is 501; an internal fault is 500"
    );
}

/// The helper's generic mapping for `NotFound` / `Invalid` is the **measured
/// status quo at b46bcce, not an endorsement**: every handler that documents a
/// 404/400/200 pre-empts these variants, and issue 37 AC4 owns the contract
/// wording. Issue 61 changes only the absent-service class.
#[tokio::test]
async fn test_not_found_and_invalid_keep_their_measured_generic_mapping() {
    for (error, variant_text) in [
        (PortError::NotFound, "not found"),
        (
            PortError::Invalid("Invalid strategy".to_string()),
            "Invalid strategy",
        ),
    ] {
        let response = helper_response(error).await;

        assert_eq!(
            response.status,
            500,
            "the generic path keeps today's mapping: {}",
            response.body_string()
        );
        assert_eq!(
            response.body.as_slice(),
            INTERNAL_BODY.as_bytes(),
            "the fixed 500 body: {}",
            response.body_string()
        );
        assert!(
            !response.body_string().contains(variant_text),
            "the variant text is never echoed: {}",
            response.body_string()
        );
    }
}

#[tokio::test]
async fn test_the_absent_service_class_comes_from_the_shared_helper_not_a_per_handler_copy() {
    let direct = helper_response(PortError::Unavailable).await;

    // The same degraded bundle drives both a direct helper consumer
    // (`memories.rs`) and a delegating one (`skill_failure` -> `port_failure`).
    let router = router_with(ApiServices::unavailable());
    let memories = api_request(
        router.clone(),
        "GET",
        "/api/memories",
        Some(API_TOKEN),
        None,
    )
    .await;
    let skills = api_request(router, "GET", "/api/skills", Some(API_TOKEN), None).await;

    for (name, response) in [
        ("helper", &direct),
        ("GET /api/memories", &memories),
        ("GET /api/skills", &skills),
    ] {
        assert_eq!(
            response.status,
            501,
            "{name} answered {}: {}",
            response.status,
            response.body_string()
        );
        assert_eq!(
            response.body.as_slice(),
            UNAVAILABLE_BODY.as_bytes(),
            "{name} must answer the documented stub body: {}",
            response.body_string()
        );
    }

    assert_eq!(
        memories.body.as_slice(),
        direct.body.as_slice(),
        "a per-handler copy that drifts must fail"
    );
    assert_eq!(
        skills.body.as_slice(),
        direct.body.as_slice(),
        "the delegating consumer must answer the same body"
    );
}

#[tokio::test]
async fn test_a_degraded_build_answers_501_on_a_store_backed_route() {
    let router = router_with(ApiServices::unavailable());

    let unauthenticated = api_request(router.clone(), "GET", "/api/memories", None, None).await;
    assert_eq!(
        unauthenticated.status,
        401,
        "the auth gate precedes the handler: {}",
        unauthenticated.body_string()
    );

    let authenticated = api_request(router, "GET", "/api/memories", Some(API_TOKEN), None).await;
    assert_eq!(
        authenticated.status,
        501,
        "a degraded build answered {}: {}",
        authenticated.status,
        authenticated.body_string()
    );
    assert_eq!(
        authenticated.body.as_slice(),
        UNAVAILABLE_BODY.as_bytes(),
        "unexpected degraded-build body: {}",
        authenticated.body_string()
    );
    assert_ne!(
        authenticated.status, 500,
        "an absent service is not an internal fault"
    );
}
