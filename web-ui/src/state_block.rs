//! Which block of the screen a view state renders.

use leptos::prelude::*;
use thaw::{MessageBar, MessageBarBody, MessageBarIntent, Spinner};

use crate::core_screens::MemoriesView;

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
