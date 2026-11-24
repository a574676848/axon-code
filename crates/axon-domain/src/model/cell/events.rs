//! `RuntimeCell` 事件与状态类型。

use crate::{AttachmentId, ByteChunkId, CellId, ExecutionRequestId, ModelProfileId, ToolCallId};

/// `RuntimeCell` 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCellState {
    /// 草稿态。
    Draft,
    /// 已入队。
    Queued,
    /// 正在运行。
    Running,
    /// 等待工具完成。
    WaitingTool(ToolCallId),
    /// 被策略阻塞，可恢复或可解释。
    Blocked(BlockReason),
    /// 已暂停，保留可恢复前态。
    Paused(PausedCellState),
    /// 已完成。
    Completed(CellResult),
    /// 已失败。
    Failed(CellFailure),
    /// 已取消。
    Cancelled(CancelReason),
}

/// 暂停前的可恢复状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PausedCellState {
    /// 正在运行。
    Running,
    /// 等待工具。
    WaitingTool(ToolCallId),
    /// 被策略阻塞。
    Blocked(BlockReason),
}

impl PausedCellState {
    pub(super) fn into_runtime_state(self) -> RuntimeCellState {
        match self {
            Self::Running => RuntimeCellState::Running,
            Self::WaitingTool(tool_call_id) => RuntimeCellState::WaitingTool(tool_call_id),
            Self::Blocked(reason) => RuntimeCellState::Blocked(reason),
        }
    }
}

/// 阻塞原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// 权限被拒绝。
    PermissionDenied,
    /// 缺少模型能力。
    ModelCapabilityMissing,
    /// 工具输出或上下文预算超限。
    ContextBudgetExceeded,
    /// 等待本机用户确认。
    NeedsLocalReview,
    /// 模型回答预览为空。
    EmptyAgentResponse,
}

/// Apply 执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyExecutionStatus {
    /// 已构造受保护的客户端写入请求。
    WriteRequestReady,
    /// 客户端确认写入完成。
    ClientWriteCompleted,
    /// 客户端执行写入失败。
    ClientWriteFailed,
    /// 写入被门禁或客户端能力阻断。
    Blocked,
}

/// 完成结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellResult {
    /// 中文摘要。
    pub summary: String,
}

/// 失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellFailure {
    /// 中文摘要。
    pub message: String,
}

/// 取消原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelReason {
    /// 中文摘要。
    pub message: String,
}

/// 输出排空状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputDrainState {
    /// 没有待排空输出。
    Drained,
    /// 仍有待排空输出。
    Pending {
        /// 最后一段已知输出。
        last_chunk_id: ByteChunkId,
    },
}

/// cell 输出关闭状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellClosureState {
    /// 尚未收到关闭边界。
    Open,
    /// 已收到关闭边界。
    Closed {
        /// 关闭前最后一段输出。
        last_chunk_id: Option<ByteChunkId>,
    },
}

/// MCP server startup 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStartupStatus {
    /// 已发现配置，等待本机启动器处理。
    Discovered,
    /// 已进入启动流程。
    Starting,
    /// 启动失败。
    Failed,
}

/// `RuntimeCell` 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCellEvent {
    /// cell 创建。
    Created {
        /// Cell ID。
        cell_id: CellId,
    },
    /// 进入运行态。
    Started {
        /// Cell ID。
        cell_id: CellId,
    },
    /// 工具调用开始。
    ToolCallStarted {
        /// Cell ID。
        cell_id: CellId,
        /// 工具调用 ID。
        tool_call_id: ToolCallId,
    },
    /// 工具调用增量。
    ToolCallDelta {
        /// Cell ID。
        cell_id: CellId,
        /// 工具调用 ID。
        tool_call_id: ToolCallId,
        /// 增量摘要。
        summary: String,
    },
    /// 工具调用完成。
    ToolCallCompleted {
        /// Cell ID。
        cell_id: CellId,
        /// 工具调用 ID。
        tool_call_id: ToolCallId,
    },
    /// 工具调用失败。
    ToolCallFailed {
        /// Cell ID。
        cell_id: CellId,
        /// 工具调用 ID。
        tool_call_id: ToolCallId,
        /// 失败摘要。
        message: String,
    },
    /// 视觉理解输出已进入 cell timeline。
    VisionUnderstandingProjected {
        /// Cell ID。
        cell_id: CellId,
        /// 执行视觉理解的模型。
        model_id: ModelProfileId,
        /// 参与视觉理解的附件 ID。
        attachment_ids: Vec<AttachmentId>,
        /// 审计摘要。
        audit_summary: String,
    },
    /// MCP server startup 状态已进入 cell timeline。
    McpServerStartupProjected {
        /// Cell ID。
        cell_id: CellId,
        /// 插件名称。
        server_id: String,
        /// MCP 配置引用。
        config_ref: String,
        /// startup 状态。
        status: McpServerStartupStatus,
        /// 审计摘要。
        audit_summary: String,
    },
    /// Agent 模型回答已进入 cell timeline。
    AgentResponseProjected {
        /// Cell ID。
        cell_id: CellId,
        /// 回答预览。
        preview: String,
        /// 完整回答字符数。
        full_content_chars: usize,
        /// 预览是否已截断。
        truncated: bool,
        /// 回答来源引用。
        source_ref: String,
    },
    /// Agent Pane apply 执行结果已进入 cell timeline。
    ApplyExecutionProjected {
        /// Cell ID。
        cell_id: CellId,
        /// 目标路径。
        path: String,
        /// 执行关联请求 ID。
        execution_request_id: ExecutionRequestId,
        /// 执行状态。
        status: ApplyExecutionStatus,
        /// 审计摘要。
        audit_summary: String,
        /// 来源引用。
        source_ref: String,
    },
    /// 终端输出进入待排空状态。
    OutputDrainPending {
        /// Cell ID。
        cell_id: CellId,
        /// 最后一段已知输出。
        last_chunk_id: ByteChunkId,
    },
    /// 终端输出已排空。
    OutputDrained {
        /// Cell ID。
        cell_id: CellId,
        /// 最后一段已排空输出。
        last_chunk_id: ByteChunkId,
    },
    /// cell 输出关闭。
    Closed {
        /// Cell ID。
        cell_id: CellId,
        /// 关闭前最后一段输出。
        last_chunk_id: Option<ByteChunkId>,
    },
    /// cell 被阻塞。
    Blocked {
        /// Cell ID。
        cell_id: CellId,
        /// 阻塞原因。
        reason: BlockReason,
    },
    /// cell 已暂停。
    Paused {
        /// Cell ID。
        cell_id: CellId,
        /// 暂停前的可恢复状态。
        previous_state: PausedCellState,
    },
    /// cell 已恢复。
    Resumed {
        /// Cell ID。
        cell_id: CellId,
        /// 恢复后的状态。
        resumed_state: RuntimeCellState,
    },
    /// cell 完成。
    Completed {
        /// Cell ID。
        cell_id: CellId,
    },
    /// cell 失败。
    Failed {
        /// Cell ID。
        cell_id: CellId,
    },
    /// cell 取消。
    Cancelled {
        /// Cell ID。
        cell_id: CellId,
    },
}
