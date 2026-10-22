//! 子执行运行器的传输无关边界。
//!
//! 子执行请求只描述 Agent 运行所需的语义输入，不携带 CLI、TUI 或渠道状态。
//! 宿主负责把自身的 provider、持久化和调度能力适配到该端口。

use crate::{
    AgentPath, AgentStatus, ProviderStructuredOutputContract, SubagentForkContextReport,
    TokenUsageTelemetry,
};
use axon_domain::DisclosurePolicy;

/// 子执行模型选择意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildExecutionModelIntent {
    /// 继承父 Agent 当前模型与渠道链。
    Inherit,
    /// 使用宿主配置的快速委托模型。
    FastDelegate,
}

/// 子执行请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildExecutionInput {
    /// 子代理规范路径。
    pub task_path: AgentPath,
    /// 初始任务消息。
    pub message: String,
    /// 本轮普通 Coding turn 的可选结构化输出契约。
    pub structured_output: Option<ProviderStructuredOutputContract>,
    /// 替换默认 system 身份的固定角色提示词。
    pub system_prompt_override: Option<String>,
    /// 是否向本轮模型暴露并允许执行工具。
    pub tools_enabled: bool,
    /// 子代理继承上下文报告。
    pub fork_context: SubagentForkContextReport,
    /// 创建时间戳，毫秒。
    pub started_at_ms: u64,
    /// 可选运行时间上限。
    pub max_runtime_ms: Option<u64>,
    /// 子执行模型选择意图。
    pub model_intent: ChildExecutionModelIntent,
}

impl ChildExecutionInput {
    /// 子代理默认只可读取当前 Workspace Memory，避免跨边界泄露 Global Memory。
    #[must_use]
    pub const fn memory_disclosure_policy(&self) -> DisclosurePolicy {
        DisclosurePolicy::Workspace
    }
}

/// 子执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildExecutionStatus {
    /// 子代理正常完成。
    Completed,
    /// 任务未被接受：执行前的准入裁决（策略门 / 治理门）拒绝了这次子执行。
    Refused,
    /// 子代理被中断。
    Interrupted,
    /// 子代理执行失败。
    Errored,
}

/// 子执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildExecutionOutput {
    /// 执行状态。
    pub status: ChildExecutionStatus,
    /// 有界完成摘要。
    pub summary: String,
    /// 本轮通过 `builtin.memory_write` 实际持久化的 Memory 数量。
    pub memory_write_count: usize,
    /// 子代理 provider turn 的真实 token 用量。
    pub token_usage: Option<TokenUsageTelemetry>,
    /// 子代理实际使用的模型展示名。
    pub model: Option<String>,
    /// 本轮是否被输出 token 上限截断过。
    ///
    /// 预算耗尽独立于失败：它是「这一轮被掐断了」而不是「这次执行出错了」，
    /// 因此必须单独承载，让终态的退出原因用固定措辞说出这件事（合同 §5.2.3）。
    pub budget_exhausted: bool,
}

impl ChildExecutionOutput {
    /// 投影为 Agent 控制面状态。
    #[must_use]
    pub fn agent_status(&self) -> AgentStatus {
        match self.status {
            ChildExecutionStatus::Completed => AgentStatus::Completed(Some(self.summary.clone())),
            ChildExecutionStatus::Refused => AgentStatus::Refused(self.summary.clone()),
            ChildExecutionStatus::Interrupted => AgentStatus::Interrupted,
            ChildExecutionStatus::Errored => AgentStatus::Errored(self.summary.clone()),
        }
    }
}

/// 子执行运行器。
///
/// 该端口只执行一次子 Agent turn；并发调度、生命周期持久化与结果投递由宿主负责。
pub trait ChildExecutionRunner: Send + Sync {
    /// 执行子任务。返回 `None` 表示当前宿主不接管此任务。
    fn run(&self, input: ChildExecutionInput) -> Option<ChildExecutionOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_execution_output_maps_terminal_status_to_agent_status() {
        let completed = ChildExecutionOutput {
            status: ChildExecutionStatus::Completed,
            summary: "已完成".to_owned(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        };
        let interrupted = ChildExecutionOutput {
            status: ChildExecutionStatus::Interrupted,
            ..completed.clone()
        };
        let refused = ChildExecutionOutput {
            status: ChildExecutionStatus::Refused,
            ..completed.clone()
        };
        let errored = ChildExecutionOutput {
            status: ChildExecutionStatus::Errored,
            ..completed.clone()
        };

        assert_eq!(
            completed.agent_status(),
            AgentStatus::Completed(Some("已完成".to_owned()))
        );
        assert_eq!(interrupted.agent_status(), AgentStatus::Interrupted);
        assert_eq!(
            refused.agent_status(),
            AgentStatus::Refused("已完成".to_owned())
        );
        assert_eq!(
            errored.agent_status(),
            AgentStatus::Errored("已完成".to_owned())
        );
    }

    #[test]
    fn child_execution_defaults_to_workspace_memory_disclosure() {
        let input = ChildExecutionInput {
            task_path: AgentPath::root(),
            message: "检查".to_owned(),
            structured_output: None,
            system_prompt_override: None,
            tools_enabled: true,
            fork_context: SubagentForkContextReport {
                messages: Vec::new(),
                original_count: 0,
                retained_count: 0,
                dropped_count: 0,
                reasons: Vec::new(),
            },
            started_at_ms: 0,
            max_runtime_ms: None,
            model_intent: ChildExecutionModelIntent::Inherit,
        };

        assert_eq!(
            input.memory_disclosure_policy(),
            axon_domain::DisclosurePolicy::Workspace
        );
    }
}
