//! Viewport 行级锚点原语。

/// 指向 viewport 内某个 cell 的稳定行级位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollAnchor {
    pub(crate) cell_id: u64,
    pub(crate) row_offset_in_cell: usize,
}
