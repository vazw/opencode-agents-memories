//! The screen state rendering.
//!
//! `ScreenState` lives in [`crate::nav`]; this module renders it and owns the
//! placeholder notice. `Ready` renders the shell landing panel, `Planned`
//! renders a documented "not yet implemented" notice naming the owning wave and
//! listing the endpoints the screen will consume, and `Live` renders the real
//! screen — today that is the W6.3 session-history timeline, the W6.5
//! portability and skills screens, the four W6.7 core screens, and the W6.6
//! `Agents` panel, which renders its own documented unavailable state rather
//! than a planned notice.
//!
//! Security: the notice interpolates only compile-time constants from the route
//! table and the path table. The session screen builds its two paths from the
//! pinned [`crate::paths::ENDPOINTS`] templates with [`encode_component`], so a project id or
//! session id is always one percent-encoded segment and can never become a
//! path, a scope or an authority. The crate has no unsanitised-HTML path at
//! all.

use leptos::callback::Callable;
use leptos::prelude::*;
use serde::Deserialize;

use crate::agents::{
    AgentsPanel, PROJECTS_UNAVAILABLE_MESSAGE, ProjectRow, ProjectsOutcome, load_projects,
    project_option_label, selected_project,
};
use crate::api::auth::{AuthContext, read_marker_token};
use crate::api::envelope::{ApiEnvelope, ApiError};
use crate::api_request::{self, RequestSpec};
use crate::confirm::{ConfirmEvent, ConfirmGate, ConfirmState, confirm_next, dispatch};
use crate::core_screens::{MemoriesPanel, ProfilePanel, SettingsPanel, StatsPanel, TagsPanel};
use crate::export_import::ExportImportPanel;
use crate::nav::ScreenState;
use crate::paths::{EndpointId, encode_component, project_path};
use crate::routes::{Owner, RouteId};
use crate::skill_library::SkillLibraryPanel;
use crate::state_block::{BlockKind, SessionState, StateBlock, session_state};
use thaw::{Badge, BadgeAppearance, BadgeColor, Button};

/// The pure placeholder description for a screen state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notice {
    pub owner: Owner,
    pub endpoints: &'static [&'static str],
}

/// `None` for a screen that is ready or live; `Some` for a planned screen.
pub fn notice_for(state: &ScreenState) -> Option<Notice> {
    match *state {
        ScreenState::Ready | ScreenState::Live { .. } => None,
        ScreenState::Planned { owner, endpoints } => Some(Notice { owner, endpoints }),
    }
}

/// Render the screen body for a resolved route.
///
/// The route is needed in addition to the state because several routes are
/// `Live`: each one dispatches to its own panel. A `Planned` route renders
/// the notice, and `Ready` renders the shell landing panel.
#[component]
pub fn ScreenBody(route: RouteId, state: ScreenState) -> impl IntoView {
    match state {
        ScreenState::Ready => view! { <HomePanel/> }.into_any(),
        ScreenState::Planned { owner, endpoints } => {
            let owner_label = owner.label();
            view! {
                <section class="planned-notice" data-testid="planned-notice">
                    <h2>"Not yet implemented"</h2>
                    <p>
                        "This screen is owned by " <strong>{owner_label}</strong> " and lands in that wave."
                    </p>
                    <p>"It will consume:"</p>
                    <ul class="planned-endpoints">
                        {endpoints
                            .iter()
                            .map(|endpoint| view! { <li><code>{*endpoint}</code></li> })
                            .collect_view()}
                    </ul>
                </section>
            }
            .into_any()
        }
        ScreenState::Live { .. } => match route {
            RouteId::Home => view! { <HomePanel/> }.into_any(),
            RouteId::ProjectMemories => view! { <MemoriesPanel/> }.into_any(),
            RouteId::Tags => view! { <TagsPanel/> }.into_any(),
            RouteId::Profile => view! { <ProfilePanel/> }.into_any(),
            RouteId::Settings => view! { <SettingsPanel/> }.into_any(),
            RouteId::SessionHistory => view! { <SessionHistoryPanel/> }.into_any(),
            RouteId::Agents => view! { <AgentsPanel/> }.into_any(),
            RouteId::SkillLibrary => view! { <SkillLibraryPanel/> }.into_any(),
            RouteId::ExportImport => view! { <ExportImportPanel/> }.into_any(),
            RouteId::Stats => view! { <StatsPanel/> }.into_any(),
        },
    }
}

// ---------------------------------------------------------------------------
// Home panel (extracted from the Ready arm)
// ---------------------------------------------------------------------------

#[component]
pub fn HomePanel() -> impl IntoView {
    view! {
        <section class="home-panel" data-testid="home-panel">
            <h2>"OpenCode Memory Explorer"</h2>
            <p>
                "Browse the memories, prompts and skills stored for this project. \
                 Pick a screen from the navigation."
            </p>
        </section>
    }
}

// ---------------------------------------------------------------------------
// W6.3 — the session-history screen
// ---------------------------------------------------------------------------

/// One timeline row, in the exact shape `GET …/sessions` answers
/// (`SessionSummary`, camelCase; unknown keys are ignored).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    /// Row id (the delete parameter).
    pub id: String,
    /// Display name from the payload.
    pub agent_name: String,
    /// Human-readable summary.
    pub summary: String,
    /// Creation time (unix millis).
    pub created_at: i64,
    /// Whether the parent link was severed by a delete (D-12).
    pub orphaned: bool,
}

/// Fill `{project_id}` (and, when given, `{session_id}`) of a pinned template
/// with percent-encoded components.
///
/// [`crate::paths::fill`] only knows `{id}`; the project-scoped templates are
/// filled by [`crate::paths::project_path`], which `session_path` delegates
/// to — the values still go through [`encode_component`], the same RFC 3986
/// encoder the single-parameter templates use. The session id, when present,
/// is this screen's own parameter and is appended here.
pub fn session_path(endpoint_id: EndpointId, project_id: &str, session_id: Option<&str>) -> String {
    let mut path = project_path(endpoint_id, project_id);
    if let Some(session_id) = session_id {
        path = path.replace("{session_id}", &encode_component(session_id));
    }
    path
}

/// The `GET /api/projects/{project_id}/sessions` request.
pub fn list_sessions_spec(project_id: &str) -> RequestSpec {
    RequestSpec::get(session_path(
        EndpointId::ListProjectSessions,
        project_id,
        None,
    ))
}

/// The `DELETE /api/projects/{project_id}/sessions/{session_id}` request.
pub fn delete_session_spec(project_id: &str, session_id: &str) -> RequestSpec {
    RequestSpec::delete(session_path(
        EndpointId::DeleteProjectSession,
        project_id,
        Some(session_id),
    ))
}

/// Decode the delete answer.
///
/// `DELETE …/sessions/{session_id}` answers `{"success":true}` with **no**
/// `data` key, while [`api_request::request`] requires a `data` payload on
/// every 2xx. This parser accepts the documented data-less success and maps
/// every other answer through the same bounded [`ApiError`] the shared seam
/// produces — no body byte is ever surfaced.
pub fn delete_outcome(status: u16, bytes: &[u8]) -> Result<(), ApiError> {
    let envelope: ApiEnvelope<serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|_| ApiError::Malformed)?;
    if (200..300).contains(&status) && envelope.success {
        return Ok(());
    }
    let message = envelope
        .error
        .or(envelope.message)
        .unwrap_or_else(|| "request failed".to_string());
    Err(ApiError::server(status, message))
}

/// Delete one session through the shared request seam.
///
/// The body is decoded by [`delete_outcome`] instead of
/// [`api_request::finish`] because this endpoint's success envelope carries no
/// `data`; the request itself is prepared by [`api_request::prepare`], so the
/// same-origin mode, credentials and token header are unchanged.
pub async fn delete_session(
    project_id: &str,
    session_id: &str,
    auth: &AuthContext,
) -> Result<(), ApiError> {
    let spec = delete_session_spec(project_id, session_id);
    let (url, init) = api_request::prepare::<serde_json::Value>(&spec, auth)?;

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
    delete_outcome(status, &bytes)
}

/// Load the project timeline into the screen's signals.
///
/// Sets `loading` on issue and clears it on settle — never on the delete path.
fn load_rows(
    spec: RequestSpec,
    auth: AuthContext,
    rows: RwSignal<Vec<SessionRow>>,
    error: RwSignal<Option<String>>,
    loading: RwSignal<bool>,
) {
    loading.set(true);
    leptos::task::spawn_local(async move {
        match api_request::request::<Vec<SessionRow>>(&spec, &auth).await {
            Ok(found) => {
                error.set(None);
                rows.set(found);
            }
            Err(failure) => {
                rows.set(Vec::new());
                error.set(Some(failure.to_string()));
            }
        }
        loading.set(false);
    });
}

/// One timeline row with its delete arm (the confirm gate swaps in place).
fn session_row_view(
    row: SessionRow,
    confirm: RwSignal<ConfirmState>,
    run_delete: Callback<String>,
) -> impl IntoView {
    let row_id = row.id.clone();
    let orphaned = row.orphaned;
    view! {
        <li class="session-row" data-testid="session-row">
            <span class="session-id">{row.id.clone()}</span>
            <span class="session-agent">{row.agent_name.clone()}</span>
            <span class="session-summary">{row.summary.clone()}</span>
            <span class="session-created">
                {format_created_at(row.created_at)}
            </span>
            {orphaned
                .then(|| {
                    // thaw 0.4.8's Badge spreads no attrs either (its root is
                    // class_list only, F4): the wrapper span carries the
                    // testid, no class (no census impact).
                    view! {
                        <span data-testid="orphaned-badge">
                            <Badge appearance=BadgeAppearance::Outline color=BadgeColor::Warning>
                                "orphaned"
                            </Badge>
                        </span>
                    }
                })}
            {move || {
                let armed = confirm.get().pending.as_deref()
                    == Some(row_id.as_str());
                if armed {
                    let on_confirm = Callback::new(move |_: ()| {
                        if let Some(run_id) = dispatch(
                            confirm,
                            ConfirmEvent::Confirm,
                        ) {
                            run_delete.run(run_id);
                        }
                    });
                    let on_cancel = Callback::new(move |_: ()| {
                        confirm
                            .set(
                                confirm_next(
                                    &confirm.get_untracked(),
                                    ConfirmEvent::Cancel,
                                )
                                    .state,
                            );
                    });
                    view! {
                        <ConfirmGate on_confirm=on_confirm on_cancel=on_cancel/>
                    }
                        .into_any()
                } else {
                    let ask_id = row_id.clone();
                    view! {
                        <Button
                            class="session-delete"
                            attr:data-testid="delete-session"
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
            }}
        </li>
    }
}

/// Format a unix-millis stamp as `YYYY-MM-DD HH:MM:SS UTC`.
///
/// The crate has no date library; the conversion is the civil-from-days
/// algorithm (Howard Hinnant), exact for every `i64` millisecond stamp, with
/// the day and time split by Euclidean division so pre-epoch stamps cannot
/// panic or wrap.
pub fn format_created_at(millis: i64) -> String {
    fn civil_from_days(days: i64) -> (i64, i64, i64) {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        (if month <= 2 { year + 1 } else { year }, month, day)
    }

    let seconds = millis.div_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

/// Extract the `project` query parameter from a URL search string.
pub fn initial_project(search: &str) -> Option<String> {
    let search = search.strip_prefix('?').unwrap_or(search);
    for pair in search.split(['&', ';']) {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        if key == "project" {
            let value = crate::paths::decode_component(value);
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// The timeline request for a project the user picked.
///
/// `Some` only when the id came out of the roster (`selected_project`): a typed
/// or deep-linked id that is not in the roster never reaches the network, and
/// an unknown request falls back to the roster's first row.
pub fn session_request(roster: &[ProjectRow], requested: Option<&str>) -> Option<RequestSpec> {
    selected_project(roster, requested).map(|id| list_sessions_spec(&id))
}

/// The `data-testid` of the roster-loading arm.
pub const SESSIONS_ROSTER_LOADING_TESTID: &str = "sessions-roster-loading";
/// The `data-testid` of the no-projects arm.
pub const SESSIONS_NO_PROJECTS_TESTID: &str = "sessions-no-projects";
/// The `data-testid` of the roster-unavailable arm (the 501 degrade).
pub const SESSIONS_ROSTER_UNAVAILABLE_TESTID: &str = "sessions-roster-unavailable";
/// The `data-testid` of the roster-failure arm.
pub const SESSIONS_ROSTER_ERROR_TESTID: &str = "sessions-roster-error";
/// The `data-testid` of the timeline-loading arm.
pub const SESSIONS_LOADING_TESTID: &str = "sessions-loading";
/// The `data-testid` of the timeline-empty arm.
pub const SESSIONS_EMPTY_TESTID: &str = "sessions-empty";
/// The fixed copy of the timeline-loading arm.
pub const SESSIONS_LOADING_MESSAGE: &str = "Loading timeline…";
/// The fixed copy of the timeline-empty arm.
pub const SESSIONS_EMPTY_MESSAGE: &str = "No sessions recorded for this project yet.";

/// The W6.3 session-history screen.
///
/// The project is picked from the `/api/projects` roster (L3), seeded by
/// `?project=`. The token comes from [`read_marker_token`] — the marker
/// `mem-web` injects into the shell — because the session routes are
/// token-gated even on loopback.
///
/// Wave 3 (issue 118): the screen's two layers split into distinct state arms.
/// The ROSTER layer maps the [`ProjectsOutcome`] one-to-one onto the
/// [`ProjectsView`] arms through the Agents screen's own classifier —
/// Loading / NoProjects / Unavailable / Failed / Ready — and only the Ready
/// arm renders the picker + timeline (a `NoProjects` or failed roster
/// suppresses the timeline entirely). The TIMELINE layer classifies through
/// [`session_state`] (error > loading > empty > ready): a timeline failure
/// renders `session-error` inside the picker arm and never disables the
/// picker, and the delete path never sets `sessions_loading` (a delete
/// settling after a load would pin the spinner).
#[component]
pub fn SessionHistoryPanel() -> impl IntoView {
    let project_id = RwSignal::new(String::new());
    let roster = RwSignal::new(Option::<ProjectsOutcome>::None);
    let rows = RwSignal::new(Vec::<SessionRow>::new());
    let error = RwSignal::new(None::<String>);
    let sessions_loading = RwSignal::new(false);
    let confirm = RwSignal::new(ConfirmState::default());
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };

    let auth_for_reload = auth.clone();
    let reload = Callback::new(move |_: ()| {
        // A timeline refresh invalidates any arm.
        confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Cancel).state);
        let project = project_id.get_untracked();
        let outcome = roster.get_untracked();
        let roster_rows = match &outcome {
            Some(ProjectsOutcome::Loaded(rows)) => rows.clone(),
            _ => Vec::new(),
        };
        match session_request(&roster_rows, Some(project.as_str())) {
            Some(spec) => load_rows(spec, auth_for_reload.clone(), rows, error, sessions_loading),
            None => {
                rows.set(Vec::new());
                error.set(Some("No project selected".to_string()));
            }
        }
    });

    // The only path from a confirmed gate to the network. The delete path
    // NEVER touches `sessions_loading`.
    let run_delete = Callback::new({
        let auth = auth.clone();
        move |session_id: String| {
            let project = project_id.get_untracked();
            let auth = auth.clone();
            leptos::task::spawn_local(async move {
                let result = delete_session(&project, &session_id, &auth).await;
                // `Finished` on EVERY exit path: success, HTTP error, transport
                // error. A transient failure must not kill the row's Delete.
                confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Finished).state);
                match result {
                    Ok(()) => {
                        error.set(None);
                        reload.run(());
                    }
                    Err(failure) => error.set(Some(failure.to_string())),
                }
            });
        }
    });

    // The roster loads on mount; the `?project=` seed picks from it.
    Effect::new({
        let auth = auth.clone();
        move |_| {
            let seed = initial_project(
                &web_sys::window()
                    .and_then(|w| w.location().search().ok())
                    .unwrap_or_default(),
            );
            let auth = auth.clone();
            leptos::task::spawn_local(async move {
                let loaded = load_projects(&auth).await;
                let loaded_rows = match &loaded {
                    ProjectsOutcome::Loaded(rows) => rows.clone(),
                    _ => Vec::new(),
                };
                match selected_project(&loaded_rows, seed.as_deref()) {
                    Some(id) => {
                        roster.set(Some(loaded));
                        project_id.set(id);
                        reload.run(());
                    }
                    None => {
                        // Nothing to pick (empty roster, unavailable or
                        // failed): the roster arm renders the state and
                        // suppresses the timeline layer entirely.
                        roster.set(Some(loaded));
                    }
                }
            });
        }
    });

    view! {
        <section class="session-history" data-testid="session-history">
            <h2>"Session history"</h2>
            <p class="session-hint">
                "Sessions are project-scoped. Pick a project to list its timeline."
            </p>
            {move || match roster.get().as_ref() {
                None => view! {
                    <StateBlock
                        kind=BlockKind::Loading
                        message="Loading projects…"
                        class="state-loading"
                        testid=SESSIONS_ROSTER_LOADING_TESTID
                    />
                }
                    .into_any(),
                Some(ProjectsOutcome::Unavailable) => view! {
                    <StateBlock
                        kind=BlockKind::Error
                        message=PROJECTS_UNAVAILABLE_MESSAGE
                        class="state-error"
                        testid=SESSIONS_ROSTER_UNAVAILABLE_TESTID
                    />
                }
                    .into_any(),
                Some(ProjectsOutcome::Loaded(loaded)) if loaded.is_empty() => view! {
                    <StateBlock
                        kind=BlockKind::Empty
                        message="No projects recorded yet."
                        class="state-empty"
                        testid=SESSIONS_NO_PROJECTS_TESTID
                    />
                }
                    .into_any(),
                Some(ProjectsOutcome::Failed(message)) => view! {
                    <StateBlock
                        kind=BlockKind::Error
                        message=message.clone()
                        class="state-error"
                        testid=SESSIONS_ROSTER_ERROR_TESTID
                    />
                }
                    .into_any(),
                Some(ProjectsOutcome::Loaded(loaded_rows)) => {
                    let roster_rows = loaded_rows.clone();
                    let timeline = move || {
                        // The timeline layer: one arm at a time through the
                        // pure classifier. A timeline failure renders INSIDE
                        // the picker arm — the picker stays usable. The row
                        // buffer is the live signal (the classifier reads the
                        // CURRENT rows, not the roster closure's copy). Every
                        // read below is TRACKED and the closure is passed as a
                        // reactive child (`{timeline}`, never `{timeline()}`):
                        // an invocation would paint one static node and never
                        // re-run, pinning `sessions-loading` forever once the
                        // roster is Ready (review-1 Finding 1).
                        let current_rows = rows.get();
                        let timeline = session_state(
                            &current_rows,
                            error.get().as_deref(),
                            sessions_loading.get(),
                        );
                        match timeline {
                            SessionState::Loading => view! {
                                <StateBlock
                                    kind=BlockKind::Loading
                                    message=SESSIONS_LOADING_MESSAGE
                                    class="state-loading"
                                    testid=SESSIONS_LOADING_TESTID
                                />
                            }
                                .into_any(),
                            SessionState::Empty => view! {
                                <StateBlock
                                    kind=BlockKind::Empty
                                    message=SESSIONS_EMPTY_MESSAGE
                                    class="state-empty"
                                    testid=SESSIONS_EMPTY_TESTID
                                />
                            }
                                .into_any(),
                            SessionState::Error(message) => view! {
                                <StateBlock
                                    kind=BlockKind::Error
                                    message=message
                                    class="state-error"
                                    testid="session-error"
                                />
                            }
                                .into_any(),
                            SessionState::Ready(ready) => view! {
                                <ul class="session-list" data-testid="session-list">
                                    {ready
                                        .into_iter()
                                        .map(|row| session_row_view(row, confirm, run_delete))
                                        .collect_view()}
                                </ul>
                            }
                                .into_any(),
                        }
                    };
                    view! {
                        <div class="session-controls">
                            <label class="session-select-label" for="sessions-project-select">
                                "Project"
                            </label>
                            <select
                                id="sessions-project-select"
                                data-testid="sessions-project-select"
                                prop:value=move || project_id.get()
                                on:change=move |event| {
                                    project_id.set(event_target_value(&event));
                                    reload.run(());
                                }
                            >
                                {roster_rows
                                    .iter()
                                    .map(|row| {
                                        view! {
                                            <option value=row.project_id.clone()>
                                                {project_option_label(row)}
                                            </option>
                                        }
                                    })
                                    .collect_view()}
                            </select>
                            <Button
                                class="load-sessions"
                                attr:data-testid="load-sessions"
                                on_click=move |_| reload.run(())
                            >
                                "Load timeline"
                            </Button>
                        </div>
                        {timeline}
                    }
                        .into_any()
                }
            }}
        </section>
    }
}
