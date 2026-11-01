//! The Agents view: who contributed to this project's durable memory.
//!
//! Four pure functions carry the whole view: [`roster`] folds the milestone
//! projection into one row per contributing agent (distinct milestone count,
//! latest source stamp, the display name from the latest source),
//! [`agents_view`] maps an optional outcome onto the six render states,
//! [`format_window`] formats a milestone's time window, and [`agent_detail`]
//! produces the per-agent drill-down (milestones newest first, each carrying its
//! server-reported source count and the facts it promoted, deduped by fact id).
//!
//! The panel issues its one request on mount (`?project=` seeds the id):
//! `GET /api/projects/{project_id}/milestones`, the W6.6 route on `mem-web`'s
//! `ROUTE_TABLE` (39 rows), pinned here as the 35th
//! [`crate::paths::ENDPOINTS`] row. It renders the documented `Unavailable`
//! state only for the `501` absent-service answer (D-38), never for a real
//! failure.

use std::collections::{BTreeMap, BTreeSet};

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::auth::{AuthContext, read_marker_token};
use crate::api::envelope::ApiError;
use crate::api_request::{self, RequestSpec};
use crate::paths::{self, EndpointId};
use crate::screens::{format_created_at, initial_project};

/// One milestone exactly as the `mem-web` projection sends it.
///
/// Mirrors `mem_episodic::Milestone` (`crates/mem-episodic/src/milestone.rs`):
/// camelCase, unknown keys ignored, and every field defaultable except `id`.
///
/// `promotedFacts` is **not** a field of `Milestone`: it is the projection's
/// join over `facts.source_ref` (the authoritative link — not
/// `fact_provenance`, whose `INSERT … ON CONFLICT DO NOTHING` rows name only
/// the FIRST promoting milestone), read through
/// `LIST_FACTS_BY_MILESTONE` in `crates/mem-facts/src/schema.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneDto {
    /// Row id (UUIDv7) — the one required field.
    pub id: String,
    /// Project scope.
    #[serde(default)]
    pub project_id: String,
    /// Agent scope (milestones are per `(project_id, agent_id)`).
    #[serde(default)]
    pub agent_id: String,
    /// The merged milestone summary.
    #[serde(default)]
    pub summary: String,
    /// The source sessions behind this milestone.
    #[serde(default)]
    pub contributors: Vec<ContributorDto>,
    /// How many source sessions were consolidated, exactly as the server sent
    /// it — the UI never re-derives this from `contributors.len()`.
    #[serde(default)]
    pub source_count: i64,
    /// The oldest consolidated session's `created_at` (unix millis).
    #[serde(default)]
    pub window_start: i64,
    /// The newest consolidated session's `created_at` (unix millis).
    #[serde(default)]
    pub window_end: i64,
    /// Creation stamp (unix millis).
    #[serde(default)]
    pub created_at: i64,
    /// Facts this milestone promoted (joined by the milestone projection).
    #[serde(default)]
    pub promoted_facts: Vec<PromotedFactDto>,
}

/// One source session behind a milestone.
///
/// Mirrors `mem_episodic::Contributor`
/// (`crates/mem-episodic/src/milestone.rs`) field for field: the source row id,
/// the session scope ids, the agent scope and display name, the source stamp,
/// and the source session's own retained summary text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributorDto {
    /// The source row's id (`agent_sessions.id`).
    pub session_row_id: String,
    /// The source session's scope id (`agent_sessions.session_id`).
    pub session_id: String,
    /// The source session's parent link, when it had one.
    #[serde(default)]
    pub parent_session_id: Option<String>,
    /// The source session's agent scope.
    pub agent_id: String,
    /// The source session's agent display name.
    pub agent_name: String,
    /// The source session's creation stamp (unix millis).
    pub created_at: i64,
    /// The source session's own retained summary text.
    pub summary: String,
}

/// One fact a milestone promoted, joined by the milestone projection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotedFactDto {
    /// Fact id (`sha256(project_id + "\x1f" + key)` upstream).
    pub id: String,
    /// The fact text.
    pub content: String,
    /// The fact's memory type, when the projection sends one.
    #[serde(default)]
    pub memory_type: Option<String>,
    /// Creation stamp (unix millis).
    pub created_at: i64,
}

/// One contributing agent, as the roster renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSummary {
    /// The agent scope id.
    pub agent_id: String,
    /// The display name from the agent's latest contributing source.
    pub display_name: String,
    /// How many distinct milestones the agent contributed to.
    pub milestone_count: usize,
    /// The greatest `created_at` among the agent's contributing sources.
    pub last_contribution: i64,
}

/// Per-agent accumulation while folding the projection.
struct AgentAccumulator {
    milestone_ids: BTreeSet<String>,
    last_contribution: i64,
}

/// The roster: one row per distinct contributing `agent_id`.
///
/// - a contributor with an empty `agent_id` is skipped (no row, no count);
/// - `milestone_count` counts **distinct milestone ids**, so two sessions of
///   the same agent inside one milestone count once;
/// - `last_contribution` is the greatest `contributor.created_at`;
/// - `display_name` comes from [`display_name_for`] — the contributing record
///   with the greatest `created_at`, ties broken to the lexicographically
///   smallest non-empty `agent_name`, empty names falling back to `agent_id`;
/// - the output is sorted by `last_contribution` desc then `agent_id` asc.
///
/// The fold is a `BTreeMap` and the output is explicitly sorted, so no
/// `HashMap` iteration order can leak into the rendered roster.
pub fn roster(milestones: &[MilestoneDto]) -> Vec<AgentSummary> {
    let mut agents: BTreeMap<String, AgentAccumulator> = BTreeMap::new();
    for milestone in milestones {
        for contributor in &milestone.contributors {
            if contributor.agent_id.is_empty() {
                continue;
            }
            let accumulator = agents
                .entry(contributor.agent_id.clone())
                .or_insert_with(|| AgentAccumulator {
                    milestone_ids: BTreeSet::new(),
                    last_contribution: contributor.created_at,
                });
            accumulator.milestone_ids.insert(milestone.id.clone());
            if contributor.created_at > accumulator.last_contribution {
                accumulator.last_contribution = contributor.created_at;
            }
        }
    }

    let mut rows: Vec<AgentSummary> = agents
        .into_iter()
        .map(|(agent_id, accumulator)| AgentSummary {
            display_name: display_name_for(&agent_id, milestones),
            agent_id,
            milestone_count: accumulator.milestone_ids.len(),
            last_contribution: accumulator.last_contribution,
        })
        .collect();
    rows.sort_by(|left, right| {
        right
            .last_contribution
            .cmp(&left.last_contribution)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    rows
}

/// The display name for one agent, by the roster's documented rule.
///
/// The latest `created_at` among the agent's contributing records wins; at that
/// stamp the lexicographically smallest non-empty `agent_name` wins; when no
/// record carries a name, the `agent_id` is the fallback. An unknown agent (or
/// an empty id) resolves to the id itself, so callers stay total.
fn display_name_for(agent_id: &str, milestones: &[MilestoneDto]) -> String {
    let mut latest: Option<i64> = None;
    let mut names: BTreeSet<String> = BTreeSet::new();
    for milestone in milestones {
        for contributor in &milestone.contributors {
            if contributor.agent_id != agent_id || contributor.agent_id.is_empty() {
                continue;
            }
            match latest {
                Some(stamp) if contributor.created_at < stamp => continue,
                Some(stamp) if contributor.created_at == stamp => {
                    if !contributor.agent_name.is_empty() {
                        names.insert(contributor.agent_name.clone());
                    }
                }
                _ => {
                    latest = Some(contributor.created_at);
                    names.clear();
                    if !contributor.agent_name.is_empty() {
                        names.insert(contributor.agent_name.clone());
                    }
                }
            }
        }
    }
    names
        .into_iter()
        .next()
        .unwrap_or_else(|| agent_id.to_string())
}

// ---------------------------------------------------------------------------
// The view oracle
// ---------------------------------------------------------------------------

/// What the milestone fetch hands the panel.
///
/// `Unavailable` is the documented 501 absent-service degrade (D-38), not an
/// error; `NeedsProject` is the pre-input state — there is no project id yet,
/// so nothing was asked and nothing failed — and only `Failed` means the
/// request itself failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsOutcome {
    /// Data arrived.
    Loaded(Vec<MilestoneDto>),
    /// The build has no data path (the documented degraded state).
    Unavailable,
    /// No project id has been entered yet (the mount state, not an error).
    NeedsProject,
    /// The request itself failed.
    Failed(String),
}

/// The six arms the panel renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsView {
    /// Before any outcome exists (the terminal pre-outcome state only — the
    /// mount always produces an outcome).
    Loading,
    /// No project id yet: prompt for one, in the informational shape.
    NeedsProject,
    /// A loaded scope with no contributing agents.
    Empty,
    /// The documented degraded state, carrying its fixed copy.
    Unavailable(&'static str),
    /// At least one contributing agent.
    Roster(Vec<AgentSummary>),
    /// A failure message — never the unavailable state.
    Failed(String),
}

/// Map an optional outcome onto the render state.
///
/// `None` is the pre-outcome state (`Loading`); a loaded roster that folds to
/// zero rows is the empty state, never an error; only `Failed` maps to
/// `Failed`.
pub fn agents_view(outcome: Option<&AgentsOutcome>) -> AgentsView {
    match outcome {
        None => AgentsView::Loading,
        Some(AgentsOutcome::Loaded(milestones)) => {
            let rows = roster(milestones);
            if rows.is_empty() {
                AgentsView::Empty
            } else {
                AgentsView::Roster(rows)
            }
        }
        Some(AgentsOutcome::Unavailable) => AgentsView::Unavailable(AGENTS_UNAVAILABLE_MESSAGE),
        Some(AgentsOutcome::NeedsProject) => AgentsView::NeedsProject,
        Some(AgentsOutcome::Failed(message)) => AgentsView::Failed(message.clone()),
    }
}

/// The fixed copy of the documented degraded state.
///
/// A `'static` constant, never a server body: the unavailable state renders
/// this fixed copy when the answer is the 501 degrade, so no response byte
/// reaches the DOM through it and the copy cannot drift from the test that
/// pins it.
pub const AGENTS_UNAVAILABLE_MESSAGE: &str = "Agent attribution is not available in this build.";

/// The empty-scope copy: an empty scope is the empty state, not an error.
pub const AGENTS_EMPTY_MESSAGE: &str = "No contributing agents in this project yet.";

/// The pre-input copy: names the actual next step, promises no picker.
pub const AGENTS_NEEDS_PROJECT_MESSAGE: &str =
    "Enter a project id above to list this project's agent roster.";

/// The `data-testid` of the loading note.
pub const AGENTS_LOADING_TESTID: &str = "agents-loading";

/// The `data-testid` of the empty note.
pub const AGENTS_EMPTY_TESTID: &str = "agents-empty";

/// The `data-testid` of the unavailable note.
pub const AGENTS_UNAVAILABLE_TESTID: &str = "agents-unavailable";

/// The `data-testid` of the roster list.
pub const AGENTS_ROSTER_TESTID: &str = "agents-roster";

/// The `data-testid` of the failure note.
pub const AGENTS_ERROR_TESTID: &str = "agents-error";

/// The `data-testid` of the drill-down section.
pub const AGENTS_DETAIL_TESTID: &str = "agents-detail";

/// The `data-testid` of one drill-down milestone.
pub const AGENTS_MILESTONE_TESTID: &str = "agents-milestone";

/// The `data-testid` of one promoted fact.
pub const AGENTS_FACT_TESTID: &str = "agents-fact";

/// The `data-testid` of one roster row button.
pub const AGENTS_ROW_TESTID: &str = "agents-row";

/// The `data-testid` of the pre-input prompt.
pub const AGENTS_PROMPT_TESTID: &str = "agents-prompt";

/// The class of the loading note.
pub const AGENTS_LOADING_CLASS: &str = "agents-loading";

/// The class of the empty note.
pub const AGENTS_EMPTY_CLASS: &str = "agents-empty";

/// The class of the unavailable note (dashed/muted — informational).
pub const AGENTS_UNAVAILABLE_CLASS: &str = "agents-unavailable";

/// The class of the roster list.
pub const AGENTS_ROSTER_CLASS: &str = "agents-roster";

/// The class of the failure note (left bar — a real failure).
pub const AGENTS_ERROR_CLASS: &str = "agents-error";

/// The class of the drill-down section.
pub const AGENTS_DETAIL_CLASS: &str = "agents-detail";

/// The class of one drill-down milestone.
pub const AGENTS_MILESTONE_CLASS: &str = "agents-milestone";

/// The class of one promoted fact.
pub const AGENTS_FACT_CLASS: &str = "agents-fact";

/// The class of one roster row button.
pub const AGENTS_ROW_CLASS: &str = "agents-row";

/// The class of the pre-input prompt (muted/dashed: nothing failed).
pub const AGENTS_PROMPT_CLASS: &str = "agents-prompt";

/// What the panel does when it mounts at a search string.
///
/// The regression this type exists to prevent is a mount that produces **no**
/// state at all: the panel then stays on `Loading` forever (the shipped B2
/// bug). Every mount yields an outcome — a fetch or a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountAction {
    /// The project id to seed the input with (empty when none was given).
    pub project: String,
    /// Whether to issue the one milestones request.
    pub fetch: bool,
    /// The pre-input notice, when the route carried no project id.
    pub notice: Option<String>,
}

/// The mount's one action for a URL search string: `""` prompts, `?project=p1`
/// seeds and fetches.
pub fn mount_action(search: &str) -> MountAction {
    match initial_project(search) {
        Some(project) => MountAction {
            project,
            fetch: true,
            notice: None,
        },
        None => MountAction {
            project: String::new(),
            fetch: false,
            notice: Some(AGENTS_NEEDS_PROJECT_MESSAGE.to_string()),
        },
    }
}

// ---------------------------------------------------------------------------
// The drill-down
// ---------------------------------------------------------------------------

/// One promoted fact, as the drill-down renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedFactView {
    /// Fact id (the dedupe key).
    pub id: String,
    /// The fact text.
    pub content: String,
    /// The fact's memory type, when the projection sent one.
    pub memory_type: Option<String>,
}

/// One milestone in an agent's drill-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MilestoneView {
    /// The milestone row id.
    pub milestone_id: String,
    /// The formatted window (`<start> → <end>`).
    pub window: String,
    /// The raw window start (unix millis).
    pub window_start: i64,
    /// The raw window end (unix millis).
    pub window_end: i64,
    /// The server's source-session count, exactly as the projection sent it.
    pub source_count: i64,
    /// The facts this milestone promoted, newest first, deduped by fact id.
    pub facts: Vec<PromotedFactView>,
}

/// One agent's drill-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDetailView {
    /// The agent scope id.
    pub agent_id: String,
    /// The display name from the agent's latest contributing source.
    pub display_name: String,
    /// The agent's milestones, newest first under the documented order.
    pub milestones: Vec<MilestoneView>,
}

/// Format a milestone window as `<start> → <end>`.
///
/// Reuses [`crate::screens::format_created_at`] — the crate has exactly one
/// date routine, and the two ends of a window must render exactly like every
/// other stamp on the screen.
pub fn format_window(start: i64, end: i64) -> String {
    format!(
        "{} → {}",
        crate::screens::format_created_at(start),
        crate::screens::format_created_at(end)
    )
}

/// The drill-down for one agent.
///
/// - only milestones with a contributor of `agent_id` are included (an empty
///   `agent_id` matches nothing, exactly like [`roster`]);
/// - milestones are ordered by `window_end` desc then `id` asc — **this is the
///   one milestone order the module uses**, so "newest" always means the first
///   milestone under it;
/// - `source_count` is carried exactly as the projection sent it, never
///   re-derived from `contributors.len()`;
/// - facts are deduped across the agent's milestones by fact id, keeping the
///   occurrence from the first milestone under that order, and each milestone's
///   facts are ordered by `created_at` desc then `id` asc;
/// - the display name follows [`roster`]'s rule, and an unknown agent yields an
///   empty milestone list with `display_name == agent_id`.
///
/// Total for empty input, unknown ids, empty ids, empty names and negative
/// (pre-epoch) timestamps: there is no panic path.
pub fn agent_detail(agent_id: &str, milestones: &[MilestoneDto]) -> AgentDetailView {
    let display_name = display_name_for(agent_id, milestones);

    let mut selected: Vec<&MilestoneDto> = milestones
        .iter()
        .filter(|milestone| {
            milestone
                .contributors
                .iter()
                .any(|contributor| !agent_id.is_empty() && contributor.agent_id == agent_id)
        })
        .collect();
    selected.sort_by(|left, right| {
        right
            .window_end
            .cmp(&left.window_end)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut seen_facts: BTreeSet<String> = BTreeSet::new();
    let mut views: Vec<MilestoneView> = Vec::with_capacity(selected.len());
    for milestone in selected {
        let mut facts: Vec<&PromotedFactDto> = milestone.promoted_facts.iter().collect();
        facts.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut fact_views: Vec<PromotedFactView> = Vec::new();
        for fact in facts {
            if !seen_facts.insert(fact.id.clone()) {
                continue;
            }
            fact_views.push(PromotedFactView {
                id: fact.id.clone(),
                content: fact.content.clone(),
                memory_type: fact.memory_type.clone(),
            });
        }
        views.push(MilestoneView {
            milestone_id: milestone.id.clone(),
            window: format_window(milestone.window_start, milestone.window_end),
            window_start: milestone.window_start,
            window_end: milestone.window_end,
            source_count: milestone.source_count,
            facts: fact_views,
        });
    }

    AgentDetailView {
        agent_id: agent_id.to_string(),
        display_name,
        milestones: views,
    }
}

// ---------------------------------------------------------------------------
// The one fetch
// ---------------------------------------------------------------------------

/// The `GET /api/projects/{project_id}/milestones` request: the panel's one
/// request.
pub fn list_milestones_spec(project_id: &str) -> RequestSpec {
    RequestSpec::get(paths::project_path(
        EndpointId::ListProjectMilestones,
        project_id,
    ))
}

/// Map an [`ApiError`] onto the panel's outcome.
///
/// Only the `501` absent-service answer is the documented D-38 degrade
/// ([`AgentsOutcome::Unavailable`]); a `500`, a transport failure and a
/// malformed body are real failures carrying the bounded Display.
pub fn outcome_for_error(error: ApiError) -> AgentsOutcome {
    match error {
        ApiError::Server { status: 501, .. } => AgentsOutcome::Unavailable,
        other => AgentsOutcome::Failed(other.to_string()),
    }
}

/// Load the project's milestone projection (the panel's one request).
pub async fn load_outcome(project_id: &str, auth: &AuthContext) -> AgentsOutcome {
    let spec = list_milestones_spec(project_id);
    match api_request::request::<Vec<MilestoneDto>>(&spec, auth).await {
        Ok(milestones) => AgentsOutcome::Loaded(milestones),
        Err(error) => outcome_for_error(error),
    }
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

/// The W6.6 Agents panel.
///
/// The shell has no project context, so the screen takes the project id from
/// its own input (the demo harness prints the seeded id next to the URL) and
/// seeds it from `?project=` when present. One `Effect` on mount fetches the
/// project-scoped milestone projection once; the Load button re-runs it.
///
/// The outcome starts `None` (= `Loading`); the documented `Unavailable` state
/// now renders a real `501` absent-service answer (the D-38 degraded
/// composition), never a deferred data path. The selected agent id renders
/// [`agent_detail`] beneath the roster once `Loaded` arrives.
#[component]
pub fn AgentsPanel() -> impl IntoView {
    let project_id = RwSignal::new(String::new());
    let outcome = RwSignal::new(None::<AgentsOutcome>);
    let selected = RwSignal::new(None::<String>);
    let auth = AuthContext {
        local_token: read_marker_token(),
        basic: None,
    };

    let reload = Callback::new({
        let auth = auth.clone();
        move |_: ()| {
            let project = project_id.get_untracked();
            if project.is_empty() {
                // One source of truth for the pre-input copy: the same constant
                // the mount prompt renders.
                outcome.set(Some(AgentsOutcome::NeedsProject));
                return;
            }
            let auth = auth.clone();
            leptos::task::spawn_local(async move {
                outcome.set(Some(load_outcome(&project, &auth).await));
            });
        }
    });

    // The mount is unconditional: with no `?project=` it renders the prompt
    // (`NeedsProject`), never the terminal `Loading` state.
    Effect::new(move |_| {
        let action = mount_action(
            &web_sys::window()
                .and_then(|w| w.location().search().ok())
                .unwrap_or_default(),
        );
        project_id.set(action.project);
        if action.fetch {
            reload.run(());
        } else if action.notice.is_some() {
            outcome.set(Some(AgentsOutcome::NeedsProject));
        }
    });

    view! {
        <section class="agents-panel" data-testid="agents-panel">
            <h2>"Agents"</h2>
            <p class="agents-hint">"Who contributed to this project's durable memory."</p>
            <div class="agents-controls">
                <input
                    id="agents-project-id"
                    name="projectId"
                    type="text"
                    class="project-input"
                    data-testid="agents-project-input"
                    placeholder="project id"
                    prop:value=move || project_id.get()
                    on:input=move |event| project_id.set(event_target_value(&event))
                />
                <button
                    type="button"
                    class="load-agents"
                    data-testid="load-agents"
                    on:click=move |_| reload.run(())
                >
                    "Load roster"
                </button>
            </div>
            {move || match agents_view(outcome.get().as_ref()) {
                AgentsView::Loading => view! {
                    <p class=AGENTS_LOADING_CLASS data-testid=AGENTS_LOADING_TESTID>
                        "Loading agents…"
                    </p>
                }
                    .into_any(),
                AgentsView::NeedsProject => view! {
                    <p class=AGENTS_PROMPT_CLASS data-testid=AGENTS_PROMPT_TESTID>
                        {AGENTS_NEEDS_PROJECT_MESSAGE}
                    </p>
                }
                    .into_any(),
                AgentsView::Empty => view! {
                    <p class=AGENTS_EMPTY_CLASS data-testid=AGENTS_EMPTY_TESTID>
                        {AGENTS_EMPTY_MESSAGE}
                    </p>
                }
                    .into_any(),
                AgentsView::Unavailable(message) => view! {
                    <p class=AGENTS_UNAVAILABLE_CLASS data-testid=AGENTS_UNAVAILABLE_TESTID>
                        {message}
                    </p>
                }
                    .into_any(),
                AgentsView::Roster(rows) => view! {
                    <ul class=AGENTS_ROSTER_CLASS data-testid=AGENTS_ROSTER_TESTID>
                        {rows
                            .into_iter()
                            .map(|row| {
                                let agent_id = row.agent_id.clone();
                                view! {
                                    <li>
                                        <button
                                            type="button"
                                            class=AGENTS_ROW_CLASS
                                            data-testid=AGENTS_ROW_TESTID
                                            on:click=move |_| selected.set(Some(agent_id.clone()))
                                        >
                                            <span class="agents-agent-name">{row.display_name.clone()}</span>
                                            <span class="agents-agent-id">{row.agent_id.clone()}</span>
                                            <span class="agents-milestone-count">{row.milestone_count}</span>
                                            <span class="agents-last-contribution">
                                                {format_created_at(row.last_contribution)}
                                            </span>
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any(),
                AgentsView::Failed(message) => view! {
                    <p class=AGENTS_ERROR_CLASS data-testid=AGENTS_ERROR_TESTID>{message}</p>
                }
                    .into_any(),
            }}
            {move || {
                let selected_id = selected.get();
                let outcome = outcome.get();
                match (selected_id, &outcome) {
                    (Some(agent_id), Some(AgentsOutcome::Loaded(rows))) => {
                        Some(detail_view(agent_detail(&agent_id, rows)))
                    }
                    _ => None,
                }
            }}
        </section>
    }
}

/// Render one agent's drill-down.
fn detail_view(detail: AgentDetailView) -> AnyView {
    view! {
        <section class=AGENTS_DETAIL_CLASS data-testid=AGENTS_DETAIL_TESTID>
            <h3>{format!("Milestones contributed by {}", detail.display_name)}</h3>
            <ul class="agents-milestone-list">
                {detail
                    .milestones
                    .into_iter()
                    .map(|milestone| {
                        view! {
                            <li class=AGENTS_MILESTONE_CLASS data-testid=AGENTS_MILESTONE_TESTID>
                                <span class="agents-window">{milestone.window}</span>
                                <span class="agents-source-count">
                                    {format!("{} source sessions", milestone.source_count)}
                                </span>
                                <ul class="agents-fact-list">
                                    {milestone
                                        .facts
                                        .into_iter()
                                        .map(|fact| {
                                            view! {
                                                <li class=AGENTS_FACT_CLASS data-testid=AGENTS_FACT_TESTID>
                                                    {fact.content}
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
        .into_any()
}
