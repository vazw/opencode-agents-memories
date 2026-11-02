//! `GET /api/user-profiles` — the identity roster (issue 115, D-3).
//!
//! The route exists so the SPA can offer every stored identity instead of
//! silently substituting one for another. It is a port-only row (ADR-0003): no
//! upstream counterpart, so this file is the contract — every stored profile is
//! listed, the composed identity is marked, the order is deterministic, the
//! list is bounded, and an unwired port degrades to the documented `501`.
//!
//! The roster is **not** an enumeration oracle: it exposes stored identities
//! only — never `profile_data`, never a centroid/anchor, never a filesystem
//! path — and it sits behind the same token gate as every `/api/*` route (that
//! gating is proved by the ROUTE_TABLE walk, not here).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::fakes::{FakeBackend, SECOND_USER_ID, USER_ID, fake_services};
use common::{api_request, app_state_with_api};
use mem_web::api::ports::{ApiServices, ProfileRecord};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The API token the roster cases configure.
const API_TOKEN: &str = "test-token";

/// A router over the seeded fake, composed under `user_id`.
fn roster_router(seed: impl FnOnce(&FakeBackend), user_id: &str) -> axum::Router {
    let (backend, services) = fake_services();
    seed(&backend);
    build_router(Arc::new(state_with(services, user_id)))
}

/// The same, over the documented unavailable port bundle.
fn unavailable_router() -> axum::Router {
    build_router(Arc::new(state_with(
        ApiServices::unavailable(),
        SECOND_USER_ID,
    )))
}

fn state_with(api: ApiServices, user_id: &str) -> mem_web::server::AppState {
    app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        api,
        user_id,
        "127.0.0.1",
    )
}

/// One profile row with an explicit prompt count (the roster's first sort key).
fn profile_row(user_id: &str, prompts: i64) -> ProfileRecord {
    ProfileRecord {
        id: format!("profile_{user_id}"),
        user_id: user_id.to_string(),
        display_name: user_id.to_string(),
        user_name: user_id.to_string(),
        user_email: user_id.to_string(),
        profile_data: r#"{"preferences":[],"patterns":[],"workflows":[],"learningPaths":[]}"#
            .to_string(),
        version: 1,
        created_at: 1_700_000_000_000,
        last_analyzed_at: 1_700_000_000_000,
        total_prompts_analyzed: prompts,
    }
}

/// The `data` array of a `200` envelope.
fn data_rows(response: &common::RawResponse) -> Vec<Value> {
    let body = response.body_string();
    let parsed: Value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("body is JSON ({error}): {body}"));
    assert_eq!(parsed["success"], true, "the success envelope: {body}");
    parsed["data"]
        .as_array()
        .unwrap_or_else(|| panic!("`data` is an array: {body}"))
        .clone()
}

#[tokio::test]
async fn test_profile_roster_lists_every_active_profile_with_the_default_marked() {
    // The composed identity is the SECOND profile: `isDefault = index == 0`
    // would mark the wrong row.
    let router = roster_router(|backend| backend.seed_second_profile(), SECOND_USER_ID);
    let response = api_request(router, "GET", "/api/user-profiles", Some(API_TOKEN), None).await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let rows = data_rows(&response);
    assert_eq!(rows.len(), 2, "every stored profile is listed: {rows:?}");

    // The seeded profile has 12 analyzed prompts, the second one 4: the
    // prompt count desc puts the seeded row first.
    assert_eq!(rows[0]["userId"], USER_ID);
    assert_eq!(rows[0]["isDefault"], false);
    assert_eq!(rows[1]["userId"], SECOND_USER_ID);
    assert_eq!(
        rows[1]["isDefault"], true,
        "the composed identity is marked"
    );

    for key in [
        "userId",
        "displayName",
        "version",
        "totalPromptsAnalyzed",
        "lastAnalyzedAt",
        "isDefault",
    ] {
        assert!(
            rows[0].get(key).is_some(),
            "the row carries {key}: {rows:?}"
        );
    }
    // The roster never leaks the profile body.
    let body = response.body_string();
    assert!(
        !body.contains("centroid") && !body.contains("anchor") && !body.contains("profileData"),
        "the roster exposes identities only: {body}"
    );
}

#[tokio::test]
async fn test_profile_roster_orders_by_prompts_then_user_id() {
    // Two keys, both exercised: prompts 9/9 tie on the primary key with
    // user_ids "b@x"/"a@x" (differing on the secondary), and 3/1 differ on the
    // primary. A single-key comparator cannot produce this order.
    let router = roster_router(
        |backend| {
            // The fixture starts with one seeded identity; these cases declare
            // their own roster exactly.
            backend.clear_profile();
            backend.seed_profile(profile_row("c@example.com", 3));
            backend.seed_profile(profile_row("b@example.com", 9));
            backend.seed_profile(profile_row("a@example.com", 9));
            backend.seed_profile(profile_row("d@example.com", 1));
        },
        NO_PROFILE_USER,
    );
    let response = api_request(router, "GET", "/api/user-profiles", Some(API_TOKEN), None).await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let order: Vec<String> = data_rows(&response)
        .iter()
        .map(|row| row["userId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        order,
        vec![
            "a@example.com".to_string(),
            "b@example.com".to_string(),
            "c@example.com".to_string(),
            "d@example.com".to_string(),
        ],
        "prompts desc, then user_id asc"
    );
}

/// A composed identity no seeded row carries (so no row is the default).
const NO_PROFILE_USER: &str = "nobody@example.com";

#[tokio::test]
async fn test_profile_roster_is_bounded() {
    let total = 201;
    let router = roster_router(
        move |backend| {
            backend.clear_profile();
            for index in 0..total {
                backend.seed_profile(profile_row(&format!("u{index:03}@example.com"), index));
            }
        },
        NO_PROFILE_USER,
    );
    let response = api_request(router, "GET", "/api/user-profiles", Some(API_TOKEN), None).await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let rows = data_rows(&response);
    assert_eq!(rows.len(), 200, "the roster is bounded at 200 rows");
    // The bound truncates the TAIL of the documented order, never the head.
    assert_eq!(rows[0]["userId"], "u200@example.com");
    assert_eq!(rows[199]["userId"], "u001@example.com");
}

#[tokio::test]
async fn test_profile_roster_answers_501_when_the_port_is_unavailable() {
    let response = api_request(
        unavailable_router(),
        "GET",
        "/api/user-profiles",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        501,
        "an unwired port is the absent-service class: {}",
        response.body_string()
    );
    assert!(
        response
            .body_string()
            .contains(mem_web::api::UNAVAILABLE_MESSAGE),
        "the documented 501 body: {}",
        response.body_string()
    );
}

/// The upstream literal, frozen: this is what upstream's own single-identity
/// install answers and it must not drift.
const NO_PROFILE_MESSAGE: &str = "No profile found. Keep chatting to build your profile.";

/// The `message` of a `200` missing-profile envelope.
fn missing_message(response: &common::RawResponse) -> String {
    let body = response.body_string();
    let parsed: Value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("body is JSON ({error}): {body}"));
    assert_eq!(parsed["success"], true, "the success envelope: {body}");
    assert_eq!(
        parsed["data"]["exists"], false,
        "the missing branch: {body}"
    );
    parsed["data"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("`message` is a string: {body}"))
        .to_string()
}

#[tokio::test]
async fn test_missing_profile_message_names_the_identity_when_others_exist() {
    // Two stored identities, neither of them the requested one: the page can
    // offer a picker, so the message says so instead of denying that any
    // profile exists. Mutant: always NO_PROFILE_MESSAGE.
    let router = roster_router(|backend| backend.seed_second_profile(), USER_ID);
    let response = api_request(
        router,
        "GET",
        "/api/user-profile?userId=ghost%40x.example",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());

    let message = missing_message(&response);
    assert!(
        message.contains("ghost@x.example"),
        "the message names the requested identity: {message}"
    );
    assert!(
        message.contains("2 other profiles"),
        "the message counts the alternatives: {message}"
    );
    assert!(
        !message.contains(USER_ID) && !message.contains(SECOND_USER_ID),
        "the message names no OTHER identity: {message}"
    );
}

#[tokio::test]
async fn test_missing_profile_with_no_stored_profiles_keeps_the_upstream_literal() {
    // Regression guard (green before and after this step): upstream's
    // single-identity install reaches exactly this state, so the literal stays
    // byte-for-byte.
    let router = roster_router(|backend| backend.clear_profile(), "ghost@x.example");
    let response = api_request(
        router,
        "GET",
        "/api/user-profile?userId=ghost%40x.example",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(missing_message(&response), NO_PROFILE_MESSAGE);
}
