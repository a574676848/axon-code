//! 事件日志错误。

/// 事件日志的失败原因。
#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    /// 文件系统操作失败。
    #[error("事件日志 IO 失败：{0}")]
    Io(#[from] std::io::Error),
    /// 编解码失败（zstd 或 JSON）。
    #[error("事件日志编解码失败：{0}")]
    Codec(String),
    /// 追加批次的首个序号与期望不一致，或批次内部不连续。
    #[error("事件序号不连续：期望 {expected}，实际 {actual}")]
    SequenceGap {
        /// 期望的序号。
        expected: u64,
        /// 实际读到的序号。
        actual: u64,
    },
    /// 格式版本不被本构建支持。
    #[error("事件日志格式版本不支持：{0}")]
    UnsupportedVersion(u32),
    /// 已提交内容损坏，不能安全解释。
    #[error("事件日志已损坏：{0}")]
    Corrupt(String),
    /// 另一个写句柄已持有该日志。
    #[error("事件日志已被其他写入者占用")]
    Locked,
    /// 追加空批次。
    #[error("事件批次不能为空")]
    EmptyBatch,
}
