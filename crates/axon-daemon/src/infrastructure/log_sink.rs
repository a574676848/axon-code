//! Daemon 日志端口投影。

/// 结构化日志 sink。
pub trait LogSink: Send + Sync {
    /// 写入一条结构化日志。
    fn append(&self, level: &str, message: &str);
}
