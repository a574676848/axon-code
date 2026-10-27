use serde::{Deserialize, Serialize};

use crate::diff::FileChange;
use axon_domain::{UserAnswerItem, UserQuestionItem};

use super::memory_surface::memory_surface_text_is_user_visible;
use super::snapshot::WireResumeSnapshot;

/// 当前 Axon wire schema 版本。
pub const WIRE_SCHEMA_VERSION: &str = "axon.wire.v3";
/// Provider retry 生命周期 kind。
pub const WIRE_LIFECYCLE_KIND_PROVIDER_RETRY: &str = "ProviderRetry";
/// Provider generation 开始生命周期 kind。
pub const WIRE_LIFECYCLE_KIND_STREAMING_STARTED: &str = "StreamingStarted";
/// Provider generation 完成生命周期 kind。
pub const WIRE_LIFECYCLE_KIND_STREAMING_COMPLETED: &str = "StreamingCompleted";

/// Wire schema 版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireSchemaVersion(pub String);

impl Default for WireSchemaVersion {
    fn default() -> Self {
        Self(WIRE_SCHEMA_VERSION.to_owned())
    }
}

/// 跨端事件相关 ID。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireCorrelationId(pub String);
/// Wire session 引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireSessionRef {
    /// Axon session ID。
    pub session_id: String,
    /// 当前 run ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// 父 session ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}
/// Wire event envelope。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireEventEnvelope {
    /// schema 版本。
    pub schema_version: WireSchemaVersion,
    /// 事件 ID。
    pub event_id: WireCorrelationId,
    /// 关联 ID。
    pub correlation_id: WireCorrelationId,
    /// 会话引用。
    pub session: WireSessionRef,
    /// 会话内单调递增序号。
    pub sequence: u64,
    /// 记录时间，Unix 毫秒。
    pub recorded_at_ms: i64,
    /// 事件载荷。
    pub kind: WireEventKind,
}
/// Wire event 载荷集合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum WireEventKind {
    /// turn 生命周期。
    TurnLifecycle(WireTurnLifecycle),
    /// Provider 请求实时状态。
    ProviderStatus(super::provider_status::WireProviderStatus),
    /// assistant 消息。
    AssistantMessage(WireAssistantMessage),
    /// 模型 reasoning part 开始。
    ReasoningStart(WireReasoningStart),
    /// 模型 reasoning 增量；provider reasoning 字段在进入 Axon wire 前归一化到这里。
    ReasoningDelta(WireReasoningDelta),
    /// 模型 reasoning part 结束。
    ReasoningEnd(WireReasoningEnd),
    /// user 消息。
    UserMessage(WireUserMessage),
    /// 工具调用。
    ToolCall(WireToolCall),
    /// 工具结果。
    ToolResult(WireToolResult),
    /// 文件变更。
    FileChange(WireFileChangeEvent),
    /// 计划更新。
    PlanUpdate(WirePlanUpdate),
    /// 评审状态。
    ReviewStatus(WireReviewStatus),
    /// Memory 状态。
    MemoryStatus(WireMemoryStatus),
    /// Skill 状态。
    SkillStatus(WireSkillStatus),
    /// Verify 状态。
    VerifyStatus(WireVerifyStatus),
    /// 后台治理状态。
    GovernanceStatus(WireGovernanceStatus),
    /// Token budget 预警。
    TokenBudgetReminder(super::token_budget_event::WireTokenBudgetReminder),
    /// 上下文代际窗口轮转。
    ContextRollover(super::token_budget_event::WireContextRollover),
    /// 用量更新。
    UsageUpdate(WireUsageUpdate),
    /// TUI 三区域 resume snapshot。
    ResumeSnapshot(Box<WireResumeSnapshot>),
    /// runtime 错误。
    RuntimeError(WireRuntimeError),
    /// 用户问答交互。
    UserQuestion(WireUserQuestion),
}

/// 用户问答交互事件载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireUserQuestion {
    /// 稳定 part ID（例如 "part:question:<call_id>"）。
    pub part_id: String,
    /// 工具调用 ID。
    pub call_id: String,
    /// 问答状态：pending / extended / settled / suspended。
    pub status: String,
    /// 提出的问题列表。
    pub questions: Vec<UserQuestionItem>,
    /// 作答列表（若已结算）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<UserAnswerItem>,
    /// 挂起交接文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// 截止时间毫秒时间戳。
    pub deadline_at_ms: i64,
}

/// 生命周期阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireLifecyclePhase {
    /// 普通 turn。
    RegularTurn,
    /// 压缩任务。
    Compact,
    /// 评审任务。
    Review,
    /// 用户 shell 任务。
    UserShell,
    /// 后台治理任务。
    Governance,
}

/// 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireLifecycleStatus {
    /// 已开始。
    Started,
    /// 运行中。
    Running,
    /// 等待输入或审批。
    Waiting,
    /// 已完成。
    Completed,
    /// 已失败。
    Failed,
    /// 已取消。
    Cancelled,
    /// 已中止。
    Aborted,
    /// 已阻断。
    Blocked,
}

impl WireTurnLifecycle {
    /// 是否为本轮的终态生命周期条目（完成或失败收尾）。
    #[must_use]
    pub fn is_terminal_run(&self) -> bool {
        self.kind.as_deref() == Some("RunLifecycle")
            && self.phase == WireLifecyclePhase::RegularTurn
            && matches!(
                self.status,
                WireLifecycleStatus::Completed
                    | WireLifecycleStatus::Failed
                    | WireLifecycleStatus::Cancelled
                    | WireLifecycleStatus::Aborted
                    | WireLifecycleStatus::Blocked
            )
    }
}

/// Turn 生命周期事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTurnLifecycle {
    /// transcript part ID，作为 lifecycle cell 的稳定身份。
    pub part_id: String,
    /// turn ID。
    pub turn_id: String,
    /// 生命周期阶段。
    pub phase: WireLifecyclePhase,
    /// 生命周期状态。
    pub status: WireLifecycleStatus,
    /// 可展示标题。
    pub title: String,
    /// 稳定事件类别。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// TUI 展示阶段，不参与模型上下文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_phase: Option<String>,
    /// 相关工具、模型或运行单元名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// provider transport 尝试序号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_index: Option<u64>,
    /// 可展示摘要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 本轮降级告警（含人格装配告警）；为空时不序列化。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// 耗时毫秒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// 运行单元开始时间戳，毫秒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// 运行单元总 token 消耗。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// 缓存读取占完整输入的命中率，单位为 basis points。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate_basis_points: Option<u16>,
}

/// 判断是否进入新的 Provider generation；失败 generation 在此边界整体丢弃。
#[must_use]
pub fn is_provider_retry_lifecycle(lifecycle: &WireTurnLifecycle) -> bool {
    lifecycle.status == WireLifecycleStatus::Running
        && lifecycle.kind.as_deref() == Some(WIRE_LIFECYCLE_KIND_PROVIDER_RETRY)
}

/// Turn summary。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTurnSummary {
    /// transcript part ID，作为 turn summary cell 的稳定身份。
    pub part_id: String,
    /// turn ID。
    pub turn_id: String,
    /// turn 耗时毫秒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// turn token 总量。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// turn 的完整 token usage，用于一致恢复收尾分隔线。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<WireUsageUpdate>,
    /// 最近 generation 平均首字耗时毫秒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_avg_ms: Option<u64>,
    /// 最近 generation 输出速率，单位为 0.1 tok/s。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_tps_x10: Option<u64>,
}

/// Assistant 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireAssistantMessage {
    /// transcript part ID，作为 TUI 渲染、回放、持久化的稳定身份。
    pub part_id: String,
    /// 消息 ID。
    pub message_id: String,
    /// 文本内容。
    pub content: String,
    /// 是否为增量。
    #[serde(default)]
    pub delta: bool,
}

/// 模型 reasoning part 开始。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireReasoningStart {
    /// transcript part ID，作为 reasoning cell 的稳定身份。
    pub part_id: String,
    /// reasoning block ID，用于把连续 delta 合并到同一个 part。
    pub reasoning_id: String,
}

/// 模型 reasoning 增量；provider reasoning 字段在进入 Axon wire 前归一化到这里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireReasoningDelta {
    /// transcript part ID，作为 reasoning cell 的稳定身份。
    pub part_id: String,
    /// reasoning block ID，用于把连续 delta 合并到同一个 cell。
    pub reasoning_id: String,
    /// 文本增量。
    pub delta: String,
}

/// 模型 reasoning part 结束。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireReasoningEnd {
    /// transcript part ID，作为 reasoning cell 的稳定身份。
    pub part_id: String,
    /// reasoning block ID。
    pub reasoning_id: String,
}

/// 模型 reasoning 历史消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireReasoningMessage {
    /// transcript part ID，作为 reasoning cell 的稳定身份。
    pub part_id: String,
    /// reasoning block ID。
    pub reasoning_id: String,
    /// reasoning 完整文本。
    pub content: String,
    /// 是否已结束。
    #[serde(default = "default_done")]
    pub done: bool,
}

fn default_done() -> bool {
    true
}

/// User 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireUserMessage {
    /// transcript part ID，作为 TUI 渲染、回放、持久化的稳定身份。
    pub part_id: String,
    /// 消息 ID。
    pub message_id: String,
    /// 文本内容。
    pub content: String,
    /// 入站时落地的本地附件绝对路径。
    ///
    /// 附件不在正文里，却属于这条消息本身；随消息一起进入事件流，
    /// 回放端才能在不额外查询历史索引的前提下还原完整的用户输入。
    #[serde(default)]
    pub local_files: Vec<String>,
}

/// 工具调用事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireToolCall {
    /// transcript part ID，作为 TUI 渲染、回放、持久化的稳定身份。
    pub part_id: String,
    /// 工具调用 ID。
    pub tool_call_id: String,
    /// 本地审批请求 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    /// 工具名。
    pub tool_name: String,
    /// 工具类别。
    pub tool_kind: String,
    /// 可展示摘要。
    pub summary: String,
    /// 调用参数摘要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_summary: Option<String>,
    /// 模型自述的调用目的概要；只用于展示，缺失时界面回落到参数投影。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// 本次调用的执行权限编码。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_permission: Option<String>,
}

/// 工具结果事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireToolResult {
    /// transcript part ID，作为 TUI 渲染、回放、持久化的稳定身份。
    pub part_id: String,
    /// 工具调用 ID。
    pub tool_call_id: String,
    /// 工具名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// 工具类别。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<String>,
    /// 状态。
    pub status: WireLifecycleStatus,
    /// 可展示摘要。
    pub summary: String,
    /// 结果片段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// 输出流名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    /// 输出序号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// 稳定进度阶段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_phase: Option<String>,
    /// 已完成项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items_completed: Option<u64>,
    /// 总项数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items_total: Option<u64>,
    /// 当前项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_item: Option<String>,
    /// 执行耗时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// 工具结构化 metadata。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// 稳定错误类型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
    /// 是否执行了文件系统写入。
    #[serde(default)]
    pub filesystem_write_performed: bool,
}

/// 文件变更事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireFileChangeEvent {
    /// transcript part ID，作为 TUI 渲染、回放、持久化的稳定身份。
    pub part_id: String,
    /// 变更来源工具 transcript part ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_part_id: Option<String>,
    /// 变更来源工具调用 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 文件路径。
    pub path: String,
    /// 文件变更协议。
    pub change: FileChange,
}

/// 计划更新。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePlanUpdate {
    /// transcript part ID，作为 plan cell 的稳定身份。
    pub part_id: String,
    /// 计划标题。
    pub title: String,
    /// 计划更新说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// 计划步骤。
    pub steps: Vec<WirePlanStep>,
}

/// 计划步骤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePlanStep {
    /// 步骤内容。
    pub step: String,
    /// 步骤状态。
    pub status: String,
    /// 循环序号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<u32>,
    /// 诊断数量。
    #[serde(default)]
    pub diagnostic_count: usize,
    /// 失败步骤数量。
    #[serde(default)]
    pub failing_step_count: usize,
}

/// Review 状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireReviewStatus {
    /// transcript part ID，作为 review cell 的稳定身份。
    pub part_id: String,
    /// 主题。
    pub subject: String,
    /// 状态。
    pub status: WireLifecycleStatus,
    /// 详情。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 详情列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

/// Memory 状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireMemorySurfaceEntry {
    /// 记忆稳定 ID 或 ref-only 引用。
    pub id: String,
    /// 面向用户展示的短标题。
    pub title: String,
    /// 面向用户展示的短摘要。
    pub summary: String,
    /// 召回相关度，范围由产生端归一化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,
}

impl WireMemorySurfaceEntry {
    /// 判断 memory surface entry 是否允许进入用户可见 transcript。
    #[must_use]
    pub fn is_user_visible(&self) -> bool {
        memory_surface_text_is_user_visible(&self.id)
            && memory_surface_text_is_user_visible(&self.title)
            && memory_surface_text_is_user_visible(&self.summary)
    }
}

impl PartialEq for WireMemorySurfaceEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.title == other.title
            && self.summary == other.summary
            && self.relevance_score.map(f32::to_bits) == other.relevance_score.map(f32::to_bits)
    }
}

impl Eq for WireMemorySurfaceEntry {}

/// Memory 状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireMemoryStatus {
    /// Memory 子系统主题。
    pub subject: String,
    /// 生命周期状态。
    pub status: WireLifecycleStatus,
    /// Memory 子系统结果编码。
    pub outcome: String,
    /// 可展示摘要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 引用 ID 或 ref-only 路径。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// 本轮已浮出到 UI 的记忆摘要；不携带 full context。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaced_memories: Vec<WireMemorySurfaceEntry>,
    /// 端侧可观测 metadata，不进入模型上下文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Skill 状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireSkillStatus {
    /// Skill 名称。
    pub skill_name: String,
    /// 生命周期状态。
    pub status: WireLifecycleStatus,
    /// Skill 子系统结果编码。
    pub outcome: String,
    /// 风险等级。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    /// 可展示摘要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 生成或更新的材料引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material_ref: Option<String>,
    /// 端侧可观测 metadata，不进入模型上下文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Verify 状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireVerifyStatus {
    /// 验证器名称。
    pub verifier: String,
    /// 生命周期状态。
    pub status: WireLifecycleStatus,
    /// Verify 子系统结果编码。
    pub outcome: String,
    /// 诊断数量。
    #[serde(default)]
    pub diagnostic_count: u64,
    /// 失败步骤数量。
    #[serde(default)]
    pub failing_step_count: u64,
    /// 循环次数。
    #[serde(default)]
    pub cycle_count: u64,
    /// 收敛状态。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convergence: Option<String>,
    /// 端侧可观测 metadata，不进入模型上下文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Post-run governance 状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireGovernanceStatus {
    /// job ID。
    pub job_id: String,
    /// 状态。
    pub status: WireLifecycleStatus,
    /// status 文件路径。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_path: Option<String>,
    /// 失败原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[path = "event_terminal.rs"]
mod event_terminal;
pub use event_terminal::{WireRuntimeError, WireUsageUpdate};

#[cfg(test)]
mod tests {
    use super::WireToolCall;

    fn tool_call() -> WireToolCall {
        WireToolCall {
            part_id: "tool-part:1".to_owned(),
            tool_call_id: "call-1".to_owned(),
            request_id: None,
            tool_name: "builtin.read_file".to_owned(),
            tool_kind: "read_file".to_owned(),
            summary: "读取文件".to_owned(),
            arguments_summary: None,
            intent: None,
            execution_permission: Some("allowed".to_owned()),
        }
    }

    #[test]
    fn tool_call_serializes_execution_permission() {
        let value = serde_json::to_value(tool_call()).expect("工具调用事件应可序列化");

        assert_eq!(value["executionPermission"], "allowed");
    }
}
