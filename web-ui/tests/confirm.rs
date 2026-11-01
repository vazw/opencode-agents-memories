#![cfg(target_arch = "wasm32")]

//! The delete-confirmation reducer.
//!
//! The three destructive rows (memory, session, skill) share one arm-then-
//! confirm reducer, so its contract is pinned once here. Each test's stated
//! mutant is a wiring bug that a DOM test would not see: a gate that deletes on
//! the first click, a gate that never deletes, a gate that deletes twice, a
//! gate that deletes the wrong row, and a gate that dies after one attempt.

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::confirm::{ConfirmEvent, ConfirmState, confirm_next};

fn armed(id: &str) -> ConfirmState {
    confirm_next(&ConfirmState::default(), ConfirmEvent::Ask(id.to_string())).state
}

#[wasm_bindgen_test]
fn test_confirm_next_never_runs_without_a_confirmed_arm() {
    // Distinguishing input: Ask("row-1") -> Cancel -> Finished -> unarmed
    // Confirm. A delete-on-click implementation runs on the Ask (today's
    // behaviour); a gate that treats any stray Confirm as a run runs on the
    // last step.
    let asked = confirm_next(
        &ConfirmState::default(),
        ConfirmEvent::Ask("row-1".to_string()),
    );
    assert_eq!(asked.run, None, "asking must never run the delete");
    assert_eq!(asked.state.pending.as_deref(), Some("row-1"));

    let cancelled = confirm_next(&asked.state, ConfirmEvent::Cancel);
    assert_eq!(cancelled.run, None);
    assert_eq!(cancelled.state.pending, None, "cancel clears the arm");

    let finished = confirm_next(&cancelled.state, ConfirmEvent::Finished);
    assert_eq!(finished.run, None);

    let stray = confirm_next(&finished.state, ConfirmEvent::Confirm);
    assert_eq!(stray.run, None, "an unarmed confirm must not run");
    assert!(!stray.state.running);

    for event in [
        ConfirmEvent::Cancel,
        ConfirmEvent::Confirm,
        ConfirmEvent::Finished,
    ] {
        let step = confirm_next(&ConfirmState::default(), event);
        assert_eq!(step.run, None, "no event runs from the mount state");
    }
}

#[wasm_bindgen_test]
fn test_confirm_next_runs_once_on_confirm() {
    // Mutant: `Confirm` returns `run = None` — a gate that never deletes.
    let step = confirm_next(
        &ConfirmState {
            pending: Some("row-1".to_string()),
            running: false,
        },
        ConfirmEvent::Confirm,
    );
    assert_eq!(step.run.as_deref(), Some("row-1"));
    assert_eq!(step.state.pending, None, "confirming consumes the arm");
    assert!(step.state.running, "confirming enters the in-flight state");
}

#[wasm_bindgen_test]
fn test_confirm_next_refuses_a_second_run_while_in_flight() {
    // Mutant: `Confirm` re-arms while `running` -> a double click deletes twice.
    let first = confirm_next(&armed("row-1"), ConfirmEvent::Confirm);
    assert_eq!(first.run.as_deref(), Some("row-1"));

    let second = confirm_next(&first.state, ConfirmEvent::Confirm);
    assert_eq!(second.run, None, "a second confirm must not run again");
    assert!(second.state.running);

    // Even a row re-armed mid-flight cannot be confirmed until `Finished`.
    let rearmed = confirm_next(&first.state, ConfirmEvent::Ask("row-2".to_string()));
    let third = confirm_next(&rearmed.state, ConfirmEvent::Confirm);
    assert_eq!(third.run, None, "in flight means in flight");
}

#[wasm_bindgen_test]
fn test_confirm_next_asks_one_row_at_a_time() {
    // Mutant: `Ask` appends instead of replacing -> confirm deletes row-1.
    let first = confirm_next(
        &ConfirmState::default(),
        ConfirmEvent::Ask("row-1".to_string()),
    );
    let second = confirm_next(&first.state, ConfirmEvent::Ask("row-2".to_string()));
    assert_eq!(
        second.state.pending.as_deref(),
        Some("row-2"),
        "re-asking replaces the armed row"
    );

    let confirmed = confirm_next(&second.state, ConfirmEvent::Confirm);
    assert_eq!(confirmed.run.as_deref(), Some("row-2"));
}

#[wasm_bindgen_test]
fn test_confirm_next_reruns_after_finished() {
    // Mutant: `Finished` clears `pending` but not `running`. Tests 1-4 all pass
    // on that mutant and production shows a permanently dead Delete button.
    let first = confirm_next(&armed("row-1"), ConfirmEvent::Confirm);
    assert_eq!(first.run.as_deref(), Some("row-1"));

    let finished = confirm_next(&first.state, ConfirmEvent::Finished);
    assert!(
        !finished.state.running,
        "Finished must clear the in-flight flag"
    );
    assert_eq!(finished.state.pending, None);

    let again = confirm_next(&finished.state, ConfirmEvent::Ask("row-1".to_string()));
    let second = confirm_next(&again.state, ConfirmEvent::Confirm);
    assert_eq!(
        second.run.as_deref(),
        Some("row-1"),
        "a second attempt after Finished must run again"
    );
}
