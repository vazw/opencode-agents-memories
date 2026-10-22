//! The upstream `web/src/lib/*` client-contract ports.
//!
//! Two dependency-free pieces of the explorer's client contract, ported from
//! `web/src/lib/memory-count.ts` and `web/src/lib/bulk-delete.ts`:
//!
//! - [`displayed_memory_count`] — which of the two totals the explorer shows;
//! - [`delete_selected_memories`] — the two-group bulk delete, driven through
//!   the [`BulkDeleteRequest`] seam so the caller owns the transport.
//!
//! The bulk delete's contract is upstream's, exactly: the prompt ids are
//! grouped first (`id.starts_with("prompt_")`) and posted to
//! `/api/prompts/bulk-delete` with `cascade: true`; the cascade-linked memory
//! ids are counted as deleted and removed from the memory group; the memory
//! group follows. The first failed group aborts the run, and the ids the
//! earlier groups deleted are still reported.

use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::{Value, json};

/// The prompt bulk-delete endpoint (upstream's literal).
pub const PROMPT_BULK_DELETE_ENDPOINT: &str = "/api/prompts/bulk-delete";
/// The memory bulk-delete endpoint (upstream's literal).
pub const MEMORY_BULK_DELETE_ENDPOINT: &str = "/api/memories/bulk-delete";

/// Upstream `ApiResult` as the bulk delete reads it: `success` plus an
/// optional error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkDeleteAnswer {
    /// Whether the group's request succeeded.
    pub success: bool,
    /// The server's error message, when it failed.
    pub error: Option<String>,
}

/// The transport seam of [`delete_selected_memories`] (upstream's
/// `BulkDeleteRequest` callback type).
#[async_trait]
pub trait BulkDeleteRequest: Send + Sync {
    /// Posts one bulk-delete group to `endpoint` with `body` and returns the
    /// server's answer.
    async fn request(&self, endpoint: &'static str, body: Value) -> BulkDeleteAnswer;
}

/// Upstream `BulkDeleteLinkHint`: the link fields of one timeline item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BulkDeleteLinkHint {
    /// The item's id.
    pub id: String,
    /// The memory this item is linked to, when it is a prompt.
    pub linked_memory_id: Option<String>,
    /// The prompt this item is linked to, when it is a memory.
    pub linked_prompt_id: Option<String>,
}

/// Upstream `BulkDeleteOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkDeleteOutcome {
    /// Whether every group succeeded.
    pub success: bool,
    /// The ids that are known to be deleted (including the cascaded ones).
    pub deleted_ids: Vec<String>,
    /// The first failed group's error.
    pub error: Option<String>,
}

/// Upstream `getDisplayedMemoryCount`: the search total while a search or a
/// tag filter is active, the store total otherwise.
pub fn displayed_memory_count(
    is_searching: bool,
    search_total: usize,
    store_total: usize,
    tag_filter_active: bool,
) -> usize {
    if is_searching || tag_filter_active {
        search_total
    } else {
        store_total
    }
}

/// Upstream `deleteSelectedMemories`: delete the prompt group first, then the
/// memory group, reporting every id that is known to be deleted.
pub async fn delete_selected_memories(
    ids: &[String],
    request: &dyn BulkDeleteRequest,
    items: &[BulkDeleteLinkHint],
) -> BulkDeleteOutcome {
    let prompt_ids: Vec<String> = ids
        .iter()
        .filter(|id| id.starts_with("prompt_"))
        .cloned()
        .collect();
    let mut memory_ids: Vec<String> = ids
        .iter()
        .filter(|id| !id.starts_with("prompt_"))
        .cloned()
        .collect();
    let mut deleted_ids: Vec<String> = Vec::new();

    if !prompt_ids.is_empty() {
        let answer = request
            .request(
                PROMPT_BULK_DELETE_ENDPOINT,
                json!({"ids": prompt_ids, "cascade": true}),
            )
            .await;
        if !answer.success {
            return BulkDeleteOutcome {
                success: false,
                deleted_ids,
                error: answer.error,
            };
        }

        deleted_ids.extend(prompt_ids.iter().cloned());
        let cascaded = cascaded_memory_ids(&prompt_ids, items);
        let cascaded_set: HashSet<String> = cascaded.iter().cloned().collect();
        deleted_ids.extend(cascaded);
        // A memory the prompt cascade already deleted is not sent again.
        memory_ids.retain(|id| !cascaded_set.contains(id));
    }

    if !memory_ids.is_empty() {
        let answer = request
            .request(
                MEMORY_BULK_DELETE_ENDPOINT,
                json!({"ids": memory_ids, "cascade": true}),
            )
            .await;
        if !answer.success {
            return BulkDeleteOutcome {
                success: false,
                deleted_ids,
                error: answer.error,
            };
        }
        deleted_ids.extend(memory_ids);
    }

    BulkDeleteOutcome {
        success: true,
        deleted_ids,
        error: None,
    }
}

/// Upstream `cascadedMemoryIds`: the memory ids the prompt group's
/// `cascade: true` also deleted — a prompt's `linkedMemoryId`, and a memory's
/// own id when its `linkedPromptId` is in the prompt group.
///
/// Insertion-ordered, de-duplicated (the JS `Set` order the upstream test
/// pins).
fn cascaded_memory_ids(prompt_ids: &[String], items: &[BulkDeleteLinkHint]) -> Vec<String> {
    if prompt_ids.is_empty() || items.is_empty() {
        return Vec::new();
    }

    let prompt_id_set: HashSet<&str> = prompt_ids.iter().map(String::as_str).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut cascaded: Vec<String> = Vec::new();

    for item in items {
        if prompt_id_set.contains(item.id.as_str())
            && let Some(memory_id) = item.linked_memory_id.as_deref()
            && seen.insert(memory_id.to_string())
        {
            cascaded.push(memory_id.to_string());
        }
        if let Some(prompt_id) = item.linked_prompt_id.as_deref()
            && prompt_id_set.contains(prompt_id)
            && seen.insert(item.id.clone())
        {
            cascaded.push(item.id.clone());
        }
    }

    cascaded
}
