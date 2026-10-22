//! Memory 核心运行时对外导出面。

pub use crate::memory_adapter::{
    MemoryAdapterCapabilities, MemoryAdapterHit, MemoryAdapterKind, MemoryAdapterRuntime,
    MemoryRecallApplicabilityScope, MemoryRecallHit, MemoryRecallQuery, MemoryRecallResult,
    MemoryRecallSourceTrace, MemoryStoreAdapter,
};
pub use crate::memory_auto_candidate::{
    MEMORY_AUTO_CANDIDATE_EVIDENCE_PREFIX, MEMORY_AUTO_CANDIDATE_SIGNATURE_PREFIX,
    MemoryAutoCandidateInput, MemoryAutoCandidateReport, MemoryAutoCandidateRuntime,
    MemoryAutoCandidateSkipReason,
};
pub use crate::memory_auto_scheduler::{
    MEMORY_AUTO_SCHEDULER_REASON_NO_CANDIDATE, MEMORY_AUTO_SCHEDULER_REASON_NOT_PLANNED,
    MEMORY_AUTO_SCHEDULER_REASON_PERSISTED, MEMORY_AUTO_SCHEDULER_SOURCE_TRACE_PREFIX,
    MemoryAutoSchedulerInput, MemoryAutoSchedulerReport, MemoryAutoSchedulerRuntime,
};
pub use crate::memory_receipt::{
    MemoryReceipt, MemoryReceiptOperation, MemoryReceiptRuntime, MemoryReceiptStatus,
};
