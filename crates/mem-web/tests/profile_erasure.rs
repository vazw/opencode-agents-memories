//! Issue 48 — the REST seam for the B2′ erasure leg.
//!
//! Covered here, on **real stores** (the production adapters, no fake store):
//!
//! - 48.7 / 48.14: `DELETE /api/user-profile` erases both profile tables on a
//!   prompt-free root — the population B2′ can never reach through a prompt
//!   event (a migrated root with a profile and zero prompts);
//! - 48.8: `PromptPort::delete_prompt` inherits the erase through
//!   `UserPromptManager::delete_prompt` (no second implementation);
//! - 48.13: the refresh readout stops advertising a learning backlog.
//!
//! The route resolves the active profile itself; the client never names a
//! `profile_id` (`DELETE /api/user-profile?userId=…`).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{api_request, app_state_with_api, temp_dir_unique};
use mem_profile::erasure::ProfileEraser;
use mem_profile::profile_store::{ProfileConfig, ProfileData, ProfileItem, ProfileStore};
use mem_profile::prompt::{USER_PROMPTS_DB_NAME, UserPromptManager};
use mem_store::config::StoreConfig;
use mem_web::api::ports::{ApiServices, PromptPort};
use mem_web::api::store_adapter::{ProductionProfile, ProductionPrompts};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The configured API token.
const API_TOKEN: &str = "test-token";
/// The default user identity the state falls back to.
const USER_ID: &str = "probe@example.com";
/// The marker carried in the seeded profile payload.
const MARKER: &str = "CANARY-48-WEB-0d4b7e21";

fn store_config(root: PathBuf) -> StoreConfig {
    StoreConfig {
        storage_path: root,
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

fn profile_data() -> ProfileData {
    ProfileData {
        preferences: vec![ProfileItem {
            description: MARKER.to_string(),
            category: Some("tools".to_string()),
            ..ProfileItem::default()
        }],
        ..ProfileData::default()
    }
}

/// Seeds one profile (with its `create` changelog) for `USER_ID`.
async fn seed_profile(store: &ProfileStore) -> String {
    store
        .create_profile(USER_ID, "Probe", "probe", USER_ID, &profile_data(), 1)
        .await
        .expect("seed the profile")
}

/// A router over the production profile/prompt adapters (the other ports stay
/// unavailable — this file exercises none of them).
fn router(store: Arc<ProfileStore>, prompts: Arc<UserPromptManager>) -> axum::Router {
    let services = ApiServices::unavailable()
        .with_profile(Arc::new(ProductionProfile::new(store)))
        .with_prompts(Arc::new(ProductionPrompts::new(prompts)));
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        USER_ID,
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

/// `SELECT COUNT(*)` on one of the two profile tables.
async fn count_rows(store: &ProfileStore, table: &str) -> i64 {
    let db = store.ready().await.expect("profile store ready");
    mem_store::db::query_one(
        db.as_ref(),
        &format!("SELECT COUNT(*) AS count FROM {table}"),
        Vec::new(),
    )
    .await
    .expect("count the rows")
    .and_then(|row| row.i64("count"))
    .unwrap_or(-1)
}

#[tokio::test]
async fn test_delete_user_profile_route_erases_a_prompt_free_root() {
    let root = temp_dir_unique("profile-erasure-route");
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        store_config(root.clone()),
        ProfileConfig::default(),
    ));
    let profile_id = seed_profile(&store).await;
    // Zero prompts: the prompt store is constructed but never opened, so the
    // route is the only surface that can erase this profile.
    let prompts = Arc::new(UserPromptManager::new(
        root.clone(),
        store_config(root.clone()),
        3,
    ));

    let response = api_request(
        router(Arc::clone(&store), prompts),
        "DELETE",
        &format!("/api/user-profile?userId={USER_ID}"),
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body: Value =
        serde_json::from_str(&response.body_string()).expect("the erase answer is JSON");
    assert_eq!(body["success"], Value::Bool(true), "{body}");
    assert_eq!(body["data"]["profileId"], profile_id, "{body}");
    assert_eq!(body["data"]["profilesDeleted"], 1, "{body}");
    assert_eq!(body["data"]["changelogsDeleted"], 1, "{body}");

    assert_eq!(
        count_rows(&store, "user_profiles").await,
        0,
        "no profile row survives the route"
    );
    assert_eq!(
        count_rows(&store, "user_profile_changelogs").await,
        0,
        "no changelog snapshot survives the route"
    );
    assert!(
        !root.join(USER_PROMPTS_DB_NAME).exists(),
        "the prompt database is untouched (and was never created)"
    );
}

#[tokio::test]
async fn test_delete_user_profile_route_answers_no_profile_found_for_an_unknown_user() {
    let root = temp_dir_unique("profile-erasure-route-missing");
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        store_config(root.clone()),
        ProfileConfig::default(),
    ));
    let prompts = Arc::new(UserPromptManager::new(
        root.clone(),
        store_config(root.clone()),
        3,
    ));

    let response = api_request(
        router(store, prompts),
        "DELETE",
        "/api/user-profile?userId=nobody%40example.com",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"No profile found"}"#
    );
}

#[tokio::test]
async fn test_prompt_port_delete_prompt_erases_the_profile() {
    let root = temp_dir_unique("profile-erasure-cascade");
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        store_config(root.clone()),
        ProfileConfig::default(),
    ));
    let manager = UserPromptManager::new(root.clone(), store_config(root.clone()), 3);
    // Prompt first, profile second: the profile's watermark is >= the prompt's
    // created_at, so the prompt is a contributing one.
    let prompt_id = manager
        .save_prompt("session-48", "message-48", "/probe", "probe prompt")
        .await
        .expect("save the prompt");
    let profile_id = seed_profile(&store).await;

    let eraser: Arc<dyn ProfileEraser> = store.clone();
    let port = ProductionPrompts::new(Arc::new(manager.with_eraser(eraser)));
    port.delete_prompt(&prompt_id)
        .await
        .expect("the port deletes the prompt through the seam");

    assert!(
        store
            .get_active_profile(USER_ID)
            .await
            .expect("read the profile")
            .is_none(),
        "the prompt path erases the contributing profile"
    );
    assert!(
        store
            .get_profile_changelogs(&profile_id, 10)
            .await
            .expect("read the changelogs")
            .is_empty(),
        "the changelog snapshots are erased with the profile"
    );
}

#[tokio::test]
async fn test_refresh_response_does_not_advertise_a_learning_backlog() {
    let root = temp_dir_unique("profile-erasure-refresh");
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        store_config(root.clone()),
        ProfileConfig::default(),
    ));
    seed_profile(&store).await;
    let prompts = Arc::new(UserPromptManager::new(
        root.clone(),
        store_config(root.clone()),
        3,
    ));

    let response = api_request(
        router(store, prompts),
        "POST",
        "/api/user-profile/refresh",
        Some(API_TOKEN),
        Some(&format!(r#"{{"userId":"{USER_ID}"}}"#)),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body: Value =
        serde_json::from_str(&response.body_string()).expect("the refresh answer is JSON");
    assert_eq!(body["success"], Value::Bool(true), "{body}");
    let data = &body["data"];
    assert!(data.get("message").is_some(), "{data}");
    assert!(data.get("profileExists").is_some(), "{data}");
    assert!(data.get("decayApplied").is_some(), "{data}");
    assert!(
        data.get("unanalyzedPrompts").is_none(),
        "the readout no longer advertises a backlog nothing consumes: {data}"
    );
    let note = data["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("disabled"),
        "the note states that automatic learning is disabled in this release: {note}"
    );
}
