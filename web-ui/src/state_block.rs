//! Which block of the Memories screen a view state renders.

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
