//! W6.7 — the four core memory screens: Memories, Tags, Profile and Settings.
//!
//! Each screen follows the W6.5 house pattern:
//!
//! 1. a **pure request spec** (`list_memories_spec`, `list_tags_spec`,
//!    `get_user_profile_spec`, `health_spec`, `stats_spec`, `cleanup_spec`)
//!    built from the pinned [`crate::paths::ENDPOINTS`] templates, so the wasm
//!    tests pin the exact request with no `fetch`;
//! 2. a **pure DTO decode** (`MemoryPage`, `TagsPage`, `ProfileView`,
//!    `HealthStatus`, `StatsView`, `CleanupReport`) matching the `mem-web`
//!    projection;
//! 3. a **pure render shape** (`memories_view`, `tags_view`, `profile_view`,
//!    `settings_view`, `cleanup_outcome_view`, and the `cleanup_next` state
//!    machine) the panel renders from, so the empty / loaded / missing / error
//!    branches are testable without a DOM;
//! 4. the **panel component** itself, which owns the signals and the one
//!    same-origin fetch seam ([`crate::api_request`]). The one destructive call
//!    in the file — `POST /api/cleanup` — is emitted only from an explicit
//!    arm-then-confirm click, never on mount, reload or a timer.
//!
//! Security: every request goes through the existing seam (`prepare` /
//! `request`), so it is same-origin, root-relative and token-bearing
//! (`x-opencode-mem-token`, never `Bearer`). Path parameters are filled through
//! [`crate::paths::encode_component`], and every rendered value is a text node
//! (Leptos escapes it) — there is no unsanitised-HTML path, and no token is ever
//! rendered, logged or persisted.

use std::collections::BTreeMap;

use leptos::callback::Callable;
use leptos::prelude::*;
use serde::Deserialize;
use thaw::{
    Badge, BadgeAppearance, Button, ButtonAppearance, Card, Input, Pagination, Table, TableBody,
    TableCell, TableHeader, TableHeaderCell, TableRow, Tag,
};

use crate::api::auth::{AuthContext, read_marker_token};
use crate::api::envelope::{ApiError, parse_envelope};
use crate::api_request::{self, RequestSpec};
use crate::confirm::{ConfirmEvent, ConfirmGate, ConfirmState, confirm_next, dispatch};
use crate::paths::{EndpointId, endpoint, fill};
use crate::screens::format_created_at;
use crate::state_block::{BlockKind, StateBlock, block_kind};

/// Render a timestamp the endpoint supplied.
///
/// `GET /api/memories` sends an ISO-8601 string (`dto.rs::safe_to_iso_string`),
/// while the session timeline sends unix milliseconds. A millisecond value goes
/// through the house formatter ([`format_created_at`]); an ISO string is already
/// human-readable and is rendered unchanged.
fn format_timestamp(raw: &str) -> String {
    match raw.parse::<i64>() {
        Ok(millis) => format_created_at(millis),
        Err(_) => raw.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Memories (W6.7 Step 4)
// ---------------------------------------------------------------------------

/// One timeline row of `GET /api/memories` (`MemoryItemDto` / `PromptItemDto`,
/// the untagged `TimelineItemDto` union; camelCase, unknown keys ignored).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    /// `"memory"` or `"prompt"` (the endpoint's discriminant).
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub id: String,
    pub content: String,
    /// The memory class (`note`, `decision`, …); prompts have none.
    #[serde(default)]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// The endpoint's ISO-8601 stamp.
    #[serde(default)]
    pub created_at: String,
    /// Prompts carry no pin flag; the default keeps one DTO for both variants.
    #[serde(default)]
    pub is_pinned: bool,
    /// Only prompt rows carry a session id.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl MemoryItem {
    /// Whether this timeline row is a captured prompt rather than a memory.
    pub fn is_prompt(&self) -> bool {
        self.kind.as_deref() == Some("prompt")
    }
}

/// The `PaginatedDto<TimelineItemDto>` payload of `GET /api/memories`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPage {
    #[serde(default)]
    pub items: Vec<MemoryItem>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub page: usize,
    #[serde(default)]
    pub page_size: usize,
    #[serde(default)]
    pub total_pages: usize,
}

/// One rendered memory row (already flattened out of the DTO).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRowView {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub is_pinned: bool,
    pub is_prompt: bool,
}

/// The Memories screen's render shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoriesView {
    Loading,
    Empty {
        total: usize,
    },
    Loaded {
        rows: Vec<MemoryRowView>,
        total: usize,
        page: usize,
        page_size: usize,
        total_pages: usize,
    },
    Error(String),
}

/// Map the listing payload onto the render shape.
pub fn memories_view(page: &MemoryPage) -> MemoriesView {
    if page.items.is_empty() {
        return MemoriesView::Empty { total: page.total };
    }
    MemoriesView::Loaded {
        rows: page
            .items
            .iter()
            .map(|item| MemoryRowView {
                id: item.id.clone(),
                content: item.content.clone(),
                tags: item.tags.clone(),
                created_at: format_timestamp(&item.created_at),
                is_pinned: item.is_pinned,
                is_prompt: item.is_prompt(),
            })
            .collect(),
        total: page.total,
        page: page.page,
        page_size: page.page_size,
        total_pages: page.total_pages,
    }
}

/// `GET /api/memories?page=…&pageSize=…[&tags=…][&includePrompts=false]`.
///
/// One `tags` pair per entry (the memory-tag filter; `tag=` is the server's
/// container-scope parameter and is never sent from here). `includePrompts` is
/// sent only when false (the server's default is true).
pub fn list_memories_spec(
    page: usize,
    page_size: usize,
    tags: &[&str],
    include_prompts: bool,
) -> RequestSpec {
    let mut spec = RequestSpec::get(endpoint(EndpointId::ListMemories).template)
        .with_query("page", page.to_string())
        .with_query("pageSize", page_size.to_string());
    for tag in tags {
        spec = spec.with_query("tags", (*tag).to_string());
    }
    if !include_prompts {
        spec = spec.with_query("includePrompts", "false");
    }
    spec
}

/// `GET /api/search?q=…&page=…&pageSize=…` (the search route takes no tag).
pub fn search_spec(q: &str, page: usize, page_size: usize) -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::Search).template)
        .with_query("q", q)
        .with_query("page", page.to_string())
        .with_query("pageSize", page_size.to_string())
}

/// The one mode rule: tags win; else a non-blank query searches; else list.
pub fn memories_request(
    query: &str,
    tags: &[String],
    page: usize,
    page_size: usize,
) -> RequestSpec {
    if !tags.is_empty() {
        let refs: Vec<&str> = tags.iter().map(String::as_str).collect();
        list_memories_spec(page, page_size, &refs, false)
    } else if !query.trim().is_empty() {
        search_spec(query.trim(), page, page_size)
    } else {
        list_memories_spec(page, page_size, &[], false)
    }
}

/// The `tags=` values of a location search string, percent-decoded, trimmed,
/// empties dropped, de-duplicated (first wins).
pub fn initial_tags(search: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pair in search.trim_start_matches('?').split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key != "tags" {
            continue;
        }
        let tag = crate::paths::decode_component(value).trim().to_string();
        if !tag.is_empty() && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

/// `POST /api/memories/{id}/pin` (the id is one percent-encoded segment).
pub fn pin_memory_spec(id: &str) -> RequestSpec {
    RequestSpec::post(fill(endpoint(EndpointId::PinMemory).template, id))
}

/// `POST /api/memories/{id}/unpin`.
pub fn unpin_memory_spec(id: &str) -> RequestSpec {
    RequestSpec::post(fill(endpoint(EndpointId::UnpinMemory).template, id))
}

/// `DELETE /api/memories/{id}`.
pub fn delete_memory_spec(id: &str) -> RequestSpec {
    RequestSpec::delete(fill(endpoint(EndpointId::DeleteMemory).template, id))
}

/// Run one request through the shared seam and hand back the raw answer.
///
/// The screen-local decoders ([`crate::screens::delete_outcome`],
/// [`health_outcome`] and [`cleanup_outcome`]) receive the raw status because
/// each one's classification depends on it as well as on the bytes:
/// `delete_outcome` and `health_outcome` decode the body first and then require
/// a `2xx` with `success: true`, while `cleanup_outcome` short-circuits `501`
/// before decoding, so a `501` body is never parsed and never reaches a
/// generic error view. The request is still prepared by
/// [`api_request::prepare`]: same-origin, same-origin credentials, token
/// header included (`x-opencode-mem-token`, never `Bearer`).
async fn fetch_status_and_bytes(
    spec: &RequestSpec,
    auth: &AuthContext,
) -> Result<(u16, Vec<u8>), ApiError> {
    let (url, init) = api_request::prepare::<serde_json::Value>(spec, auth)?;
    let window = web_sys::window().ok_or(ApiError::Transport)?;
    let response =
        wasm_bindgen_futures::JsFuture::from(window.fetch_with_str_and_init(&url, &init))
            .await
            .map_err(api_request::map_transport)?;
    let response: web_sys::Response =
        wasm_bindgen::JsCast::dyn_into(response).map_err(api_request::map_transport)?;
    let status = response.status();
    let buffer = wasm_bindgen_futures::JsFuture::from(
        response.array_buffer().map_err(|_| ApiError::Transport)?,
    )
    .await
    .map_err(api_request::map_transport)?;
    Ok((status, js_sys::Uint8Array::new(&buffer).to_vec()))
}

/// Run a data-less mutation through the shared seam.
///
/// `POST …/pin`, `POST …/unpin` and `DELETE /api/memories/{id}` all answer a
/// success envelope without the `data` payload [`api_request::request`]
/// requires, so the body goes through the same bounded
/// [`crate::screens::delete_outcome`] decoder the session delete uses. The
/// request itself is still prepared by [`api_request::prepare`]: same-origin,
/// same-origin credentials, token header included.
async fn perform_action(spec: &RequestSpec, auth: &AuthContext) -> Result<(), ApiError> {
    let (status, bytes) = fetch_status_and_bytes(spec, auth).await?;
    crate::screens::delete_outcome(status, &bytes)
}

/// How many characters of a memory body a card shows before "Show more".
pub const PREVIEW_CHARS: usize = 700;

/// Split a body into a preview of at most [`PREVIEW_CHARS`] characters (cut at a
/// word boundary when one exists) and the rest. Lossless: `preview + rest`
/// is the original, and the cut is always on a character boundary.
pub fn memory_preview(content: &str) -> (String, Option<String>) {
    let Some((limit, _)) = content.char_indices().nth(PREVIEW_CHARS) else {
        return (content.to_string(), None);
    };
    let head = &content[..limit];
    let cut = head
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(at, c)| at + c.len_utf8())
        .filter(|at| *at > 0)
        .unwrap_or(limit);
    (content[..cut].to_string(), Some(content[cut..].to_string()))
}

/// The most tag chips a card renders.
pub const VISIBLE_TAGS: usize = 4;

/// The first [`VISIBLE_TAGS`] tags and how many more there are.
pub fn visible_tags(tags: &[String]) -> (&[String], usize) {
    let shown = tags.len().min(VISIBLE_TAGS);
    (&tags[..shown], tags.len() - shown)
}

/// A card's whole tag set as a JSON array (the `data-tags` attribute). Built by
/// `serde_json`, so a tag containing `"` or `\` cannot break the attribute.
pub fn tags_json(tags: &[String]) -> String {
    serde_json::Value::Array(
        tags.iter()
            .cloned()
            .map(serde_json::Value::String)
            .collect(),
    )
    .to_string()
}

/// "1-20 of 3827" (1-based, inclusive); "0 of 0" when empty.
pub fn range_text(page: usize, page_size: usize, total: usize) -> String {
    if total == 0 || page_size == 0 {
        return format!("0 of {total}");
    }
    let start = page.max(1).saturating_sub(1).saturating_mul(page_size) + 1;
    let end = page.max(1).saturating_mul(page_size).min(total);
    format!("{start}-{end} of {total}")
}

/// One rendered memory row with its pin/unpin/delete controls.
fn memory_row(
    row: MemoryRowView,
    reload: Callback<()>,
    action_error: RwSignal<Option<String>>,
    confirm: RwSignal<ConfirmState>,
    request_delete: Callback<String>,
    on_tag: Callback<String>,
    auth: AuthContext,
) -> impl IntoView {
    let pinned = row.is_pinned;
    let is_prompt = row.is_prompt;

    let pin_id = row.id.clone();
    let pin_auth = auth.clone();
    let pin = move |_| {
        let spec = if pinned {
            unpin_memory_spec(&pin_id)
        } else {
            pin_memory_spec(&pin_id)
        };
        let auth = pin_auth.clone();
        leptos::task::spawn_local(async move {
            match perform_action(&spec, &auth).await {
                Ok(()) => {
                    action_error.set(None);
                    reload.run(());
                }
                Err(failure) => action_error.set(Some(failure.to_string())),
            }
        });
    };

    // The row's Delete button only arms the gate; the gate's Confirm button is
    // the one path to `request_delete`, and the request spec there is built
    // from `step.run` alone (never from render state).
    let delete_area = {
        let id = row.id.clone();
        move || {
            let armed = confirm.get().pending.as_deref() == Some(id.as_str());
            if armed {
                let on_confirm = Callback::new(move |_: ()| {
                    if let Some(run_id) = dispatch(confirm, ConfirmEvent::Confirm) {
                        request_delete.run(run_id);
                    }
                });
                let on_cancel = Callback::new(move |_: ()| {
                    confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Cancel).state);
                });
                view! { <ConfirmGate on_confirm=on_confirm on_cancel=on_cancel/> }.into_any()
            } else {
                let ask_id = id.clone();
                view! {
                    <Button
                        appearance=ButtonAppearance::Subtle
                        attr:data-testid="memory-delete"
                        on_click=move |_| {
                            confirm
                                .set(
                                    confirm_next(
                                        &confirm.get_untracked(),
                                        ConfirmEvent::Ask(ask_id.clone()),
                                    )
                                        .state,
                                );
                        }
                    >
                        "Delete"
                    </Button>
                }
                .into_any()
            }
        }
    };

    let kind = if is_prompt { "prompt" } else { "memory" };
    let data_tags = tags_json(&row.tags);
    let (shown, hidden) = visible_tags(&row.tags);
    let shown: Vec<String> = shown.to_vec();
    let more_label =
        (hidden > 0).then(|| format!("{hidden} more tags: {}", row.tags[shown.len()..].join(", ")));
    let (preview, rest) = memory_preview(&row.content);

    view! {
        <li data-testid="memory-row" data-tags=data_tags>
            <Card class="memory-card">
                <div class="memory-card-head">
                    <Badge>{kind}</Badge>
                    <span data-testid="memory-created">{row.created_at}</span>
                    {pinned
                        .then(|| {
                            view! { <Badge appearance=BadgeAppearance::Outline>"pinned"</Badge> }
                        })}
                </div>
                <div class="memory-content" data-testid="memory-content">
                    {preview}
                </div>
                {rest
                    .map(|rest| {
                        view! {
                            <details>
                                <summary>"Show more"</summary>
                                {rest}
                            </details>
                        }
                    })}
                <div class="memory-card-tags" data-testid="memory-tags">
                    {shown
                        .into_iter()
                        .map(|tag| {
                            let label = format!("Filter by tag: {tag}");
                            let value = tag.clone();
                            view! {
                                <button
                                    type="button"
                                    class="chip"
                                    aria-label=label
                                    on:click=move |_| on_tag.run(value.clone())
                                >
                                    <Tag>{tag}</Tag>
                                </button>
                            }
                        })
                        .collect_view()}
                    {more_label
                        .map(|label| {
                            view! {
                                <span class="memory-tag-more" aria-label=label>
                                    {format!("+{hidden} more")}
                                </span>
                            }
                        })}
                </div>
                {(!is_prompt)
                    .then(move || {
                        view! {
                            <div class="memory-card-foot">
                                <Button
                                    appearance=ButtonAppearance::Subtle
                                    attr:data-testid="memory-pin"
                                    on_click=pin
                                >
                                    {if pinned { "Unpin" } else { "Pin" }}
                                </Button>
                                {delete_area}
                            </div>
                        }
                    })}
            </Card>
        </li>
    }
}

/// The W6.7 Memories screen.
#[component]
pub fn MemoriesPanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let page = RwSignal::new(1_usize);
    let page_size = RwSignal::new(20_usize);
    let query = RwSignal::new(String::new());
    let tags = RwSignal::new(
        web_sys::window()
            .and_then(|window| window.location().search().ok())
            .map(|search| initial_tags(&search))
            .unwrap_or_default(),
    );
    let state = RwSignal::new(MemoriesView::Loading);
    let action_error = RwSignal::new(None::<String>);
    let confirm = RwSignal::new(ConfirmState::default());

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            // A list refresh invalidates any arm: a stale pending id must never
            // survive to delete a row the user can no longer see.
            confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Cancel).state);
            let spec = memories_request(
                &query.get_untracked(),
                &tags.get_untracked(),
                page.get_untracked(),
                page_size.get_untracked(),
            );
            state.set(MemoriesView::Loading);
            leptos::task::spawn_local(async move {
                match api_request::request::<MemoryPage>(&spec, &auth).await {
                    Ok(found) => state.set(memories_view(&found)),
                    Err(failure) => state.set(MemoriesView::Error(failure.to_string())),
                }
            });
        }
    });

    // Every fetch driven by a filter change goes back to page 1: through the
    // page effect when the page moves, directly when it was already 1 (never both).
    let refresh = Callback::new(move |_: ()| {
        if page.get_untracked() == 1 {
            reload.run(());
        } else {
            page.set(1);
        }
    });

    // The initial load and every page change. `reload` reads everything else
    // untracked, so this effect depends on `page` alone.
    Effect::new(move |_| {
        let _ = page.get();
        reload.run(());
    });

    // The only path from a confirmed gate to the network.
    let request_delete = Callback::new({
        let auth = auth.clone();
        move |id: String| {
            let spec = delete_memory_spec(&id);
            let auth = auth.clone();
            leptos::task::spawn_local(async move {
                let result = perform_action(&spec, &auth).await;
                // `Finished` on EVERY exit path, before the reload: a transient
                // failure must not leave `running` stuck, or the row's Delete
                // button never works again for the rest of the session.
                confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Finished).state);
                match result {
                    Ok(()) => {
                        action_error.set(None);
                        reload.run(());
                    }
                    Err(failure) => action_error.set(Some(failure.to_string())),
                }
            });
        }
    });

    let add_tag = Callback::new(move |tag: String| {
        let already = tags.with_untracked(|current| current.contains(&tag));
        if !already {
            tags.update(|current| current.push(tag));
            refresh.run(());
        }
    });
    let remove_tag = move |tag: String| {
        tags.update(|current| current.retain(|existing| existing != &tag));
        refresh.run(());
    };
    let clear_tags = move |_| {
        tags.set(Vec::new());
        refresh.run(());
    };

    let page_count = Signal::derive(move || match state.get() {
        MemoriesView::Loaded { total_pages, .. } => total_pages.max(1),
        _ => 1,
    });
    let range = move || match state.get() {
        MemoriesView::Loaded {
            total,
            page: current,
            page_size: size,
            ..
        } => range_text(current, size, total),
        MemoriesView::Empty { total } => format!("0 of {total}"),
        _ => String::new(),
    };

    view! {
        <section class="memories-panel" data-testid="memories-panel">
            <h2>"Memories"</h2>
            <div class="memories-filters" data-testid="memories-filters">
                <Input
                    id="memories-search"
                    name="q"
                    class="memories-search"
                    attr:data-testid="memories-search"
                    placeholder="Search memories"
                    value=query
                    on:keydown=move |event: leptos::ev::KeyboardEvent| {
                        if event.key() == "Enter" {
                            refresh.run(());
                        }
                    }
                />
                {move || {
                    tags.get()
                        .into_iter()
                        .map(|tag| {
                            let label = format!("Remove tag: {tag}");
                            let value = tag.clone();
                            view! {
                                <button
                                    type="button"
                                    class="chip"
                                    aria-label=label
                                    on:click=move |_| remove_tag(value.clone())
                                >
                                    <Tag>{tag}</Tag>
                                </button>
                            }
                        })
                        .collect_view()
                }}
                {move || {
                    (!tags.get().is_empty())
                        .then(|| {
                            view! {
                                <Button
                                    appearance=ButtonAppearance::Subtle
                                    attr:data-testid="memories-clear"
                                    on_click=clear_tags
                                >
                                    "Clear"
                                </Button>
                            }
                        })
                }}
                <Button
                    class="memories-reload"
                    attr:data-testid="memories-reload"
                    on_click=move |_| refresh.run(())
                >
                    "Reload"
                </Button>
                <span class="memories-range" data-testid="memories-range">{range}</span>
            </div>
            {move || {
                let has_text = !query.get().trim().is_empty();
                let has_tags = !tags.get().is_empty();
                (has_text && has_tags)
                    .then(|| {
                        view! {
                            <p class="memories-tag-note" data-testid="memories-tag-note">
                                "Tag filter active: the search text is ignored."
                            </p>
                        }
                    })
            }}
            {move || {
                action_error
                    .get()
                    .map(|message| {
                        view! {
                            <p class="memories-action-error" data-testid="memories-action-error">
                                {message}
                            </p>
                        }
                    })
            }}
            {move || {
                let current = state.get();
                match (block_kind(&current), current) {
                    (BlockKind::Loading, _) => view! {
                        <StateBlock
                            kind=BlockKind::Loading
                            message="Loading memories…"
                            class="state-loading"
                            testid="memories-loading"
                        />
                    }
                    .into_any(),
                    (BlockKind::Empty, MemoriesView::Empty { total }) => view! {
                        <StateBlock
                            kind=BlockKind::Empty
                            message=format!("No memories match (total {total}).")
                            class="state-empty"
                            testid="memories-empty"
                        />
                    }
                    .into_any(),
                    (BlockKind::Error, MemoriesView::Error(message)) => view! {
                        <StateBlock
                            kind=BlockKind::Error
                            message=message
                            class="state-error"
                            testid="memories-error"
                        />
                    }
                    .into_any(),
                    (_, MemoriesView::Loaded { rows, .. }) => {
                        view! {
                            <ul class="memory-list" data-testid="memories-list">
                                {rows
                                    .into_iter()
                                    .map(|row| {
                                        memory_row(
                                            row,
                                            reload,
                                            action_error,
                                            confirm,
                                            request_delete,
                                            add_tag,
                                            auth.clone(),
                                        )
                                    })
                                    .collect_view()}
                            </ul>
                        }
                            .into_any()
                    }
                    _ => ().into_any(),
                }
            }}
            <div class="memories-pager" data-testid="memories-pager">
                <Pagination page=page page_count=page_count/>
            </div>
        </section>
    }
}

// ---------------------------------------------------------------------------
// Tags (W6.7 Step 5)
// ---------------------------------------------------------------------------

/// One `TagInfoDto` of `GET /api/tags`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub tag: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub project_path: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub git_repo_url: Option<String>,
}

/// The `{project, user}` payload of `GET /api/tags`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagsPage {
    #[serde(default)]
    pub project: Vec<TagInfo>,
    #[serde(default)]
    pub user: Vec<TagInfo>,
    /// Memory tags ranked by use (older servers omit the key).
    #[serde(default)]
    pub memory_tags: Vec<MemoryTagInfo>,
    /// Distinct memory tags matching the query (before the limit window).
    #[serde(default)]
    pub memory_tag_total: usize,
}

/// One ranked memory tag of `GET /api/tags`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MemoryTagInfo {
    pub tag: String,
    #[serde(default)]
    pub count: u64,
}

/// One rendered tag row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRowView {
    pub tag: String,
    pub display_name: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
}

impl TagRowView {
    /// The best human label: the display name, else the project/user name, else
    /// the raw container tag (never empty).
    pub fn label(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.project_name.clone())
            .or_else(|| self.user_name.clone())
            .unwrap_or_else(|| self.tag.clone())
    }
}

/// The Tags screen's render shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagsView {
    Loading,
    Empty,
    Loaded {
        project: Vec<TagRowView>,
        user: Vec<TagRowView>,
        memory_tags: Vec<MemoryTagInfo>,
        memory_tag_total: usize,
    },
    Error(String),
}

/// Map the tags payload onto the render shape.
pub fn tags_view(page: &TagsPage) -> TagsView {
    if page.project.is_empty() && page.user.is_empty() && page.memory_tags.is_empty() {
        return TagsView::Empty;
    }
    let map = |info: &TagInfo| TagRowView {
        tag: info.tag.clone(),
        display_name: info.display_name.clone(),
        project_path: info.project_path.clone(),
        project_name: info.project_name.clone(),
        user_name: info.user_name.clone(),
        user_email: info.user_email.clone(),
    };
    TagsView::Loaded {
        project: page.project.iter().map(map).collect(),
        user: page.user.iter().map(map).collect(),
        memory_tags: page.memory_tags.clone(),
        memory_tag_total: page.memory_tag_total,
    }
}

/// `GET /api/tags`.
pub fn list_tags_spec() -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::ListTags).template)
}

/// One tag group (project or user) with its count.
fn tag_group(title: &'static str, test_id: &'static str, tags: Vec<TagRowView>) -> impl IntoView {
    let count = tags.len();
    view! {
        <section class="tag-group" id=test_id data-testid=test_id>
            <h3>{format!("{title} ({count})")}</h3>
            <ul class="tag-list">
                {tags
                    .into_iter()
                    .map(|tag| {
                        let label = tag.label();
                        let path = tag.project_path.clone().unwrap_or_default();
                        view! {
                            <li class="tag-row" data-testid="tag-row">
                                <span class="tag-label" data-testid="tag-label">{label}</span>
                                <code class="tag-code" data-testid="tag-code">{tag.tag}</code>
                                {(!path.is_empty())
                                    .then(|| {
                                        view! {
                                            <span class="tag-path" data-testid="tag-path">{path}</span>
                                        }
                                    })}
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

/// The W6.7 Tags screen.
#[component]
pub fn TagsPanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let state = RwSignal::new(TagsView::Loading);

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            state.set(TagsView::Loading);
            leptos::task::spawn_local(async move {
                match api_request::request::<TagsPage>(&list_tags_spec(), &auth).await {
                    Ok(found) => state.set(tags_view(&found)),
                    Err(failure) => state.set(TagsView::Error(failure.to_string())),
                }
            });
        }
    });
    Effect::new(move |_| reload.run(()));

    view! {
        <section class="tags-panel" data-testid="tags-panel">
            <h2>"Tags"</h2>
            <Button
                class="tags-reload"
                attr:data-testid="tags-reload"
                on_click=move |_| reload.run(())
            >
                "Reload"
            </Button>
            {move || {
                match state.get() {
                    TagsView::Loading => view! {
                        <StateBlock
                            kind=BlockKind::Loading
                            message="Loading tags…"
                            class="state-loading"
                            testid="tags-loading"
                        />
                    }
                    .into_any(),
                    TagsView::Empty => view! {
                        <StateBlock
                            kind=BlockKind::Empty
                            message="No tags yet. Add a memory to a project or your profile."
                            class="state-empty"
                            testid="tags-empty"
                        />
                    }
                    .into_any(),
                    TagsView::Loaded { project, user, memory_tags, memory_tag_total } => {
                        let shown = memory_tags.len();
                        view! {
                            {tag_group("Project", "tag-group-project", project)}
                            <section class="tag-group" data-testid="tag-group-memory">
                                <h3>
                                    {format!("Memory tags ({shown} of {memory_tag_total})")}
                                </h3>
                                <Card class="tags-card">
                                    <div class="tags-scroll" id="memory-tags-table" data-testid="memory-tags-table">
                                        <Table>
                                            <TableHeader>
                                                <TableRow>
                                                    <TableHeaderCell>"Tag"</TableHeaderCell>
                                                    <TableHeaderCell>"Memories"</TableHeaderCell>
                                                </TableRow>
                                            </TableHeader>
                                            <TableBody>
                                                {memory_tags
                                                    .into_iter()
                                                    .map(|row| {
                                                        let href = format!(
                                                            "/project-memories?tags={}",
                                                            crate::paths::encode_component(&row.tag),
                                                        );
                                                        view! {
                                                            <TableRow>
                                                                <TableCell>
                                                                    <a class="tags-tag" href=href>{row.tag}</a>
                                                                </TableCell>
                                                                <TableCell>
                                                                    <Badge appearance=BadgeAppearance::Tint>
                                                                        {row.count.to_string()}
                                                                    </Badge>
                                                                </TableCell>
                                                            </TableRow>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </TableBody>
                                        </Table>
                                    </div>
                                </Card>
                            </section>
                            {(!user.is_empty())
                                .then(|| tag_group("User", "tag-group-user", user))}
                        }
                            .into_any()
                    }
                    TagsView::Error(message) => view! {
                        <StateBlock
                            kind=BlockKind::Error
                            message=message
                            class="state-error"
                            testid="tags-error"
                        />
                    }
                    .into_any(),
                }
            }}
        </section>
    }
}

// ---------------------------------------------------------------------------
// Profile (W6.7 Step 6)
// ---------------------------------------------------------------------------

/// One profile item of `profileData` (upstream `ProfileItem`, camelCase).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileItem {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub frequency: Option<i64>,
    #[serde(default)]
    pub steps: Option<Vec<String>>,
}

/// `profileData` — the three learned sections.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileData {
    #[serde(default)]
    pub preferences: Vec<ProfileItem>,
    #[serde(default)]
    pub patterns: Vec<ProfileItem>,
    #[serde(default)]
    pub workflows: Vec<ProfileItem>,
}

/// `GET /api/user-profile` — ONE struct with optional fields so BOTH the
/// populated `ProfileDto` and the `ProfileMissingDto` answer decode.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    #[serde(default)]
    pub exists: bool,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_analyzed_at: Option<String>,
    #[serde(default)]
    pub total_prompts_analyzed: Option<i64>,
    #[serde(default)]
    pub profile_data: Option<ProfileData>,
    /// Only the missing answer carries this.
    #[serde(default)]
    pub message: Option<String>,
}

/// The three rendered profile sections.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileSections {
    pub preferences: Vec<ProfileItemView>,
    pub patterns: Vec<ProfileItemView>,
    pub workflows: Vec<ProfileItemView>,
}

/// One rendered profile item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileItemView {
    pub description: String,
    pub category: Option<String>,
    pub confidence: Option<f64>,
    pub frequency: Option<i64>,
    pub steps: Vec<String>,
}

/// The Profile screen's render shape.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileScreen {
    Loading,
    Missing {
        user_id: String,
        message: String,
    },
    Loaded {
        display_name: String,
        version: i64,
        last_analyzed_at: String,
        sections: ProfileSections,
    },
    Error(String),
}

/// Map the profile payload onto the render shape.
pub fn profile_view(profile: &ProfileView) -> ProfileScreen {
    if !profile.exists {
        return ProfileScreen::Missing {
            user_id: profile.user_id.clone().unwrap_or_default(),
            message: profile
                .message
                .clone()
                .unwrap_or_else(|| "No profile found yet.".to_string()),
        };
    }
    let data = profile.profile_data.clone().unwrap_or_default();
    let map = |item: &ProfileItem| ProfileItemView {
        description: item.description.clone(),
        category: item.category.clone(),
        confidence: item.confidence,
        frequency: item.frequency,
        steps: item.steps.clone().unwrap_or_default(),
    };
    ProfileScreen::Loaded {
        display_name: profile.display_name.clone().unwrap_or_default(),
        version: profile.version.unwrap_or_default(),
        last_analyzed_at: profile.last_analyzed_at.clone().unwrap_or_default(),
        sections: ProfileSections {
            preferences: data.preferences.iter().map(map).collect(),
            patterns: data.patterns.iter().map(map).collect(),
            workflows: data.workflows.iter().map(map).collect(),
        },
    }
}

/// `GET /api/user-profile` for the composed identity.
pub fn get_user_profile_spec() -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::UserProfile).template)
}

/// `GET /api/user-profile?userId=…` for one chosen identity.
///
/// The identity travels as a QUERY value, never as a path segment: it is a
/// selector bound by the handler, and `build_url` percent-encodes it (an email
/// with `@` becomes `%40`).
pub fn get_user_profile_spec_for(user_id: &str) -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::UserProfile).template).with_query("userId", user_id)
}

/// `GET /api/user-profiles` — the identity roster (issue 115).
pub fn list_user_profiles_spec() -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::ListUserProfiles).template)
}

/// One row of the identity roster, as `GET /api/user-profiles` sends it.
///
/// Identity-level fields only: the route never carries `profileData`, so a
/// roster read cannot become a second profile-payload path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRosterItem {
    pub user_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub version: i64,
    #[serde(default)]
    pub total_prompts_analyzed: i64,
    #[serde(default)]
    pub last_analyzed_at: String,
    /// Whether this row is the identity the server composed.
    #[serde(default)]
    pub is_default: bool,
}

/// Which identity control the profile page renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityControl {
    /// One stored identity, and it is the composed one: nothing to choose.
    Hidden,
    /// Several identities, or none of them the composed one: offer the picker.
    Picker {
        options: Vec<ProfileRosterItem>,
        // The composed identity when the roster carries it; `None` renders the
        // "— choose an identity —" placeholder. Never defaulted to the first
        // row (D-3: no silent substitution).
        selected: Option<String>,
    },
}

/// The identity control for a roster and the composed identity.
///
/// `Hidden` iff the roster is non-empty, holds exactly one row, and that row is
/// the composed identity. Every other shape is a `Picker`, whose `selected` is
/// the composed identity only when the roster actually carries it.
pub fn profile_identity_control(
    roster: &[ProfileRosterItem],
    default_user_id: &str,
) -> IdentityControl {
    let selected = roster
        .iter()
        .find(|row| row.user_id == default_user_id)
        .map(|row| row.user_id.clone());
    if roster.len() == 1 && selected.is_some() {
        return IdentityControl::Hidden;
    }
    IdentityControl::Picker {
        options: roster.to_vec(),
        selected,
    }
}

/// One profile section list.
fn profile_section(
    title: &'static str,
    test_id: &'static str,
    items: Vec<ProfileItemView>,
) -> impl IntoView {
    let count = items.len();
    view! {
        <section class="profile-section" data-testid=test_id>
            <h3>{format!("{title} ({count})")}</h3>
            <ul class="profile-items">
                {items
                    .into_iter()
                    .map(|item| {
                        let meta = item
                            .category
                            .clone()
                            .unwrap_or_else(|| "uncategorised".to_string());
                        let steps = item.steps.join(" → ");
                        view! {
                            <li class="profile-item" data-testid="profile-item">
                                <span class="profile-description" data-testid="profile-description">
                                    {item.description}
                                </span>
                                <span class="profile-category" data-testid="profile-category">
                                    {meta}
                                </span>
                                {(!steps.is_empty())
                                    .then(|| {
                                        view! {
                                            <span class="profile-steps" data-testid="profile-steps">
                                                {steps}
                                            </span>
                                        }
                                    })}
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

/// The W6.7 Profile screen.
///
/// Issue 115: the panel offers every stored identity instead of assuming one.
/// A single-identity install renders exactly what it rendered before plus one
/// muted identity line; a root that holds several identities (or none of them
/// the composed one) renders a picker and never substitutes a row for the
/// composed identity.
#[component]
pub fn ProfilePanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let state = RwSignal::new(ProfileScreen::Loading);
    // The identity the rendered answer belongs to (the server's `userId`).
    let identity = RwSignal::new(String::new());
    let control = RwSignal::new(IdentityControl::Hidden);
    let chosen = RwSignal::new(None::<String>);

    // `None` asks for the composed identity (no `userId` query); `Some(id)` asks
    // for that identity explicitly.
    let load_profile = Callback::new({
        let auth = auth.clone();
        move |user_id: Option<String>| {
            let auth = auth.clone();
            state.set(ProfileScreen::Loading);
            leptos::task::spawn_local(async move {
                let spec = match user_id.as_deref() {
                    Some(id) => get_user_profile_spec_for(id),
                    None => get_user_profile_spec(),
                };
                match api_request::request::<ProfileView>(&spec, &auth).await {
                    Ok(found) => {
                        identity.set(found.user_id.clone().unwrap_or_default());
                        state.set(profile_view(&found));
                    }
                    Err(failure) => state.set(ProfileScreen::Error(failure.to_string())),
                }
            });
        }
    });

    // Reload keeps the current selection: a refresh of "this identity" must not
    // silently walk back to the composed one.
    let reload = Callback::new(move |_: ()| load_profile.run(chosen.get_untracked()));

    // One roster read on mount decides the control, then one profile read.
    Effect::new(move |_| {
        let auth = auth.clone();
        leptos::task::spawn_local(async move {
            let roster =
                api_request::request::<Vec<ProfileRosterItem>>(&list_user_profiles_spec(), &auth)
                    .await;
            // A roster read failure is NOT a profile failure: the page still
            // answers the composed identity, it just cannot offer a picker.
            let Ok(roster) = roster else {
                load_profile.run(None);
                return;
            };
            let default_user_id = roster
                .iter()
                .find(|row| row.is_default)
                .map(|row| row.user_id.clone())
                .unwrap_or_default();
            let resolved = profile_identity_control(&roster, &default_user_id);
            let initial = match &resolved {
                IdentityControl::Hidden => None,
                IdentityControl::Picker { selected, .. } => selected.clone(),
            };
            chosen.set(initial.clone());
            control.set(resolved);
            load_profile.run(initial);
        });
    });

    let on_select = move |event: web_sys::Event| {
        let value = event_target_value(&event);
        if value.is_empty() {
            return;
        }
        chosen.set(Some(value.clone()));
        load_profile.run(Some(value));
    };

    view! {
        <section class="profile-panel" data-testid="profile-panel">
            <h2>"User profile"</h2>
            <p class="profile-identity" data-testid="profile-identity">
                "identity: " <code>{move || identity.get()}</code>
            </p>
            {move || match control.get() {
                IdentityControl::Hidden => ().into_any(),
                IdentityControl::Picker { options, selected } => {
                    view! {
                        <select
                            id="profile-identity-select"
                            class="profile-identity-select"
                            data-testid="profile-identity-select"
                            on:change=on_select
                        >
                            <option value="" prop:selected=selected.is_none() disabled=true>
                                "— choose an identity —"
                            </option>
                            {options
                                .into_iter()
                                .map(|row| {
                                    let is_selected = selected.as_deref() == Some(row.user_id.as_str());
                                    let label = if row.display_name.trim().is_empty() {
                                        row.user_id.clone()
                                    } else {
                                        format!("{} — {} prompts", row.display_name, row.total_prompts_analyzed)
                                    };
                                    view! {
                                        <option value=row.user_id.clone() prop:selected=is_selected>
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    }
                        .into_any()
                }
            }}
            <Button
                class="profile-reload"
                attr:data-testid="profile-reload"
                on_click=move |_| reload.run(())
            >
                "Reload"
            </Button>
            {move || {
                match state.get() {
                    ProfileScreen::Loading => view! {
                        <StateBlock
                            kind=BlockKind::Loading
                            message="Loading profile…"
                            class="state-loading"
                            testid="profile-loading"
                        />
                    }
                    .into_any(),
                    ProfileScreen::Missing { user_id, message } => {
                        view! {
                            <div class="profile-missing" data-testid="profile-missing">
                                <p>{message}</p>
                                <p class="profile-user">
                                    "User: " <code data-testid="profile-user-id">{user_id}</code>
                                </p>
                            </div>
                        }
                            .into_any()
                    }
                    ProfileScreen::Loaded {
                        display_name,
                        version,
                        last_analyzed_at,
                        sections,
                    } => {
                        view! {
                            <div class="profile-meta" data-testid="profile-meta">
                                <span class="profile-name" data-testid="profile-name">
                                    {display_name}
                                </span>
                                <span class="profile-version" data-testid="profile-version">
                                    {format!("version {version}")}
                                </span>
                                <span class="profile-analyzed" data-testid="profile-analyzed">
                                    {format!("last analyzed {last_analyzed_at}")}
                                </span>
                            </div>
                            {profile_section(
                                "Preferences",
                                "profile-preferences",
                                sections.preferences,
                            )}
                            {profile_section("Patterns", "profile-patterns", sections.patterns)}
                            {profile_section("Workflows", "profile-workflows", sections.workflows)}
                        }
                            .into_any()
                    }
                    ProfileScreen::Error(message) => view! {
                        <StateBlock
                            kind=BlockKind::Error
                            message=message
                            class="state-error"
                            testid="profile-error"
                        />
                    }
                    .into_any(),
                }
            }}
        </section>
    }
}

// ---------------------------------------------------------------------------
// Settings (W6.7 Step 7)
// ---------------------------------------------------------------------------

/// The BARE `GET /api/health` envelope (`{"success":true,"status":"ok",
/// "authEnabled":…}` — no `data` wrapper).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatus {
    pub success: bool,
    pub status: String,
    #[serde(default)]
    pub auth_enabled: bool,
}

/// The `byScope` object of `GET /api/stats`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ByScopeView {
    #[serde(default)]
    pub user: u64,
    #[serde(default)]
    pub project: u64,
}

/// One shard row of `GET /api/stats` (upstream `StatsShard`; camelCase, every
/// field defaulted so older or partial payloads still decode).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardStatsView {
    #[serde(default)]
    pub shard_id: Option<i64>,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub scope_hash: String,
    #[serde(default)]
    pub shard_index: u32,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_exists: bool,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub registered_vector_count: u64,
    #[serde(default)]
    pub memory_count: u64,
    #[serde(default)]
    pub oldest_created_at: Option<String>,
    #[serde(default)]
    pub newest_created_at: Option<String>,
    #[serde(default)]
    pub duplicate_tag_count: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// One day of the 14-day daily series (upstream `StatsDaily`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyStatsView {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub captures: u64,
    #[serde(default)]
    pub capture_skipped: u64,
    #[serde(default)]
    pub profile_injections: u64,
    #[serde(default)]
    pub compaction_injections: u64,
    #[serde(default)]
    pub errors: BTreeMap<String, u64>,
}

/// The profile array counts (upstream `ProfileStats.counts`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCountsView {
    #[serde(default)]
    pub preferences: u64,
    #[serde(default)]
    pub patterns: u64,
    #[serde(default)]
    pub workflows: u64,
}

/// The profile summary section (upstream `ProfileStats`) — counts only, never
/// the profile contents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatsView {
    #[serde(default)]
    pub exists: bool,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub counts: Option<ProfileCountsView>,
    #[serde(default)]
    pub last_analyzed_at: Option<String>,
    #[serde(default)]
    pub total_prompts_analyzed: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

/// The `GET /api/stats` data payload.
///
/// The diagnostic sections are `#[serde(default)]` (upstream marks them
/// optional: "older or partially available responses can omit diagnostic
/// sections"), so a legacy `{total, byScope, byType, generatedAt}` payload
/// decodes with empty defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub by_scope: ByScopeView,
    #[serde(default)]
    pub by_type: BTreeMap<String, u64>,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub shards: Vec<ShardStatsView>,
    #[serde(default)]
    pub profile: Option<ProfileStatsView>,
    #[serde(default)]
    pub daily: Vec<DailyStatsView>,
    #[serde(default)]
    pub daily_error: Option<String>,
}

/// Upstream `StatsView.svelte` L43–50: scope ascending, then memory count
/// descending, then shard index ascending.
pub fn sorted_shards(shards: &[ShardStatsView]) -> Vec<ShardStatsView> {
    let mut sorted = shards.to_vec();
    sorted.sort_by(|left, right| {
        left.scope
            .cmp(&right.scope)
            .then(right.memory_count.cmp(&left.memory_count))
            .then(left.shard_index.cmp(&right.shard_index))
    });
    sorted
}

/// Upstream L51–53: the tallest daily counter, at least 1 (the bar-width
/// division base never zeroes).
pub fn daily_peak(daily: &[DailyStatsView]) -> u64 {
    daily
        .iter()
        .flat_map(|day| {
            [
                day.captures,
                day.capture_skipped,
                day.profile_injections,
                day.compaction_injections,
            ]
        })
        .max()
        .unwrap_or(0)
        .max(1)
}

/// Upstream L55–59: buckets with a positive count, sorted by bucket name.
pub fn error_buckets(day: &DailyStatsView) -> Vec<(String, u64)> {
    let mut buckets: Vec<(String, u64)> = day
        .errors
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(bucket, count)| (bucket.clone(), *count))
        .collect();
    buckets.sort_by(|left, right| left.0.cmp(&right.0));
    buckets
}

/// Upstream L105: the active-shards card counts `isActive` flags only.
pub fn active_shard_count(shards: &[ShardStatsView]) -> usize {
    shards.iter().filter(|shard| shard.is_active).count()
}

/// The Settings screen's render shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsScreen {
    Loading,
    Loaded {
        status: String,
        auth_enabled: bool,
        total: u64,
        by_user: u64,
        by_project: u64,
        by_type: BTreeMap<String, u64>,
    },
    Error(String),
}

/// `GET /api/health`.
pub fn health_spec() -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::Health).template)
}

/// `GET /api/stats`.
pub fn stats_spec() -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::Stats).template)
}

/// Decode the BARE health envelope.
///
/// `/api/health` has no `data` key, while [`api_request::request`] requires one
/// on every 2xx, so the screen reads the bare body here (the
/// [`crate::screens::delete_outcome`] pattern). The message is fixed: no body
/// byte is ever surfaced.
pub fn health_outcome(status: u16, bytes: &[u8]) -> Result<HealthStatus, ApiError> {
    let health: HealthStatus = serde_json::from_slice(bytes).map_err(|_| ApiError::Malformed)?;
    if (200..300).contains(&status) && health.success {
        return Ok(health);
    }
    Err(ApiError::server(status, "health check failed"))
}

/// Fetch the health envelope through the shared seam.
pub async fn health(auth: &AuthContext) -> Result<HealthStatus, ApiError> {
    let (status, bytes) = fetch_status_and_bytes(&health_spec(), auth).await?;
    health_outcome(status, &bytes)
}

/// Map the health + stats pair onto the render shape.
pub fn settings_view(health: &HealthStatus, stats: &StatsView) -> SettingsScreen {
    if !health.success {
        return SettingsScreen::Error("health check failed".to_string());
    }
    SettingsScreen::Loaded {
        status: health.status.clone(),
        auth_enabled: health.auth_enabled,
        total: stats.total,
        by_user: stats.by_scope.user,
        by_project: stats.by_scope.project,
        by_type: stats.by_type.clone(),
    }
}

// ---------------------------------------------------------------------------
// Stats (this wave)
// ---------------------------------------------------------------------------

/// The stats screen's render shape: the fetch state machine. Like upstream's
/// `createStatsStore`, a failed refresh keeps the last snapshot visible.
#[derive(Debug, Clone, PartialEq)]
pub enum StatsScreen {
    Loading,
    Loaded(StatsView),
    Failed {
        message: String,
        last: Option<StatsView>,
    },
}

/// Fetch `GET /api/stats` through the shared seam (the stats envelope carries
/// `data`, so no bare-body decoder is needed).
async fn stats_data(auth: &AuthContext) -> Result<StatsView, ApiError> {
    api_request::request::<StatsView>(&stats_spec(), auth).await
}

/// The `—` cell for an absent timestamp (upstream `date(value)`).
fn stats_date_or_dash(value: Option<&String>) -> String {
    value.map_or_else(|| "—".to_string(), |stamp| stamp.clone())
}

/// The bar-width percentage of one counter against the peak (`max(1, …)`).
fn stats_bar_width(count: u64, peak: u64) -> String {
    let percent = (count as f64 / peak as f64) * 100.0;
    format!("width: {percent:.1}%")
}

/// The overview cards, the daily table and the shards table (upstream
/// `StatsView.svelte`'s render logic, English-only — D8: no i18n layer in
/// this UI; upstream's ~240 keys are a named follow-up).
fn stats_overview(stats: &StatsView, active_shards: usize) -> AnyView {
    let profile_cell = match stats.profile.as_ref() {
        Some(profile) if profile.error.is_some() => {
            let error = profile.error.clone().unwrap_or_default();
            view! {
                <p class="stats-profile-warning">"Profile unavailable"</p>
                <p class="stats-profile-error">{error}</p>
            }
            .into_any()
        }
        None => view! { <p class="stats-profile-warning">"Profile unavailable"</p> }.into_any(),
        Some(profile) if !profile.exists => {
            view! { <p class="stats-profile-empty">"No profile yet"</p> }.into_any()
        }
        Some(profile) => {
            let counts = profile.counts.as_ref().map(|counts| {
                view! {
                    <dl class="stats-profile-counts">
                        <dt>"Preferences"</dt>
                        <dd>{counts.preferences}</dd>
                        <dt>"Patterns"</dt>
                        <dd>{counts.patterns}</dd>
                        <dt>"Workflows"</dt>
                        <dd>{counts.workflows}</dd>
                    </dl>
                }
            });
            let prompts = profile
                .total_prompts_analyzed
                .map(|total| view! { <p class="stats-profile-prompts">{format!("Prompts analyzed: {total}")}</p> });
            view! {
                <p class="stats-profile-version">
                    {format!("Version {}", profile.version.map_or("—".to_string(), |v| v.to_string()))}
                </p>
                <p class="stats-profile-analyzed">
                    {format!("Last analyzed: {}", stats_date_or_dash(profile.last_analyzed_at.as_ref()))}
                </p>
                {counts}
                {prompts}
            }
                .into_any()
        }
    };

    let generated = if stats.generated_at.is_empty() {
        "—".to_string()
    } else {
        stats.generated_at.clone()
    };

    let total = stats.total;
    let scope_meta = format!(
        "user {} · project {}",
        stats.by_scope.user, stats.by_scope.project
    );
    let shards_len = stats.shards.len();

    view! {
        <section class="stats-overview" aria-label="Stats overview">
            <Card class="stats-card" attr:data-testid="stats-card-total">
                <h3>"Total memories"</h3>
                <p class="stats-card-value">{total}</p>
                <p class="stats-card-meta">{scope_meta}</p>
            </Card>
            <Card class="stats-card" attr:data-testid="stats-card-shards">
                <h3>"Active shards"</h3>
                <p class="stats-card-value">{active_shards}</p>
                <p class="stats-card-meta">{format!("{} shards total", shards_len)}</p>
            </Card>
            <Card class="stats-card" attr:data-testid="stats-card-profile">
                <h3>"Profile"</h3>
                {profile_cell}
            </Card>
            <Card class="stats-card" attr:data-testid="stats-card-generated">
                <h3>"Generated"</h3>
                <p class="stats-card-value stats-generated">{generated}</p>
                <p class="stats-card-meta">"Snapshot of this answer"</p>
            </Card>
        </section>
    }
    .into_any()
}

/// The 14-day daily table: one row per day, counter bars scaled by
/// [`daily_peak`], error buckets collapsible per row.
fn stats_daily_table(daily: &[DailyStatsView]) -> AnyView {
    if daily.is_empty() {
        return view! { <p class="stats-empty">"No activity recorded"</p> }.into_any();
    }
    let peak = daily_peak(daily);
    let rows: Vec<_> = daily
        .iter()
        .map(|day| {
            let buckets = error_buckets(day);
            let errors_cell = if buckets.is_empty() {
                view! { <span class="stats-no-errors">"no errors"</span> }.into_any()
            } else {
                let total: u64 = buckets.iter().map(|(_, count)| count).sum();
                let items: Vec<_> = buckets
                    .into_iter()
                    .map(|(bucket, count)| {
                        view! { <li class="stats-bucket">{format!("{bucket} ×{count}")}</li> }
                    })
                    .collect();
                view! {
                    <details class="stats-buckets">
                        <summary>{total}</summary>
                        <ul>{items}</ul>
                    </details>
                }
                .into_any()
            };
            let row_cells = [
                ("Captures", day.captures),
                ("Skipped", day.capture_skipped),
                ("Profile", day.profile_injections),
                ("Compaction", day.compaction_injections),
            ];
            let cells: Vec<_> = row_cells
                .into_iter()
                .map(|(label, count)| {
                    view! {
                        <td class="stats-counter">
                            <span class="stats-count">{count}</span>
                            <span class="stats-bar" style=stats_bar_width(count, peak)></span>
                            <span class="visually-hidden">{label}</span>
                        </td>
                    }
                })
                .collect();
            view! {
                <tr class="stats-day">
                    <th scope="row">{day.date.clone()}</th>
                    {cells}
                    <td class="stats-errors-cell">{errors_cell}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <table class="stats-daily" data-testid="stats-daily-table">
            <thead>
                <tr>
                    <th scope="col">"Date"</th>
                    <th scope="col">"Captures"</th>
                    <th scope="col">"Skipped"</th>
                    <th scope="col">"Profile"</th>
                    <th scope="col">"Compaction"</th>
                    <th scope="col">"Errors"</th>
                </tr>
            </thead>
            <tbody>{rows}</tbody>
        </table>
    }
    .into_any()
}

/// The shards table, [`sorted_shards`] order: scope, file, vectors, dates,
/// duplicates and the per-shard error.
fn stats_shards_table(shards: &[ShardStatsView]) -> AnyView {
    if shards.is_empty() {
        return view! { <p class="stats-empty">"No shards registered"</p> }.into_any();
    }
    let rows: Vec<_> = sorted_shards(shards)
        .into_iter()
        .map(|shard| {
            let hash = shard.scope_hash.chars().take(8).collect::<String>();
            let hash = if hash.is_empty() { "—".to_string() } else { hash };
            let file_state = if shard.file_exists { "present" } else { "missing" };
            let active = if shard.is_active { "active" } else { "inactive" };
            let mismatch = (shard.memory_count != shard.registered_vector_count).then(|| {
                view! { <p class="stats-mismatch">"count mismatch"</p> }
            });
            let error = shard.error.clone().map(|error| {
                view! { <p class="stats-shard-error">{error}</p> }
            });
            view! {
                <tr class="stats-shard-row">
                    <td>
                        <p>{shard.scope.clone()}</p>
                        <p class="stats-hash">{hash}</p>
                    </td>
                    <td>
                        <p>{shard.file_name.clone()}</p>
                        <p class={if shard.file_exists { "stats-file-ok" } else { "stats-file-missing" }}>
                            {file_state}
                        </p>
                        {error}
                    </td>
                    <td>{shard.shard_index}</td>
                    <td>{active}</td>
                    <td>
                        {format!("{} / {}", shard.memory_count, shard.registered_vector_count)}
                        {mismatch}
                    </td>
                    <td>
                        <p>{format!("oldest: {}", stats_date_or_dash(shard.oldest_created_at.as_ref()))}</p>
                        <p>{format!("newest: {}", stats_date_or_dash(shard.newest_created_at.as_ref()))}</p>
                    </td>
                    <td>{shard.duplicate_tag_count}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <table class="stats-shards" data-testid="stats-shards-table">
            <thead>
                <tr>
                    <th scope="col">"Scope"</th>
                    <th scope="col">"File"</th>
                    <th scope="col">"Index"</th>
                    <th scope="col">"Status"</th>
                    <th scope="col">"Memories / vectors"</th>
                    <th scope="col">"Dates"</th>
                    <th scope="col">"Duplicate tags"</th>
                </tr>
            </thead>
            <tbody>{rows}</tbody>
        </table>
    }
    .into_any()
}

/// The loaded body: overview cards, the dailyError alert, the daily table and
/// the shards table.
fn stats_loaded_view(stats: &StatsView) -> AnyView {
    let active_shards = active_shard_count(&stats.shards);
    let daily_error = stats.daily_error.clone().map(|daily_error| {
        view! { <p class="stats-daily-error">{format!("Daily series error: {daily_error}")}</p> }
    });
    view! {
        {stats_overview(stats, active_shards)}
        {daily_error}
        <section class="stats-section" aria-label="Daily activity">
            <h3>"Daily activity (14 days)"</h3>
            {stats_daily_table(&stats.daily)}
        </section>
        <section class="stats-section" aria-label="Shards">
            <h3>"Shards"</h3>
            {stats_shards_table(&stats.shards)}
        </section>
    }
    .into_any()
}

/// The stats screen: `GET /api/stats` decoded into [`StatsView`], with a
/// manual reload and the last snapshot kept on error.
#[component]
pub fn StatsPanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let state = RwSignal::new(StatsScreen::Loading);

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            let previous = state.get_untracked();
            leptos::task::spawn_local(async move {
                match stats_data(&auth).await {
                    Ok(stats) => state.set(StatsScreen::Loaded(stats)),
                    Err(failure) => {
                        let last = match previous {
                            StatsScreen::Loaded(stats) => Some(stats),
                            StatsScreen::Failed { last, .. } => last,
                            StatsScreen::Loading => None,
                        };
                        state.set(StatsScreen::Failed {
                            message: failure.to_string(),
                            last,
                        });
                    }
                }
            });
        }
    });
    Effect::new(move |_| reload.run(()));

    view! {
        <section class="stats-panel" data-testid="stats-panel">
            <h2>"Stats"</h2>
            <Button
                class="stats-reload"
                attr:data-testid="stats-reload"
                on_click=move |_| reload.run(())
            >
                "Reload"
            </Button>
            {move || match state.get() {
                StatsScreen::Loading => view! {
                    <StateBlock
                        kind=BlockKind::Loading
                        message="Loading stats…"
                        class="state-loading"
                        testid="stats-loading"
                    />
                }
                .into_any(),
                StatsScreen::Loaded(stats) => stats_loaded_view(&stats),
                StatsScreen::Failed { message, last } => {
                    let stale = last.is_some().then(|| {
                        view! {
                            <p class="stats-stale" data-testid="stats-stale">
                                "Showing the last successful snapshot."
                            </p>
                        }
                    });
                    let body = last.map(|stats| stats_loaded_view(&stats));
                    view! {
                        <StateBlock
                            kind=BlockKind::Error
                            message=message
                            class="state-error"
                            testid="stats-error"
                        />
                        {stale}
                        {body}
                    }
                    .into_any()
                }
            }}
        </section>
    }
}

// ---------------------------------------------------------------------------
// Maintenance (issue 42 / W8.4)
// ---------------------------------------------------------------------------

/// `POST /api/cleanup`.
///
/// Bodyless by design: the server owns the retention window and the scope of
/// the sweep, so the client sends nothing to interpret.
pub fn cleanup_spec() -> RequestSpec {
    RequestSpec::post(endpoint(EndpointId::Cleanup).template)
}

/// The frozen five-field cleanup report (issue 39).
///
/// Every field is **required** on purpose: no `#[serde(default)]` anywhere, so
/// a partial or `null` body classifies as a malformed response instead of
/// rendering a zero the server never sent (D-3).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub deleted_count: u64,
    pub user_count: u64,
    pub project_count: u64,
    pub prompts_deleted: u64,
    pub protected_memories_skipped: u64,
}

/// What one cleanup run answered.
///
/// `Unavailable` (the `501` build) and `Refused` (a `200 success:false`
/// business decision) are outcomes, not failures — only `Failed` means the
/// screen could not get an answer at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    Ran(CleanupReport),
    Refused(String),
    Unavailable,
    Failed(String),
}

/// Classify one `/api/cleanup` answer.
///
/// `501` is checked **first**, before a single body byte is decoded: the
/// documented degraded-server answer can arrive with any body (a proxy page, a
/// truncated response) and must still render as unavailable rather than as the
/// permanent error a generic `ApiError::Server { status: 501 }` would produce
/// (D-2). A `2xx` failure envelope is a business refusal, not an error (D-5);
/// everything else keeps the existing health/stats wording.
pub fn cleanup_outcome(status: u16, bytes: &[u8]) -> CleanupOutcome {
    if status == 501 {
        return CleanupOutcome::Unavailable;
    }
    match parse_envelope::<CleanupReport>(status, bytes) {
        Ok(report) => CleanupOutcome::Ran(report),
        Err(ApiError::Server {
            status: 200..=299,
            message,
        }) => CleanupOutcome::Refused(message),
        Err(failure) => CleanupOutcome::Failed(failure.to_string()),
    }
}

/// The Maintenance section's arming / in-flight state.
///
/// `last` survives re-arming so the previous report stays readable; it is
/// replaced only by the next finished run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupState {
    pub armed: bool,
    pub running: bool,
    pub last: Option<CleanupOutcome>,
}

/// The user's intent on the section's two buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupAction {
    Arm,
    Cancel,
    Confirm,
}

/// Everything that can move [`CleanupState`] forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupEvent {
    Action(CleanupAction),
    Finished(CleanupOutcome),
}

/// One reducer step: the next state, plus whether to start a server run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupStep {
    pub state: CleanupState,
    pub run: bool,
}

/// The reducer for the maintenance section.
///
/// Total and side-effect free. `run` is true **iff** the event is
/// `Action(Confirm)` from an armed, non-running state — the single transition
/// that may reach the network. Arming, cancelling, a queued click while a run is
/// in flight and every `Finished` outcome all return `run == false`, so no
/// mount, reload or polling path can start a cleanup.
/// `last` is preserved by every `Action` and overwritten only by `Finished`.
pub fn cleanup_next(state: &CleanupState, event: CleanupEvent) -> CleanupStep {
    let mut next = state.clone();
    let mut run = false;
    match event {
        CleanupEvent::Action(CleanupAction::Arm) => {
            if !next.running {
                next.armed = true;
            }
        }
        CleanupEvent::Action(CleanupAction::Cancel) => {
            if !next.running {
                next.armed = false;
            }
        }
        CleanupEvent::Action(CleanupAction::Confirm) => {
            if next.armed && !next.running {
                next.armed = false;
                next.running = true;
                run = true;
            }
        }
        CleanupEvent::Finished(outcome) => {
            next.armed = false;
            next.running = false;
            next.last = Some(outcome);
        }
    }
    CleanupStep { state: next, run }
}

/// The three shapes the section's single action can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupStatus {
    Idle,
    Armed,
    Running,
}

/// Which button shape the section is in.
pub fn cleanup_status(state: &CleanupState) -> CleanupStatus {
    if state.running {
        CleanupStatus::Running
    } else if state.armed {
        CleanupStatus::Armed
    } else {
        CleanupStatus::Idle
    }
}

/// The action button's label; the armed step must be readable.
pub fn cleanup_action_label(status: CleanupStatus) -> &'static str {
    match status {
        CleanupStatus::Idle => "Run cleanup…",
        CleanupStatus::Armed => "Confirm cleanup",
        CleanupStatus::Running => "Cleaning up…",
    }
}

/// Whether the action button is disabled (in flight, nothing else).
pub fn cleanup_disabled(state: &CleanupState) -> bool {
    state.running
}

/// The fixed copy of the documented degraded-server state.
///
/// Deliberately **not** the server's own 501 text: the panel never renders a
/// body from a 501 (the classifier discards it before decoding — D-2), so a
/// proxy-supplied body cannot reach the screen and the UI copy cannot drift
/// with a server constant.
pub const CLEANUP_UNAVAILABLE_MESSAGE: &str = "Cleanup is not available in this build.";

/// The warning shown between arming and confirming.
pub const CLEANUP_ARMED_NOTICE: &str =
    "This deletes memories older than the retention window. Confirm to run it.";

/// The `data-testid` of the rendered report.
pub const CLEANUP_REPORT_TESTID: &str = "maintenance-report";

/// The `data-testid` of the business-refusal note.
pub const CLEANUP_REFUSED_TESTID: &str = "maintenance-refused";

/// The `data-testid` of the unavailable note.
pub const CLEANUP_UNAVAILABLE_TESTID: &str = "maintenance-unavailable";

/// The `data-testid` of the failure note.
pub const CLEANUP_ERROR_TESTID: &str = "maintenance-error";

/// The `data-testid` of the armed notice.
pub const CLEANUP_ARMED_TESTID: &str = "maintenance-armed";

/// The class of the rendered report.
pub const CLEANUP_REPORT_CLASS: &str = "maintenance-report";

/// The class of the business-refusal note.
pub const CLEANUP_REFUSED_CLASS: &str = "maintenance-refused";

/// The class of the unavailable note.
pub const CLEANUP_UNAVAILABLE_CLASS: &str = "maintenance-unavailable";

/// The class of the failure note.
pub const CLEANUP_ERROR_CLASS: &str = "maintenance-error";

/// One rendered report row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupReportRow {
    pub testid: &'static str,
    pub label: &'static str,
    pub hint: Option<&'static str>,
    pub count: u64,
}

/// The five report rows, in issue 39's field order.
///
/// The counts are shown exactly as the server answered them: the UI does not
/// re-derive cross-field sums, so a backend grouping slip cannot be masked by
/// browser arithmetic (D-3).
pub fn cleanup_report_rows(report: &CleanupReport) -> [CleanupReportRow; 5] {
    [
        CleanupReportRow {
            testid: "maintenance-deleted",
            label: "Memories deleted",
            hint: None,
            count: report.deleted_count,
        },
        CleanupReportRow {
            testid: "maintenance-user",
            label: "User memories deleted",
            hint: None,
            count: report.user_count,
        },
        CleanupReportRow {
            testid: "maintenance-project",
            label: "Project memories deleted",
            hint: None,
            count: report.project_count,
        },
        CleanupReportRow {
            testid: "maintenance-prompts",
            label: "Prompts deleted",
            hint: None,
            count: report.prompts_deleted,
        },
        CleanupReportRow {
            testid: "maintenance-protected",
            label: "Memories spared",
            hint: Some("Spared because a linked prompt expired recently."),
            count: report.protected_memories_skipped,
        },
    ]
}

/// The outcome block's render shape.
///
/// The `Report` arm carries the five rows inline: they are `Copy` and built on
/// the render path, so boxing them would add an allocation per render for no
/// gain. The variant-size spread is the price of that.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcomeView {
    Nothing,
    Report([CleanupReportRow; 5]),
    Refused(String),
    Unavailable(&'static str),
    Failed(String),
}

/// Map the last outcome onto the render shape.
pub fn cleanup_outcome_view(outcome: Option<&CleanupOutcome>) -> CleanupOutcomeView {
    match outcome {
        None => CleanupOutcomeView::Nothing,
        Some(CleanupOutcome::Ran(report)) => {
            CleanupOutcomeView::Report(cleanup_report_rows(report))
        }
        Some(CleanupOutcome::Refused(message)) => CleanupOutcomeView::Refused(message.clone()),
        Some(CleanupOutcome::Unavailable) => {
            CleanupOutcomeView::Unavailable(CLEANUP_UNAVAILABLE_MESSAGE)
        }
        Some(CleanupOutcome::Failed(message)) => CleanupOutcomeView::Failed(message.clone()),
    }
}

/// Run one confirmed cleanup and classify its answer.
///
/// Reads the raw status and bytes because a `501` has to be recognised before
/// the body is decoded ([`cleanup_outcome`], D-2).
async fn run_cleanup(auth: &AuthContext) -> CleanupOutcome {
    match fetch_status_and_bytes(&cleanup_spec(), auth).await {
        Ok((status, bytes)) => cleanup_outcome(status, &bytes),
        Err(failure) => CleanupOutcome::Failed(failure.to_string()),
    }
}

/// The W6.7 Settings screen.
///
/// It reads `/api/health` and `/api/stats` on every mount, and adds the
/// Maintenance section, which calls `POST /api/cleanup` **only** after an
/// explicit arm-then-confirm click — never on mount, never on reload, never on
/// a timer. A `501` from that endpoint is rendered as the documented "not
/// available in this build" state (D-15), not as an error. `/api/deduplicate`
/// and `/api/migration/*` remain uncalled (Decision 3).
#[component]
pub fn SettingsPanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let state = RwSignal::new(SettingsScreen::Loading);

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            state.set(SettingsScreen::Loading);
            leptos::task::spawn_local(async move {
                let health_status = health(&auth).await;
                let stats = api_request::request::<StatsView>(&stats_spec(), &auth).await;
                match (health_status, stats) {
                    (Ok(health), Ok(stats)) => state.set(settings_view(&health, &stats)),
                    (Err(failure), _) | (_, Err(failure)) => {
                        state.set(SettingsScreen::Error(failure.to_string()))
                    }
                }
            });
        }
    });
    Effect::new(move |_| reload.run(()));

    // The Maintenance section's only path to the network: this callback is
    // reachable from the two `on:click` handlers below and from nowhere else
    // (no reactive effect, no mount hook, no interval, no call from `reload`).
    let cleanup = RwSignal::new(CleanupState::default());
    let dispatch = Callback::new({
        let auth = auth.clone();
        move |action: CleanupAction| {
            let step = cleanup_next(&cleanup.get_untracked(), CleanupEvent::Action(action));
            cleanup.set(step.state);
            if !step.run {
                return;
            }
            let auth = auth.clone();
            leptos::task::spawn_local(async move {
                let outcome = run_cleanup(&auth).await;
                cleanup.update(|current| {
                    *current = cleanup_next(current, CleanupEvent::Finished(outcome)).state;
                });
            });
        }
    });

    view! {
        <section class="settings-panel" data-testid="settings-panel">
            <h2>"Settings"</h2>
            <Button
                class="settings-reload"
                attr:data-testid="settings-reload"
                on_click=move |_| reload.run(())
            >
                "Reload"
            </Button>
            {move || {
                match state.get() {
                    SettingsScreen::Loading => view! {
                        <StateBlock
                            kind=BlockKind::Loading
                            message="Loading status…"
                            class="state-loading"
                            testid="settings-loading"
                        />
                    }
                    .into_any(),
                    SettingsScreen::Loaded {
                        status,
                        auth_enabled,
                        total,
                        by_user,
                        by_project,
                        by_type,
                    } => {
                        let types = if by_type.is_empty() {
                            "(none)".to_string()
                        } else {
                            by_type
                                .iter()
                                .map(|(kind, count)| format!("{kind}: {count}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };
                        view! {
                            <dl class="settings-status" data-testid="settings-status">
                                <dt>"Health"</dt>
                                <dd data-testid="settings-health">
                                    {format!("{status} · authEnabled: {auth_enabled}")}
                                </dd>
                                <dt>"Memories"</dt>
                                <dd data-testid="settings-total">{format!("total {total}")}</dd>
                                <dt>"By scope"</dt>
                                <dd data-testid="settings-by-scope">
                                    {format!("user {by_user} · project {by_project}")}
                                </dd>
                                <dt>"By type"</dt>
                                <dd data-testid="settings-by-type">{types}</dd>
                            </dl>
                        }
                            .into_any()
                    }
                    SettingsScreen::Error(message) => view! {
                        <StateBlock
                            kind=BlockKind::Error
                            message=message
                            class="state-error"
                            testid="settings-error"
                        />
                    }
                    .into_any(),
                }
            }}
            {move || {
                // Bound first: `cleanup_outcome_view(snapshot.last.as_ref())`
                // needs an owner for the borrow (a temporary is E0716).
                let snapshot = cleanup.get();
                let status = cleanup_status(&snapshot);
                let outcome = cleanup_outcome_view(snapshot.last.as_ref());
                let armed_notice = (status == CleanupStatus::Armed).then(|| {
                    view! {
                        <p class="maintenance-armed" data-testid=CLEANUP_ARMED_TESTID>
                            {CLEANUP_ARMED_NOTICE}
                        </p>
                    }
                });
                let cancel = (status == CleanupStatus::Armed).then(|| {
                    view! {
                        <Button
                            appearance=ButtonAppearance::Secondary
                            class="maintenance-cancel"
                            attr:data-testid="maintenance-cancel"
                            on_click=move |_| dispatch.run(CleanupAction::Cancel)
                        >
                            "Cancel"
                        </Button>
                    }
                });
                let outcome_view = match outcome {
                    CleanupOutcomeView::Nothing => ().into_any(),
                    CleanupOutcomeView::Report(rows) => {
                        view! {
                            <h4>"Last cleanup"</h4>
                            <dl class=CLEANUP_REPORT_CLASS data-testid=CLEANUP_REPORT_TESTID>
                                {rows
                                    .into_iter()
                                    .map(|row| {
                                        let hint = row
                                            .hint
                                            .map(|hint| {
                                                view! { <span class="maintenance-hint">{hint}</span> }
                                            });
                                        view! {
                                            <dt>{row.label}</dt>
                                            <dd data-testid=row.testid>
                                                <span class="maintenance-count">{row.count}</span>
                                                {hint}
                                            </dd>
                                        }
                                    })
                                    .collect_view()}
                            </dl>
                        }
                            .into_any()
                    }
                    CleanupOutcomeView::Refused(message) => {
                        view! {
                            <p class=CLEANUP_REFUSED_CLASS data-testid=CLEANUP_REFUSED_TESTID>
                                {message}
                            </p>
                        }
                            .into_any()
                    }
                    CleanupOutcomeView::Unavailable(message) => {
                        view! {
                            <p class=CLEANUP_UNAVAILABLE_CLASS data-testid=CLEANUP_UNAVAILABLE_TESTID>
                                {message}
                            </p>
                        }
                            .into_any()
                    }
                    CleanupOutcomeView::Failed(message) => {
                        view! {
                            <p class=CLEANUP_ERROR_CLASS data-testid=CLEANUP_ERROR_TESTID>{message}</p>
                        }
                            .into_any()
                    }
                };
                view! {
                    <section
                        class="maintenance"
                        data-testid="maintenance-section"
                        aria-label="Maintenance"
                    >
                        <h3>"Maintenance"</h3>
                        <p>
                            "Cleanup deletes memories older than the retention window. It is never run automatically."
                        </p>
                        {armed_notice}
                        <Button
                            appearance=ButtonAppearance::Primary
                            class="maintenance-action"
                            attr:data-testid="maintenance-action"
                            disabled=Signal::derive(move || cleanup_disabled(&cleanup.get()))
                            on_click=move |_| {
                                let action = if cleanup.get_untracked().armed {
                                    CleanupAction::Confirm
                                } else {
                                    CleanupAction::Arm
                                };
                                dispatch.run(action);
                            }
                        >
                            {cleanup_action_label(status)}
                        </Button>
                        {cancel}
                        {outcome_view}
                    </section>
                }
            }}
        </section>
    }
}
