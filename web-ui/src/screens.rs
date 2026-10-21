//! The screen state rendering.
//!
//! `ScreenState` lives in [`crate::nav`]; this module renders it and owns the
//! placeholder notice. `Ready` renders the shell landing panel, `Planned`
//! renders a documented "not yet implemented" notice naming the owning wave and
//! listing the endpoints the screen will consume, and `Live` renders the real
//! screen — today that is the W6.3 session-history timeline.
//!
//! Security: the notice interpolates only compile-time constants from the route
//! table and the path table. The session screen builds its two paths from the
//! pinned [`ENDPOINTS`] templates with [`encode_component`], so a project id or
//! session id is always one percent-encoded segment and can never become a
//! path, a scope or an authority. The crate has no unsanitised-HTML path at
//! all.

use leptos::callback::Callable;
use leptos::prelude::*;
use serde::Deserialize;

use crate::api::auth::{AuthContext, read_marker_token};
use crate::api::envelope::{ApiEnvelope, ApiError};
use crate::api_request::{self, RequestSpec};
use crate::core_screens::{MemoriesPanel, ProfilePanel, SettingsPanel, TagsPanel};
use crate::nav::ScreenState;
use crate::paths::{ENDPOINTS, EndpointId, EndpointSpec, encode_component};
use crate::routes::{Owner, RouteId};

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
/// `Live`: each one dispatches to its own panel (W6.7's four core screens plus
/// W6.3's session timeline). A `Planned` route renders the notice, and `Ready`
/// renders the shell landing panel.
#[component]
pub fn ScreenBody(route: RouteId, state: ScreenState) -> impl IntoView {
    match state {
        ScreenState::Ready => view! {
            <section class="home-panel" data-testid="home-panel">
                <h2>"OpenCode Memory Explorer"</h2>
                <p>
                    "This is the app shell. The navigation lists every planned screen; \
                     each one is owned by a later wave and says so on its own panel."
                </p>
            </section>
        }
        .into_any(),
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
            RouteId::ProjectMemories => view! { <MemoriesPanel/> }.into_any(),
            RouteId::Tags => view! { <TagsPanel/> }.into_any(),
            RouteId::Profile => view! { <ProfilePanel/> }.into_any(),
            RouteId::Settings => view! { <SettingsPanel/> }.into_any(),
            // `Live` + any other route is today only the W6.3 timeline; `Home`
            // never reaches here (`state_for` answers `Ready`).
            _ => view! { <SessionHistoryPanel/> }.into_any(),
        },
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

/// The pinned endpoint spec for an [`EndpointId`].
fn endpoint_spec(id: EndpointId) -> EndpointSpec {
    *ENDPOINTS
        .iter()
        .find(|spec| spec.id == id)
        .expect("the endpoint table has a row for every EndpointId")
}

/// Fill `{project_id}` (and, when given, `{session_id}`) of a pinned template
/// with percent-encoded components.
///
/// [`crate::paths::fill`] only knows `{id}`, so the two W6.3 templates get
/// their own filler — the values still go through [`encode_component`], the
/// same RFC 3986 encoder the single-parameter templates use.
pub fn session_path(endpoint: EndpointId, project_id: &str, session_id: Option<&str>) -> String {
    let spec = endpoint_spec(endpoint);
    let mut path = spec
        .template
        .replace("{project_id}", &encode_component(project_id));
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
fn load_rows(
    project_id: String,
    auth: AuthContext,
    rows: RwSignal<Vec<SessionRow>>,
    error: RwSignal<Option<String>>,
) {
    leptos::task::spawn_local(async move {
        let spec = list_sessions_spec(&project_id);
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
    });
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

/// The W6.3 session-history screen.
///
/// The shell has no project context, so the screen takes the project id from
/// its own input (the dev harness prints the seeded id next to the URL). The
/// token comes from [`read_marker_token`] — the marker `mem-web` injects into
/// the shell — because the session routes are token-gated even on loopback.
#[component]
pub fn SessionHistoryPanel() -> impl IntoView {
    let project_id = RwSignal::new(String::new());
    let rows = RwSignal::new(Vec::<SessionRow>::new());
    let error = RwSignal::new(None::<String>);
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let project = project_id.get_untracked();
            if project.is_empty() {
                rows.set(Vec::new());
                error.set(Some("Enter a project id to load the timeline".to_string()));
                return;
            }
            load_rows(project, auth.clone(), rows, error);
        }
    });

    let delete = Callback::new(move |session_id: String| {
        let project = project_id.get_untracked();
        let auth = auth.clone();
        leptos::task::spawn_local(async move {
            match delete_session(&project, &session_id, &auth).await {
                Ok(()) => {
                    error.set(None);
                    reload.run(());
                }
                Err(failure) => error.set(Some(failure.to_string())),
            }
        });
    });

    view! {
        <section class="session-history" data-testid="session-history">
            <h2>"Session history"</h2>
            <p class="session-hint">
                "Sessions are project-scoped. Paste the project id the harness printed."
            </p>
            <div class="session-controls">
                <input
                    id="session-project-id"
                    name="projectId"
                    type="text"
                    class="project-input"
                    data-testid="project-input"
                    placeholder="demo-project"
                    prop:value=move || project_id.get()
                    on:input=move |event| project_id.set(event_target_value(&event))
                />
                <button
                    type="button"
                    class="load-sessions"
                    data-testid="load-sessions"
                    on:click=move |_| reload.run(())
                >
                    "Load timeline"
                </button>
            </div>
            {move || {
                error
                    .get()
                    .map(|message| {
                        view! {
                            <p class="session-error" data-testid="session-error">{message}</p>
                        }
                    })
            }}
            <ul class="session-list" data-testid="session-list">
                {move || {
                    rows
                        .get()
                        .into_iter()
                        .map(|row| {
                            let id = row.id.clone();
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
                                            view! {
                                                <span class="orphaned-badge" data-testid="orphaned-badge">
                                                    "orphaned"
                                                </span>
                                            }
                                        })}
                                    <button
                                        type="button"
                                        class="session-delete"
                                        data-testid="delete-session"
                                        on:click=move |_| delete.run(id.clone())
                                    >
                                        "Delete"
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()
                }}
            </ul>
        </section>
    }
}
