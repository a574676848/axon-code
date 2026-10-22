//! RenderViewport 内容缓存键与 resize 保留策略。

use crate::surface::{cell::RenderCell, cell_id::CellId};

pub(crate) const RESIZE_WIDTH_TOLERANCE_COLUMNS: u16 = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ResizeReflowCacheDelta {
    pub(crate) events: u64,
    pub(crate) retained_entries: u64,
    pub(crate) evicted_entries: u64,
}

impl ResizeReflowCacheDelta {
    pub(crate) fn merge(self, other: Self) -> Self {
        Self {
            events: self.events.saturating_add(other.events),
            retained_entries: self.retained_entries.saturating_add(other.retained_entries),
            evicted_entries: self.evicted_entries.saturating_add(other.evicted_entries),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CellCacheKey {
    pub(crate) cell_id: CellId,
    pub(crate) revision: u64,
    pub(crate) width: u16,
    pub(crate) expanded: bool,
    pub(crate) transcript_key: u64,
}

impl CellCacheKey {
    pub(crate) fn from_cell(cell: &dyn RenderCell, expanded: bool, width: u16) -> Self {
        Self {
            cell_id: cell.id(),
            revision: cell.revision(),
            width,
            expanded,
            transcript_key: cell.transcript_key(),
        }
    }

    pub(crate) fn within_resize_tolerance(self, width: u16) -> bool {
        width_delta(self.width, width) <= RESIZE_WIDTH_TOLERANCE_COLUMNS
    }
}

pub(crate) const fn width_delta(left: u16, right: u16) -> u16 {
    left.abs_diff(right)
}
