//! The Export / Import screen (W6.5).
use leptos::callback::Callable;
use leptos::prelude::*;

use crate::api::auth::{AuthContext, read_marker_token};
use crate::api_request::{self, RequestSpec};
use crate::core_screens::{
    MemoryItem, MemoryPage, ProfileData, ProfileItem, ProfileView, TagInfo, TagsPage,
};
use crate::paths::{EndpointId, endpoint};

pub const EXPLORER_EXPORT_KIND: &str = "opencode-mem-explorer-export";
pub const MAX_IMPORT_BYTES: usize = 100 * 1024 * 1024;
pub const IMPORT_APPLY_SUPPORTED: bool = false;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    ExplorerExport,
    PortabilityBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportError {
    Malformed,
    Unrecognized,
    UnsupportedVersion,
    MissingMemories,
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPreview {
    pub format: ImportFormat,
    pub memories: usize,
    pub tags: usize,
    pub profile: bool,
    pub milestones: usize,
    pub staging: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone)]
pub struct ExportInputs<'a> {
    pub memories: &'a [MemoryItem],
    pub tags: &'a TagsPage,
    pub profile: Option<&'a ProfileView>,
    pub truncated: bool,
}

/// The rows the export document actually contains: prompt rows never travel
/// (the walk already sends `includePrompts=false`; this is the row-level
/// guarantee). The panel's `export-preview` count reads the same function, so
/// the preview cannot disagree with the document it previews.
pub fn exportable_memories(memories: &[MemoryItem]) -> Vec<&MemoryItem> {
    memories.iter().filter(|m| !m.is_prompt()).collect()
}

/// One `TagInfo` as the export document spells it (camelCase, `null` for
/// absent optionals — the decode shape of `GET /api/tags`).
///
/// Built here rather than by deriving `Serialize` on `core_screens`: the
/// document is this module's contract, and the screen models stay decode-only.
fn tag_to_json(tag: &TagInfo) -> serde_json::Value {
    serde_json::json!({
        "tag": tag.tag,
        "displayName": tag.display_name,
        "userName": tag.user_name,
        "userEmail": tag.user_email,
        "projectPath": tag.project_path,
        "projectName": tag.project_name,
        "gitRepoUrl": tag.git_repo_url,
    })
}

/// One profile item as the document spells it.
fn profile_item_to_json(item: &ProfileItem) -> serde_json::Value {
    serde_json::json!({
        "description": item.description,
        "category": item.category,
        "confidence": item.confidence,
        "frequency": item.frequency,
        "steps": item.steps,
    })
}

/// `profileData` — the three learned sections.
fn profile_data_to_json(data: &ProfileData) -> serde_json::Value {
    serde_json::json!({
        "preferences": data.preferences.iter().map(profile_item_to_json).collect::<Vec<_>>(),
        "patterns": data.patterns.iter().map(profile_item_to_json).collect::<Vec<_>>(),
        "workflows": data.workflows.iter().map(profile_item_to_json).collect::<Vec<_>>(),
    })
}

/// The whole `GET /api/user-profile` answer as the document spells it.
fn profile_to_json(profile: &ProfileView) -> serde_json::Value {
    serde_json::json!({
        "exists": profile.exists,
        "id": profile.id,
        "userId": profile.user_id,
        "displayName": profile.display_name,
        "userName": profile.user_name,
        "userEmail": profile.user_email,
        "version": profile.version,
        "createdAt": profile.created_at,
        "lastAnalyzedAt": profile.last_analyzed_at,
        "totalPromptsAnalyzed": profile.total_prompts_analyzed,
        "profileData": profile.profile_data.as_ref().map(profile_data_to_json),
        "message": profile.message,
    })
}

pub fn export_document(input: &ExportInputs<'_>, exported_at: &str) -> serde_json::Value {
    let memories: Vec<serde_json::Value> = exportable_memories(input.memories)
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "content": m.content,
                "type": m.memory_type,
                "tags": m.tags,
                "createdAt": m.created_at,
                "isPinned": m.is_pinned,
            })
        })
        .collect();
    let tags = serde_json::json!({
        "project": input.tags.project.iter().map(tag_to_json).collect::<Vec<_>>(),
        "user": input.tags.user.iter().map(tag_to_json).collect::<Vec<_>>(),
    });
    let profile = match input.profile {
        Some(p) => profile_to_json(p),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "kind": EXPLORER_EXPORT_KIND,
        "schemaVersion": 1,
        "exportedAt": exported_at,
        "source": { "container": "all-visible", "route": "/export-import" },
        "memories": memories,
        "tags": tags,
        "profile": profile,
        "truncated": input.truncated,
    })
}

pub fn export_file_name(exported_at: &str) -> String {
    format!("opencode-mem-export-{}.json", exported_at)
}

/// Format an epoch-milliseconds instant as the export stamp
/// `YYYYMMDDTHHMMSSZ` (UTC).
///
/// Pure on purpose: the browser supplies `js_sys::Date::now()`, and the
/// formatter is unit-tested against fixed instants. The civil-date conversion
/// is Howard Hinnant's `civil_from_days` (proleptic Gregorian, valid for
/// every instant this screen can produce).
pub fn format_exported_at(epoch_millis: u64) -> String {
    let secs = epoch_millis / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// Days since 1970-01-01 -> `(year, month, day)`, proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The export preview line.
///
/// The walk pages up to 25×200 memories, so a count rendered before the walk
/// settles is a lie that also enables the Download button (U7): an early click
/// exported an almost-empty file.
pub fn export_preview_text(loaded: bool, count: usize) -> String {
    if loaded {
        format!("memories: {count}")
    } else {
        "Loading export…".to_string()
    }
}

/// Download is disabled until **all three** loads (memories, tags, profile)
/// have settled — success or error, both settle.
pub fn export_download_disabled(loaded: bool) -> bool {
    !loaded
}

/// The download's stamp: the browser clock, formatted by the pure formatter.
fn export_stamp_now() -> String {
    format_exported_at(js_sys::Date::now() as u64)
}

pub fn export_pages_spec(page: usize, page_size: usize) -> RequestSpec {
    RequestSpec::get(endpoint(EndpointId::ListMemories).template)
        .with_query("page", page.to_string())
        .with_query("pageSize", page_size.to_string())
        .with_query("includePrompts", "false")
}

pub fn should_continue(page: usize, page_size: usize, received: usize, total_pages: usize) -> bool {
    if received < page_size {
        return false;
    }
    if page >= total_pages {
        return false;
    }
    if page >= 25 {
        return false;
    }
    true
}

pub fn check_import_len(len: usize) -> Result<(), ImportError> {
    if len > MAX_IMPORT_BYTES {
        return Err(ImportError::TooLarge);
    }
    Ok(())
}

#[allow(clippy::collapsible_if)]
pub fn import_preview(bytes: &[u8]) -> Result<ImportPreview, ImportError> {
    check_import_len(bytes.len())?;
    let text = std::str::from_utf8(bytes).map_err(|_| ImportError::Malformed)?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| ImportError::Malformed)?;

    if let Some(sv) = value.get("schemaVersion")
        && sv.as_u64() != Some(1)
    {
        return Err(ImportError::UnsupportedVersion);
    }

    let has_plugin = value.get("plugin").is_some();
    if has_plugin {
        let memories = value
            .get("memories")
            .and_then(|m| m.as_array())
            .ok_or(ImportError::MissingMemories)?
            .len();
        let milestones = value
            .get("milestones")
            .and_then(|m| m.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let staging = value
            .get("staging")
            .and_then(|m| m.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let profile = value.get("profiles").is_some()
            || value.get("profile").map(|p| !p.is_null()).unwrap_or(false);
        return Ok(ImportPreview {
            format: ImportFormat::PortabilityBundle,
            memories,
            tags: 0,
            profile,
            milestones,
            staging,
            skipped: 0,
        });
    }

    let kind = value.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    if kind != EXPLORER_EXPORT_KIND {
        return Err(ImportError::Unrecognized);
    }

    let memories_arr = value
        .get("memories")
        .and_then(|m| m.as_array())
        .ok_or(ImportError::MissingMemories)?;
    let memories = memories_arr.len();
    let mut skipped = 0usize;
    for m in memories_arr {
        if m.get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            skipped += 1;
        }
    }
    let tags = value
        .get("tags")
        .map(|t| {
            let p = t
                .get("project")
                .and_then(|a| a.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let u = t
                .get("user")
                .and_then(|a| a.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            p + u
        })
        .unwrap_or(0);
    let profile = value.get("profile").map(|p| !p.is_null()).unwrap_or(false);
    let milestones = value
        .get("milestones")
        .and_then(|m| m.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let staging = value
        .get("staging")
        .and_then(|m| m.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    Ok(ImportPreview {
        format: ImportFormat::ExplorerExport,
        memories,
        tags,
        profile,
        milestones,
        staging,
        skipped,
    })
}

#[component]
pub fn ExportImportPanel() -> impl IntoView {
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };
    let memories = RwSignal::new(Vec::<MemoryItem>::new());
    let tags = RwSignal::new(TagsPage {
        project: vec![],
        user: vec![],
    });
    let profile = RwSignal::new(None::<ProfileView>);
    let truncated = RwSignal::new(false);
    // U7: `loaded` is derived from three settled tasks; the generation counter
    // keeps a superseded reload from publishing a half-filled export.
    let settled = RwSignal::new(0u8);
    let generation = RwSignal::new(0u64);
    let loaded = Memo::new(move |_| settled.get() >= 3);
    let import_text = RwSignal::new(String::new());
    let import_preview_result = RwSignal::new(None::<ImportPreview>);
    let import_error = RwSignal::new(None::<String>);

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let auth = auth.clone();
            // Fail closed on every reload.
            let generation_now = generation.get_untracked() + 1;
            generation.set(generation_now);
            settled.set(0);
            let auth_m = auth.clone();
            leptos::task::spawn_local(async move {
                let mut all: Vec<MemoryItem> = Vec::new();
                let mut page = 1usize;
                let page_size = 200usize;
                let mut trunc = false;
                loop {
                    let spec = export_pages_spec(page, page_size);
                    match api_request::request::<MemoryPage>(&spec, &auth_m).await {
                        Ok(p) => {
                            all.extend(p.items.clone());
                            if !should_continue(
                                page,
                                page_size,
                                p.items.len(),
                                p.total_pages.max(1),
                            ) {
                                if page >= 25 {
                                    trunc = true;
                                }
                                break;
                            }
                            page += 1;
                            if page > 25 {
                                trunc = true;
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                memories.set(all);
                truncated.set(trunc);
                // Exactly once per task, on every exit path.
                if generation.get_untracked() == generation_now {
                    settled.update(|count| *count += 1);
                }
            });
            let auth_t = auth.clone();
            leptos::task::spawn_local(async move {
                let spec = RequestSpec::get(endpoint(EndpointId::ListTags).template);
                if let Ok(t) = api_request::request::<TagsPage>(&spec, &auth_t).await {
                    tags.set(t);
                }
                if generation.get_untracked() == generation_now {
                    settled.update(|count| *count += 1);
                }
            });
            let auth_p = auth.clone();
            leptos::task::spawn_local(async move {
                let spec = RequestSpec::get(endpoint(EndpointId::UserProfile).template);
                if let Ok(p) = api_request::request::<ProfileView>(&spec, &auth_p).await {
                    profile.set(Some(p));
                }
                if generation.get_untracked() == generation_now {
                    settled.update(|count| *count += 1);
                }
            });
        }
    });

    Effect::new(move |_| reload.run(()));

    let download = Callback::new(move |_: ()| {
        let mems = memories.get_untracked();
        let tgs = tags.get_untracked();
        let prof = profile.get_untracked();
        let input = ExportInputs {
            memories: &mems,
            tags: &tgs,
            profile: prof.as_ref(),
            truncated: truncated.get_untracked(),
        };
        let exported_at = export_stamp_now();
        let doc = export_document(&input, &exported_at);
        let json = serde_json::to_string(&doc).unwrap_or_default();
        let fname = export_file_name(&exported_at);
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            use web_sys::{Blob, BlobPropertyBag, Url};
            let bag = BlobPropertyBag::new();
            bag.set_type("application/json");
            let Ok(blob) = Blob::new_with_str_sequence_and_options(
                &js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(&json)),
                &bag,
            ) else {
                return;
            };
            let Ok(url) = Url::create_object_url_with_blob(&blob) else {
                return;
            };
            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(document) = window.document() else {
                return;
            };
            let Ok(anchor) = document.create_element("a") else {
                return;
            };
            let anchor: web_sys::HtmlAnchorElement = anchor.unchecked_into();
            anchor.set_href(&url);
            anchor.set_download(&fname);
            anchor.click();
            let _ = Url::revoke_object_url(&url);
        }
        let _ = json;
        let _ = fname;
    });

    let on_import_input = Callback::new(move |val: String| {
        import_text.set(val.clone());
        let bytes = val.as_bytes();
        match import_preview(bytes) {
            Ok(p) => {
                import_preview_result.set(Some(p));
                import_error.set(None);
            }
            Err(e) => {
                import_preview_result.set(None);
                import_error.set(Some(format!("{:?}", e)));
            }
        }
    });

    view! {
        <section class="export-import-panel" data-testid="export-import-panel">
            <h2>"Export / Import"</h2>
            <div class="export-preview" data-testid="export-preview">
                {move || export_preview_text(
                    loaded.get(),
                    exportable_memories(&memories.get()).len(),
                )}
            </div>
            <button type="button" class="export-download" data-testid="export-download"
                prop:disabled=move || export_download_disabled(loaded.get())
                on:click=move |_| download.run(())>
                "Download export"
            </button>
            <label for="import-json">"Import JSON"</label>
            <textarea id="import-json" class="import-input" data-testid="import-input"
                prop:value=move || import_text.get()
                on:input=move |e| on_import_input.run(event_target_value(&e)) />
            {move || import_preview_result.get().map(|p| view! {
                <div class="import-preview" data-testid="import-preview">
                    {format!("{}", p.memories)}
                </div>
            })}
            <div class="import-unsupported" data-testid="import-unsupported">
                "Import apply is not supported in the browser; use the server-side portability import."
            </div>
            {move || import_error.get().map(|m| view! {
                <p class="export-import-error" data-testid="export-import-error">{m}</p>
            })}
        </section>
    }
}
