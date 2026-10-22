//! 用户问答交互运行时等待端口与预算类型定义。

use crate::RunCancelToken;
use axon_domain::UserQuestionRecord;

/// 等待预算配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestionWaitBudget {
    /// 初始单窗口上限毫秒数。
    pub initial_window_ms: u64,
    /// 退避倍数。
    pub backoff_factor: u32,
    /// 总等待预算毫秒数。
    pub total_budget_ms: u64,
    /// 取消与关停检查唤醒周期毫秒数。
    pub poll_interval_ms: u64,
}

impl Default for QuestionWaitBudget {
    fn default() -> Self {
        Self {
            initial_window_ms: super::user_question_limits::DEFAULT_WAIT_WINDOW_MS,
            backoff_factor: super::user_question_limits::BACKOFF_FACTOR,
            total_budget_ms: super::user_question_limits::TOTAL_BUDGET_MS,
            poll_interval_ms: super::user_question_limits::CANCELLATION_POLL_INTERVAL_MS,
        }
    }
}

/// 等待终局结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionWaitResult {
    /// 预算内收到用户答案，直接作为工具返回值回灌继续执行。
    Answered {
        /// 用户作答的 JSON 字符串。
        answer_json: String,
        /// 作答来源渠道。
        channel: Option<String>,
    },
    /// 总等待预算耗尽，任务经 AwaitExternal 挂起。
    Suspended {
        /// 挂起交接文案。
        handoff_text: String,
    },
    /// 等待被主动中断、取消或 Daemon 关停。
    Aborted {
        /// 中断原因。
        reason: String,
    },
}

/// 宿主提供的用户提问等待端口。
pub trait UserQuestionPort: Send + Sync {
    /// 注册等待并进入有界等待，直到作答、挂起或被中断。
    fn wait_for_answer(
        &self,
        record: &UserQuestionRecord,
        budget: QuestionWaitBudget,
        cancel_token: &RunCancelToken,
    ) -> Result<QuestionWaitResult, String>;
}
