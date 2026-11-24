//! 子代理控制面共享的状态。

use serde::{Deserialize, Serialize};

/// Sub-agent 状态枚举。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// 待初始化。
    #[default]
    PendingInit,
    /// 运行中。
    Running,
    /// 已中断。
    Interrupted,
    /// 已关闭。
    Shutdown,
    /// 未找到。
    NotFound,
    /// 已完成（带可选消息）。
    Completed(Option<String>),
    /// 任务未被接受（带拒绝原因）。
    ///
    /// 与 [`Self::Errored`] 区分：拒绝发生在执行前，父代理既不应把它理解为
    /// 「执行中出错」，也不应理解为「做完了」。
    Refused(String),
    /// 出错（带错误信息）。
    Errored(String),
}
