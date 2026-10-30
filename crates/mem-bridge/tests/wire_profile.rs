//! Issue 70 wave 1 — the `profile` wire method (T19-T25).
//!
//! The oracles run through the production `Engine::dispatch`: the read shape,
//! the merge, the four refusals (in order, before any store write) and the
//! always-released learning lock.

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, MapScopeResolver, TestDir, build_wire_fixture, unit_vector};
use mem_bridge::jsonrpc::{Response, parse_line};
use mem_profile::profile_store::{ProfileData, ProfileItem};
use serde_json::{Value, json};

const HASH: &str = "a1a1a1a1a1a1a1a1";

/// One project, the map resolver, and the wire fixture over a temp root.
async fn fixture(prefix: &str, email: Option<&str>) -> common::WireFixture {
    let resolver = Arc::new(MapScopeResolver::single(
        "/probe/a",
        MapScopeResolver::project_tag(HASH),
        email,
    ));
    build_wire_fixture(
        TestDir::new(prefix),
        resolver,
        Arc::new(FakeEmbedder::with_vector(unit_vector(0))),
        None,
    )
    .await
}

async fn dispatch(engine: &mem_bridge::engine::Engine, params: Value) -> Response {
    let request = parse_line(
        &json!({"id": "p", "jsonrpc": "2.0", "method": "profile", "params": params}).to_string(),
    )
    .expect("profile request parses");
    engine.dispatch(&request).await
}

fn error_code(response: &Response) -> i32 {
    response
        .error
        .as_ref()
        .map(|error| error.code)
        .expect("error envelope")
}

fn data(response: &Response) -> &Value {
    response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref())
        .expect("advisory data")
}

fn result(response: &Response) -> &Value {
    response.result.as_ref().expect("result envelope")
}

fn item(description: &str) -> ProfileItem {
    ProfileItem {
        description: description.to_string(),
        ..ProfileItem::default()
    }
}

// ---------------------------------------------------------------------------
// T19 — no profile is exactly `{"profile": null}`
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_profile_read_returns_null_when_absent() {
    let fixture = fixture("wire-profile-absent", Some("probe@example.com")).await;
    let response = dispatch(
        &fixture.engine,
        json!({"session_id": "session-profile", "project_id": "/probe/a"}),
    )
    .await;
    assert_eq!(result(&response), &json!({ "profile": null }));
}

// ---------------------------------------------------------------------------
// T20 — the public shape never carries centroid/anchor
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_profile_read_returns_the_public_profile() {
    let fixture = fixture("wire-profile-public", Some("probe@example.com")).await;
    fixture
        .profile
        .create_profile(
            "probe@example.com",
            "Probe",
            "probe",
            "probe@example.com",
            &ProfileData {
                preferences: vec![ProfileItem {
                    description: "prefers tabs".to_string(),
                    centroid: Some(vec![0.1, 0.2]),
                    anchor: Some(vec![0.3, 0.4]),
                    ..ProfileItem::default()
                }],
                ..ProfileData::default()
            },
            0,
        )
        .await
        .expect("seed the profile");

    let response = dispatch(
        &fixture.engine,
        json!({"session_id": "session-profile", "project_id": "/probe/a"}),
    )
    .await;
    let item = &result(&response)["profile"]["preferences"][0];
    assert_eq!(item["description"], "prefers tabs");
    assert!(
        item.get("centroid").is_none(),
        "centroid is internal similarity state and never leaves the engine"
    );
    assert!(item.get("anchor").is_none());
    assert_eq!(result(&response)["profile"]["version"], 1);
}

// ---------------------------------------------------------------------------
// T21 — a write merges into the existing profile (never overwrites)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_profile_write_merges_and_reads_back_the_preference() {
    let fixture = fixture("wire-profile-merge", Some("probe@example.com")).await;
    fixture
        .profile
        .create_profile(
            "probe@example.com",
            "Probe",
            "probe",
            "probe@example.com",
            &ProfileData {
                preferences: vec![item("seeded preference")],
                ..ProfileData::default()
            },
            0,
        )
        .await
        .expect("seed the profile");

    let write = dispatch(
        &fixture.engine,
        json!({
            "session_id": "session-profile",
            "project_id": "/probe/a",
            "content": "prefers dark mode",
        }),
    )
    .await;
    assert_eq!(result(&write), &json!({ "saved": true }));

    let read = dispatch(
        &fixture.engine,
        json!({"session_id": "session-profile", "project_id": "/probe/a"}),
    )
    .await;
    let preferences = result(&read)["profile"]["preferences"]
        .as_array()
        .expect("preferences");
    assert_eq!(preferences.len(), 2, "the write merges, never replaces");
    let descriptions: Vec<&str> = preferences
        .iter()
        .map(|item| item["description"].as_str().expect("description"))
        .collect();
    assert!(descriptions.contains(&"seeded preference"));
    assert!(descriptions.contains(&"prefers dark mode"));
}

// ---------------------------------------------------------------------------
// T22 — blank and private content are refused before any write
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_profile_write_rejects_blank_and_private_content() {
    let fixture = fixture("wire-profile-privacy", Some("probe@example.com")).await;

    let cases = [
        ("   ", "blank"),
        ("<private>a</private><private>b</private>", "private"),
        ("<private>a</private>", "private"),
    ];
    for (content, reason) in cases {
        let response = dispatch(
            &fixture.engine,
            json!({
                "session_id": "session-profile",
                "project_id": "/probe/a",
                "content": content,
            }),
        )
        .await;
        assert_eq!(error_code(&response), -32602);
        assert_eq!(
            data(&response).get("reason").and_then(Value::as_str),
            Some(reason),
            "content {content:?} must be refused as {reason}"
        );
    }
    assert!(
        fixture
            .profile
            .get_active_profile("probe@example.com")
            .await
            .expect("read profiles")
            .is_none(),
        "no profile was written"
    );
}

// ---------------------------------------------------------------------------
// T23 — a write without a resolved email never invents the `unknown` user
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_profile_write_requires_a_resolved_user_email() {
    let fixture = fixture("wire-profile-email", None).await;
    let response = dispatch(
        &fixture.engine,
        json!({
            "session_id": "session-profile",
            "project_id": "/probe/a",
            "content": "prefers dark mode",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    assert_eq!(
        data(&response).get("reason").and_then(Value::as_str),
        Some("no_user_email")
    );
    assert!(
        fixture
            .profile
            .get_active_profile("unknown")
            .await
            .expect("read profiles")
            .is_none(),
        "the read fallback is never a write identity"
    );
}

// ---------------------------------------------------------------------------
// T24 — the learning lock is fail-closed
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_profile_write_fails_closed_while_the_learning_lock_is_held() {
    let fixture = fixture("wire-profile-lock", Some("probe@example.com")).await;
    let root = fixture.storage_root();
    let second = mem_profile::learning_lock::LearningLock::new(
        mem_profile::learning_lock::LearningLockOptions::new(
            root.clone(),
            common::store_config(root),
        ),
    );
    let guard = second.try_acquire(None).await.expect("hold the lock");

    let response = dispatch(
        &fixture.engine,
        json!({
            "session_id": "session-profile",
            "project_id": "/probe/a",
            "content": "prefers dark mode",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32603);
    assert_eq!(
        data(&response).get("reason").and_then(Value::as_str),
        Some("learning_locked")
    );
    assert_eq!(data(&response).get("retryable"), Some(&json!(true)));
    assert!(
        fixture
            .profile
            .get_active_profile("probe@example.com")
            .await
            .expect("read profiles")
            .is_none(),
        "a locked write stores nothing"
    );
    guard.release().await;
}

// ---------------------------------------------------------------------------
// T25 — the guard is released on the error path (the parse failure)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_profile_write_releases_the_learning_lock_on_a_failed_write() {
    let fixture = fixture("wire-profile-release", Some("probe@example.com")).await;
    let profile_id = fixture
        .profile
        .create_profile(
            "probe@example.com",
            "Probe",
            "probe",
            "probe@example.com",
            &ProfileData {
                preferences: vec![item("seeded preference")],
                ..ProfileData::default()
            },
            0,
        )
        .await
        .expect("seed the profile");
    // A malformed stored profile_data: the write acquires the lock, reads the
    // row, fails to parse, and must release before answering.
    let db = fixture.profile.ready().await.expect("profile db");
    db.execute(
        "UPDATE user_profiles SET profile_data = ? WHERE id = ?",
        vec![
            mem_store::db::Value::Text("{not-json".to_string()),
            mem_store::db::Value::Text(profile_id),
        ],
    )
    .await
    .expect("corrupt the stored row");

    let response = dispatch(
        &fixture.engine,
        json!({
            "session_id": "session-profile",
            "project_id": "/probe/a",
            "content": "prefers dark mode",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32603);
    assert_eq!(
        data(&response).get("reason").and_then(Value::as_str),
        Some("profile_unreadable")
    );

    assert!(
        !fixture.learning_lock.is_held().await,
        "the guard must be released on the failed-write path"
    );
    let guard = fixture
        .learning_lock
        .try_acquire(None)
        .await
        .expect("the lock is free after the failed write");
    guard.release().await;
}
