//! TUI 实时运行事件模型。

use axon_domain::ToolExecutionPermission;
use axon_protocol::diff::FileChange;
use axon_protocol::wire::{WireEventEnvelope, WireResumeSnapshot, WireRuntimeError};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::surface::{
    event_envelope::{RuntimeCorrelationId, RuntimeEventEnvelope, RuntimeEventSource},
    runtime_payload::{RuntimePayload, RuntimePayloadKind},
    watchers::RuntimeWatcherEvent,
};

use super::super::run_output_diff::AgentTuiDiffLine;
use super::run_output_models::{AgentTuiToolKind, AgentTuiToolReceipt, AgentTuiToolReceiptDetails};

/// TUI 运行中实时事件。
pub type AgentTuiRuntimeEvent = RuntimeEventEnvelope<AgentTuiRuntimePayload>;

/// TUI runtime typed payload。
#[derive(Debug)]
pub enum AgentTuiRuntimePayload {
    /// 批量 assistant 文本增量。
    BatchedAssistantDelta {
        /// assistant part id。
        part_id: String,
        /// 文本增量。
        deltas: Vec<String>,
    },
    /// reasoning part 开始。
    ReasoningPartStarted {
        /// reasoning part id。
        part_id: String,
    },
    /// 批量 reasoning 文本增量。
    BatchedReasoningDelta {
        /// reasoning part id。
        part_id: String,
        /// 文本增量。
        deltas: Vec<String>,
    },
    /// reasoning part 结束。
    ReasoningPartEnded {
        /// reasoning part id。
        part_id: String,
    },
    /// 普通 TUI runtime progress wire 事件。
    WireEvent {
        /// 跨端 wire 事件。
        event: Box<WireEventEnvelope>,
    },
    /// TUI 三区域 wire resume snapshot 已加载。
    WireResumeSnapshot(Box<WireResumeSnapshot>),
    /// wire runtime 错误。
    WireRuntimeError(WireRuntimeError),
    /// 外部 watcher 状态事件。
    Watcher(RuntimeWatcherEvent),
    /// TUI 请求取消当前 runtime turn。
    CancelRequested(AgentTuiCancelPayload),
    /// TUI 请求中断当前 runtime turn。
    InterruptRequested(AgentTuiInterruptPayload),
    /// Daemon durable turn 订阅连接状态。
    SubscriptionPhase(DaemonSubscriptionPhase),
    /// 跨进程非持久运行时指标观测帧。
    Observation(axon_domain::RuntimeObservationFrame),
}

/// CLI/TUI 到 Daemon 的 durable turn 订阅连接状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DaemonSubscriptionPhase {
    /// 订阅已就绪。
    Connected,
    /// 实时流异常或空闲，正在恢复连接。
    Reconnecting,
    /// 已确认 durable Interaction 仍在处理，正在补齐事件。
    Resyncing,
}

impl DaemonSubscriptionPhase {
    /// 返回对外展示的稳定阶段名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Resyncing => "resyncing",
        }
    }
}

impl RuntimePayload for AgentTuiRuntimePayload {
    fn payload_kind(&self) -> RuntimePayloadKind {
        match self {
            Self::BatchedAssistantDelta { .. } => Self::KIND_BATCHED_ASSISTANT_DELTA,
            Self::ReasoningPartStarted { .. } => Self::KIND_REASONING_PART_STARTED,
            Self::BatchedReasoningDelta { .. } => Self::KIND_BATCHED_REASONING_DELTA,
            Self::ReasoningPartEnded { .. } => Self::KIND_REASONING_PART_ENDED,
            Self::WireEvent { .. } => Self::KIND_WIRE_EVENT,
            Self::WireResumeSnapshot(_) => Self::KIND_WIRE_RESUME_SNAPSHOT,
            Self::WireRuntimeError(_) => Self::KIND_WIRE_RUNTIME_ERROR,
            Self::Watcher(_) => Self::KIND_WATCHER,
            Self::CancelRequested(_) => Self::KIND_CANCEL_REQUESTED,
            Self::InterruptRequested(_) => Self::KIND_INTERRUPT_REQUESTED,
            Self::SubscriptionPhase(_) => Self::KIND_SUBSCRIPTION_PHASE,
            Self::Observation(_) => Self::KIND_OBSERVATION,
        }
    }
}

impl AgentTuiRuntimePayload {
    pub(crate) const KIND_BATCHED_ASSISTANT_DELTA: RuntimePayloadKind =
        RuntimePayloadKind::new("batched_assistant_delta");
    pub(crate) const KIND_REASONING_PART_STARTED: RuntimePayloadKind =
        RuntimePayloadKind::new("reasoning_part_started");
    pub(crate) const KIND_BATCHED_REASONING_DELTA: RuntimePayloadKind =
        RuntimePayloadKind::new("batched_reasoning_delta");
    pub(crate) const KIND_REASONING_PART_ENDED: RuntimePayloadKind =
        RuntimePayloadKind::new("reasoning_part_ended");
    pub(crate) const KIND_WIRE_EVENT: RuntimePayloadKind = RuntimePayloadKind::new("wire_event");
    pub(crate) const KIND_WIRE_RESUME_SNAPSHOT: RuntimePayloadKind =
        RuntimePayloadKind::new("wire_resume_snapshot");
    pub(crate) const KIND_WIRE_RUNTIME_ERROR: RuntimePayloadKind =
        RuntimePayloadKind::new("wire_runtime_error");
    pub(crate) const KIND_WATCHER: RuntimePayloadKind = RuntimePayloadKind::new("watcher");
    pub(crate) const KIND_CANCEL_REQUESTED: RuntimePayloadKind =
        RuntimePayloadKind::new("cancel_requested");
    pub(crate) const KIND_INTERRUPT_REQUESTED: RuntimePayloadKind =
        RuntimePayloadKind::new("interrupt_requested");
    pub(crate) const KIND_SUBSCRIPTION_PHASE: RuntimePayloadKind =
        RuntimePayloadKind::new("subscription_phase");
    pub(crate) const KIND_OBSERVATION: RuntimePayloadKind = RuntimePayloadKind::new("observation");

    #[cfg(test)]
    pub(crate) const ALL_KINDS: &'static [RuntimePayloadKind] = &[
        Self::KIND_BATCHED_ASSISTANT_DELTA,
        Self::KIND_REASONING_PART_STARTED,
        Self::KIND_BATCHED_REASONING_DELTA,
        Self::KIND_REASONING_PART_ENDED,
        Self::KIND_WIRE_EVENT,
        Self::KIND_WIRE_RESUME_SNAPSHOT,
        Self::KIND_WIRE_RUNTIME_ERROR,
        Self::KIND_WATCHER,
        Self::KIND_CANCEL_REQUESTED,
        Self::KIND_INTERRUPT_REQUESTED,
        Self::KIND_SUBSCRIPTION_PHASE,
        Self::KIND_OBSERVATION,
    ];

    fn correlation_id(&self) -> Option<RuntimeCorrelationId> {
        match self {
            Self::WireEvent { event, .. } => Some(RuntimeCorrelationId::from_key(
                event.correlation_id.0.clone(),
            )),
            Self::BatchedAssistantDelta { part_id, .. } => {
                Some(RuntimeCorrelationId::from_key(part_id.clone()))
            }
            Self::ReasoningPartStarted { .. }
            | Self::BatchedReasoningDelta { .. }
            | Self::ReasoningPartEnded { .. }
            | Self::WireResumeSnapshot(_)
            | Self::WireRuntimeError(_)
            | Self::Watcher(_)
            | Self::CancelRequested(_)
            | Self::InterruptRequested(_)
            | Self::SubscriptionPhase(_)
            | Self::Observation(_) => None,
        }
    }
}

/// 创建 runtime event。
#[must_use]
pub fn agent_tui_runtime_event(
    source: RuntimeEventSource,
    payload: AgentTuiRuntimePayload,
) -> AgentTuiRuntimeEvent {
    let correlation_id = payload.correlation_id();
    let mut event = RuntimeEventEnvelope::new(source, payload);
    if let Some(correlation_id) = correlation_id {
        event = event.with_correlation_id(correlation_id);
    }
    event
}

/// 创建 Axon CLI runtime event。
#[must_use]
pub fn axon_cli_runtime_event(payload: AgentTuiRuntimePayload) -> AgentTuiRuntimeEvent {
    agent_tui_runtime_event(RuntimeEventSource::AxonCli, payload)
}

/// 创建 TUI 控制命令 runtime event。
#[must_use]
pub fn tui_control_runtime_event(payload: AgentTuiRuntimePayload) -> AgentTuiRuntimeEvent {
    agent_tui_runtime_event(RuntimeEventSource::TuiControlCommand, payload)
}

/// 创建 runtime driver runtime event。
#[must_use]
pub fn runtime_driver_event(payload: AgentTuiRuntimePayload) -> AgentTuiRuntimeEvent {
    agent_tui_runtime_event(RuntimeEventSource::RuntimeSubmissionDriver, payload)
}

/// 创建 watcher runtime event。
#[must_use]
pub fn watcher_runtime_event(payload: AgentTuiRuntimePayload) -> AgentTuiRuntimeEvent {
    agent_tui_runtime_event(RuntimeEventSource::Watcher, payload)
}

/// Runtime cancel payload。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiCancelPayload {
    /// 用户可见原因。
    pub reason: String,
}

impl AgentTuiCancelPayload {
    /// 创建 cancel payload。
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Runtime interrupt payload。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiInterruptPayload {
    /// 用户可见原因。
    pub reason: String,
}

impl AgentTuiInterruptPayload {
    /// 创建 interrupt payload。
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// TUI 工具完成视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolCompletionView {
    /// transcript part ID。
    pub part_id: String,
    /// Provider returned tool call id。
    pub provider_call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 原始工具类型编码。
    pub kind: String,
    /// 工具语义类型。
    pub semantic_kind: AgentTuiToolKind,
    /// 执行状态。
    pub status: String,
    /// 工具结果摘要。
    pub summary: String,
    /// 工具结构化 metadata。
    pub metadata: Option<serde_json::Value>,
    /// 错误类型。
    pub error_type: Option<String>,
    /// 是否执行了文件系统写入。
    pub filesystem_write_performed: bool,
    /// 真实工具执行耗时，单位毫秒。
    pub duration_ms: Option<u64>,
}

impl AgentTuiToolCompletionView {
    /// 转换为 TUI 工具回执，复用既有 settle/render 路径。
    #[must_use]
    pub fn into_receipt(self) -> AgentTuiToolReceipt {
        AgentTuiToolReceipt::with_id(
            self.part_id,
            self.provider_call_id,
            self.tool_name,
            AgentTuiToolReceiptDetails {
                kind: self.semantic_kind,
                status: self.status,
                summary: self.summary,
                metadata: self.metadata,
                error_type: self.error_type,
                filesystem_write_performed: self.filesystem_write_performed,
            },
        )
    }
}

/// TUI 工具执行视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolExecutionView {
    /// 审批请求 ID。
    pub request_id: u64,
    /// transcript part ID。
    pub part_id: String,
    /// provider tool call id。
    pub provider_call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 原始工具类型编码。
    pub kind: String,
    /// 工具语义类型。
    pub semantic_kind: AgentTuiToolKind,
    /// 工具参数预览。
    pub arguments_preview: String,
    /// 模型自述的调用目的概要；只用于折叠态展示，缺失时回落参数投影。
    pub intent: Option<String>,
    /// 本次调用的执行权限。
    pub execution_permission: ToolExecutionPermission,
}

/// TUI 工具 diff 快照视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolDiffSnapshotView {
    /// 稳定 diff part id。
    pub id: String,
    /// 被该 diff 快照替换或关联的 tool transcript part ID。
    pub tool_part_id: Option<String>,
    /// provider tool call id。
    pub provider_call_id: String,
    /// Diff 标题。
    pub title: String,
    /// 文件变更列表。
    pub changes: BTreeMap<PathBuf, FileChange>,
    /// 文本写入次数。
    pub text_writes_applied: usize,
    /// 重命名次数。
    pub renames_applied: usize,
}

impl AgentTuiToolDiffSnapshotView {
    /// 转换为一等 Diff 行。
    #[must_use]
    pub fn into_diff(self) -> AgentTuiDiffLine {
        AgentTuiDiffLine::with_id(
            self.id,
            self.title,
            self.changes,
            self.text_writes_applied,
            self.renames_applied,
        )
    }
}

/// TUI 工具 stdout/stderr 增量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolOutputChunk {
    /// transcript part ID。
    pub part_id: String,
    /// provider tool call id。
    pub provider_call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 工具语义类型。
    pub semantic_kind: AgentTuiToolKind,
    /// stdout 或 stderr。
    pub stream: String,
    /// 输出序号。
    pub sequence: u64,
    /// 输出文本。
    pub text: String,
}

/// TUI 工具进度视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolProgressView {
    /// transcript part ID。
    pub part_id: String,
    /// provider tool call id。
    pub provider_call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 工具语义类型。
    pub semantic_kind: AgentTuiToolKind,
    /// 稳定阶段。
    pub phase: String,
    /// 已完成项。
    pub items_completed: Option<u64>,
    /// 总项数。
    pub items_total: Option<u64>,
    /// 当前项。
    pub current_item: Option<String>,
    /// 工具执行耗时。
    pub elapsed_ms: Option<u64>,
    /// 简短说明。
    pub message: Option<String>,
}
