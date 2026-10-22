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
use axum::response::Response;
use serde::Deserialize;

use super::dto::{DeletedCountDto, DeletedPromptDto};
use super::envelope;
use super::memories::delete_memory_by_id;
use super::ports::PromptRow;
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
        Ok(Some(deleted_memory)) => envelope::ok(&DeletedPromptDto { deleted_memory }),
        Ok(None) => envelope::business_error("Prompt not found"),
        Err(response) => response,
    }
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
/// `Ok(Some(deleted_memory))` means the prompt existed and was deleted,
/// `Ok(None)` means it does not exist (upstream's `"Prompt not found"`)
/// and `Err` carries the already-built response for a port failure (D-9).
async fn delete_prompt_by_id(
    state: &Arc<AppState>,
    id: &str,
    cascade: bool,
    endpoint: &'static str,
) -> Result<Option<bool>, Response> {
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

    state
        .api
        .prompts
        .delete_prompt(id)
        .await
        .map_err(|error| port_failure(endpoint, error))?;
    Ok(Some(deleted_memory))
}
