//! 结构化 cell 身份的单点归属。

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

/// 结构化渲染 cell 的稳定身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CellId(NonZeroU64);

impl CellId {
    /// 使用非零原始值构造 cell id。
    #[must_use]
    pub const fn new(raw: NonZeroU64) -> Self {
        Self(raw)
    }

    /// 返回非零原始值。
    #[must_use]
    pub const fn get(self) -> NonZeroU64 {
        self.0
    }
}

/// 单调分配 `CellId` 的轻量分配器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellIdAllocator {
    next: NonZeroU64,
}

impl Default for CellIdAllocator {
    fn default() -> Self {
        Self {
            next: NonZeroU64::MIN,
        }
    }
}

impl CellIdAllocator {
    /// 分配下一个 cell id。
    #[must_use]
    pub fn next_cell_id(&mut self) -> CellId {
        let current = self.next;
        self.next = current.checked_add(1).unwrap_or(NonZeroU64::MIN);
        CellId::new(current)
    }
}
