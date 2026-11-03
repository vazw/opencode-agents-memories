//! `GET /api/tags` — upstream `api-handlers.ts` L125–164
//! (`collectDistinctTags` + `handleListTags`).
//!
//! Upstream walks the shards of each scope, projects the distinct container
//! tags, keeps only those carrying the canonical marker (`_project_` /
//! `_user_`) and de-duplicates across shards (first shard wins — the upstream
//! `Map` insertion order).

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Response;

use mem_core::scope::Scope;

use super::dto::{MemoryTagDto, TagInfoDto, TagsDto};
use super::envelope;
use super::port_failure;
use super::ports::{MemoryRow, TagRow};
use super::query_first;
use crate::server::AppState;

/// The canonical marker of the project scope (upstream `"_project_"`).
const PROJECT_MARKER: &str = "_project_";
/// The canonical marker of the user scope (upstream `"_user_"`).
const USER_MARKER: &str = "_user_";

/// The default and the maximum `tagLimit`.
const TAG_LIMIT_DEFAULT: usize = 50;
const TAG_LIMIT_MAX: usize = 200;
/// The longest `tagQuery` (chars) applied; longer ones are truncated.
const TAG_QUERY_MAX_CHARS: usize = 128;

/// Rank the memory tags of `rows` (whole trimmed tags, case-preserving): count
/// descending, then tag ascending. `query` narrows case-insensitively by
/// substring over the tag text; the returned total is the distinct count
/// **after** the query and before the `limit`/`offset` window.
pub fn rank_memory_tags(
    rows: &[MemoryRow],
    query: &str,
    limit: usize,
    offset: usize,
) -> (Vec<MemoryTagDto>, usize) {
    let needle = query.trim().to_ascii_lowercase();
    let mut counts: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for row in rows {
        let Some(raw) = row.tags.as_deref() else {
            continue;
        };
        for tag in raw.split(',').map(str::trim).filter(|tag| !tag.is_empty()) {
            if needle.is_empty() || tag.to_ascii_lowercase().contains(&needle) {
                *counts.entry(tag.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut ranked: Vec<MemoryTagDto> = counts
        .into_iter()
        .map(|(tag, count)| MemoryTagDto { tag, count })
        .collect();
    ranked.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    let total = ranked.len();
    let window = ranked.into_iter().skip(offset).take(limit).collect();
    (window, total)
}

/// Every canonical-scope memory row of every shard (dedupe by id).
async fn all_scope_rows(state: &Arc<AppState>) -> Result<Vec<MemoryRow>, Response> {
    let shards = state
        .api
        .memory
        .shards(None, None)
        .await
        .map_err(|error| port_failure("GET /api/tags", error))?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut rows = Vec::new();
    for shard in shards {
        let batch = state
            .api
            .memory
            .all_memories(&shard)
            .await
            .map_err(|error| port_failure("GET /api/tags", error))?;
        for row in batch {
            if (row.container_tag.contains(PROJECT_MARKER)
                || row.container_tag.contains(USER_MARKER))
                && seen.insert(row.id.clone())
            {
                rows.push(row);
            }
        }
    }
    Ok(rows)
}

/// `GET /api/tags` — the distinct tags of both scopes plus the ranked memory
/// tags (`tagQuery`, `tagLimit`, `tagOffset`).
pub async fn list_tags(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let tag_query: String = query_first(&query, "tagQuery")
        .unwrap_or_default()
        .chars()
        .take(TAG_QUERY_MAX_CHARS)
        .collect();
    let limit = query_first(&query, "tagLimit")
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(TAG_LIMIT_DEFAULT)
        .clamp(1, TAG_LIMIT_MAX);
    let offset = query_first(&query, "tagOffset")
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let project = match collect_distinct_tags(&state, Scope::Project, PROJECT_MARKER).await {
        Ok(tags) => tags,
        Err(response) => return response,
    };
    let user = match collect_distinct_tags(&state, Scope::User, USER_MARKER).await {
        Ok(tags) => tags,
        Err(response) => return response,
    };
    let rows = match all_scope_rows(&state).await {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let (memory_tags, memory_tag_total) = rank_memory_tags(&rows, &tag_query, limit, offset);
    envelope::ok(&TagsDto {
        project,
        user,
        memory_tags,
        memory_tag_total,
    })
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
