//! The skills endpoints (upstream `api-handlers.ts` L1673–1816) and the two
//! OpenCode catalog paths (upstream `web-server.ts` L755–786).
//!
//! The catalog paths answer **outside** the envelope: `/skills/index.json`
//! returns the bare `{"skills":[…]}` object and `/skills/{id}/{id}.md` returns
//! raw markdown (D-11's conditional token posture lives in `gate.rs`).
//!
//! Two hardening rules sit on the REST endpoints, both recorded:
//! - the `?tag` filter **and** the body's `containerTag` go through the same
//!   guard as every other scoped endpoint (D-16, D-18) — a malformed tag is a
//!   `400` and never reaches the [`SkillPort`](super::ports::SkillPort);
//! - a skill id that is empty or carries a path separator is a `400`
//!   `"Invalid skill id"` (D-20), and a not-found skill answers the fixed
//!   `"Skill not found"` without echoing the id (D-19).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

use super::dto::{CatalogEntryDto, CatalogIndexDto, SkillDeletedDto, SkillDto, SkillsListDto};
use super::envelope;
use super::guard::{INVALID_CONTAINER_TAG, resolve_scoped_request};
use super::ports::{CatalogEntry, NewSkill, PortError, Skill, SkillPatch};
use super::{json_body_required, port_failure, query_first};
use crate::server::AppState;

/// The fixed not-found message (D-19 — the untrusted id is never echoed).
const SKILL_NOT_FOUND: &str = "Skill not found";
/// The fixed invalid-id message (upstream's route-layer literal).
const INVALID_SKILL_ID: &str = "Invalid skill id";
/// The required-field message of `POST /api/skills` (upstream's literal).
const SKILL_FIELDS_REQUIRED: &str = "id, name, and content are required";

/// The `POST /api/skills` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSkillRequest {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    content: Option<String>,
    autoinvoke: Option<bool>,
    enabled: Option<bool>,
    container_tag: Option<String>,
}

/// The `PUT /api/skills/{id}` body (`None` = the field was not sent).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSkillRequest {
    name: Option<String>,
    description: Option<String>,
    content: Option<String>,
    autoinvoke: Option<bool>,
    enabled: Option<bool>,
    container_tag: Option<String>,
}

/// Whether a skill id is storable: non-empty and free of path separators.
///
/// The literal `/` shape never reaches the handler (matchit needs exactly one
/// segment) — the percent-encoded `%2F` does, and is refused here (D-20).
fn is_valid_skill_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.contains('\\')
}

/// Maps one port skill to its wire shape (`skillToApi`).
fn skill_dto(skill: Skill) -> SkillDto {
    SkillDto {
        id: skill.id,
        name: skill.name,
        description: skill.description,
        content: skill.content,
        autoinvoke: skill.autoinvoke,
        enabled: skill.enabled,
        container_tag: skill.container_tag,
        created_at: super::dto::safe_to_iso_string(Some(skill.created_at)),
        updated_at: super::dto::safe_to_iso_string(Some(skill.updated_at)),
    }
}

/// Maps a skills port failure to the endpoint's documented failure status.
///
/// Upstream maps the skills failures explicitly (`web-server.ts:723/729/732/
/// 738/742/748/751`): `POST`/`PUT` answer `400`, `GET`/`DELETE` answer `404`.
/// A store failure stays the fixed `500` (D-9).
fn skill_failure(endpoint: &'static str, status: StatusCode, error: PortError) -> Response {
    match error {
        PortError::Invalid(message) => envelope::json(status, envelope::error(&message)),
        other => port_failure(endpoint, other),
    }
}

/// `GET /api/skills` — upstream `handleListSkills`.
pub async fn list_skills(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    // Upstream `searchParams.get("tag") || undefined`: an empty value means
    // "no filter". A non-empty value must resolve through the guard (D-16).
    let container_tag = query_first(&query, "tag").filter(|tag| !tag.is_empty());
    if let Some(tag) = container_tag
        && resolve_scoped_request(tag).is_err()
    {
        return envelope::bad_request(INVALID_CONTAINER_TAG);
    }
    let enabled_only = query_first(&query, "enabledOnly") == Some("true");

    match state.api.skills.list(container_tag, enabled_only).await {
        Ok(skills) => envelope::ok(&SkillsListDto {
            items: skills.into_iter().map(skill_dto).collect(),
        }),
        Err(error) => skill_failure("GET /api/skills", StatusCode::OK, error),
    }
}

/// `POST /api/skills` — upstream `handleCreateSkill`.
pub async fn create_skill(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: CreateSkillRequest = match json_body_required(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(id) = request.id.filter(|id| !id.is_empty()) else {
        return envelope::json(
            StatusCode::BAD_REQUEST,
            envelope::error(SKILL_FIELDS_REQUIRED),
        );
    };
    let Some(name) = request.name.filter(|name| !name.is_empty()) else {
        return envelope::json(
            StatusCode::BAD_REQUEST,
            envelope::error(SKILL_FIELDS_REQUIRED),
        );
    };
    let Some(content) = request.content.filter(|content| !content.is_empty()) else {
        return envelope::json(
            StatusCode::BAD_REQUEST,
            envelope::error(SKILL_FIELDS_REQUIRED),
        );
    };
    if !is_valid_skill_id(&id) {
        return envelope::bad_request(INVALID_SKILL_ID);
    }
    if let Some(tag) = request.container_tag.as_deref()
        && resolve_scoped_request(tag).is_err()
    {
        return envelope::bad_request(INVALID_CONTAINER_TAG);
    }

    let new_skill = NewSkill {
        id,
        name,
        description: request.description,
        content,
        autoinvoke: request.autoinvoke,
        enabled: request.enabled,
        container_tag: request.container_tag,
    };
    match state.api.skills.create(new_skill).await {
        Ok(skill) => envelope::ok(&skill_dto(skill)),
        Err(error) => skill_failure("POST /api/skills", StatusCode::BAD_REQUEST, error),
    }
}

/// `GET /api/skills/{id}` — upstream `handleGetSkill`.
pub async fn get_skill(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !is_valid_skill_id(&id) {
        return envelope::bad_request(INVALID_SKILL_ID);
    }
    match state.api.skills.get(&id).await {
        Ok(Some(skill)) => envelope::ok(&skill_dto(skill)),
        Ok(None) => envelope::json(StatusCode::NOT_FOUND, envelope::error(SKILL_NOT_FOUND)),
        Err(error) => skill_failure("GET /api/skills/{id}", StatusCode::NOT_FOUND, error),
    }
}

/// `PUT /api/skills/{id}` — upstream `handleUpdateSkill`.
pub async fn update_skill(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    // Upstream's route layer refuses the id before it reads the body.
    if !is_valid_skill_id(&id) {
        return envelope::bad_request(INVALID_SKILL_ID);
    }
    let request: UpdateSkillRequest = match json_body_required(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Some(tag) = request.container_tag.as_deref()
        && resolve_scoped_request(tag).is_err()
    {
        return envelope::bad_request(INVALID_CONTAINER_TAG);
    }

    let patch = SkillPatch {
        name: request.name,
        description: request.description,
        content: request.content,
        autoinvoke: request.autoinvoke,
        enabled: request.enabled,
        container_tag: request.container_tag,
    };
    match state.api.skills.update(&id, patch).await {
        Ok(skill) => envelope::ok(&skill_dto(skill)),
        // Upstream answers `handleUpdateSkill`'s failure at 400; a missing row
        // is the fixed message (the port never carries the id).
        Err(PortError::Invalid(message)) => {
            let message = if message.is_empty() || message.contains(&id) {
                SKILL_NOT_FOUND
            } else {
                message.as_str()
            };
            envelope::json(StatusCode::BAD_REQUEST, envelope::error(message))
        }
        Err(error) => port_failure("PUT /api/skills/{id}", error),
    }
}

/// `DELETE /api/skills/{id}` — upstream `handleDeleteSkill`.
pub async fn delete_skill(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !is_valid_skill_id(&id) {
        return envelope::bad_request(INVALID_SKILL_ID);
    }
    match state.api.skills.delete(&id).await {
        Ok(true) => envelope::ok(&SkillDeletedDto { deleted: true }),
        Ok(false) => envelope::json(StatusCode::NOT_FOUND, envelope::error(SKILL_NOT_FOUND)),
        Err(error) => skill_failure("DELETE /api/skills/{id}", StatusCode::NOT_FOUND, error),
    }
}

/// Maps one port catalog entry to its wire shape (`{name, version, files}`).
fn catalog_entry_dto(entry: CatalogEntry) -> CatalogEntryDto {
    CatalogEntryDto {
        name: entry.name,
        version: entry.version,
        files: entry.files,
    }
}

/// `GET /skills/index.json` — the bare OpenCode catalog object.
pub async fn catalog_index(State(state): State<Arc<AppState>>) -> Response {
    match state.api.skills.catalog_index().await {
        Ok(entries) => {
            let dto = CatalogIndexDto {
                skills: entries.into_iter().map(catalog_entry_dto).collect(),
            };
            match serde_json::to_string(&dto) {
                Ok(body) => envelope::raw_json(StatusCode::OK, body),
                Err(_) => envelope::internal_error(),
            }
        }
        // Upstream answers 500 with the error envelope when the catalog
        // cannot be produced.
        Err(PortError::Unavailable) => envelope::internal_error(),
        Err(error) => super::port_failure("GET /skills/index.json", error),
    }
}

/// `GET /skills/{id}/{file}` — the raw markdown, only when
/// `file == "{id}.md"` (otherwise a plain-text 404, upstream's shape).
///
/// A path separator in the (percent-decoded) id is also a plain-text 404: the
/// catalog layout has exactly two segments, and a stored id can never carry a
/// separator (`POST`/`PUT` refuse one).
pub async fn catalog_file(
    State(state): State<Arc<AppState>>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_skill_id(&id) || file != format!("{id}.md") {
        return envelope::text(StatusCode::NOT_FOUND, "text/plain", "Not Found".to_string());
    }
    match state.api.skills.catalog_markdown(&id).await {
        Ok(Some(markdown)) => {
            envelope::text(StatusCode::OK, "text/markdown; charset=utf-8", markdown)
        }
        Ok(None) => envelope::text(StatusCode::NOT_FOUND, "text/plain", "Not Found".to_string()),
        Err(PortError::Unavailable) => envelope::internal_error(),
        Err(error) => super::port_failure("GET /skills/{id}/{file}", error),
    }
}
