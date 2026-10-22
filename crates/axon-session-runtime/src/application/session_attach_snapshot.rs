//! Attach snapshot 分块器。

use serde_json::Value;

/// Snapshot 分块器，避免一次 IPC 写入超大 JSON。
#[derive(Debug, Clone)]
pub struct SnapshotChunker {
    chunk_bytes: usize,
}

impl SnapshotChunker {
    /// 创建分块器；零值使用安全默认值。
    #[must_use]
    pub fn new(chunk_bytes: usize) -> Self {
        Self {
            chunk_bytes: chunk_bytes.max(1),
        }
    }

    /// 将 JSON snapshot 编码为 UTF-8 分块。
    #[must_use]
    pub fn split(&self, snapshot: &Value) -> Vec<String> {
        let encoded = serde_json::to_string(snapshot).unwrap_or_else(|_| "null".to_owned());
        encoded
            .as_bytes()
            .chunks(self.chunk_bytes)
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect()
    }
}
