//! Focus ownership for the arm-then-confirm dialog.
//!
//! thaw 0.4.8's `FocusTrap` only installs the Esc listener (plus the
//! `aria-hidden` bookkeeping); it never moves focus and it does not wrap `Tab`
//! at the ends of the surface. So the shell owns three things here:
//!
//! 1. [`tab_target`] — the pure boundary decision (first/last wrap, empty list),
//!    exercised by `tests/dialog_focus.rs` in the wasm suite;
//! 2. [`trap_tab`] — the DOM half: collect the surface's focusable elements,
//!    ask [`tab_target`], and wrap;
//! 3. [`return_focus`] — hand focus back to the control that opened the dialog,
//!    or to the site's documented fallback when that control is gone (a deleted
//!    row unmounts its own Delete button one network round-trip later).
//!
//! thaw's `Button` exposes no `node_ref`, so a site hands over the element that
//! *wraps* its control; every helper resolves to the control inside.
//!
//! Nothing here ever steals focus from a live element: the moment some
//! connected element owns focus, the watcher stops.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use leptos::wasm_bindgen::JsCast;
use leptos::{ev::KeyboardEvent, prelude::*};
use web_sys::{Element, HtmlElement, Node};

/// Where one `Tab` press at the edge of the surface should land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTarget {
    /// Move to the first focusable element.
    First,
    /// Move to the last focusable element.
    Last,
    /// The press is interior (or there is nothing to wrap to): leave it alone.
    Inside,
}

/// The boundary decision for one `Tab` press.
///
/// `count` is the number of focusable elements in the surface, `active` the
/// index of the currently focused one (`None` when focus is outside the surface
/// or on the surface itself), and `reverse` whether Shift is held.
///
/// Total and side-effect free: an empty surface always answers
/// [`TabTarget::Inside`] — there is nowhere to wrap to, so the press must not be
/// swallowed — and an interior press never moves focus.
pub fn tab_target(count: usize, active: Option<usize>, reverse: bool) -> TabTarget {
    if count == 0 {
        return TabTarget::Inside;
    }
    match active {
        None if reverse => TabTarget::Last,
        None => TabTarget::First,
        Some(0) if reverse => TabTarget::Last,
        Some(index) if !reverse && index + 1 == count => TabTarget::First,
        Some(_) => TabTarget::Inside,
    }
}

/// The focusable descendants a surface owns, in document order.
const FOCUSABLE: &str = "button:not([disabled]),[href],input:not([disabled]),\
select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex=\"-1\"])";

/// How many watch ticks a focus hand-back may run for (≈2 s at 60 Hz).
const WATCH_TICKS: u32 = 120;

/// Wrap `Tab`/`Shift-Tab` at the surface's edges.
///
/// Called from the surface wrapper's `keydown`. Interior presses and an empty
/// surface are left to the browser, so a surface holding no control can never
/// trap focus permanently.
pub fn trap_tab(event: &KeyboardEvent, container: &Element) {
    if event.key() != "Tab" {
        return;
    }
    let Ok(nodes) = container.query_selector_all(FOCUSABLE) else {
        return;
    };
    let count = nodes.length() as usize;
    let active = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.active_element())
        .and_then(|focused| {
            (0..nodes.length())
                .find(|index| {
                    nodes
                        .get(*index)
                        .is_some_and(|node| node.is_same_node(Some(focused.as_ref())))
                })
                .map(|index| index as usize)
        });
    let node = match tab_target(count, active, event.shift_key()) {
        TabTarget::First => nodes.get(0),
        TabTarget::Last => nodes.get(nodes.length().saturating_sub(1)),
        TabTarget::Inside => return,
    };
    if let Some(node) = node
        && focus(&node)
    {
        event.prevent_default();
    }
}

/// Hand focus back once the surface has closed.
///
/// Two facts shape this: a cancelled gesture leaves the invoker mounted, while a
/// confirmed delete unmounts it only after the reload returns — and the user may
/// move focus in the meantime. So the first write happens immediately and a
/// bounded watch then follows the DOM, stopping the moment focus sits somewhere
/// that is neither the invoker nor the fallback (that is the user, and their
/// choice outranks ours):
///
/// - the invoker is usable → focus it and keep watching, because a confirmed
///   delete is about to unmount it;
/// - the invoker is gone or disabled → focus the fallback and stop;
/// - the user focused something → stop, untouched.
pub fn return_focus<Invoker, Fallback>(invoker: Option<Invoker>, fallback: Option<Fallback>)
where
    Invoker: JsCast,
    Fallback: JsCast,
{
    tick(
        invoker.map(|node| node.unchecked_into::<HtmlElement>()),
        fallback.map(|node| node.unchecked_into::<HtmlElement>()),
        Rc::new(Cell::new(WATCH_TICKS)),
        Rc::new(Cell::new(false)),
    );
}

/// One step of [`return_focus`]'s watch.
fn tick(
    invoker: Option<HtmlElement>,
    fallback: Option<HtmlElement>,
    frames: Rc<Cell<u32>>,
    pending: Rc<Cell<bool>>,
) {
    pending.set(false);
    if frames.get() == 0 {
        return;
    }
    frames.set(frames.get() - 1);
    let invoker_control = invoker.as_ref().and_then(control_of);
    let fallback_control = fallback.as_ref().and_then(control_of);
    if user_moved_focus(invoker.as_ref(), fallback.as_ref()) {
        return;
    }
    if let Some(control) = invoker_control {
        if control.focus().is_ok() {
            schedule(invoker, fallback, frames, pending);
        }
    } else if let Some(control) = fallback_control {
        let _ = control.focus();
    }
}

/// Re-arm the watch for one more tick.
///
/// Two schedulers race: an animation frame for a visible tab and a short timer
/// for a hidden one. MEASURED in the lane: a background CDP tab never runs its
/// animation-frame callbacks, so rAF alone would silently never hand focus back.
/// Whichever fires first clears `pending`; the other returns without a write.
fn schedule(
    invoker: Option<HtmlElement>,
    fallback: Option<HtmlElement>,
    frames: Rc<Cell<u32>>,
    pending: Rc<Cell<bool>>,
) {
    pending.set(true);
    {
        let (invoker, fallback) = (invoker.clone(), fallback.clone());
        let (frames, pending) = (frames.clone(), pending.clone());
        request_animation_frame(move || {
            if pending.get() {
                tick(invoker, fallback, frames, pending);
            }
        });
    }
    set_timeout(
        move || {
            if pending.get() {
                tick(invoker, fallback, frames, pending);
            }
        },
        Duration::from_millis(16),
    );
}

/// Whether focus sits on something that is neither the invoker nor the fallback.
///
/// The dialog's own control unmounts on close and the browser parks focus on
/// `body`; neither counts as the user having chosen, so the watch keeps going.
fn user_moved_focus(invoker: Option<&HtmlElement>, fallback: Option<&HtmlElement>) -> bool {
    let Some(active) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.active_element())
    else {
        return false;
    };
    let active: &Node = &active;
    if !active.is_connected() || matches!(active.node_name().as_str(), "BODY" | "HTML") {
        return false;
    }
    let ours = [invoker, fallback]
        .into_iter()
        .flatten()
        .any(|handle| handle.is_same_node(Some(active)) || handle.contains(Some(active)));
    if ours {
        return false;
    }
    // thaw's `Dialog` fades out: for a few frames after the gesture settles the
    // control that had focus is still connected inside the closing dialog. That
    // is not the user choosing, so it must not stop the watch.
    let closing = active
        .dyn_ref::<Element>()
        .is_some_and(|element| element.closest(".thaw-dialog").ok().flatten().is_some());
    !closing
}

/// The control inside a site's handle, when it can take focus.
fn control_of(handle: &HtmlElement) -> Option<HtmlElement> {
    if !handle.is_connected() {
        return None;
    }
    let control: HtmlElement = if handle.tag_name() == "BUTTON" {
        handle.clone()
    } else {
        handle
            .query_selector("button")
            .ok()
            .flatten()?
            .unchecked_into()
    };
    (!control.has_attribute("disabled")).then_some(control)
}

/// Focus an element only when it can actually take it.
fn focus(node: &Node) -> bool {
    let Some(element) = node.dyn_ref::<HtmlElement>() else {
        return false;
    };
    if element.has_attribute("disabled") {
        return false;
    }
    element.focus().is_ok()
}
