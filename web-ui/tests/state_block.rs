//! `block_kind` classifies every Memories view state.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::core_screens::MemoriesView;
use web_ui::state_block::{BlockKind, block_kind};

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
