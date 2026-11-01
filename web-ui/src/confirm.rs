//! The shared arm-then-confirm gate for the three destructive rows.
//!
//! A memory, a session and a skill are deleted by the same two-step gesture:
//! the row's Delete button only **arms** an id, and the row then renders a
//! [`ConfirmGate`] whose Confirm button is the only path to the network. The
//! reducer is flatter than the Maintenance section's `cleanup_next` (one event
//! enum, no outcome payload, no report field) but the same contract: total,
//! side-effect free, and `run` is the single transition that may reach the
//! network.
//!
//! Failure mode this exists to prevent: a gate that stays `running` after a
//! transient failure. `Finished` is the only transition that clears `running`,
//! so every wiring site must dispatch it on **every** exit path of the request
//! — success, HTTP error and transport error — not only where the list reloads.

use leptos::html;
use leptos::prelude::*;

/// The confirmation copy, one string for all three rows.
pub const CONFIRM_DELETE_PROMPT: &str = "Delete permanently? This cannot be undone.";

/// One row's arm/confirm state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfirmState {
    /// The row id the user armed, if any.
    pub pending: Option<String>,
    /// A confirmed delete is in flight.
    pub running: bool,
}

/// Everything that can move [`ConfirmState`] forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmEvent {
    /// The row's Delete button: arm this id.
    Ask(String),
    /// The gate's Cancel button: disarm.
    Cancel,
    /// The gate's Confirm button: run the armed id.
    Confirm,
    /// The request returned, on any exit path.
    Finished,
}

/// One reducer step: the next state, plus the id to delete (if any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmStep {
    pub state: ConfirmState,
    pub run: Option<String>,
}

/// The reducer for one row's gate.
///
/// Total and side-effect free. `run` is `Some(id)` **iff** the event is
/// `Confirm` from an armed, non-running state — the single transition that may
/// reach the network. `Ask` only arms (re-asking replaces the previous row),
/// `Cancel` only disarms, and `Finished` clears both fields.
pub fn confirm_next(state: &ConfirmState, event: ConfirmEvent) -> ConfirmStep {
    let mut next = state.clone();
    let mut run = None;
    match event {
        ConfirmEvent::Ask(id) => next.pending = Some(id),
        ConfirmEvent::Cancel => next.pending = None,
        ConfirmEvent::Confirm => {
            if next.pending.is_some() && !next.running {
                run = next.pending.take();
                next.running = true;
            }
        }
        ConfirmEvent::Finished => {
            next.pending = None;
            next.running = false;
        }
    }
    ConfirmStep { state: next, run }
}

/// Apply one event to the signal and hand back the id to run, if any.
///
/// The wiring sites build their request spec only from this return value, so no
/// mount, reload or render path can start a delete.
pub fn dispatch(signal: RwSignal<ConfirmState>, event: ConfirmEvent) -> Option<String> {
    let step = confirm_next(&signal.get_untracked(), event);
    signal.set(step.state);
    step.run
}

/// The inline confirmation the armed row renders.
///
/// Four hooks (`confirm-gate`, `confirm-gate-text`, `confirm-gate-confirm`,
/// `confirm-gate-cancel`) live in `public/styles.css`, so the gate is covered by
/// the class-coverage gate like every other component. The confirm button takes
/// focus on mount: arming replaces the row's Delete button, and a keyboard user
/// must not be stranded on `<body>`.
#[component]
pub fn ConfirmGate(on_confirm: Callback<()>, on_cancel: Callback<()>) -> impl IntoView {
    let confirm_ref = NodeRef::<html::Button>::new();
    Effect::new(move |_| {
        if let Some(button) = confirm_ref.get() {
            let _ = button.focus();
        }
    });

    view! {
        <span class="confirm-gate" data-testid="confirm-gate" role="group" aria-label="Confirm delete">
            <span class="confirm-gate-text" data-testid="confirm-gate-text">{CONFIRM_DELETE_PROMPT}</span>
            <button
                type="button"
                class="confirm-gate-confirm"
                data-testid="confirm-gate-confirm"
                node_ref=confirm_ref
                on:click=move |_| on_confirm.run(())
            >
                "Confirm delete"
            </button>
            <button
                type="button"
                class="confirm-gate-cancel"
                data-testid="confirm-gate-cancel"
                on:click=move |_| on_cancel.run(())
            >
                "Cancel"
            </button>
        </span>
    }
}
