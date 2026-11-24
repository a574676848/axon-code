//! Agent Loop Memory 写入触发器。
//!
//! 目标态写入逻辑由 `axon-memory-write` 承担，runtime 只保留事件转接面。

pub use axon_memory_write::{
    MEMORY_WRITE_TRIGGER_DEFAULT_IMPORTANCE_BASIS_POINTS, MemoryWriteScope,
    MemoryWriteTriggerInput, MemoryWriteTriggerReport, MemoryWriteTriggerRuntime,
    memory_scope_refs, memory_scope_refs_with_namespace, personal_memory_scope,
};
