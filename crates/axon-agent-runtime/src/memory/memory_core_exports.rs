//! Memory 核心运行时对外导出面。

pub use crate::memory_adapter::{
    MemoryAdapterCapabilities, MemoryAdapterHit, MemoryAdapterKind, MemoryAdapterRuntime,
    MemoryRecallApplicabilityScope, MemoryRecallHit, MemoryRecallQuery, MemoryRecallResult,
    MemoryRecallSourceTrace, MemoryStoreAdapter,
};
pub use crate::memory_receipt::{
    MemoryReceipt, MemoryReceiptOperation, MemoryReceiptRuntime, MemoryReceiptStatus,
};
