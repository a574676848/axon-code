//! 统一 turn runtime 的审计事件类型与稳定 part ID 规则。
//!
//! 从 `agent_turn_runtime` 拆出：事件类型是纯数据契约，与 turn loop 的控制流无关，
//! 单独成文件后 loop 本身可以保持在单文件上限内。

use axon_domain::TurnId;
use serde::{Deserialize, Serialize};

use super::{provider_retry::now_ms, turn_id_serde};
use crate::{RuntimeTaskKind, RuntimeTaskOutcome};

/// 统一 turn runtime 事件类型。
///
/// 对齐 Codex 的工作流时间线粒度，增强用户可见的中间状态追踪。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentTurnEventKind {
    /// 开始新的 provider turn。
    TurnStarted,

    /// Provider streaming 开始（第一个 token 到达前）。
    StreamingStarted,

    /// 收到第一个 token（TTFT 指标点）。
    FirstTokenReceived,

    /// 开始接收 reasoning 内容。
    ReasoningStarted,

    /// Reasoning 阶段完成。
    ReasoningCompleted,

    /// Provider streaming 成功完成。
    ProviderCompleted,

    /// Provider transport 正在按策略重试。
    ProviderRetry,

    /// 开始执行工具调用。
    ToolExecutionStarted,

    /// 单个工具调用完成。
    ToolCallCompleted,

    /// 所有工具执行完成，准备继续下一轮。
    ToolResultsRecorded,

    /// 已进入等待外部审批或外部动作阶段。
    AwaitingExternal,

    /// 上下文压缩开始。
    CompactionStarted,

    /// 上下文压缩完成。
    CompactionCompleted,

    /// 整个 turn runtime 已完成。
    Completed,

    /// Turn 因错误失败。
    Failed,

    /// Turn 被用户中断。
    Interrupted,
}

/// 统一 turn runtime 审计事件。
///
/// 扩展对齐 Codex，增加 duration、metadata、phase 等维度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnEvent {
    /// 当前 turn ID。
    #[serde(with = "turn_id_serde")]
    pub turn_id: TurnId,

    /// Runtime task 类型。
    pub task_kind: RuntimeTaskKind,

    /// Runtime task outcome。
    pub task_outcome: RuntimeTaskOutcome,

    /// 事件类型。
    pub kind: AgentTurnEventKind,

    /// 稳定 transcript part ID，用于 live/replay/resume 合并同一渲染 cell。
    #[serde(default, rename = "partId")]
    pub part_id: String,

    /// 记录时间（Unix 毫秒）。
    pub recorded_at_ms: u64,

    /// 可审计摘要。
    pub summary: String,

    /// 事件耗时（毫秒），用于性能分析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// 相关工具名称（针对工具相关事件）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,

    /// Provider 返回的 tool call id，用于 live/replay 归并同一个工具生命周期。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,

    /// 工具或阶段状态。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

    /// 事件阶段标签（reasoning/tool/external）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,

    /// 扩展元数据（JSON 格式）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl AgentTurnEvent {
    /// 创建基础事件。
    pub fn new(turn_id: TurnId, kind: AgentTurnEventKind, summary: impl Into<String>) -> Self {
        let summary = summary.into();
        Self {
            turn_id,
            task_kind: RuntimeTaskKind::from_turn_event(kind, None),
            task_outcome: RuntimeTaskOutcome::from_turn_event(kind, None, Some(&summary), None),
            kind,
            part_id: agent_turn_event_part_id(turn_id, kind),
            recorded_at_ms: now_ms(),
            summary,
            duration_ms: None,
            tool_name: None,
            provider_call_id: None,
            status: None,
            phase: None,
            metadata: None,
        }
    }

    /// 设置事件耗时。
    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self.task_outcome = self.task_outcome.with_duration_ms(duration_ms);
        self
    }

    /// 设置工具名称。
    pub fn with_tool_name(mut self, tool_name: impl Into<String>) -> Self {
        let tool_name = tool_name.into();
        self.task_kind = RuntimeTaskKind::from_turn_event(self.kind, Some(&tool_name));
        self.tool_name = Some(tool_name);
        self
    }

    /// 设置 provider tool call id。
    pub fn with_provider_call_id(mut self, provider_call_id: impl Into<String>) -> Self {
        self.provider_call_id = Some(provider_call_id.into());
        self
    }

    /// 设置稳定 transcript part id。
    pub fn with_part_id(mut self, part_id: impl Into<String>) -> Self {
        self.part_id = part_id.into();
        self
    }

    /// 设置工具或阶段状态。
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        let status = status.into();
        self.task_outcome = RuntimeTaskOutcome::from_turn_event(
            self.kind,
            Some(&status),
            Some(&self.summary),
            self.duration_ms,
        );
        self.status = Some(status);
        self
    }

    /// 设置阶段标签。
    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }

    /// 设置元数据。
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Build the default stable transcript part ID for a runtime turn event.
pub fn agent_turn_event_part_id(turn_id: TurnId, kind: AgentTurnEventKind) -> String {
    format!(
        "lifecycle:{}:turn-{}",
        agent_turn_event_kind_code(kind),
        turn_id.0
    )
}

/// Build the stable transcript part ID for a provider retry lifecycle event.
pub fn agent_turn_provider_retry_part_id(turn_id: TurnId, attempt_index: usize) -> String {
    format!(
        "lifecycle:provider-retry:turn-{}:attempt-{attempt_index}",
        turn_id.0
    )
}

fn agent_turn_event_kind_code(kind: AgentTurnEventKind) -> &'static str {
    match kind {
        AgentTurnEventKind::TurnStarted => "turn-started",
        AgentTurnEventKind::StreamingStarted => "streaming-started",
        AgentTurnEventKind::FirstTokenReceived => "first-token-received",
        AgentTurnEventKind::ReasoningStarted => "reasoning-started",
        AgentTurnEventKind::ReasoningCompleted => "reasoning-completed",
        AgentTurnEventKind::ProviderCompleted => "provider-completed",
        AgentTurnEventKind::ProviderRetry => "provider-retry",
        AgentTurnEventKind::ToolExecutionStarted => "tool-execution",
        AgentTurnEventKind::ToolCallCompleted => "tool-execution",
        AgentTurnEventKind::ToolResultsRecorded => "tool-results-recorded",
        AgentTurnEventKind::AwaitingExternal => "awaiting-external",
        AgentTurnEventKind::CompactionStarted => "compaction",
        AgentTurnEventKind::CompactionCompleted => "compaction",
        AgentTurnEventKind::Completed => "completed",
        AgentTurnEventKind::Failed => "failed",
        AgentTurnEventKind::Interrupted => "interrupted",
    }
}
