//! W6.7 — the four core memory screens: Memories, Tags, Profile and Settings.
//!
//! Each screen follows the W6.5 house pattern:
//!
//! 1. a **pure request spec** (`list_memories_spec`, `list_tags_spec`,
//!    `get_user_profile_spec`, `health_spec`, `stats_spec`) built from the
//!    pinned [`crate::paths::ENDPOINTS`] templates, so the wasm tests pin the
//!    exact request with no `fetch`;
//! 2. a **pure DTO decode** (`MemoryPage`, `TagsPage`, `ProfileView`,
//!    `HealthStatus`, `StatsView`) matching the `mem-web` projection;
//! 3. a **pure render shape** (`memories_view`, `tags_view`, `profile_view`,
//!    `settings_view`) the panel renders from, so the empty / loaded / missing /
//!    error branches are testable without a DOM;
//! 4. the **panel component** itself, which owns the signals and the one
//!    same-origin fetch seam ([`crate::api_request`]).
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

use crate::api::auth::{AuthContext, read_marker_token};
use crate::api::envelope::ApiError;
use crate::api_request::{self, RequestSpec};
use crate::paths::{ENDPOINTS, EndpointId, EndpointSpec, fill};
use crate::screens::format_created_at;

/// One pinned endpoint spec, for the templates the screens build.
fn endpoint(id: EndpointId) -> EndpointSpec {
    *ENDPOINTS
        .iter()
        .find(|spec| spec.id == id)
        .expect("the endpoint table has a row for every EndpointId")
}

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

/// `GET /api/memories?page=…&pageSize=…[&tag=…][&includePrompts=false]`.
///
/// One `tag` pair per filter entry; `includePrompts` is sent only when false
/// (the server's default is true, so the redundant `true` is omitted).
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
        spec = spec.with_query("tag", (*tag).to_string());
    }
    if !include_prompts {
        spec = spec.with_query("includePrompts", "false");
    }
    spec
}

/// `GET /api/search?q=…&page=…&pageSize=…[&tag=…]`.
pub fn search_spec(q: &str, page: usize, page_size: usize, tags: &[&str]) -> RequestSpec {
    let mut spec = RequestSpec::get(endpoint(EndpointId::Search).template)
        .with_query("q", q)
        .with_query("page", page.to_string())
        .with_query("pageSize", page_size.to_string());
    for tag in tags {
        spec = spec.with_query("tag", (*tag).to_string());
    }
    spec
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

/// Run a data-less mutation through the shared seam.
///
/// `POST …/pin`, `POST …/unpin` and `DELETE /api/memories/{id}` all answer a
/// success envelope without the `data` payload [`api_request::request`]
/// requires, so the body goes through the same bounded
/// [`crate::screens::delete_outcome`] decoder the session delete uses. The
/// request itself is still prepared by [`api_request::prepare`]: same-origin,
/// same-origin credentials, token header included.
async fn perform_action(spec: &RequestSpec, auth: &AuthContext) -> Result<(), ApiError> {
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
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    crate::screens::delete_outcome(status, &bytes)
}

/// One rendered memory row with its pin/unpin/delete controls.
fn memory_row(
    row: MemoryRowView,
    reload: Callback<()>,
    action_error: RwSignal<Option<String>>,
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

    let delete_id = row.id.clone();
    let delete = move |_| {
        let spec = delete_memory_spec(&delete_id);
        let auth = auth.clone();
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

    let tags = if row.tags.is_empty() {
        "(no tags)".to_string()
    } else {
        row.tags.join(", ")
    };
    let kind = if is_prompt { "prompt" } else { "memory" };

    view! {
        <li class="memory-row" data-testid="memory-row">
            <span class="memory-kind" data-testid="memory-kind">{kind}</span>
            <span class="memory-content" data-testid="memory-content">{row.content}</span>
            <span class="memory-tags" data-testid="memory-tags">{tags}</span>
            <span class="memory-created" data-testid="memory-created">
                {row.created_at}
            </span>
            {pinned
                .then(|| {
                    view! {
                        <span class="memory-pinned" data-testid="memory-pinned">
                            "pinned"
                        </span>
                    }
                })}
            {(!is_prompt)
                .then(move || {
                    view! {
                        <button
                            type="button"
                            class="memory-pin"
                            data-testid="memory-pin"
                            on:click=pin
                        >
                            {if pinned { "Unpin" } else { "Pin" }}
                        </button>
                        <button
                            type="button"
                            class="memory-delete"
                            data-testid="memory-delete"
                            on:click=delete
                        >
                            "Delete"
                        </button>
                    }
                })}
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
    let tag_filter = RwSignal::new(String::new());
    let state = RwSignal::new(MemoriesView::Loading);
    let action_error = RwSignal::new(None::<String>);

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            let current = page.get_untracked();
            let size = page_size.get_untracked();
            let text = query.get_untracked();
            let tags: Vec<String> = tag_filter
                .get_untracked()
                .split(',')
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect();
            let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
            // A query routes to `/api/search`; an empty query is the listing.
            let spec = if text.trim().is_empty() {
                list_memories_spec(current, size, &tag_refs, false)
            } else {
                search_spec(text.trim(), current, size, &tag_refs)
            };
            state.set(MemoriesView::Loading);
            leptos::task::spawn_local(async move {
                match api_request::request::<MemoryPage>(&spec, &auth).await {
                    Ok(found) => state.set(memories_view(&found)),
                    Err(failure) => state.set(MemoriesView::Error(failure.to_string())),
                }
            });
        }
    });

    // The initial load: the panel renders data on mount with no interaction.
    // Every read in `reload` is untracked, so this effect runs exactly once.
    Effect::new(move |_| reload.run(()));

    let previous = Callback::new(move |_: ()| {
        if page.get_untracked() > 1 {
            page.set(page.get_untracked() - 1);
            reload.run(());
        }
    });
    let next = Callback::new(move |_: ()| {
        page.set(page.get_untracked() + 1);
        reload.run(());
    });

    view! {
        <section class="memories-panel" data-testid="memories-panel">
            <h2>"Memories"</h2>
            <div class="memories-controls">
                <input
                    id="memories-search"
                    name="q"
                    type="text"
                    class="memories-search"
                    data-testid="memories-search"
                    placeholder="Search memories"
                    prop:value=move || query.get()
                    on:input=move |event| query.set(event_target_value(&event))
                />
                <input
                    id="memories-tags"
                    name="tags"
                    type="text"
                    class="memories-tag-filter"
                    data-testid="memories-tag-filter"
                    placeholder="tag,tag"
                    prop:value=move || tag_filter.get()
                    on:input=move |event| tag_filter.set(event_target_value(&event))
                />
                <button
                    type="button"
                    class="memories-reload"
                    data-testid="memories-reload"
                    on:click=move |_| reload.run(())
                >
                    "Reload"
                </button>
            </div>
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
                match state.get() {
                    MemoriesView::Loading => {
                        view! {
                            <p class="panel-loading" data-testid="memories-loading">
                                "Loading memories…"
                            </p>
                        }
                            .into_any()
                    }
                    MemoriesView::Empty { total } => {
                        view! {
                            <p class="memories-empty" data-testid="memories-empty">
                                {format!("No memories yet (total {total}).")}
                            </p>
                        }
                            .into_any()
                    }
                    MemoriesView::Loaded {
                        rows,
                        total,
                        page: current,
                        page_size: size,
                        total_pages,
                    } => {
                        view! {
                            <div class="memories-pager" data-testid="memories-pager">
                                <button
                                    type="button"
                                    class="memories-prev"
                                    data-testid="memories-prev"
                                    on:click=move |_| previous.run(())
                                >
                                    "Previous"
                                </button>
                                <span class="memories-count" data-testid="memories-count">
                                    {format!(
                                        "{total} rows · page {current} of {total_pages} · {size} per page",
                                    )}
                                </span>
                                <button
                                    type="button"
                                    class="memories-next"
                                    data-testid="memories-next"
                                    on:click=move |_| next.run(())
                                >
                                    "Next"
                                </button>
                            </div>
                            <ul class="memories-list" data-testid="memories-list">
                                {rows
                                    .into_iter()
                                    .map(|row| {
                                        memory_row(row, reload, action_error, auth.clone())
                                    })
                                    .collect_view()}
                            </ul>
                        }
                            .into_any()
                    }
                    MemoriesView::Error(message) => {
                        view! {
                            <p class="memories-error" data-testid="memories-error">{message}</p>
                        }
                            .into_any()
                    }
                }
            }}
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
    },
    Error(String),
}

/// Map the tags payload onto the render shape.
pub fn tags_view(page: &TagsPage) -> TagsView {
    if page.project.is_empty() && page.user.is_empty() {
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
        <section class="tag-group" data-testid=test_id>
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
            <button
                type="button"
                class="tags-reload"
                data-testid="tags-reload"
                on:click=move |_| reload.run(())
            >
                "Reload"
            </button>
            {move || {
                match state.get() {
                    TagsView::Loading => {
                        view! {
                            <p class="panel-loading" data-testid="tags-loading">"Loading tags…"</p>
                        }
                            .into_any()
                    }
                    TagsView::Empty => {
                        view! {
                            <p class="tags-empty" data-testid="tags-empty">
                                "No tags yet. Add a memory to a project or your profile."
                            </p>
                        }
                            .into_any()
                    }
                    TagsView::Loaded { project, user } => {
                        view! {
                            {tag_group("Project", "tag-group-project", project)}
                            {tag_group("User", "tag-group-user", user)}
                        }
                            .into_any()
                    }
                    TagsView::Error(message) => {
                        view! { <p class="tags-error" data-testid="tags-error">{message}</p> }
                            .into_any()
                    }
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

/// `GET /api/user-profile`.
pub fn get_user_profile_spec() -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::UserProfile).template)
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
#[component]
pub fn ProfilePanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let state = RwSignal::new(ProfileScreen::Loading);

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            state.set(ProfileScreen::Loading);
            leptos::task::spawn_local(async move {
                match api_request::request::<ProfileView>(&get_user_profile_spec(), &auth).await {
                    Ok(found) => state.set(profile_view(&found)),
                    Err(failure) => state.set(ProfileScreen::Error(failure.to_string())),
                }
            });
        }
    });
    Effect::new(move |_| reload.run(()));

    view! {
        <section class="profile-panel" data-testid="profile-panel">
            <h2>"User profile"</h2>
            <button
                type="button"
                class="profile-reload"
                data-testid="profile-reload"
                on:click=move |_| reload.run(())
            >
                "Reload"
            </button>
            {move || {
                match state.get() {
                    ProfileScreen::Loading => {
                        view! {
                            <p class="panel-loading" data-testid="profile-loading">
                                "Loading profile…"
                            </p>
                        }
                            .into_any()
                    }
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
                    ProfileScreen::Error(message) => {
                        view! {
                            <p class="profile-error" data-testid="profile-error">{message}</p>
                        }
                            .into_any()
                    }
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

/// The `GET /api/stats` data payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub by_scope: ByScopeView,
    #[serde(default)]
    pub by_type: BTreeMap<String, u64>,
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
    let (url, init) = api_request::prepare::<serde_json::Value>(&health_spec(), auth)?;
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
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
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

/// The W6.7 Settings screen.
///
/// It reads ONLY `/api/health` and `/api/stats`. The rest of the Settings path
/// group (`/api/cleanup`, `/api/deduplicate`, `/api/migration/*`) is
/// maintenance-backed and answers the documented stub in this build, so calling
/// it would show a permanent error instead of data (Decision 3).
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

    view! {
        <section class="settings-panel" data-testid="settings-panel">
            <h2>"Settings"</h2>
            <button
                type="button"
                class="settings-reload"
                data-testid="settings-reload"
                on:click=move |_| reload.run(())
            >
                "Reload"
            </button>
            {move || {
                match state.get() {
                    SettingsScreen::Loading => {
                        view! {
                            <p class="panel-loading" data-testid="settings-loading">
                                "Loading status…"
                            </p>
                        }
                            .into_any()
                    }
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
                    SettingsScreen::Error(message) => {
                        view! {
                            <p class="settings-error" data-testid="settings-error">{message}</p>
                        }
                            .into_any()
                    }
                }
            }}
        </section>
    }
}
