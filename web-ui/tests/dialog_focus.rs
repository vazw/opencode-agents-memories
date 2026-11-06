#![cfg(target_arch = "wasm32")]

//! The dialog's Tab-boundary decision.
//!
//! `trap_tab` is the DOM half; this is the half that decides whether a `Tab`
//! press should be swallowed and where it should land. Every case below is a
//! keyboard-only path a user can hit in the confirm dialog, so the decision is
//! pinned as a pure function.

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::dialog_focus::{TabTarget, tab_target};

#[wasm_bindgen_test]
fn dialog_focus_helper_boundaries() {
    // Distinguishing input: a three-control surface, focused on its last
    // control, Tab forward. A non-wrapping implementation answers Inside and
    // lets focus leave the dialog.
    assert_eq!(tab_target(3, Some(2), false), TabTarget::First);
    // Reverse from the first control wraps to the last.
    assert_eq!(tab_target(3, Some(0), true), TabTarget::Last);
    // Interior presses are left to the browser.
    assert_eq!(tab_target(3, Some(1), false), TabTarget::Inside);
    assert_eq!(tab_target(3, Some(1), true), TabTarget::Inside);
    // Focus outside the surface (or on the surface itself) enters it.
    assert_eq!(tab_target(3, None, false), TabTarget::First);
    assert_eq!(tab_target(3, None, true), TabTarget::Last);
    // A single-control surface wraps onto itself rather than escaping.
    assert_eq!(tab_target(1, Some(0), false), TabTarget::First);
    assert_eq!(tab_target(1, Some(0), true), TabTarget::Last);
    // An empty surface has nowhere to wrap to: the press must not be swallowed.
    assert_eq!(tab_target(0, None, false), TabTarget::Inside);
    assert_eq!(tab_target(0, None, true), TabTarget::Inside);
    // An out-of-range index is not an edge of the live list.
    assert_eq!(tab_target(3, Some(9), false), TabTarget::Inside);
    assert_eq!(tab_target(3, Some(9), true), TabTarget::Inside);
}
