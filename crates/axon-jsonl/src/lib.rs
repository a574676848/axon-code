//! 统一的 JSONL 分片存储合同。
//!
//! 组件只理解 UTF-8 JSONL 字节行，不解释业务记录 schema。业务侧继续使用
//! 逻辑路径 `name.jsonl`，物理数据固定拆分为 `name.000000.jsonl` 起的分片。

mod store;

pub use store::{DEFAULT_SHARD_MAX_BYTES, SHARD_SEQUENCE_WIDTH, ShardedJsonlStore};

#[cfg(test)]
mod tests;
