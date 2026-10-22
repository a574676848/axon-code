//! 打断机制验证与加固（AXON-P3-1）。
//!
//! 端到端验证 Esc 流式中途打断、打断后状态一致性。
//! 对标 Codex TUI 的 Interrupt 机制 + 人工纠偏入口。

use serde::{Deserialize, Serialize};

/// 打断来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptSource {
    /// 用户主动 Esc。
    UserKeyboard,
    /// 超时自动中断。
    Timeout,
    /// 系统信号（SIGINT/SIGTERM）。
    SystemSignal,
    /// 远程控制中断。
    RemoteControl,
}

/// 打断清理状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptCleanupStatus {
    /// 清理成功，无残留状态。
    Clean,
    /// 有残留 tool call，已记录但未执行。
    PartialToolCall,
    /// 有残留流式输出，已截断。
    TruncatedStream,
    /// 有残留后台任务，已取消但可能延迟完成。
    BackgroundTaskPending,
}

/// 打断上下文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptContext {
    /// 打断来源。
    pub source: InterruptSource,
    /// 打断时间戳（毫秒）。
    pub timestamp_ms: u64,
    /// 被中断的 cell ID（如果有）。
    pub cell_id: Option<u64>,
    /// 被中断的 job ID（如果有）。
    pub job_id: Option<u64>,
    /// 清理状态。
    pub cleanup_status: InterruptCleanupStatus,
    /// 是否允许恢复。
    pub resumable: bool,
    /// 中断原因描述。
    pub reason: String,
}

impl InterruptContext {
    /// 创建用户键盘中断上下文。
    #[must_use]
    pub fn user_keyboard(timestamp_ms: u64, reason: impl Into<String>) -> Self {
        Self {
            source: InterruptSource::UserKeyboard,
            timestamp_ms,
            cell_id: None,
            job_id: None,
            cleanup_status: InterruptCleanupStatus::Clean,
            resumable: false,
            reason: reason.into(),
        }
    }

    /// 创建超时中断上下文。
    #[must_use]
    pub fn timeout(timestamp_ms: u64, cell_id: u64, reason: impl Into<String>) -> Self {
        Self {
            source: InterruptSource::Timeout,
            timestamp_ms,
            cell_id: Some(cell_id),
            job_id: None,
            cleanup_status: InterruptCleanupStatus::Clean,
            resumable: true,
            reason: reason.into(),
        }
    }

    /// 标记有残留 tool call。
    #[must_use]
    pub fn with_partial_tool_call(mut self) -> Self {
        self.cleanup_status = InterruptCleanupStatus::PartialToolCall;
        self.resumable = false;
        self
    }

    /// 标记有截断的流式输出。
    #[must_use]
    pub fn with_truncated_stream(mut self) -> Self {
        self.cleanup_status = InterruptCleanupStatus::TruncatedStream;
        self
    }

    /// 标记有后台任务待清理。
    #[must_use]
    pub fn with_background_task_pending(mut self, job_id: u64) -> Self {
        self.cleanup_status = InterruptCleanupStatus::BackgroundTaskPending;
        self.job_id = Some(job_id);
        self
    }
}

/// 打断恢复检查点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptCheckpoint {
    /// 检查点 ID。
    pub id: String,
    /// 创建时间戳（毫秒）。
    pub created_at_ms: u64,
    /// 关联的 cell ID。
    pub cell_id: u64,
    /// 关联的 turn ID。
    pub turn_id: Option<String>,
    /// 最后完整的 tool result（如果有）。
    pub last_tool_result: Option<String>,
    /// 会话状态快照。
    pub session_snapshot: String,
}

impl InterruptCheckpoint {
    /// 创建新检查点。
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        created_at_ms: u64,
        cell_id: u64,
        session_snapshot: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            created_at_ms,
            cell_id,
            turn_id: None,
            last_tool_result: None,
            session_snapshot: session_snapshot.into(),
        }
    }

    /// 设置 turn ID。
    #[must_use]
    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    /// 设置最后的 tool result。
    #[must_use]
    pub fn with_last_tool_result(mut self, result: impl Into<String>) -> Self {
        self.last_tool_result = Some(result.into());
        self
    }
}

/// 打断恢复策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptRecoveryStrategy {
    /// 从最后检查点恢复。
    FromCheckpoint,
    /// 重新开始整个 turn。
    RestartTurn,
    /// 放弃并清理。
    Abandon,
}

/// 打断验证结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptValidation {
    /// 是否干净中断（无残留状态）。
    pub is_clean: bool,
    /// 是否可恢复。
    pub is_resumable: bool,
    /// 检查的状态项。
    pub checked_items: Vec<String>,
    /// 发现的问题。
    pub issues: Vec<String>,
    /// 建议的恢复策略。
    pub recommended_strategy: InterruptRecoveryStrategy,
}

impl InterruptValidation {
    /// 创建验证结果。
    #[must_use]
    pub fn new() -> Self {
        Self {
            is_clean: true,
            is_resumable: true,
            checked_items: Vec::new(),
            issues: Vec::new(),
            recommended_strategy: InterruptRecoveryStrategy::FromCheckpoint,
        }
    }

    /// 添加检查项。
    pub fn check_item(&mut self, item: impl Into<String>) {
        self.checked_items.push(item.into());
    }

    /// 添加问题。
    pub fn add_issue(&mut self, issue: impl Into<String>) {
        self.issues.push(issue.into());
        self.is_clean = false;
    }

    /// 标记不可恢复。
    pub fn mark_not_resumable(&mut self) {
        self.is_resumable = false;
        self.recommended_strategy = InterruptRecoveryStrategy::Abandon;
    }

    /// 推荐重启 turn。
    pub fn recommend_restart(&mut self) {
        self.recommended_strategy = InterruptRecoveryStrategy::RestartTurn;
    }
}

impl Default for InterruptValidation {
    fn default() -> Self {
        Self::new()
    }
}

/// 打断机制验证器。
///
/// 端到端验证打断机制的健壮性。
#[derive(Debug, Default, Clone)]
pub struct InterruptValidator {
    /// 验证历史。
    validations: Vec<InterruptValidation>,
}

impl InterruptValidator {
    /// 创建新验证器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 验证打断后的状态一致性。
    #[must_use]
    pub fn validate_interrupt_consistency(
        &mut self,
        context: &InterruptContext,
    ) -> InterruptValidation {
        let mut validation = InterruptValidation::new();

        // 检查清理状态
        validation.check_item("cleanup_status");
        match context.cleanup_status {
            InterruptCleanupStatus::Clean => {
                // 理想状态，无需处理
            }
            InterruptCleanupStatus::PartialToolCall => {
                validation.add_issue("残留未完成的 tool call，可能导致状态不一致");
                validation.mark_not_resumable();
            }
            InterruptCleanupStatus::TruncatedStream => {
                validation.add_issue("流式输出被截断，可能丢失部分响应");
                if context.resumable {
                    validation.recommend_restart();
                }
            }
            InterruptCleanupStatus::BackgroundTaskPending => {
                validation.add_issue("后台任务仍在运行，需等待清理完成");
            }
        }

        // 检查恢复能力
        validation.check_item("resumability");
        if !context.resumable && matches!(context.source, InterruptSource::UserKeyboard) {
            validation.add_issue("用户主动中断，会话状态可能不完整");
        }

        // 检查 cell/job 一致性
        validation.check_item("cell_job_consistency");
        if context.cell_id.is_some() && context.job_id.is_none() {
            validation.add_issue("cell 存在但 job 缺失，可能未正确关联长任务");
        }

        self.validations.push(validation.clone());
        validation
    }

    /// 获取验证历史。
    #[must_use]
    pub fn validation_history(&self) -> &[InterruptValidation] {
        &self.validations
    }

    /// 统计清理成功率。
    #[must_use]
    pub fn clean_interrupt_rate(&self) -> f64 {
        if self.validations.is_empty() {
            return 100.0;
        }
        let clean_count = self.validations.iter().filter(|v| v.is_clean).count();
        (clean_count as f64 / self.validations.len() as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_validation_recommends_abandon_for_partial_tool_call() {
        let mut validator = InterruptValidator::new();
        let ctx = InterruptContext::user_keyboard(1000, "test").with_partial_tool_call();
        let validation = validator.validate_interrupt_consistency(&ctx);

        assert_eq!(
            validation.recommended_strategy,
            InterruptRecoveryStrategy::Abandon
        );
        assert!(!validation.is_resumable);
        assert_eq!(validator.validation_history().len(), 1);
    }

    #[test]
    fn interrupt_validation_recommends_restart_for_truncated_stream() {
        let mut validator = InterruptValidator::new();
        let ctx = InterruptContext::timeout(1000, 42, "超时").with_truncated_stream();
        let validation = validator.validate_interrupt_consistency(&ctx);

        assert_eq!(
            validation.recommended_strategy,
            InterruptRecoveryStrategy::RestartTurn
        );
        assert!(validation.is_resumable);
    }
}
