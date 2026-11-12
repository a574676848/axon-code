//! 后台执行的外部会话投影句柄。
//!
//! 后台执行（Goal 与 cron）跑的是独立子代理，逐字稿由运行侧直接写进执行会话。
//! 这样一来，收口事件（`turn_completed`）已经由运行侧在回合结束的当下写入，
//! 宿主侧再追加一遍就会让界面把同一轮收口两次：分割线与摘要会多出一条。
//!
//! 因此把「投影」显式交给 Worker：它既提供逐条落库的通道，也报告本轮终态是否
//! 已由运行侧写完。Worker 只在**没人写过**时补追加，绝不做双重落库。

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::ports::TurnEventSink;

/// Worker 执行期的会话投影。
#[derive(Clone)]
pub struct ExecutionProjection {
    sink: Arc<dyn TurnEventSink>,
    terminal_written: Arc<AtomicBool>,
}

impl ExecutionProjection {
    /// 绑定投影通道与「终态已写」标记。
    ///
    /// 标记由宿主持有并交给运行侧：运行侧每写一条终态就置位一次，
    /// 因此 Worker 读到的是**真实是否写过**，不是按类型猜出来的。
    #[must_use]
    pub fn new(sink: Arc<dyn TurnEventSink>, terminal_written: Arc<AtomicBool>) -> Self {
        Self {
            sink,
            terminal_written,
        }
    }

    /// 逐条事件的落点。
    #[must_use]
    pub fn sink(&self) -> &dyn TurnEventSink {
        self.sink.as_ref()
    }

    /// 本轮终态是否已由运行侧落库。
    #[must_use]
    pub fn terminal_written(&self) -> bool {
        self.terminal_written.load(Ordering::Acquire)
    }
}

impl std::fmt::Debug for ExecutionProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionProjection")
            .field("terminal_written", &self.terminal_written())
            .finish_non_exhaustive()
    }
}
