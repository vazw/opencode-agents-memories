//! The Skill Library screen (W6.5).
//!
//! The pure half (decode, view mapping, filter, draft validation, the id
//! algorithm, the five pinned request specs) is unit-tested; the panel half is
//! smoked through the browser instrument (list, search, create, deduplicated
//! create, detail-prefilled edit, delete).
//!
//! `slugify_skill_id` folds Latin-1/Latin-Extended-A diacritics explicitly
//! ([`fold_latin`]) instead of relying on Unicode NFKD. The upstream
//! implementation is `NFKD` + strip-combining-marks; this crate may not add a
//! Unicode-normalisation dependency, so the fold covers the common European
//! accented letters and their ligatures and leaves every other non-ASCII
//! character to the "replace a non-ASCII run with `-`" rule. Reduced coverage
//! vs full NFKD: scripts whose marks are not in the table (Greek, Cyrillic,
//! Vietnamese stacked marks beyond the listed letters, …) are transliterated
//! lossily — "café" is `cafe`, "北京" is still the empty slug — which is the
//! documented limitation, not a change to the algorithm's shape.

use leptos::callback::Callable;
use leptos::html;
use leptos::prelude::*;
use serde::Deserialize;

use crate::api::auth::{AuthContext, read_marker_token};
use crate::api::envelope::ApiError;
use crate::api_request::{self, RequestSpec};
use crate::confirm::{ConfirmDeleteButton, ConfirmEvent, ConfirmState, confirm_next};
use crate::paths::{EndpointId, endpoint, fill};
use crate::state_block::{BlockKind, StateBlock};
use thaw::{Button, ButtonType, Card, Input, Tag};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub content: String,
    #[serde(default = "default_true")]
    pub autoinvoke: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub container_tag: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsPage {
    #[serde(default)]
    pub items: Vec<SkillItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRowView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub updated_at: String,
    pub enabled: bool,
    pub autoinvoke: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsView {
    Loading,
    Empty,
    Loaded { rows: Vec<SkillRowView> },
    Error(String),
}

pub fn skills_view(page: &SkillsPage) -> SkillsView {
    let rows: Vec<SkillRowView> = page
        .items
        .iter()
        .map(|i| SkillRowView {
            id: i.id.clone(),
            name: i.name.clone(),
            description: i.description.clone(),
            updated_at: i.updated_at.clone(),
            enabled: i.enabled,
            autoinvoke: i.autoinvoke,
        })
        .collect();
    if rows.is_empty() {
        SkillsView::Empty
    } else {
        SkillsView::Loaded { rows }
    }
}

/// A failed list fetch as the view renders it.
///
/// The panel routes `GET /api/skills` failures through here, so the error
/// branch is a testable mapping rather than an inline constructor.
pub fn skills_error_view(message: &str) -> SkillsView {
    SkillsView::Error(message.to_string())
}

pub fn filter_skills(rows: &[SkillRowView], query: &str) -> Vec<SkillRowView> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|r| {
            r.id.to_lowercase().contains(&q)
                || r.name.to_lowercase().contains(&q)
                || r.description.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDraft {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub autoinvoke: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftError {
    NameRequired,
    ContentRequired,
}

pub fn validate_draft(draft: &SkillDraft) -> Result<(), DraftError> {
    if draft.name.trim().is_empty() {
        return Err(DraftError::NameRequired);
    }
    if draft.content.trim().is_empty() {
        return Err(DraftError::ContentRequired);
    }
    Ok(())
}

pub fn skill_body(draft: &SkillDraft, include_id: bool) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert(
        "name".to_string(),
        serde_json::Value::String(draft.name.trim().to_string()),
    );
    body.insert(
        "description".to_string(),
        serde_json::Value::String(draft.description.trim().to_string()),
    );
    body.insert(
        "content".to_string(),
        serde_json::Value::String(draft.content.clone()),
    );
    body.insert(
        "autoinvoke".to_string(),
        serde_json::Value::Bool(draft.autoinvoke),
    );
    body.insert(
        "enabled".to_string(),
        serde_json::Value::Bool(draft.enabled),
    );
    if include_id {
        body.insert(
            "id".to_string(),
            serde_json::Value::String(draft.id.clone()),
        );
    }
    serde_json::Value::Object(body)
}

pub fn slugify_skill_id(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ('\u{0300}'..='\u{036f}').contains(&ch) {
            continue;
        }
        match fold_latin(ch) {
            Some(base) => out.push_str(base),
            None => out.push(ch),
        }
    }
    let lower: String = out.to_lowercase();
    let mut slug = String::new();
    let mut in_sep = false;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            in_sep = false;
        } else {
            if !in_sep {
                slug.push('-');
            }
            in_sep = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    let mut slug = if slug.len() > 64 {
        slug[..64].to_string()
    } else {
        slug
    };
    while slug.ends_with('-') {
        slug.pop();
    }
    if is_kebab_valid(&slug) {
        slug
    } else {
        String::new()
    }
}

/// The explicit diacritic fold: one Latin base letter (or ASCII digraph) for
/// each accented character this crate knows.
///
/// This is the dependency-free stand-in for the upstream `NFKD` +
/// strip-marks step. It covers Latin-1 and Latin Extended-A (the accented
/// letters a skill name written in a European language can use) plus the three
/// common ligatures. Anything not listed is left as-is, so it lands in the
/// non-ASCII separator rule; the module doc records that reduced coverage.
fn fold_latin(ch: char) -> Option<&'static str> {
    let base = match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å'
        | 'Ā' | 'Ă' | 'Ą' => "a",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' | 'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => "c",
        'ď' | 'đ' | 'Ď' | 'Đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' | 'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ'
        | 'Ė' | 'Ę' | 'Ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' | 'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'Ì' | 'Í' | 'Î' | 'Ï' | 'Ĩ' | 'Ī'
        | 'Ĭ' | 'Į' | 'İ' => "i",
        'ĺ' | 'ļ' | 'ľ' | 'ł' | 'Ĺ' | 'Ļ' | 'Ľ' | 'Ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'Ñ' | 'Ń' | 'Ņ' | 'Ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø'
        | 'Ō' | 'Ŏ' | 'Ő' => "o",
        'ŕ' | 'ŗ' | 'ř' | 'Ŕ' | 'Ŗ' | 'Ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'Ś' | 'Ŝ' | 'Ş' | 'Š' => "s",
        'ţ' | 'ť' | 'ŧ' | 'Ţ' | 'Ť' | 'Ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ'
        | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "u",
        'ý' | 'ÿ' | 'ŷ' | 'Ý' | 'Ÿ' | 'Ŷ' => "y",
        'ź' | 'ż' | 'ž' | 'Ź' | 'Ż' | 'Ž' => "z",
        'æ' | 'Æ' => "ae",
        'œ' | 'Œ' => "oe",
        'ß' => "ss",
        'þ' | 'Þ' => "th",
        'ð' | 'Ð' => "d",
        _ => return None,
    };
    Some(base)
}

fn is_kebab_valid(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    let mut prev_hyphen = false;
    for c in chars {
        if c == '-' {
            if prev_hyphen {
                return false;
            }
            prev_hyphen = true;
        } else if c.is_ascii_alphanumeric() {
            prev_hyphen = false;
        } else {
            return false;
        }
    }
    !prev_hyphen
}

pub fn allocate_skill_id(name: &str, taken: &[String]) -> String {
    let base = {
        let s = slugify_skill_id(name);
        if s.is_empty() { "skill".to_string() } else { s }
    };
    if !taken.contains(&base) {
        return base;
    }
    for n in 2..=9999usize {
        let suffix = format!("-{}", n);
        let max_base = 64usize.saturating_sub(suffix.len()).max(1);
        let trimmed_base = if base.len() > max_base {
            base[..max_base].trim_end_matches('-').to_string()
        } else {
            base.clone()
        };
        let candidate = format!("{}{}", trimmed_base, suffix);
        if is_kebab_valid(&candidate) && !taken.contains(&candidate) {
            return candidate;
        }
    }
    let millis = js_sys::Date::now() as u64;
    let mut candidate = format!("skill-{}", base36(millis));
    if candidate.len() > 64 {
        candidate = candidate[..64].to_string();
    }
    while candidate.ends_with('-') {
        candidate.pop();
    }
    candidate
}

fn base36(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = String::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize] as char);
        n /= 36;
    }
    out.chars().rev().collect()
}

pub fn list_skills_spec() -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::ListSkills).template)
}

pub fn get_skill_spec(id: &str) -> RequestSpec {
    RequestSpec::get(fill(endpoint(EndpointId::GetSkill).template, id))
}

pub fn create_skill_spec(draft: &SkillDraft) -> RequestSpec {
    RequestSpec::post(endpoint(EndpointId::CreateSkill).template).with_body(skill_body(draft, true))
}

pub fn update_skill_spec(id: &str, draft: &SkillDraft) -> RequestSpec {
    RequestSpec::put(fill(endpoint(EndpointId::UpdateSkill).template, id))
        .with_body(skill_body(draft, false))
}

pub fn delete_skill_spec(id: &str) -> RequestSpec {
    RequestSpec::delete(fill(endpoint(EndpointId::DeleteSkill).template, id))
}

#[component]
pub fn SkillLibraryPanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let rows = RwSignal::new(Vec::<SkillRowView>::new());
    let search = RwSignal::new(String::new());
    let state = RwSignal::new(SkillsView::Loading);
    let error = RwSignal::new(None::<String>);
    let form_open = RwSignal::new(false);
    let editing_id = RwSignal::new(None::<String>);
    let draft_name = RwSignal::new(String::new());
    let draft_description = RwSignal::new(String::new());
    let draft_content = RwSignal::new(String::new());
    let draft_autoinvoke = RwSignal::new(true);
    let draft_enabled = RwSignal::new(true);
    let form_error = RwSignal::new(None::<String>);
    let confirm = RwSignal::new(ConfirmState::default());
    // Focus returns here when a deleted row took its own Delete button with it.
    let row_fallback = NodeRef::<html::Span>::new();

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            // A list refresh invalidates any arm.
            confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Cancel).state);
            state.set(SkillsView::Loading);
            leptos::task::spawn_local(async move {
                match api_request::request::<SkillsPage>(&list_skills_spec(), &auth).await {
                    Ok(page) => {
                        state.set(skills_view(&page));
                        rows.set(skills_view_rows(&page));
                    }
                    Err(failure) => state.set(skills_error_view(&failure.to_string())),
                }
            });
        }
    });

    Effect::new(move |_| reload.run(()));

    let open_create = Callback::new(move |_: ()| {
        editing_id.set(None);
        draft_name.set(String::new());
        draft_description.set(String::new());
        draft_content.set(String::new());
        draft_autoinvoke.set(true);
        draft_enabled.set(true);
        form_error.set(None);
        form_open.set(true);
    });

    let open_edit = Callback::new({
        let auth = auth.clone();
        move |id: String| {
            let auth = auth.clone();
            let id2 = id.clone();
            leptos::task::spawn_local(async move {
                match api_request::request::<SkillItem>(&get_skill_spec(&id2), &auth).await {
                    Ok(item) => {
                        editing_id.set(Some(item.id.clone()));
                        draft_name.set(item.name.clone());
                        draft_description.set(item.description.clone());
                        draft_content.set(item.content.clone());
                        draft_autoinvoke.set(item.autoinvoke);
                        draft_enabled.set(item.enabled);
                        form_error.set(None);
                        form_open.set(true);
                    }
                    Err(failure) => {
                        error.set(Some(failure.to_string()));
                    }
                }
            });
        }
    });

    let save = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            let taken = rows
                .get_untracked()
                .iter()
                .map(|r| r.id.clone())
                .collect::<Vec<_>>();
            let editing = editing_id.get_untracked();
            let is_edit = editing.is_some();
            let id = match editing {
                Some(existing) => existing,
                None => allocate_skill_id(&draft_name.get_untracked(), &taken),
            };
            let draft = SkillDraft {
                id: id.clone(),
                name: draft_name.get_untracked(),
                description: draft_description.get_untracked(),
                content: draft_content.get_untracked(),
                autoinvoke: draft_autoinvoke.get_untracked(),
                enabled: draft_enabled.get_untracked(),
            };
            if let Err(e) = validate_draft(&draft) {
                form_error.set(Some(match e {
                    DraftError::NameRequired => "name required".to_string(),
                    DraftError::ContentRequired => "content required".to_string(),
                }));
                return;
            }
            let spec = if is_edit {
                update_skill_spec(&id, &draft)
            } else {
                create_skill_spec(&draft)
            };
            leptos::task::spawn_local(async move {
                let res: Result<serde_json::Value, ApiError> =
                    api_request::request(&spec, &auth).await;
                match res {
                    Ok(_) => {
                        form_open.set(false);
                        // The write is only half the story: the list is the
                        // screen's state, so a successful mutation re-reads it.
                        reload.run(());
                    }
                    Err(failure) => form_error.set(Some(failure.to_string())),
                }
            });
        }
    });

    // The only path from a confirmed gate to the network.
    let request_delete = Callback::new({
        let auth = auth.clone();
        move |id: String| {
            let auth = auth.clone();
            let spec = delete_skill_spec(&id);
            leptos::task::spawn_local(async move {
                let result = api_request::request::<serde_json::Value>(&spec, &auth).await;
                // `Finished` on EVERY exit path: success, HTTP error, transport
                // error. A transient failure must not kill the row's Delete.
                confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Finished).state);
                match result {
                    Ok(_) => {
                        error.set(None);
                        reload.run(());
                    }
                    // A refused delete (404, offline, …) must be visible: the
                    // row would otherwise stay on screen with no explanation.
                    Err(failure) => error.set(Some(failure.to_string())),
                }
            });
        }
    });

    let cancel = Callback::new(move |_: ()| {
        form_open.set(false);
    });

    let form_id_display = move || {
        if let Some(id) = editing_id.get() {
            id
        } else {
            let name = draft_name.get();
            if name.trim().is_empty() {
                String::new()
            } else {
                allocate_skill_id(
                    &name,
                    &rows
                        .get_untracked()
                        .iter()
                        .map(|r| r.id.clone())
                        .collect::<Vec<_>>(),
                )
            }
        }
    };

    let visible_rows = move || filter_skills(&rows.get(), &search.get());

    view! {
        <section class="skill-library-panel" data-testid="skill-library-panel">
            <h2>"Skill Library"</h2>
            <Input
                id="skill-search"
                name="q"
                class="skill-search"
                attr:data-testid="skill-search"
                attr:aria-label="Search skills"
                placeholder="Search skills"
                value=search
            />
            <span node_ref=row_fallback>
                <Button
                    class="skill-create"
                    attr:data-testid="skill-create"
                    on_click=move |_| open_create.run(())
                >
                    "New skill"
                </Button>
            </span>

            {move || match state.get() {
                SkillsView::Loading => view! {
                    <StateBlock
                        kind=BlockKind::Loading
                        message="Loading…"
                        class="state-loading"
                        testid="skills-loading"
                    />
                }
                .into_any(),
                SkillsView::Empty => view! {
                    <StateBlock
                        kind=BlockKind::Empty
                        message="No skills"
                        class="state-empty"
                        testid="skills-empty"
                    />
                }
                .into_any(),
                SkillsView::Error(msg) => view! {
                    <StateBlock
                        kind=BlockKind::Error
                        message=msg
                        class="state-error"
                        testid="skills-error"
                    />
                }
                .into_any(),
                SkillsView::Loaded { .. } => {
                    let filtered = visible_rows();
                    if filtered.is_empty() && rows.get().is_empty() {
                        view! {
                            <StateBlock
                                kind=BlockKind::Empty
                                message="No skills"
                                class="state-empty"
                                testid="skills-empty"
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <ul class="skill-list">
                                {filtered.into_iter().map(|row| {
                                    let id = row.id.clone();
                                    let id2 = row.id.clone();
                                    let row_id = row.id.clone();
                                    let delete_ref = NodeRef::<html::Span>::new();
                                    view! {
                                        <li class="skill-row" data-testid="skill-row">
                                            <code class="skill-id" data-testid="skill-id">{row.id.clone()}</code>
                                            <span class="skill-name" data-testid="skill-name">{row.name.clone()}</span>
                                            <span class="skill-description" data-testid="skill-description">{row.description.clone()}</span>
                                            <span class="skill-updated" data-testid="skill-updated">{row.updated_at.clone()}</span>
                                            {if row.enabled { view! { <Tag class="skill-enabled-badge" attr:data-testid="skill-enabled-badge">"enabled"</Tag> }.into_any() } else { view! { <span></span> }.into_any() }}
                                            {if row.autoinvoke { view! { <Tag class="skill-autoinvoke-badge" attr:data-testid="skill-autoinvoke-badge">"auto"</Tag> }.into_any() } else { view! { <span></span> }.into_any() }}
                                            <Button
                                                class="skill-edit"
                                                attr:data-testid="skill-edit"
                                                attr:aria-label=format!("Edit {}", id)
                                                on_click=move |_| open_edit.run(id2.clone())
                                            >
                                                "Edit"
                                            </Button>
                                            <span class="skill-delete" node_ref=delete_ref>
                                                <ConfirmDeleteButton
                                                    confirm=confirm
                                                    id=row_id.clone()
                                                    invoker=delete_ref
                                                    fallback=row_fallback
                                                    testid="skill-delete"
                                                    aria_label=format!("Delete {row_id}")
                                                    on_run=request_delete
                                                />
                                            </span>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }
                }
            }}

            {move || error.get().map(|m| view! {
                <StateBlock
                    kind=BlockKind::Error
                    message=m
                    class="state-error"
                    testid="skills-error"
                />
            })}

            {move || if form_open.get() {
                view! {
                    <Card>
                        <form class="skill-form" data-testid="skill-form"
                            on:submit=move |ev| {
                                // A form with no submit control still fires an
                                // implicit submit on Enter; keep it on the page
                                // (the Save button carries the write).
                                ev.prevent_default();
                            }>
                            <code class="skill-form-id" data-testid="skill-form-id">{form_id_display()}</code>
                            <label for="skill-name">"Name"</label>
                            <Input
                                id="skill-name"
                                class="skill-form-name"
                                attr:data-testid="skill-form-name"
                                value=draft_name
                            />
                            <label for="skill-description">"Description"</label>
                            <Input
                                id="skill-description"
                                class="skill-form-description"
                                attr:data-testid="skill-form-description"
                                value=draft_description
                            />
                            <label for="skill-content">"Content"</label>
                            <textarea id="skill-content" class="skill-form-content" data-testid="skill-form-content"
                                prop:value=move || draft_content.get()
                                on:input=move |e| draft_content.set(event_target_value(&e)) />
                            <label>
                                <input type="checkbox" id="skill-autoinvoke" name="autoinvoke"
                                    class="skill-form-autoinvoke" data-testid="skill-form-autoinvoke"
                                    prop:checked=move || draft_autoinvoke.get()
                                    on:change=move |e| draft_autoinvoke.set(event_target_checked(&e)) />
                                "Auto-invoke"
                            </label>
                            <label>
                                <input type="checkbox" id="skill-enabled" name="enabled"
                                    class="skill-form-enabled" data-testid="skill-form-enabled"
                                    prop:checked=move || draft_enabled.get()
                                    on:change=move |e| draft_enabled.set(event_target_checked(&e)) />
                                "Enabled"
                            </label>
                            {move || form_error.get().map(|m| view! { <p class="skill-form-error">{m}</p> })}
                            <Button
                                class="skill-save"
                                attr:data-testid="skill-save"
                                button_type=ButtonType::Button
                                on_click=move |_| save.run(())
                            >
                                "Save skill"
                            </Button>
                            <Button
                                class="skill-cancel"
                                attr:data-testid="skill-cancel"
                                button_type=ButtonType::Button
                                on_click=move |_| cancel.run(())
                            >
                                "Cancel"
                            </Button>
                        </form>
                    </Card>
                }.into_any()
            } else {
                view! { <span></span> }.into_any()
            }}
        </section>
    }
}

fn skills_view_rows(page: &SkillsPage) -> Vec<SkillRowView> {
    match skills_view(page) {
        SkillsView::Loaded { rows } => rows,
        _ => Vec::new(),
    }
}

fn event_target_checked(ev: &leptos::ev::Event) -> bool {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.checked())
        .unwrap_or(false)
}
