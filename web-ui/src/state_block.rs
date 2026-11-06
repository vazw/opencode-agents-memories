//! Which block of the screen a view state renders.

use leptos::prelude::*;
use thaw::{MessageBar, MessageBarBody, MessageBarIntent, Spinner};

use crate::core_screens::MemoriesView;

/// The timeline layer of the Sessions screen: exactly one of the four arms
/// renders.
///
/// Wave 3 (issue 118). The classifier is pure so the arm boundaries are
/// table-testable without a DOM; the panel renders one [`StateBlock`] per arm
/// (or the row list for [`SessionState::Ready`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// A timeline fetch is in flight.
    Loading,
    /// The fetch answered with an empty timeline.
    Empty,
    /// The fetch failed (or no project could be resolved); the string is the
    /// bounded error copy.
    Error(String),
    /// Rows arrived.
    Ready(Vec<crate::screens::SessionRow>),
}

/// Classify the timeline layer: error > loading > empty > ready.
///
/// An error always wins — a failure must never be masked by a spinner or by
/// stale rows. A load in flight beats whatever the row buffer still holds (the
/// previous project's timeline must not flash as current). With neither, an
/// empty buffer is the empty arm and only populated rows are Ready.
pub fn session_state(
    rows: &[crate::screens::SessionRow],
    error: Option<&str>,
    loading: bool,
) -> SessionState {
    if let Some(message) = error {
        return SessionState::Error(message.to_string());
    }
    if loading {
        return SessionState::Loading;
    }
    if rows.is_empty() {
        return SessionState::Empty;
    }
    SessionState::Ready(rows.to_vec())
}

/// Map the roster outcome onto the Sessions screen's picker arms, one-to-one
/// over [`crate::agents::ProjectsView`].
///
/// The Sessions screen renders the SAME five arms the Agents panel renders —
/// the classification already exists, so this is a pure forward, never a
/// re-derivation: `None` is the pre-outcome loading arm, an empty loaded
/// roster is `NoProjects` (never an error), and only the 501 degrade answers
/// `Unavailable`.
pub fn roster_session_view(
    outcome: Option<&crate::agents::ProjectsOutcome>,
) -> crate::agents::ProjectsView {
    crate::agents::projects_view(outcome)
}

/// The four mutually exclusive blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Loading,
    Empty,
    Error,
    Ready,
}

/// Classify a view state.
pub fn block_kind(state: &MemoriesView) -> BlockKind {
    match state {
        MemoriesView::Loading => BlockKind::Loading,
        MemoriesView::Empty { .. } => BlockKind::Empty,
        MemoriesView::Error(_) => BlockKind::Error,
        MemoriesView::Loaded { .. } => BlockKind::Ready,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateDisplay {
    Spinner { label: String },
    Info { message: String },
    Error { message: String },
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateBlockModel {
    pub display: StateDisplay,
    pub testid: Option<&'static str>,
}

pub fn state_block_model(
    kind: BlockKind,
    message: Option<&str>,
    testid: Option<&'static str>,
) -> StateBlockModel {
    let display = match kind {
        BlockKind::Loading => StateDisplay::Spinner {
            label: message.unwrap_or("Loading…").to_string(),
        },
        BlockKind::Empty => StateDisplay::Info {
            message: message.unwrap_or("No records found.").to_string(),
        },
        BlockKind::Error => StateDisplay::Error {
            message: message.unwrap_or("An error occurred.").to_string(),
        },
        BlockKind::Ready => StateDisplay::Ready,
    };
    StateBlockModel { display, testid }
}

pub const STATE_BLOCK_CLASS: &str = "state-block";

#[component]
pub fn StateBlock(
    #[prop(into)] kind: Signal<BlockKind>,
    #[prop(optional, into)] message: MaybeProp<String>,
    #[prop(optional, into)] class: MaybeProp<String>,
    #[prop(optional, into)] testid: Option<&'static str>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    move || {
        let msg = message.get();
        let cls = class.get().unwrap_or_default();
        let model = state_block_model(kind.get(), msg.as_deref(), testid);
        let testid_attr = model.testid;

        match model.display {
            StateDisplay::Spinner { label } => view! {
                <div class={[STATE_BLOCK_CLASS, cls.as_str()].join(" ")} data-testid=testid_attr>
                    <Spinner label=label />
                </div>
            }
            .into_any(),
            StateDisplay::Info { message } => view! {
                <div class={[STATE_BLOCK_CLASS, cls.as_str()].join(" ")} data-testid=testid_attr>
                    <MessageBar intent=MessageBarIntent::Info>
                        <MessageBarBody>{message}</MessageBarBody>
                    </MessageBar>
                </div>
            }
            .into_any(),
            StateDisplay::Error { message } => view! {
                <div class={[STATE_BLOCK_CLASS, cls.as_str()].join(" ")} data-testid=testid_attr>
                    <MessageBar intent=MessageBarIntent::Error>
                        <MessageBarBody>{message}</MessageBarBody>
                    </MessageBar>
                </div>
            }
            .into_any(),
            StateDisplay::Ready => match &children {
                Some(ch) => ch().into_any(),
                None => view! { <div class={[STATE_BLOCK_CLASS, cls.as_str()].join(" ")} data-testid=testid_attr></div> }.into_any(),
            },
        }
    }
}
