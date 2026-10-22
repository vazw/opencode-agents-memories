//! `GET /api/tags` — upstream `api-handlers.ts` L125–164
//! (`collectDistinctTags` + `handleListTags`).
//!
//! Upstream walks the shards of each scope, projects the distinct container
//! tags, keeps only those carrying the canonical marker (`_project_` /
//! `_user_`) and de-duplicates across shards (first shard wins — the upstream
//! `Map` insertion order).

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;

use mem_core::scope::Scope;

use super::dto::{TagInfoDto, TagsDto};
use super::envelope;
use super::port_failure;
use super::ports::TagRow;
use crate::server::AppState;

/// The canonical marker of the project scope (upstream `"_project_"`).
const PROJECT_MARKER: &str = "_project_";
/// The canonical marker of the user scope (upstream `"_user_"`).
const USER_MARKER: &str = "_user_";

/// `GET /api/tags` — the distinct tags of both scopes.
pub async fn list_tags(State(state): State<Arc<AppState>>) -> Response {
    let project = match collect_distinct_tags(&state, Scope::Project, PROJECT_MARKER).await {
        Ok(tags) => tags,
        Err(response) => return response,
    };
    let user = match collect_distinct_tags(&state, Scope::User, USER_MARKER).await {
        Ok(tags) => tags,
        Err(response) => return response,
    };
    envelope::ok(&TagsDto { project, user })
}

/// Collects one scope's distinct tagged rows, marker-filtered and de-duplicated.
async fn collect_distinct_tags(
    state: &Arc<AppState>,
    scope: Scope,
    marker: &str,
) -> Result<Vec<TagInfoDto>, Response> {
    let shards = state
        .api
        .memory
        .shards(Some(scope), None)
        .await
        .map_err(|error| port_failure("GET /api/tags", error))?;

    let mut seen: HashSet<String> = HashSet::new();
    let mut tags = Vec::new();
    for shard in shards {
        let rows = state
            .api
            .memory
            .distinct_tags(&shard)
            .await
            .map_err(|error| port_failure("GET /api/tags", error))?;
        for row in rows {
            if row.container_tag.is_empty()
                || !row.container_tag.contains(marker)
                || seen.contains(&row.container_tag)
            {
                continue;
            }
            seen.insert(row.container_tag.clone());
            tags.push(tag_info_dto(row));
        }
    }
    Ok(tags)
}

/// Upstream `tagInfoFromDistinctRow`.
fn tag_info_dto(row: TagRow) -> TagInfoDto {
    TagInfoDto {
        tag: row.container_tag,
        display_name: row.display_name,
        user_name: row.user_name,
        user_email: row.user_email,
        project_path: row.project_path,
        project_name: row.project_name,
        git_repo_url: row.git_repo_url,
    }
}
