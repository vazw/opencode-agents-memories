//! The memory endpoints (upstream `api-handlers.ts` L166–872).
//!
//! Slices land here in step order: the add path (Step 3), the list / delete /
//! update / pin / unpin paths (Step 6), the search path (Step 7) and the bulk
//! operations (Step 8). Every scoped path resolves its container tag through
//! [`super::guard`] **before** any store or embedding work (D-17, D-10).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use mem_embed::embedder::EmbedTask;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::dto::{
    AddedMemoryDto, DeletedCountDto, DeletedMemoryDto, MemoryItemDto, PaginatedDto, PromptItemDto,
    safe_to_iso_string, split_tags,
};
use super::envelope;
use super::guard::{INVALID_CONTAINER_TAG, resolve_scoped_request};
use super::ports::{MemoryRow, MemoryUpdate, NewMemory, PromptRow, SearchHit, ShardRef};
use super::{
    json_body_or_default, json_body_required, parse_pagination, port_failure, query_first,
    query_values,
};
use crate::server::AppState;

/// Upstream's `listMemories` limit for a tag-filtered walk.
const TAG_FILTER_LIMIT: usize = 10_000;

/// The user-scope marker (upstream's `String.includes("_user_")`).
const USER_MARKER: &str = "_user_";
/// The project-scope marker.
const PROJECT_MARKER: &str = "_project_";

/// A timeline item of `GET /api/memories` — a memory or a prompt. The enum is
/// untagged so each variant serializes as its own struct, keeping the pinned
/// field order. The memory DTO is boxed: it carries the whole row projection
/// and would otherwise dominate every enum value in the page buffer.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TimelineItemDto {
    Memory(Box<MemoryItemDto>),
    Prompt(PromptItemDto),
}

/// Upstream `handleAddMemory`'s request body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMemoryRequest {
    content: Option<String>,
    container_tag: Option<String>,
    #[serde(rename = "type")]
    memory_type: Option<String>,
    tags: Option<Vec<String>>,
    display_name: Option<String>,
    user_name: Option<String>,
    user_email: Option<String>,
    project_path: Option<String>,
    project_name: Option<String>,
    git_repo_url: Option<String>,
}

/// Upstream `handleUpdateMemory`'s request body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMemoryRequest {
    content: Option<String>,
    #[serde(rename = "type")]
    memory_type: Option<String>,
    tags: Option<Vec<String>>,
}

/// Upstream `handleBulkDelete`'s request body (`cascade` defaults to true).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkDeleteMemoriesRequest {
    ids: Option<Vec<String>>,
    cascade: Option<bool>,
}

/// `POST /api/memories` — upstream `handleAddMemory`.
///
/// The container tag is resolved through the guard **first** (D-17): a
/// rejected tag never burns an embedding and never reaches the store.
pub async fn add_memory(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: AddMemoryRequest = match json_body_required(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let Some(content) = request.content.filter(|content| !content.is_empty()) else {
        return envelope::business_error("content and containerTag are required");
    };
    let Some(container_tag) = request
        .container_tag
        .filter(|container_tag| !container_tag.is_empty())
    else {
        return envelope::business_error("content and containerTag are required");
    };

    // D-17: resolve the scope before the embedding, unlike upstream.
    let scoped = match resolve_scoped_request(&container_tag) {
        Ok(scoped) => scoped,
        Err(_) => return envelope::bad_request(INVALID_CONTAINER_TAG),
    };

    // Upstream trims and lowercases the tags, keeping the (possibly empty)
    // entries.
    let tags: Vec<String> = request
        .tags
        .unwrap_or_default()
        .iter()
        .map(|tag| tag.trim().to_lowercase())
        .collect();

    let embedding_input = if tags.is_empty() {
        content.clone()
    } else {
        format!("{content}\nTags: {}", tags.join(", "))
    };
    let vector = match state
        .api
        .embed
        .embed(&embedding_input, EmbedTask::Document)
        .await
    {
        Ok(vector) => vector,
        Err(error) => return port_failure("POST /api/memories", error),
    };
    let tags_vector = if tags.is_empty() {
        None
    } else {
        let input = format!("Topics: {}", tags.join(", "));
        match state.api.embed.embed(&input, EmbedTask::Document).await {
            Ok(vector) => Some(vector),
            Err(error) => return port_failure("POST /api/memories", error),
        }
    };

    let id = new_memory_id();
    let now = chrono::Utc::now().timestamp_millis();
    let record = NewMemory {
        id: id.clone(),
        content,
        vector,
        tags_vector,
        container_tag,
        tags: if tags.is_empty() {
            None
        } else {
            Some(tags.join(","))
        },
        memory_type: request.memory_type,
        created_at: now,
        updated_at: now,
        metadata: Some(r#"{"source":"api"}"#.to_string()),
        session_id: None,
        display_name: request.display_name,
        user_name: request.user_name,
        user_email: request.user_email,
        project_path: request.project_path,
        project_name: request.project_name,
        git_repo_url: request.git_repo_url,
    };

    match state
        .api
        .memory
        .insert_memory(scoped.scope, &scoped.hash, record)
        .await
    {
        Ok(()) => envelope::ok(&AddedMemoryDto { id }),
        Err(error) => port_failure("POST /api/memories", error),
    }
}

/// One timeline entry, carrying the sort keys upstream reads off the raw row.
enum Entry {
    Memory {
        item: Box<MemoryItemDto>,
        created_at: i64,
        is_pinned: bool,
        linked_prompt_id: Option<String>,
    },
    Prompt {
        item: PromptItemDto,
        created_at: i64,
        linked_memory_id: Option<String>,
    },
}

impl Entry {
    fn created_at(&self) -> i64 {
        match self {
            Entry::Memory { created_at, .. } | Entry::Prompt { created_at, .. } => *created_at,
        }
    }

    fn is_pinned(&self) -> bool {
        match self {
            Entry::Memory { is_pinned, .. } => *is_pinned,
            Entry::Prompt { .. } => false,
        }
    }

    fn into_item(self) -> TimelineItemDto {
        match self {
            Entry::Memory { item, .. } => TimelineItemDto::Memory(item),
            Entry::Prompt { item, .. } => TimelineItemDto::Prompt(item),
        }
    }
}

/// The two halves of an upstream `linkedPairs` entry.
#[derive(Default)]
struct LinkedPair {
    memory: Option<usize>,
    prompt: Option<usize>,
}

/// The most `tags=` values a listing applies.
const TAGS_MAX_VALUES: usize = 32;
/// The longest `tags=` value (in chars) a listing applies; longer ones drop.
const TAGS_MAX_CHARS: usize = 128;

/// Trim and ASCII-lowercase, nothing else. The single tag normalizer.
pub fn normalize_tag(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// The `tags=` values of a request: normalized, empties and over-long values
/// dropped, de-duplicated, at most [`TAGS_MAX_VALUES`]. Never rejects.
fn tags_filter(query: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in query_values(query, "tags") {
        let tag = normalize_tag(raw.as_str());
        if tag.is_empty() || tag.chars().count() > TAGS_MAX_CHARS || out.contains(&tag) {
            continue;
        }
        out.push(tag);
        if out.len() == TAGS_MAX_VALUES {
            break;
        }
    }
    out
}

/// Whether a row's comma-separated tag column carries any wanted tag, by
/// whole-tag equality. `wanted` is normalized here (blanks drop); a `wanted`
/// that is empty afterwards matches everything.
pub fn tags_match(row_tags: Option<&str>, wanted: &[String]) -> bool {
    let wanted: Vec<String> = wanted
        .iter()
        .map(|tag| normalize_tag(tag))
        .filter(|tag| !tag.is_empty())
        .collect();
    if wanted.is_empty() {
        return true;
    }
    match row_tags {
        None => false,
        Some(raw) => raw
            .split(',')
            .map(normalize_tag)
            .any(|tag| !tag.is_empty() && wanted.contains(&tag)),
    }
}

/// `GET /api/memories` — upstream `handleListMemories`.
pub async fn list_memories(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let (page, page_size) = match parse_pagination(&query) {
        Ok(pagination) => pagination,
        Err(response) => return response,
    };

    let filter_tags: Vec<String> = query_values(&query, "tag")
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    let tags = tags_filter(&query);
    let include_prompts = query_first(&query, "includePrompts") != Some("false") && tags.is_empty();

    let memories = match collect_memories(&state, &filter_tags, &tags).await {
        Ok(memories) => memories,
        Err(response) => return response,
    };

    let mut entries: Vec<Entry> = memories.into_iter().map(memory_entry).collect();

    if include_prompts {
        let prompts = match collect_prompts(&state, &filter_tags).await {
            Ok(prompts) => prompts,
            Err(response) => return response,
        };
        entries.extend(prompts.into_iter().map(prompt_entry));
    }

    // Upstream's linked-pair grouping: a memory carrying `linkedPromptId` and
    // a prompt carrying `linkedMemoryId` join into one pair; anything else is
    // standalone. An incomplete pair's surviving half is standalone too.
    let mut pairs: Vec<LinkedPair> = Vec::new();
    let mut pair_index: HashMap<String, usize> = HashMap::new();
    let mut standalone: Vec<usize> = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        match entry {
            Entry::Memory {
                linked_prompt_id: Some(prompt_id),
                ..
            } => match pair_index.get(prompt_id) {
                Some(pair) => pairs[*pair].memory = Some(index),
                None => {
                    pair_index.insert(prompt_id.clone(), pairs.len());
                    pairs.push(LinkedPair {
                        memory: Some(index),
                        prompt: None,
                    });
                }
            },
            Entry::Prompt {
                item,
                linked_memory_id: Some(_),
                ..
            } => match pair_index.get(&item.id) {
                Some(pair) => pairs[*pair].prompt = Some(index),
                None => {
                    pair_index.insert(item.id.clone(), pairs.len());
                    pairs.push(LinkedPair {
                        memory: None,
                        prompt: Some(index),
                    });
                }
            },
            _ => standalone.push(index),
        }
    }

    // a complete pair is a typed (memory, prompt) index pair: both Options are
    // bound once here, so no later read can find them absent (ADR-0013 D-3)
    let mut complete: Vec<(usize, usize)> = Vec::new();
    for pair in pairs {
        match (pair.memory, pair.prompt) {
            (Some(memory), Some(prompt)) => complete.push((memory, prompt)),
            (Some(memory), None) => standalone.push(memory),
            (None, Some(prompt)) => standalone.push(prompt),
            (None, None) => {}
        }
    }

    // The sort key is the group's memory for a pair (upstream `a.memory.isPinned`
    // / `a.memory.createdAt`), the item itself otherwise. Stable sort: pinned
    // first, then `createdAt` descending.
    let mut order: Vec<(bool, i64, Vec<usize>)> = Vec::new();
    for (memory, prompt) in &complete {
        order.push((
            entries[*memory].is_pinned(),
            entries[*memory].created_at(),
            vec![*memory, *prompt],
        ));
    }
    for index in standalone {
        order.push((
            entries[index].is_pinned(),
            entries[index].created_at(),
            vec![index],
        ));
    }
    order.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));

    let timeline: Vec<usize> = order
        .into_iter()
        .flat_map(|(_, _, indices)| indices)
        .collect();

    let total = timeline.len();
    let total_pages = total.div_ceil(page_size);
    let offset = (page - 1).saturating_mul(page_size);
    let mut slots: Vec<Option<Entry>> = entries.into_iter().map(Some).collect();
    let items: Vec<TimelineItemDto> = timeline
        .into_iter()
        .skip(offset)
        .take(page_size)
        .filter_map(|index| slots[index].take())
        .map(Entry::into_item)
        .collect();

    envelope::ok(&PaginatedDto {
        items,
        total,
        page,
        page_size,
        total_pages,
    })
}

/// The scoped memory rows of a listing request (dedupe by id, first wins).
async fn collect_memories(
    state: &Arc<AppState>,
    filter_tags: &[String],
    tags: &[String],
) -> Result<Vec<MemoryRow>, Response> {
    let mut seen = std::collections::HashSet::new();
    let mut memories = Vec::new();

    if filter_tags.is_empty() {
        let shards = state
            .api
            .memory
            .shards(None, None)
            .await
            .map_err(|error| port_failure("GET /api/memories", error))?;
        for shard in shards {
            let rows = state
                .api
                .memory
                .all_memories(&shard)
                .await
                .map_err(|error| port_failure("GET /api/memories", error))?;
            for row in rows {
                // Defense in depth: only canonical scope rows are listed.
                if !(row.container_tag.contains(PROJECT_MARKER)
                    || row.container_tag.contains(USER_MARKER))
                {
                    continue;
                }
                if !tags_match(row.tags.as_deref(), tags) {
                    continue;
                }
                if seen.insert(row.id.clone()) {
                    memories.push(row);
                }
            }
        }
        return Ok(memories);
    }

    for filter_tag in filter_tags {
        // A malformed explicit tag rejects the whole request (never a fallback
        // to the unfiltered listing — D-10).
        let scoped = resolve_scoped_request(filter_tag)
            .map_err(|_| envelope::bad_request(INVALID_CONTAINER_TAG))?;
        let shards = state
            .api
            .memory
            .shards(Some(scoped.scope), Some(&scoped.hash))
            .await
            .map_err(|error| port_failure("GET /api/memories", error))?;
        for shard in shards {
            let rows = state
                .api
                .memory
                .list_memories_by_container_tag(&shard, filter_tag, TAG_FILTER_LIMIT)
                .await
                .map_err(|error| port_failure("GET /api/memories", error))?;
            for row in rows {
                if !tags_match(row.tags.as_deref(), tags) {
                    continue;
                }
                if seen.insert(row.id.clone()) {
                    memories.push(row);
                }
            }
        }
    }
    Ok(memories)
}

/// The prompt rows merged into a listing (`Promise.all`-equivalent order).
async fn collect_prompts(
    state: &Arc<AppState>,
    filter_tags: &[String],
) -> Result<Vec<PromptRow>, Response> {
    let mut by_id: HashMap<String, PromptRow> = HashMap::new();
    if filter_tags.is_empty() {
        let prompts = state
            .api
            .prompts
            .captured_prompts(None)
            .await
            .map_err(|error| port_failure("GET /api/memories", error))?;
        for prompt in prompts {
            by_id.insert(prompt.id.clone(), prompt);
        }
        return Ok(by_id.into_values().collect());
    }

    for filter_tag in filter_tags {
        let project_path = get_project_path_from_tag(state, filter_tag).await?;
        let prompts = state
            .api
            .prompts
            .captured_prompts(project_path.as_deref())
            .await
            .map_err(|error| port_failure("GET /api/memories", error))?;
        for prompt in prompts {
            by_id.insert(prompt.id.clone(), prompt);
        }
    }
    Ok(by_id.into_values().collect())
}

/// Upstream `getProjectPathFromTag`: the first distinct-tag row of every
/// shard whose container tag matches, carrying a project path.
async fn get_project_path_from_tag(
    state: &Arc<AppState>,
    container_tag: &str,
) -> Result<Option<String>, Response> {
    let shards = state
        .api
        .memory
        .shards(None, None)
        .await
        .map_err(|error| port_failure("GET /api/memories", error))?;
    for shard in shards {
        let rows = state
            .api
            .memory
            .distinct_tags(&shard)
            .await
            .map_err(|error| port_failure("GET /api/memories", error))?;
        for row in rows {
            if row.container_tag == container_tag
                && let Some(project_path) = row.project_path.filter(|path| !path.is_empty())
            {
                return Ok(Some(project_path));
            }
        }
    }
    Ok(None)
}

/// One memory row as the timeline entry upstream builds.
fn memory_entry(row: MemoryRow) -> Entry {
    let metadata: Option<Value> = row
        .metadata
        .as_deref()
        .filter(|raw| !raw.is_empty())
        .and_then(|raw| serde_json::from_str(raw).ok());
    let linked_prompt_id = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("promptId"))
        .and_then(|prompt_id| prompt_id.as_str())
        .map(str::to_string);

    Entry::Memory {
        item: Box::new(MemoryItemDto {
            r#type: "memory",
            id: row.id,
            content: row.content,
            memory_type: row.memory_type,
            tags: split_tags(row.tags.as_deref()),
            created_at: safe_to_iso_string(Some(row.created_at)),
            updated_at: row
                .updated_at
                .map(|updated_at| safe_to_iso_string(Some(updated_at))),
            metadata,
            linked_prompt_id: linked_prompt_id.clone(),
            display_name: row.display_name,
            user_name: row.user_name,
            user_email: row.user_email,
            project_path: row.project_path,
            project_name: row.project_name,
            git_repo_url: row.git_repo_url,
            is_pinned: row.is_pinned,
        }),
        created_at: row.created_at,
        is_pinned: row.is_pinned,
        linked_prompt_id,
    }
}

/// One prompt row as the timeline entry upstream builds.
fn prompt_entry(row: PromptRow) -> Entry {
    Entry::Prompt {
        item: PromptItemDto {
            r#type: "prompt",
            id: row.id,
            session_id: row.session_id,
            content: row.content,
            created_at: safe_to_iso_string(Some(row.created_at)),
            project_path: row.project_path,
            linked_memory_id: row.linked_memory_id.clone(),
        },
        created_at: row.created_at,
        linked_memory_id: row.linked_memory_id,
    }
}

/// `DELETE /api/memories/{id}` — upstream `handleDeleteMemory`.
pub async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    if id.is_empty() {
        return envelope::business_error("id is required");
    }
    let cascade = query_first(&query, "cascade") == Some("true");

    let Some((shard, memory)) = (match find_memory(&state, &id, "DELETE /api/memories/{id}").await {
        Ok(found) => found,
        Err(response) => return response,
    }) else {
        return envelope::business_error("Memory not found");
    };

    let linked_prompt_id = metadata_prompt_id(memory.metadata.as_deref());
    let mut deleted_prompt = false;
    if cascade && let Some(prompt_id) = linked_prompt_id {
        if let Err(error) = state.api.prompts.delete_prompt(&prompt_id).await {
            return port_failure("DELETE /api/memories/{id}", error);
        }
        deleted_prompt = true;
    }

    match state.api.memory.delete_memory(&shard, &id).await {
        Ok(()) => envelope::ok(&DeletedMemoryDto {
            deleted_prompt,
            erasure: super::prompts::delete_erasure("memories"),
        }),
        Err(error) => port_failure("DELETE /api/memories/{id}", error),
    }
}

/// `POST /api/memories/bulk-delete` — upstream `handleBulkDelete`.
///
/// `cascade` defaults to **true** here (`body.cascade !== false`), unlike the
/// single-delete endpoint, where `?cascade=true` turns it on. Every id is
/// deleted individually and the rows that existed are counted.
pub async fn bulk_delete_memories(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: BulkDeleteMemoriesRequest = match json_body_required(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(ids) = request.ids.filter(|ids| !ids.is_empty()) else {
        return envelope::business_error("ids array is required");
    };
    let cascade = request.cascade.unwrap_or(true);

    let mut deleted = 0_usize;
    for id in ids {
        match delete_memory_by_id(&state, &id, cascade, "POST /api/memories/bulk-delete").await {
            Ok(true) => deleted += 1,
            // A missing row is upstream's soft failure: skipped, not counted.
            Ok(false) => {}
            Err(response) => return response,
        }
    }

    envelope::ok(&DeletedCountDto { deleted })
}

/// `PUT /api/memories/{id}` — upstream `handleUpdateMemory`.
///
/// The new embedding is computed **before** anything is written (upstream's
/// comment: "safe — no data deleted yet"); the port's `update_memory` owns the
/// delete+insert transaction.
pub async fn update_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    if id.is_empty() {
        return envelope::business_error("id is required");
    }
    let request: UpdateMemoryRequest = json_body_or_default(&body);

    let Some((shard, existing)) = (match find_memory(&state, &id, "PUT /api/memories/{id}").await {
        Ok(found) => found,
        Err(response) => return response,
    }) else {
        return envelope::business_error("Memory not found");
    };

    let new_content = request
        .content
        .filter(|content| !content.is_empty())
        .unwrap_or_else(|| existing.content.clone());
    let tags: Vec<String> = match request.tags {
        Some(tags) => tags,
        None => split_tags(existing.tags.as_deref()),
    };

    let vector = match state
        .api
        .embed
        .embed(&new_content, EmbedTask::Document)
        .await
    {
        Ok(vector) => vector,
        Err(error) => return port_failure("PUT /api/memories/{id}", error),
    };
    let tags_vector = if tags.is_empty() {
        None
    } else {
        let input = format!("Topics: {}", tags.join(", "));
        match state.api.embed.embed(&input, EmbedTask::Document).await {
            Ok(vector) => Some(vector),
            Err(error) => return port_failure("PUT /api/memories/{id}", error),
        }
    };

    let update = MemoryUpdate {
        id: id.clone(),
        content: new_content,
        vector,
        tags_vector,
        container_tag: existing.container_tag,
        tags: if tags.is_empty() {
            None
        } else {
            Some(tags.join(","))
        },
        memory_type: request.memory_type.or(existing.memory_type),
        created_at: existing.created_at,
        updated_at: chrono::Utc::now().timestamp_millis(),
        metadata: existing.metadata.clone(),
        session_id: existing.session_id.clone().or_else(|| {
            mem_store::vector_utils::parse_session_id_from_metadata(existing.metadata.as_deref())
        }),
        display_name: existing.display_name,
        user_name: existing.user_name,
        user_email: existing.user_email,
        project_path: existing.project_path,
        project_name: existing.project_name,
        git_repo_url: existing.git_repo_url,
    };

    match state.api.memory.update_memory(&shard, update).await {
        Ok(()) => envelope::ok_empty(),
        Err(error) => port_failure("PUT /api/memories/{id}", error),
    }
}

/// `POST /api/memories/{id}/pin` — upstream `handlePinMemory`.
pub async fn pin_memory(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    set_pinned(&state, &id, true).await
}

/// `POST /api/memories/{id}/unpin` — upstream `handleUnpinMemory`.
pub async fn unpin_memory(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    set_pinned(&state, &id, false).await
}

/// The shared pin/unpin body.
async fn set_pinned(state: &Arc<AppState>, id: &str, pinned: bool) -> Response {
    if id.is_empty() {
        return envelope::business_error("id is required");
    }
    let Some((shard, _)) = (match find_memory(state, id, "POST /api/memories/{id}/pin").await {
        Ok(found) => found,
        Err(response) => return response,
    }) else {
        return envelope::business_error("Memory not found");
    };

    let result = if pinned {
        state.api.memory.pin_memory(&shard, id).await
    } else {
        state.api.memory.unpin_memory(&shard, id).await
    };
    match result {
        Ok(()) => envelope::ok_empty(),
        Err(error) => port_failure("POST /api/memories/{id}/pin", error),
    }
}

/// Locates a memory across every shard (upstream's `for (const shard of …)`).
pub(super) async fn find_memory(
    state: &Arc<AppState>,
    id: &str,
    endpoint: &'static str,
) -> Result<Option<(ShardRef, MemoryRow)>, Response> {
    let shards = state
        .api
        .memory
        .shards(None, None)
        .await
        .map_err(|error| port_failure(endpoint, error))?;
    for shard in shards {
        match state.api.memory.memory_by_id(&shard, id).await {
            Ok(Some(memory)) => return Ok(Some((shard, memory))),
            Ok(None) => continue,
            Err(error) => return Err(port_failure(endpoint, error)),
        }
    }
    Ok(None)
}

/// Deletes one memory by id (upstream `handleDeleteMemory`'s body, shared by
/// the single delete path, the bulk endpoint and the prompt cascade).
///
/// `Ok(true)` means the row was found and deleted, `Ok(false)` means no such
/// row exists (upstream's `"Memory not found"` business error — a soft
/// failure the bulk endpoint skips), and `Err` carries the already-built
/// response for a port failure (D-9).
pub(super) async fn delete_memory_by_id(
    state: &Arc<AppState>,
    id: &str,
    cascade: bool,
    endpoint: &'static str,
) -> Result<bool, Response> {
    let Some((shard, memory)) = (match find_memory(state, id, endpoint).await {
        Ok(found) => found,
        Err(response) => return Err(response),
    }) else {
        return Ok(false);
    };

    if cascade
        && let Some(prompt_id) = metadata_prompt_id(memory.metadata.as_deref())
        && let Err(error) = state.api.prompts.delete_prompt(&prompt_id).await
    {
        return Err(port_failure(endpoint, error));
    }

    match state.api.memory.delete_memory(&shard, id).await {
        Ok(()) => Ok(true),
        Err(error) => Err(port_failure(endpoint, error)),
    }
}

/// Upstream `safeJSONParse(metadata)?.promptId`.
fn metadata_prompt_id(metadata: Option<&str>) -> Option<String> {
    let metadata = metadata.filter(|raw| !raw.is_empty())?;
    let parsed: Value = serde_json::from_str(metadata).ok()?;
    parsed
        .get("promptId")
        .and_then(|prompt_id| prompt_id.as_str())
        .map(str::to_string)
}

/// `PUT /api/memories/bulk-delete` — the synthetic row 8. Upstream answers it
/// through `handleUpdateMemory("bulk-delete", …)`: no such id exists, so the
/// business error is `"Memory not found"` at HTTP 200 (see B2).
pub async fn update_bulk_delete_path() -> Response {
    envelope::business_error("Memory not found")
}

/// Upstream's `mem_${Date.now()}_${Math.random().toString(36).substring(2, 11)}`
/// id: the millisecond stamp plus 9 random hex characters.
fn new_memory_id() -> String {
    let millis = chrono::Utc::now().timestamp_millis();
    let mut bytes = [0_u8; 8];
    let suffix = match getrandom::fill(&mut bytes) {
        Ok(()) => bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        // The id only has to be unique; a timestamp collision is impossible
        // within one millisecond for the same process, and the process id
        // disambiguates a forked worker.
        Err(_) => format!("{millis:x}{:x}", std::process::id()),
    };
    let suffix = &suffix[..suffix.len().min(9)];
    format!("mem_{millis}_{suffix}")
}

/// The search item union (`SearchResultItem` upstream).
use super::dto::{SearchItemDto, SearchMemoryDto, SearchPromptDto};

/// One ranked search entry: the DTO plus the sort keys upstream re-reads.
struct SearchEntry {
    similarity: f64,
    created_at: String,
    item: SearchItemDto,
}

/// `GET /api/search` — upstream `handleSearch` (the route-level
/// `"query parameter required"` check lives here, keeping upstream's status
/// and message).
pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let Some(query_text) = query_first(&query, "q").filter(|text| !text.is_empty()) else {
        return envelope::business_error("query parameter required");
    };
    let (page, page_size) = match parse_pagination(&query) {
        Ok(pagination) => pagination,
        Err(response) => return response,
    };
    let filter_tags: Vec<String> = query_values(&query, "tag")
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();

    let query_vector = match state.api.embed.embed(query_text, EmbedTask::Query).await {
        Ok(vector) => vector,
        Err(error) => return port_failure("GET /api/search", error),
    };

    let mut entries: Vec<SearchEntry> = Vec::new();
    let prompts: Vec<PromptRow> = if filter_tags.is_empty() {
        // Global search: every distinct container tag, invalid ones skipped
        // (upstream's `tryExtractScopeFromContainerTag` branch — never widened).
        let tags = match distinct_container_tags(&state).await {
            Ok(tags) => tags,
            Err(response) => return response,
        };
        for container_tag in tags {
            let Ok(scope_ref) = mem_core::scope::extract_scope_from_container_tag(&container_tag)
            else {
                tracing::warn!(
                    reason = "invalid_container_tag",
                    tag_length = container_tag.len(),
                    "skipping an unparseable container tag during global search"
                );
                continue;
            };
            let Some(hash) = scope_ref.hash.map(|hash| hash.as_str().to_string()) else {
                continue;
            };
            let shards = match state
                .api
                .memory
                .shards(Some(scope_ref.scope), Some(&hash))
                .await
            {
                Ok(shards) => shards,
                Err(error) => return port_failure("GET /api/search", error),
            };
            // W3: the authority at shard selection, before ANY shard query or
            // prompt read — a mismatched scope answers 409, never a partial.
            if let Err(error) = state.api.memory.verify_shards(&shards).await {
                return port_failure("GET /api/search", error);
            }
            for shard in shards {
                match state
                    .api
                    .memory
                    .search_in_shard(
                        &shard,
                        &query_vector,
                        &container_tag,
                        page_size,
                        Some(query_text),
                    )
                    .await
                {
                    Ok(hits) => entries.extend(hits.into_iter().map(search_memory_entry)),
                    // Upstream logs a per-shard failure and keeps searching.
                    Err(error) => {
                        tracing::warn!(error = %error, "search shard failed");
                    }
                }
            }
        }
        match state
            .api
            .prompts
            .search_prompts(query_text, None, page_size * 2)
            .await
        {
            Ok(prompts) => prompts,
            Err(error) => return port_failure("GET /api/search", error),
        }
    } else {
        let mut seen = std::collections::HashSet::new();
        let mut prompt_by_id: HashMap<String, PromptRow> = HashMap::new();
        for filter_tag in &filter_tags {
            // An explicit malformed tag rejects the request (H2(i)).
            let scoped = match resolve_scoped_request(filter_tag) {
                Ok(scoped) => scoped,
                Err(_) => return envelope::bad_request(INVALID_CONTAINER_TAG),
            };
            let shards = match state
                .api
                .memory
                .shards(Some(scoped.scope), Some(&scoped.hash))
                .await
            {
                Ok(shards) => shards,
                Err(error) => return port_failure("GET /api/search", error),
            };
            // W3: same authority, same place, for the tag-filtered branch.
            if let Err(error) = state.api.memory.verify_shards(&shards).await {
                return port_failure("GET /api/search", error);
            }
            for shard in shards {
                match state
                    .api
                    .memory
                    .search_in_shard(
                        &shard,
                        &query_vector,
                        filter_tag,
                        page_size * 2,
                        Some(query_text),
                    )
                    .await
                {
                    Ok(hits) => {
                        for hit in hits {
                            if seen.insert(hit.id.clone()) {
                                entries.push(search_memory_entry(hit));
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "search shard failed");
                    }
                }
            }
            let project_path = match get_project_path_from_tag(&state, filter_tag).await {
                Ok(project_path) => project_path,
                Err(response) => return response,
            };
            match state
                .api
                .prompts
                .search_prompts(query_text, project_path.as_deref(), page_size * 2)
                .await
            {
                Ok(prompts) => {
                    for prompt in prompts {
                        prompt_by_id.insert(prompt.id.clone(), prompt);
                    }
                }
                Err(error) => return port_failure("GET /api/search", error),
            }
        }
        prompt_by_id.into_values().collect()
    };

    entries.extend(prompts.into_iter().map(search_prompt_entry));

    // Similarity descending, then `createdAt` descending (upstream's
    // `b.createdAt.localeCompare(a.createdAt)` over the ISO strings).
    entries.sort_by(|left, right| {
        right
            .similarity
            .partial_cmp(&left.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.created_at.cmp(&left.created_at))
    });

    let total = entries.len();
    let total_pages = total.div_ceil(page_size);
    let offset = (page - 1).saturating_mul(page_size);
    let mut items: Vec<SearchItemDto> = entries
        .into_iter()
        .skip(offset)
        .take(page_size)
        .map(|entry| entry.item)
        .collect();

    // The linked counterparts missing from the page are appended as context
    // (`similarity: 0`, `isContext: true`).
    let mut missing_prompt_ids: Vec<String> = Vec::new();
    let mut missing_memory_ids: Vec<String> = Vec::new();
    for item in &items {
        match item {
            SearchItemDto::Memory(memory) => {
                if let Some(prompt_id) = memory.linked_prompt_id.as_deref()
                    && !items
                        .iter()
                        .any(|item| matches!(item, SearchItemDto::Prompt(prompt) if prompt.id == prompt_id))
                {
                    missing_prompt_ids.push(prompt_id.to_string());
                }
            }
            SearchItemDto::Prompt(prompt) => {
                if let Some(memory_id) = prompt.linked_memory_id.as_deref()
                    && !items
                        .iter()
                        .any(|item| matches!(item, SearchItemDto::Memory(memory) if memory.id == memory_id))
                {
                    missing_memory_ids.push(memory_id.to_string());
                }
            }
        }
    }

    if !missing_prompt_ids.is_empty() {
        match state.api.prompts.prompts_by_ids(&missing_prompt_ids).await {
            Ok(prompts) => {
                for prompt in prompts {
                    items.push(SearchItemDto::Prompt(SearchPromptDto {
                        r#type: "prompt",
                        id: prompt.id,
                        session_id: prompt.session_id,
                        content: prompt.content,
                        created_at: safe_to_iso_string(Some(prompt.created_at)),
                        project_path: prompt.project_path,
                        linked_memory_id: prompt.linked_memory_id,
                        similarity: 0.0,
                        is_context: Some(true),
                    }));
                }
            }
            Err(error) => return port_failure("GET /api/search", error),
        }
    }

    if !missing_memory_ids.is_empty() {
        let shards = match state.api.memory.shards(None, None).await {
            Ok(shards) => shards,
            Err(error) => return port_failure("GET /api/search", error),
        };
        for shard in shards {
            for memory_id in &missing_memory_ids {
                let Ok(Some(memory)) = state.api.memory.memory_by_id(&shard, memory_id).await
                else {
                    continue;
                };
                if items
                    .iter()
                    .any(|item| matches!(item, SearchItemDto::Memory(existing) if &existing.id == memory_id))
                {
                    continue;
                }
                items.push(SearchItemDto::Memory(context_memory_dto(memory)));
            }
        }
    }

    envelope::ok(&PaginatedDto {
        items,
        total,
        page,
        page_size,
        total_pages,
    })
}

/// Every distinct container tag of every shard (project shards first, upstream
/// `getAllMemoryShards` order).
async fn distinct_container_tags(state: &Arc<AppState>) -> Result<Vec<String>, Response> {
    let shards = state
        .api
        .memory
        .shards(None, None)
        .await
        .map_err(|error| port_failure("GET /api/search", error))?;
    let mut seen = std::collections::HashSet::new();
    let mut tags = Vec::new();
    for shard in shards {
        let rows = state
            .api
            .memory
            .distinct_tags(&shard)
            .await
            .map_err(|error| port_failure("GET /api/search", error))?;
        for row in rows {
            if !row.container_tag.is_empty() && seen.insert(row.container_tag.clone()) {
                tags.push(row.container_tag);
            }
        }
    }
    Ok(tags)
}

/// One memory search hit as upstream's `FormattedMemory`.
fn search_memory_entry(hit: SearchHit) -> SearchEntry {
    let metadata = hit.metadata;
    let memory_type = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("type"))
        .and_then(|memory_type| memory_type.as_str())
        .map(str::to_string);
    let updated_at = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("updatedAt"))
        .and_then(|updated_at| updated_at.as_i64())
        .map(|updated_at| safe_to_iso_string(Some(updated_at)));
    let linked_prompt_id = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("promptId"))
        .and_then(|prompt_id| prompt_id.as_str())
        .map(str::to_string);
    let created_at = safe_to_iso_string(Some(hit.created_at));

    SearchEntry {
        similarity: hit.similarity,
        created_at: created_at.clone(),
        item: SearchItemDto::Memory(SearchMemoryDto {
            r#type: "memory",
            id: hit.id,
            content: hit.content,
            memory_type,
            tags: hit.tags,
            created_at,
            updated_at,
            similarity: hit.similarity,
            metadata,
            display_name: hit.display_name,
            user_name: hit.user_name,
            user_email: hit.user_email,
            project_path: hit.project_path,
            project_name: hit.project_name,
            git_repo_url: hit.git_repo_url,
            is_pinned: hit.is_pinned,
            linked_prompt_id,
            is_context: None,
        }),
    }
}

/// One prompt search hit (`similarity: 1.0` upstream).
fn search_prompt_entry(row: PromptRow) -> SearchEntry {
    let created_at = safe_to_iso_string(Some(row.created_at));
    SearchEntry {
        similarity: 1.0,
        created_at: created_at.clone(),
        item: SearchItemDto::Prompt(SearchPromptDto {
            r#type: "prompt",
            id: row.id,
            session_id: row.session_id,
            content: row.content,
            created_at,
            project_path: row.project_path,
            linked_memory_id: row.linked_memory_id,
            similarity: 1.0,
            is_context: None,
        }),
    }
}

/// The context memory appended for a linked prompt whose memory missed the
/// page (`similarity: 0`, `isContext: true`).
fn context_memory_dto(row: MemoryRow) -> SearchMemoryDto {
    let metadata: Option<Value> = row
        .metadata
        .as_deref()
        .filter(|raw| !raw.is_empty())
        .and_then(|raw| serde_json::from_str(raw).ok());
    let linked_prompt_id = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("promptId"))
        .and_then(|prompt_id| prompt_id.as_str())
        .map(str::to_string);
    let updated_at = row
        .updated_at
        .map(|updated_at| safe_to_iso_string(Some(updated_at)));

    SearchMemoryDto {
        r#type: "memory",
        id: row.id,
        content: row.content,
        memory_type: row.memory_type,
        tags: split_tags(row.tags.as_deref()),
        created_at: safe_to_iso_string(Some(row.created_at)),
        updated_at,
        similarity: 0.0,
        metadata,
        display_name: row.display_name,
        user_name: row.user_name,
        user_email: row.user_email,
        project_path: row.project_path,
        project_name: row.project_name,
        git_repo_url: row.git_repo_url,
        is_pinned: row.is_pinned,
        linked_prompt_id,
        is_context: Some(true),
    }
}
