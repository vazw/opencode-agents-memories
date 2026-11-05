//! `block_kind` classifies every Memories view state, and `state_block_model`
//! acts as a pure presentation oracle for StateBlock rendering.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::core_screens::MemoriesView;
use web_ui::state_block::{
    BlockKind, StateBlockModel, StateDisplay, block_kind, state_block_model,
};

#[wasm_bindgen_test]
fn test_block_kind_classifies_every_state() {
    assert_eq!(block_kind(&MemoriesView::Loading), BlockKind::Loading);
    assert_eq!(
        block_kind(&MemoriesView::Empty { total: 0 }),
        BlockKind::Empty
    );
    assert_eq!(
        block_kind(&MemoriesView::Error("x".to_string())),
        BlockKind::Error
    );
    assert_eq!(
        block_kind(&MemoriesView::Loaded {
            rows: vec![],
            total: 0,
            page: 1,
            page_size: 20,
            total_pages: 0
        }),
        BlockKind::Ready
    );
}

#[wasm_bindgen_test]
fn test_state_block_model_loading() {
    // Distinguishing input: BlockKind::Loading with None message and Some("loading-testid")
    let model = state_block_model(BlockKind::Loading, None, Some("loading-testid"));
    assert_eq!(
        model,
        StateBlockModel {
            display: StateDisplay::Spinner {
                label: "Loading…".to_string(),
            },
            testid: Some("loading-testid"),
        }
    );
}

#[wasm_bindgen_test]
fn test_state_block_model_empty() {
    // Distinguishing input: BlockKind::Empty with None message
    let model = state_block_model(BlockKind::Empty, None, None);
    assert_eq!(
        model,
        StateBlockModel {
            display: StateDisplay::Info {
                message: "No records found.".to_string(),
            },
            testid: None,
        }
    );
}

#[wasm_bindgen_test]
fn test_state_block_model_error() {
    // Distinguishing input: BlockKind::Error with custom "Network failure" message
    let model = state_block_model(
        BlockKind::Error,
        Some("Network failure"),
        Some("error-testid"),
    );
    assert_eq!(
        model,
        StateBlockModel {
            display: StateDisplay::Error {
                message: "Network failure".to_string(),
            },
            testid: Some("error-testid"),
        }
    );
}

#[wasm_bindgen_test]
fn test_state_block_model_ready() {
    // Distinguishing input: BlockKind::Ready
    let model = state_block_model(BlockKind::Ready, None, None);
    assert_eq!(
        model,
        StateBlockModel {
            display: StateDisplay::Ready,
            testid: None,
        }
    );
}
