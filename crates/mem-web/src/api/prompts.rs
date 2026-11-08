//! The prompt handlers (upstream `api-handlers.ts` L937–974).
//!
//! `DELETE /api/prompts/{id}` mirrors upstream's `handleDeletePrompt`: the
//! `?cascade=true` flag also deletes the prompt's linked memory, and the
//! answer carries the `deletedMemory` flag. `POST /api/prompts/bulk-delete`
//! mirrors `handleBulkDeletePrompts`: `cascade` defaults to **true** and the
//! answer counts the prompts that existed and were deleted.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

use super::dto::{
    DeletedCountDto, DeletedPromptDto, ErasureStateDto, SurvivingDerivedDto, not_erased_map,
};
use super::envelope;
use super::memories::delete_memory_by_id;
use super::ports::{PromptErasure, PromptRow};
use super::{json_body_required, port_failure, query_first};
use crate::server::AppState;

/// Upstream `handleBulkDeletePrompts`'s request body (`cascade` defaults to
/// true, matching `body.cascade !== false` in `web-server.ts`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkDeletePromptsRequest {
    ids: Option<Vec<String>>,
    cascade: Option<bool>,
}

/// `DELETE /api/prompts/{id}` — upstream `handleDeletePrompt`.
pub async fn delete_prompt(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    if id.is_empty() {
        return envelope::business_error("id is required");
    }
    let cascade = query_first(&query, "cascade") == Some("true");

    match delete_prompt_by_id(&state, &id, cascade, "DELETE /api/prompts/{id}").await {
        Ok(Some(erasure)) => envelope::ok(&DeletedPromptDto {
            deleted_memory: erasure.deleted_memory,
            erasure: delete_erasure("user-prompts"),
        }),
        Ok(None) => envelope::business_error("Prompt not found"),
        Err(response) => response,
    }
}

/// `GET /api/prompts/{id}/erasure` — the disclosure status read (ADR-0019 H9).
///
/// Answers from the prompt's existence and the tier's `sweep_state` row: an
/// unknown id is the 404 envelope, and a prompt that still exists is `pending`.
pub async fn erasure(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if id.is_empty() {
        return envelope::business_error("id is required");
    }
    match state.api.prompts.prompt_by_id(&id).await {
        Ok(Some(_)) => envelope::ok(&status_erasure(&state, "user-prompts").await),
        Ok(None) => envelope::json(StatusCode::NOT_FOUND, envelope::error("Prompt not found")),
        Err(error) => port_failure("GET /api/prompts/{id}/erasure", error),
    }
}

/// Builds one tier's erasure state from explicit labels. `sweep_state` is
/// data: a tier with a stamp answers `swept`, one without answers
/// `notYetSwept`, a failed read `sweepFailed`, and a tier the sweep never
/// covers `noSweepForTier`. The vault renders `unknown` (no vault port answers
/// here), never `no`.
fn erasure_dto(
    tier: &'static str,
    lifecycle: &'static str,
    sweep_state: &'static str,
) -> ErasureStateDto {
    ErasureStateDto {
        state: lifecycle,
        tier,
        sweep_state,
        surviving_derived: SurvivingDerivedDto {
            memories: None,
            milestones_scrubbed: 0,
        },
        not_erased: not_erased_map(None),
    }
}

/// The DELETE answer: `PendingErase`, always (ADR-0019 :165-167). A delete's
/// residue postdates any stamp, so the tier's `sweep_state` row cannot make it
/// `Erased`; the prompts tier's medium step has not run, and memories have no
/// sweep at all.
pub(super) fn delete_erasure(tier: &'static str) -> ErasureStateDto {
    let sweep_state = if tier == "memories" {
        "noSweepForTier"
    } else {
        "notYetSwept"
    };
    erasure_dto(tier, "pending", sweep_state)
}

/// The status read (H9): the tier's stamp, mapped through D-19-3. A stamp is
/// `swept`, its absence `notYetSwept`, and a failed read `sweepFailed` — never
/// `notYetSwept`, which would claim a sweep never ran when the tool cannot look.
pub(super) async fn status_erasure(state: &Arc<AppState>, tier: &'static str) -> ErasureStateDto {
    let sweep_state = if tier == "memories" {
        "noSweepForTier"
    } else {
        match state.api.prompts.sweep_state(tier).await {
            Ok(Some(_)) => "swept",
            Ok(None) => "notYetSwept",
            Err(_) => "sweepFailed",
        }
    };
    erasure_dto(tier, "pending", sweep_state)
}

/// `POST /api/prompts/bulk-delete` — upstream `handleBulkDeletePrompts`.
pub async fn bulk_delete_prompts(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: BulkDeletePromptsRequest = match json_body_required(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(ids) = request.ids.filter(|ids| !ids.is_empty()) else {
        return envelope::business_error("ids array is required");
    };
    let cascade = request.cascade.unwrap_or(true);

    let mut deleted = 0_usize;
    for id in ids {
        match delete_prompt_by_id(&state, &id, cascade, "POST /api/prompts/bulk-delete").await {
            Ok(Some(_)) => deleted += 1,
            // A missing prompt is upstream's soft failure: skipped, not counted.
            Ok(None) => {}
            Err(response) => return response,
        }
    }

    envelope::ok(&DeletedCountDto { deleted })
}

/// Deletes one prompt (upstream `handleDeletePrompt`'s body).
///
/// `Ok(Some(..))` means the prompt existed and was deleted, `Ok(None)` means it
/// does not exist (upstream's `"Prompt not found"`) and `Err` carries the
/// already-built response for a port failure (D-9).
async fn delete_prompt_by_id(
    state: &Arc<AppState>,
    id: &str,
    cascade: bool,
    endpoint: &'static str,
) -> Result<Option<DeletedPromptOutcome>, Response> {
    let prompt: Option<PromptRow> = state
        .api
        .prompts
        .prompt_by_id(id)
        .await
        .map_err(|error| port_failure(endpoint, error))?;
    let Some(prompt) = prompt else {
        return Ok(None);
    };

    let mut deleted_memory = false;
    if cascade && let Some(memory_id) = prompt.linked_memory_id.as_deref() {
        // Upstream's `handleDeleteMemory(linkedMemoryId, false)`: a missing
        // linked memory is a soft failure and the prompt is still deleted.
        if delete_memory_by_id(state, memory_id, false, "DELETE /api/memories/{id}").await? {
            deleted_memory = true;
        }
    }

    let erasure = state
        .api
        .prompts
        .delete_prompt(id)
        .await
        .map_err(|error| port_failure(endpoint, error))?;
    Ok(Some(DeletedPromptOutcome {
        deleted_memory,
        erasure,
    }))
}

/// What `delete_prompt_by_id` reports: the upstream flag plus the erasure the
/// adapter returned.
struct DeletedPromptOutcome {
    deleted_memory: bool,
    /// What the delete actually erased (`profiles_deleted`, `changelogs_deleted`,
    /// `existed`). Carried for the next wave's disclosure — the DTO has no field
    /// for these counts yet (ADR-0019 R-19-3: `surviving_derived` carries only
    /// `memories`, which is `null` until provenance lands, and
    /// `milestones_scrubbed`, which is 0 by construction), so W6c reads none of
    /// it. Deleting the field instead would discard the only record of the
    /// cascade the delete performed.
    #[allow(dead_code)]
    erasure: PromptErasure,
}
