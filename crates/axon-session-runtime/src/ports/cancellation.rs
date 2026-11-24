//! Session Action 取消信号端口。

/// 可轮询的取消信号。
pub trait CancellationSignal: Send + Sync {
    /// 返回当前 Action 是否已请求取消。
    fn is_cancelled(&self) -> bool;
}
