//! The shared arm-then-confirm dialog for the three destructive rows.
//!
//! A memory, a session and a skill are deleted by the same two-step gesture:
//! the row's Delete button only **arms** an id, and a modal [`ConfirmDialog`]
//! then asks for the second click; its Confirm button is the only path to the
//! network. The Delete button itself never unmounts, so focus can return to the
//! exact node the user pressed. The
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
use leptos::reactive::wrappers::write::SignalSetter;
use thaw::{
    Button, ButtonAppearance, ButtonRef, ComponentRef, Dialog, DialogActions, DialogBody,
    DialogSurface, DialogTitle,
};

use crate::dialog_focus::{return_focus, trap_tab};

/// The confirmation copy, one string for all three rows.
pub const CONFIRM_DELETE_PROMPT: &str = "Delete permanently? This cannot be undone.";

/// The unconditional kept-memories sentence: what a delete leaves behind even
/// when the count is not derivable (`survivingDerived.memories == null`).
pub const DELETE_KEPT_UNCONDITIONAL: &str =
    "Memories learned from this prompt were kept and can be deleted separately.";

/// The session-transcript sentence (ADR-0019 D-19-11 obligation 2): a delete
/// never claims the transcript is gone.
pub const DELETE_TRANSCRIPT_NOTICE: &str =
    "The session transcript still contains it until you purge the session.";

/// The three-surface delete copy (issue 43 W6c, D-19-11 obligation 2): history
/// removal now, the kept derived memories (the count only when it is a number),
/// and the transcript notice.
pub fn delete_disclosure_copy(memories: Option<u64>) -> String {
    match memories {
        Some(count) => format!(
            "Removed from the prompt history now; {count} memories learned from it were kept and \
             can be deleted separately. {DELETE_TRANSCRIPT_NOTICE}"
        ),
        None => format!(
            "Removed from the prompt history now. {DELETE_KEPT_UNCONDITIONAL} \
             {DELETE_TRANSCRIPT_NOTICE}"
        ),
    }
}

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

/// The dialog's accessible name, and the id of the title it points at.
pub const CONFIRM_DIALOG_TESTID: &str = "confirm-dialog";

/// The title element's id: the surface's `aria-labelledby` target.
pub const CONFIRM_TITLE_ID: &str = "confirm-dialog-title";

/// The modal arm-then-confirm surface.
///
/// The reducer is untouched: `open` is **derived** from it and the setter only
/// translates a close request into `ConfirmEvent::Cancel`, so thaw's own
/// `open.set(false)` (Esc, or the mask) disarms instead of merely hiding the
/// surface. `mask_closeable=false` keeps a stray click from destroying the
/// gesture.
#[component]
pub fn ConfirmDialog(
    /// The read half of thaw's `Model<bool>`: `pending.is_some() && !running`.
    /// (thaw does not re-export `Model`, so the two halves travel as props and
    /// are paired into the tuple form at the `Dialog` below.)
    open: Signal<bool>,
    /// The write half: `false` disarms, `true` is ignored. thaw writes `false`
    /// on Esc and on a mask click.
    #[prop(into)]
    set_open: SignalSetter<bool>,
    /// Confirm: run the armed id (the reducer decides whether it may).
    on_confirm: Callback<()>,
    /// Cancel: disarm.
    on_cancel: Callback<()>,
) -> impl IntoView {
    let confirm_ref = ComponentRef::<ButtonRef>::default();
    let surface = NodeRef::<html::Div>::new();
    // The ref loads a tick after `open` flips, so the effect re-runs on it.
    Effect::new(move |_| {
        if open.get()
            && let Some(button) = confirm_ref.get()
        {
            button.focus();
        }
    });

    view! {
        <Dialog open=(open, set_open) mask_closeable=false>
            <DialogSurface attr:aria-labelledby=CONFIRM_TITLE_ID>
                <DialogBody>
                    <div
                        class="confirm-dialog"
                        data-testid=CONFIRM_DIALOG_TESTID
                        node_ref=surface
                        on:keydown=move |event: leptos::ev::KeyboardEvent| {
                            if let Some(element) = surface.get_untracked() {
                                trap_tab(&event, &element);
                            }
                        }
                    >
                        <DialogTitle attr:id=CONFIRM_TITLE_ID attr:data-testid="confirm-dialog-title">
                            {CONFIRM_DELETE_PROMPT}
                        </DialogTitle>
                        <p class="confirm-disclosure" data-testid="confirm-erasure-disclosure">
                            {delete_disclosure_copy(None)}
                        </p>
                        <DialogActions attr:data-testid="confirm-dialog-actions">
                            <Button
                                appearance=ButtonAppearance::Primary
                                comp_ref=confirm_ref
                                attr:data-testid="confirm-gate-confirm"
                                on_click=move |_| on_confirm.run(())
                            >
                                "Confirm delete"
                            </Button>
                            <Button
                                appearance=ButtonAppearance::Secondary
                                attr:data-testid="confirm-gate-cancel"
                                on_click=move |_| on_cancel.run(())
                            >
                                "Cancel"
                            </Button>
                        </DialogActions>
                    </div>
                </DialogBody>
            </DialogSurface>
        </Dialog>
    }
}

/// One row's Delete button together with the dialog it arms.
///
/// The button is no longer swapped out when the row arms: it stays mounted for
/// the whole gesture, which is what lets focus come back to the exact node the
/// user pressed (and lets a raw-button census see one button, not two).
#[component]
pub fn ConfirmDeleteButton(
    /// The screen's single gate reducer.
    confirm: RwSignal<ConfirmState>,
    /// The row id this button deletes.
    id: String,
    /// The element wrapping this button (thaw's `Button` exposes no
    /// `node_ref`, and the wrapper also carries the row's own class hook), so
    /// focus returns to the control inside it.
    invoker: NodeRef<html::Span>,
    /// Where focus goes when the row is gone before the request returns.
    fallback: NodeRef<html::Span>,
    /// The button's `data-testid`.
    #[prop(into)]
    testid: String,
    /// Kept per site: the memory card's Delete was `Subtle`, the other two took
    /// thaw's default.
    #[prop(optional)]
    appearance: Option<ButtonAppearance>,
    #[prop(optional, into)] aria_label: Option<String>,
    /// Emitted with the id the reducer released.
    on_run: Callback<String>,
) -> impl IntoView {
    let armed = Signal::derive({
        let id = id.clone();
        move || confirm.get().pending.as_deref() == Some(id.as_str())
    });
    let open = Signal::derive(move || {
        let state = confirm.get();
        state.pending.is_some() && !state.running
    });
    let set_open = SignalSetter::map(move |value: bool| {
        if !value {
            confirm.set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Cancel).state);
        }
    });
    track_focus_return(confirm, id.clone(), invoker, fallback);

    let ask = {
        let id = id.clone();
        move |_| {
            confirm
                .set(confirm_next(&confirm.get_untracked(), ConfirmEvent::Ask(id.clone())).state);
        }
    };
    view! {
        <Button
            appearance=appearance.unwrap_or_default()
            attr:data-testid=testid
            attr:aria-label=aria_label
            on_click=ask
        >
            "Delete"
        </Button>
        {move || {
            armed
                .get()
                .then(|| {
                    view! {
                        <ConfirmDialog
                            open=open
                            set_open=set_open
                            on_confirm=Callback::new(move |_: ()| {
                                if let Some(run_id) = dispatch(confirm, ConfirmEvent::Confirm) {
                                    on_run.run(run_id);
                                }
                            })
                            on_cancel=Callback::new(move |_: ()| {
                                confirm
                                    .set(
                                        confirm_next(
                                                &confirm.get_untracked(),
                                                ConfirmEvent::Cancel,
                                            )
                                            .state,
                                    );
                            })
                        />
                    }
                })
        }}
    }
}

/// Hand focus back when the gesture ends, whichever way it ended.
///
/// Each row tracks its own stage — idle, armed, confirmed-and-in-flight — so
/// only the row that owned the dialog restores focus, and it does so on the
/// last step. For a confirmed delete that is after `Finished`, by which time
/// the row may have unmounted: that is what the fallback is for. A second row
/// armed mid-gesture takes the stage to `Other` and restores nothing, so the
/// new Delete button keeps the focus the user just gave it.
fn track_focus_return(
    confirm: RwSignal<ConfirmState>,
    id: String,
    invoker: NodeRef<html::Span>,
    fallback: NodeRef<html::Span>,
) {
    let stage = StoredValue::new(Stage::Idle);
    Effect::new(move |_| {
        let state = confirm.get();
        let next = if state.pending.as_deref() == Some(id.as_str()) {
            Stage::Armed
        } else if state.pending.is_some() {
            Stage::Other
        } else if state.running && stage.get_value() == Stage::Armed {
            Stage::Running
        } else {
            Stage::Idle
        };
        let previous = stage.get_value();
        stage.set_value(next);
        if matches!(previous, Stage::Armed | Stage::Running) && next == Stage::Idle {
            return_focus(invoker.get_untracked(), fallback.get_untracked());
        }
    });
}

/// Focus return for a section whose invoker is one permanent button.
///
/// The maintenance action is never swapped out — it is only disabled while
/// armed — so there is no row to chase and no separate fallback: the same node
/// takes focus back once the gesture settles.
pub fn track_section_focus_return(active: Signal<bool>, invoker: NodeRef<html::Span>) {
    let was_active = StoredValue::new(false);
    Effect::new(move |_| {
        let now = active.get();
        let previous = was_active.get_value();
        was_active.set_value(now);
        if previous && !now {
            return_focus(invoker.get_untracked(), invoker.get_untracked());
        }
    });
}

/// Which part of the arm-then-confirm gesture a row is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// No gesture, or another row's.
    Idle,
    /// This row is armed and the dialog is open.
    Armed,
    /// This row's delete is in flight.
    Running,
    /// Another row armed while this one was armed.
    Other,
}
