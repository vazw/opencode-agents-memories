//! Issue 130 — a registered shard whose DB file was deleted while the engine
//! runs must not be silently resurrected by the open-first read lanes.
//!
//! The matrix (measured on the engine, issue 130 REPORTED rows): `GET /api/tags`
//! (row 1), unfiltered `GET /api/memories` (row 2) and `GET /api/memories?tag=`
//! (row 3) recreate the missing file through `open_shard`'s create-on-open and
//! answer the D-9 500. The fix (this wave) installs a stat-before-open guard on
//! the four read lanes and answers the refusal through the new typed arm
//! (`PortError::ShardFileMissing` → 503).
//!
//! Assertion split pinned by the plan-gate's Step-2 record (the five-test
//! truth):
//! - **A** carries the no-resurrection pin (`!exists()` + the fixed envelope
//!   body discipline) and must stay green under the Step-2 interim mapping —
//!   it pins no status.
//! - **B/C** carry the 503 status pin (RED pre-implementation and RED again in
//!   Step 2 until the typed arm lands).
//! - Bodies assert membership in the fixed envelope set (`Internal Server
//!   Error` and the 503 message) plus the never-echo rule — no path, no file
//!   name, no errno. Any echo fails these assertions.
//!
//! File-deletion hygiene is copied from `scope_wide_delete.rs`: drop the real
//! store (no cached handles left), delete the file, and only then hand a
//! fresh manager to the router. Post-deletion checks are response-only plus
//! `!exists()` — NEVER a `row_ids` call, which would itself open (and
//! recreate) the deleted file.
//!
//! NOTE on the interim Step-2 state: until the typed arm lands the guard
//! answers through `PortError::Store` (the frozen 500), so A is green and B/C
//! red on exactly the 503 assertion. Step 3 flips B/C green by construction —
//! no test is ever edited to make it pass.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::real_store::{RealStore, create_shard, fresh_manager, insert, new_store, unit_x};
use common::{api_request, app_state_with_api, throwaway_log_dir};
use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_web::api::ports::ApiServices;
use mem_web::api::store_adapter::ProductionMemory;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::{Value, json};

/// The configured API token (`scope_wide_delete.rs` uses the same).
const API_TOKEN: &str = "test-token";

/// The project hash the fixture registers — `scope_wide_delete.rs`'s `HASH`
/// (`a1b2c3d4e5f67890`). The container tag derives from this constant; it is
/// never copied from `TAG_SHARD_0`/`TAG_SHARD_1` (those spell `…67891`/`…67892`,
/// hashes the fixture does NOT register — the r1 vacuous-input defect).
pub const HASH: &str = "a1b2c3d4e5f67890";

/// The registered container tag, derived from [`HASH`] at runtime: the same
/// spelling the fixture inserts rows under, so `resolve_scoped_request`
/// resolves the tag to the registered `(Project, a1b2c3d4e5f67890)` shard.
fn tag() -> String {
    format!("opencode_project_{HASH}")
}

/// The fixed D-9 500 error text (`envelope::INTERNAL_ERROR`) — the generic
/// envelope the read lane answered pre-fix and answers again under the interim
/// `PortError::Store` mapping.
const FIXED_INTERNAL_ERROR: &str = "Internal Server Error";

/// The fixed 503 refusal message (`PortError::ShardFileMissing`'s envelope).
const FIXED_FILE_MISSING: &str =
    "shard storage is temporarily unavailable — the shard file is missing";

/// The id the fixture seeds ("seed-id").
const SEED_ID: &str = "seed-id";

/// The production memory port over an explicitly built manager (the
/// deleted-file tests hand in a fresh manager with no cached handles —
/// `scope_wide_delete.rs`'s `router_for` shape verbatim).
fn router_for(manager: ShardManager) -> axum::Router {
    let services = ApiServices::unavailable().with_memory(Arc::new(ProductionMemory::new(manager)));
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        throwaway_log_dir(),
        services,
        "unknown",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

/// The parsed JSON body of a response.
fn json_body(response: &common::RawResponse) -> Value {
    serde_json::from_slice(&response.body).expect("the response body is JSON")
}

/// The file name component of a shard path.
fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .expect("the shard path carries a file name")
}

/// One registered project shard holding one memory; the fixture owns the temp
/// storage root.
async fn seeded_project_shard(label: &str) -> (RealStore, ShardInfo) {
    let store = new_store(label);
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    insert(
        &store,
        &shard,
        SEED_ID,
        "seeded memory body",
        &tag(),
        1000,
        unit_x(),
    )
    .await;
    (store, shard)
}

/// The pieces a deleted-file test needs after the store is dropped: the
/// config (for fresh managers).
struct DeletedFixture {
    config: StoreConfig,
}

/// Drops the store, deletes the shard file, and asserts it is really gone
/// (`scope_wide_delete.rs`'s hygiene at :355-361).
fn drop_store_and_delete_shard(store: RealStore, shard: &ShardInfo) -> DeletedFixture {
    let config = store.config.clone();
    let shard_path = shard.db_path.clone();
    drop(store);
    std::fs::remove_file(&shard_path).expect("remove the shard database");
    assert!(!shard_path.exists(), "the shard file is really gone");
    DeletedFixture { config }
}

/// Asserts a response body carries ONE of the fixed refusal envelopes and
/// nothing more: no path separator, no file name echo, no errno text. This is
/// the never-echo rule of the wave — the `file_name` rides the log channel
/// only (D-9 / ADR-0015 D-1).
fn assert_fixed_refusal_body(response: &common::RawResponse, file_name: &str) {
    let body = json_body(response);
    assert_eq!(
        body["success"],
        json!(false),
        "the refusal is an error envelope"
    );
    let error = body["error"]
        .as_str()
        .expect("the refusal envelope carries an error string")
        .to_string();
    assert!(
        error == FIXED_INTERNAL_ERROR || error == FIXED_FILE_MISSING,
        "the refusal error stays a member of the fixed envelope set: {error}"
    );
    assert!(
        !error.contains('/'),
        "the refusal never echoes a path: {error}"
    );
    assert!(
        !error.contains(file_name),
        "the refusal never echoes the file name: {error}"
    );
    assert!(
        !error.contains("No such file"),
        "the refusal never echoes an errno text: {error}"
    );
}

/// Asserts a healthy listing response and that the seeded row is in it.
fn assert_seeded_row_present(response: &common::RawResponse) {
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = json_body(response);
    let items = body["data"]["items"]
        .as_array()
        .expect("the timeline carries items");
    assert!(
        items
            .iter()
            .any(|item| item["type"] == json!("memory") && item["id"] == json!(SEED_ID)),
        "the seeded row is listed: {}",
        response.body_string()
    );
}

/// Assertion A's wiring: a deleted registered shard must not be resurrected by
/// the unfiltered memory read. REPORTED matrix row 2 (pre-fix: 500 +
/// recreated — the recreated file is schema-less, so the walk 500s).
#[tokio::test]
async fn test_deleted_registered_shard_read_refuses_without_resurrecting_get_memories() {
    let (store, shard) = seeded_project_shard("shard-resurrect-a").await;
    let shard_path = shard.db_path.clone();
    let file_name = file_name_of(&shard_path);

    // The healthy control (mandatory): the request walks the registered
    // shard's `all_memories` leg BEFORE anything is deleted.
    let control = api_request(
        router_for(store.manager.clone()),
        "GET",
        "/api/memories?includePrompts=false",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_seeded_row_present(&control);

    let fixture = drop_store_and_delete_shard(store, &shard);

    let response = api_request(
        router_for(fresh_manager(&fixture.config)),
        "GET",
        "/api/memories?includePrompts=false",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_fixed_refusal_body(&response, &file_name);
    assert!(
        !shard_path.exists(),
        "the read must never re-create a missing registered shard file"
    );
}

/// Assertion B: row 1, `GET /api/tags`. Expect 503 + fixed body + file absent
/// (pre-fix: 500 + recreated). RED on the 503 assertion until the typed arm
/// lands (green on the body half even under the interim `Store` mapping).
#[tokio::test]
async fn test_deleted_registered_shard_read_answers_503_get_tags() {
    let (store, shard) = seeded_project_shard("shard-resurrect-b").await;
    let shard_path = shard.db_path.clone();
    let file_name = file_name_of(&shard_path);

    // The healthy control: the tags legs run FIRST on this route
    // (collect_distinct_tags at tags.rs:114 before all_scope_rows at :122),
    // so green pre-deletion proves the route reaches this shard's
    // `distinct_tags` leg.
    let control = api_request(
        router_for(store.manager.clone()),
        "GET",
        "/api/tags",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(control.status, 200, "{}", control.body_string());
    let control_body = json_body(&control);
    assert!(
        control_body["data"]["project"]
            .as_array()
            .expect("project tags array")
            .iter()
            .any(|entry| entry["tag"] == json!(tag())),
        "the registered tag is listed: {}",
        control.body_string()
    );

    let fixture = drop_store_and_delete_shard(store, &shard);

    let response = api_request(
        router_for(fresh_manager(&fixture.config)),
        "GET",
        "/api/tags",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        503,
        "row 1: the deleted registered shard answers the typed 503: {}",
        response.body_string()
    );
    assert_fixed_refusal_body(&response, &file_name);
    assert!(
        !shard_path.exists(),
        "the tags walk must never re-create a missing registered shard file"
    );
}

/// Assertion C: row 3, `GET /api/memories?tag=<registered tag>`. The tag is
/// derived from the fixture's REGISTERED hash — a tag over an unregistered
/// hash would resolve `(Project, "…67891")`, answer 200 for the wrong reason
/// (the r1 vacuous-input defect the gate caught). The healthy control proves
/// the tag actually reaches the registered shard's
/// `list_memories_by_container_tag` leg. Expect 503 + fixed body + file
/// absent (pre-fix: 500 + recreated).
#[tokio::test]
async fn test_deleted_registered_shard_read_answers_503_get_memories_tagged() {
    let (store, shard) = seeded_project_shard("shard-resurrect-c").await;
    let shard_path = shard.db_path.clone();
    let file_name = file_name_of(&shard_path);

    // The healthy control (MANDATORY per the gate): the same tagged request
    // returns the seeded row BEFORE the file is removed.
    let control_path = format!("/api/memories?tag={}&includePrompts=false", tag());
    let control = api_request(
        router_for(store.manager.clone()),
        "GET",
        &control_path,
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_seeded_row_present(&control);

    let fixture = drop_store_and_delete_shard(store, &shard);

    let response = api_request(
        router_for(fresh_manager(&fixture.config)),
        "GET",
        &control_path,
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        503,
        "row 3: the deleted registered shard answers the typed 503: {}",
        response.body_string()
    );
    assert_fixed_refusal_body(&response, &file_name);
    assert!(
        !shard_path.exists(),
        "the tagged read must never re-create a missing registered shard file"
    );
}

/// Assertion D — the create-on-open NEGATIVE pin (GET shape ONLY): with the
/// shard file PRESENT the unfiltered read answers 200 and lists the seeded
/// row, and the file is still there afterwards. NEVER the POST shape:
/// `ApiServices::unavailable()` wires `embed: UnavailablePort` and POST embeds
/// BEFORE insert (memories.rs:125-133), so a POST would answer 501 for the
/// wrong reason. Also covers the fresh-root case structurally: the fixture's
/// explicit `create_shard` registration path never routes through the guarded
/// read lane.
#[tokio::test]
async fn test_present_registered_shard_read_answers_200() {
    let (store, shard) = seeded_project_shard("shard-resurrect-d").await;
    let shard_path = shard.db_path.clone();

    let response = api_request(
        router_for(store.manager.clone()),
        "GET",
        "/api/memories?includePrompts=false",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_seeded_row_present(&response);
    assert!(
        shard_path.exists(),
        "a present shard file is never touched by the read"
    );
}

/// Assertion E — the stats lane pin: `collect_shard_stats` self-guards with
/// its own existence check (stats.rs:108), so `GET /api/stats` on this same
/// deleted-file fixture still answers 200 with `fileExists:false` and the
/// pinned upstream text `Shard DB file missing: <fileName>` (the contract of
/// `test_stats_missing_shard_file_sets_file_name_only_error_and_succeeds`,
/// stats_contract.rs:505-556, re-pinned on the real-store fixture). The read
/// guard must never change this lane. The deleted-file fixture is the ONLY
/// dead shard registered, so it contributes exactly zero to the totals.
#[tokio::test]
async fn test_stats_lane_unaffected_by_read_guard() {
    let (store, shard) = seeded_project_shard("shard-resurrect-e").await;
    let shard_path = shard.db_path.clone();
    let file_name = file_name_of(&shard_path);
    let fixture = drop_store_and_delete_shard(store, &shard);

    let response = api_request(
        router_for(fresh_manager(&fixture.config)),
        "GET",
        "/api/stats",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = json_body(&response);

    let shards = body["data"]["shards"]
        .as_array()
        .expect("the stats carry the shard rows");
    assert_eq!(shards.len(), 1, "only the fixture shard is registered");
    let dead = &shards[0];
    assert_eq!(dead["scopeHash"], json!(HASH));
    assert_eq!(dead["fileExists"], json!(false));
    assert_eq!(
        dead["error"],
        json!(format!("Shard DB file missing: {file_name}")),
        "the pinned fileName-only error text"
    );
    let error_text = dead["error"].as_str().expect("an error string");
    assert!(!error_text.contains('/'), "never a path: {error_text}");
    assert_eq!(dead["memoryCount"], json!(0));
    assert_eq!(body["data"]["total"], json!(0));
    assert!(
        !shard_path.exists(),
        "the stats lane must not resurrect the file either"
    );
}
