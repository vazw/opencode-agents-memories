//! Issue 43 W6c (ADR-0019 H9, D-19-3/D-19-9/D-19-11) — the erasure disclosure.
//!
//! A delete answer must never be readable as a complete erasure, so both delete
//! endpoints carry an ALWAYS-present `erasure` object beside their untouched
//! upstream flag. The status read answers from data (the tier's `sweep_state`
//! stamp), and the `notErased` map renders the vault's tri-state — `unknown`,
//! never `no`, when the tool cannot look.
//!
//! Every case drives the shipped router (`build_router` + `oneshot`) with the
//! API token, so the auth gate, the 1 MiB body cap and the D-8 Host check run.
#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::PathBuf;

use common::fakes::{MEMORY_ID, PROMPT_ID, USER_ID, fake_services};
use common::{api_request, app_state_with_api};
use mem_web::api::dto::not_erased_map;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// A router over the seeded fake backend.
fn router() -> axum::Router {
    let (_backend, services) = fake_services();
    router_for(services)
}

/// A router over a caller-supplied service bundle (a fake with a seeded
/// `sweep_state`, or one whose `sweep_state` read fails).
fn router_for(services: mem_web::api::ports::ApiServices) -> axum::Router {
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
    build_router(std::sync::Arc::new(state))
}

/// The parsed `data` object of a successful envelope.
async fn data_of(method: &str, path: &str) -> serde_json::Value {
    data_of_with(router(), method, path).await
}

/// The parsed `data` object of a successful envelope, over a given router.
async fn data_of_with(router: axum::Router, method: &str, path: &str) -> serde_json::Value {
    let response = api_request(router, method, path, Some(API_TOKEN), None).await;
    assert_eq!(response.status, 200, "{method} {path}: {response:?}");
    let body = response.body_string();
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("{path} is JSON: {error}: {body}"));
    assert_eq!(parsed["success"], serde_json::json!(true), "{path}: {body}");
    parsed["data"].clone()
}

/// Asserts an erasure object carries labels only — never a path.
fn assert_labels_not_paths(erasure: &serde_json::Value, tag: &str) {
    let mut stack = vec![erasure.clone()];
    while let Some(value) = stack.pop() {
        match value {
            serde_json::Value::String(text) => assert!(
                !text.contains('/') && !text.contains('\\'),
                "{tag}: a label must not be a path, got {text:?}"
            ),
            serde_json::Value::Array(items) => stack.extend(items),
            serde_json::Value::Object(map) => stack.extend(map.into_values()),
            _ => {}
        }
    }
}

/// (a) `DELETE /api/prompts/{id}`: the additive `erasure` object is present,
/// `deletedMemory` is untouched, and the tier is the prompt tier.
#[tokio::test]
async fn test_delete_prompt_answer_carries_the_erasure_state() {
    let data = data_of("DELETE", &format!("/api/prompts/{PROMPT_ID}")).await;

    assert_eq!(
        data["deletedMemory"],
        serde_json::json!(false),
        "the upstream flag is untouched: {data}"
    );
    let erasure = &data["erasure"];
    assert!(
        !erasure.is_null(),
        "the erasure state is ALWAYS present on a delete answer: {data}"
    );
    assert_eq!(
        erasure["state"],
        serde_json::json!("pending"),
        "a delete is PendingErase: the bytes leave only at the next sweep (ADR-0019 :165-167)"
    );
    assert_eq!(erasure["tier"], serde_json::json!("user-prompts"));
    assert_eq!(
        erasure["sweepState"],
        serde_json::json!("notYetSwept"),
        "a delete's medium step has not run, whatever the tier's stamp says: {data}"
    );
    assert_labels_not_paths(erasure, "DELETE /api/prompts/{id}");
}

/// (b) `DELETE /api/memories/{id}`: the same additive object, `deletedPrompt`
/// untouched, and the memory tier never claims a sweep it has no stamp for.
#[tokio::test]
async fn test_delete_memory_answer_carries_the_erasure_state() {
    let data = data_of("DELETE", &format!("/api/memories/{MEMORY_ID}")).await;

    assert_eq!(
        data["deletedPrompt"],
        serde_json::json!(false),
        "the upstream flag is untouched: {data}"
    );
    let erasure = &data["erasure"];
    assert!(!erasure.is_null(), "{data}");
    assert_eq!(
        erasure["state"],
        serde_json::json!("pending"),
        "a memory delete is PendingErase with noSweepForTier (ADR-0019 :166-167): {data}"
    );
    assert_eq!(erasure["tier"], serde_json::json!("memories"));
    assert_eq!(erasure["sweepState"], serde_json::json!("noSweepForTier"));
    assert_labels_not_paths(erasure, "DELETE /api/memories/{id}");
}

/// (c)+(d) A delete answer never names the `milestones` tier, and the
/// milestones counter is zero by construction (the sweep has not run).
#[tokio::test]
async fn test_a_delete_answer_never_names_milestones_and_scrubs_zero() {
    for path in [
        format!("/api/prompts/{PROMPT_ID}"),
        format!("/api/memories/{MEMORY_ID}"),
    ] {
        let data = data_of("DELETE", &path).await;
        let erasure = &data["erasure"];
        assert_ne!(
            erasure["tier"],
            serde_json::json!("milestones"),
            "{path}: a delete is not a milestone scrub: {data}"
        );
        assert_eq!(
            erasure["survivingDerived"]["milestonesScrubbed"],
            serde_json::json!(0),
            "{path}: zero by construction on a delete: {data}"
        );
    }
}

/// (e) The erasure object's keys are the amended D-19-3 vocabulary — a rename
/// here would silently change the contract the UI renders.
#[tokio::test]
async fn test_the_erasure_object_uses_the_amended_vocabulary() {
    let data = data_of("DELETE", &format!("/api/prompts/{PROMPT_ID}")).await;
    let erasure = data["erasure"].as_object().expect("an object");
    let mut keys: Vec<&str> = erasure.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "notErased",
            "state",
            "survivingDerived",
            "sweepState",
            "tier"
        ],
        "the erasure object's shape is the disclosure contract"
    );
    let surviving = erasure["survivingDerived"].as_object().expect("an object");
    let mut keys: Vec<&str> = surviving.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["memories", "milestonesScrubbed"]);
}

/// (f) `GET /api/prompts/{id}/erasure`: an existing prompt is `pending` (it was
/// never deleted), an unknown id is the 404 envelope.
#[tokio::test]
async fn test_status_read_answers_pending_and_404_for_an_unknown_id() {
    let data = data_of("GET", &format!("/api/prompts/{PROMPT_ID}/erasure")).await;
    assert_eq!(data["state"], serde_json::json!("pending"));
    assert_eq!(data["tier"], serde_json::json!("user-prompts"));
    assert_eq!(data["sweepState"], serde_json::json!("notYetSwept"));
    assert_labels_not_paths(&data, "GET /api/prompts/{id}/erasure");

    let response = api_request(
        router(),
        "GET",
        "/api/prompts/does-not-exist/erasure",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 404, "{response:?}");
    assert!(
        response.body_string().contains(r#""success":false"#),
        "an unknown id uses the envelope"
    );
}

/// (h) F2: a seeded stamp answers `swept` on the status read — the M1 killer.
///
/// Distinguishing input: `sweep_state` returns `Ok(Some(..))`. A mapping that
/// answers `notYetSwept` for a swept tier survives every other case in this
/// file (they all see the fake's `Ok(None)` default), which is exactly the
/// vacuous oracle the review measured.
#[tokio::test]
async fn test_a_swept_tier_answers_swept_from_the_stamp() {
    let (backend, services) = fake_services();
    backend.set_sweep_stamp(Some(1_700_000_000_000));
    let data = data_of_with(
        router_for(services),
        "GET",
        &format!("/api/prompts/{PROMPT_ID}/erasure"),
    )
    .await;
    assert_eq!(
        data["sweepState"],
        serde_json::json!("swept"),
        "a tier with a stamp answers `swept`, never `notYetSwept`: {data}"
    );
    assert_eq!(
        data["state"],
        serde_json::json!("pending"),
        "Erased is the sweep report's answer, never the HTTP DTO's (ADR-0019 :167): {data}"
    );
}

/// (i) F1: a swept stamp must not turn a DELETE into `erased` — a delete's
/// residue postdates any stamp (ADR-0019 :165-167).
#[tokio::test]
async fn test_a_swept_stamp_does_not_turn_a_delete_into_erased() {
    let (backend, services) = fake_services();
    backend.set_sweep_stamp(Some(1_700_000_000_000));
    let data = data_of_with(
        router_for(services),
        "DELETE",
        &format!("/api/prompts/{PROMPT_ID}"),
    )
    .await;
    assert_eq!(
        data["erasure"]["state"],
        serde_json::json!("pending"),
        "a delete after a completed sweep is still PendingErase: {data}"
    );
    assert_eq!(
        data["erasure"]["sweepState"],
        serde_json::json!("notYetSwept"),
        "the medium step has not run for THIS delete: {data}"
    );
}

/// (j) F2: a failed stamp read answers `sweepFailed`, never `notYetSwept`
/// (a failed read is not evidence that no sweep ran).
#[tokio::test]
async fn test_a_failed_stamp_read_answers_sweep_failed() {
    let (backend, services) = fake_services();
    backend.set_sweep_read_failure(true);
    let data = data_of_with(
        router_for(services),
        "GET",
        &format!("/api/prompts/{PROMPT_ID}/erasure"),
    )
    .await;
    assert_eq!(
        data["sweepState"],
        serde_json::json!("sweepFailed"),
        "a failed stamp read is sweepFailed, never notYetSwept: {data}"
    );
}

/// (g) The vault's tri-state, both directions: a failed open is `unknown`, a
/// known-closed vault is `no` — the D-19-9 pair, never conflated.
#[test]
fn test_vault_tri_state_is_unknown_on_a_failed_open_and_no_when_known() {
    let unknown = not_erased_map(None);
    let vault = unknown
        .iter()
        .find(|entry| entry.tier == "vault")
        .expect("the vault tier is always listed");
    assert_eq!(vault.erased, "unknown", "a failed open is never `no`");

    let known = not_erased_map(Some(false));
    let vault = known
        .iter()
        .find(|entry| entry.tier == "vault")
        .expect("the vault tier is always listed");
    assert_eq!(vault.erased, "no");

    let erased = not_erased_map(Some(true));
    let vault = erased
        .iter()
        .find(|entry| entry.tier == "vault")
        .expect("the vault tier is always listed");
    assert_eq!(vault.erased, "yes");

    for entry in unknown {
        assert!(
            !entry.tier.contains('/') && !entry.reason.contains('/'),
            "labels never paths: {entry:?}"
        );
    }
}
