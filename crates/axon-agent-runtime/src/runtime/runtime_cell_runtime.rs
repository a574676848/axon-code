use axon_domain::{
    AxonResult, BlockReason, ByteChunkId, CancelReason, CellFailure, CellId, CellResult,
    ModelProfileId, PaneId, PermissionSnapshotId, RuntimeCell, ToolCallId,
};

use crate::{AgentApplyExecutionProjection, AgentResponseProjection, McpStartupProjection};

/// RuntimeCell 管理器。
#[derive(Debug, Default, Clone)]
pub struct RuntimeCellRuntime {
    next_cell_id: u64,
}

impl RuntimeCellRuntime {
    /// 创建 RuntimeCell。
    pub fn create_cell(
        &mut self,
        pane_id: PaneId,
        cwd: impl Into<String>,
        model_profile_id: ModelProfileId,
        permission_snapshot_id: PermissionSnapshotId,
    ) -> RuntimeCell {
        self.next_cell_id += 1;
        RuntimeCell::new(
            CellId(self.next_cell_id),
            pane_id,
            cwd,
            model_profile_id,
            permission_snapshot_id,
        )
    }

    /// 启动 cell。
    pub fn start_cell(&self, cell: &mut RuntimeCell) -> AxonResult<()> {
        cell.start()
    }

    /// 开始等待工具。
    pub fn start_tool_call(
        &self,
        cell: &mut RuntimeCell,
        tool_call_id: ToolCallId,
    ) -> AxonResult<()> {
        cell.wait_for_tool(tool_call_id)
    }

    /// 工具完成。
    pub fn complete_tool_call(&self, cell: &mut RuntimeCell) -> AxonResult<()> {
        cell.tool_completed()
    }

    /// 记录工具调用增量。
    pub fn record_tool_delta(
        &self,
        cell: &mut RuntimeCell,
        tool_call_id: ToolCallId,
        summary: impl Into<String>,
    ) -> AxonResult<()> {
        cell.tool_delta(tool_call_id, summary)
    }

    /// 记录工具调用失败。
    pub fn fail_tool_call(
        &self,
        cell: &mut RuntimeCell,
        tool_call_id: ToolCallId,
        message: impl Into<String>,
    ) -> AxonResult<()> {
        cell.tool_failed(
            tool_call_id,
            CellFailure {
                message: message.into(),
            },
        )
    }

    /// 将视觉理解输出投影进 RuntimeCell timeline。
    pub fn project_vision_understanding(
        &self,
        cell: &mut RuntimeCell,
        input: &crate::PreparedVisionInput,
    ) -> AxonResult<()> {
        let attachment_ids = input
            .attachment_refs
            .iter()
            .map(|attachment| attachment.id)
            .collect();
        cell.project_vision_understanding(
            input.model_id,
            attachment_ids,
            input.audit_summary.clone(),
        )
    }

    /// 将 MCP server startup 状态投影进 RuntimeCell timeline。
    pub fn project_mcp_server_startup(
        &self,
        cell: &mut RuntimeCell,
        projection: &McpStartupProjection,
    ) -> AxonResult<()> {
        cell.project_mcp_server_startup(
            projection.server_id.clone(),
            projection.config_ref.clone(),
            projection.status,
            projection.audit_summary.clone(),
        )
    }

    /// 将 Agent 模型回答投影进 RuntimeCell timeline。
    pub fn project_agent_response(
        &self,
        cell: &mut RuntimeCell,
        response: &AgentResponseProjection,
    ) -> AxonResult<()> {
        cell.project_agent_response(
            response.preview.clone(),
            response.full_content_chars,
            response.truncated,
            response.source_ref.clone(),
        )
    }

    /// 将 Agent Pane apply 执行结果投影进 RuntimeCell timeline。
    pub fn project_apply_execution(
        &self,
        cell: &mut RuntimeCell,
        projection: &AgentApplyExecutionProjection,
    ) -> AxonResult<()> {
        cell.project_apply_execution(
            projection.path.clone(),
            projection.execution_request_id,
            projection.status,
            projection.summary.clone(),
            projection.source_ref.clone(),
        )
    }

    /// 标记输出待排空。
    pub fn mark_output_pending(
        &self,
        cell: &mut RuntimeCell,
        last_chunk_id: ByteChunkId,
    ) -> AxonResult<()> {
        cell.output_pending(last_chunk_id)
    }

    /// 标记输出已排空。
    pub fn mark_output_drained(
        &self,
        cell: &mut RuntimeCell,
        last_chunk_id: ByteChunkId,
    ) -> AxonResult<()> {
        cell.output_drained(last_chunk_id)
    }

    /// 标记 cell 输出关闭。
    pub fn close_cell(
        &self,
        cell: &mut RuntimeCell,
        last_chunk_id: Option<ByteChunkId>,
    ) -> AxonResult<()> {
        cell.close(last_chunk_id)
    }

    /// 阻塞 cell。
    pub fn block_cell(&self, cell: &mut RuntimeCell, reason: BlockReason) -> AxonResult<()> {
        cell.block(reason)
    }

    /// 暂停 cell。
    pub fn pause_cell(&self, cell: &mut RuntimeCell) -> AxonResult<()> {
        cell.pause()
    }

    /// 恢复 cell。
    pub fn resume_cell(&self, cell: &mut RuntimeCell) -> AxonResult<()> {
        cell.resume()
    }

    /// 完成 cell。
    pub fn complete_cell(
        &self,
        cell: &mut RuntimeCell,
        summary: impl Into<String>,
    ) -> AxonResult<()> {
        cell.complete(CellResult {
            summary: summary.into(),
        })
    }

    /// 失败 cell。
    pub fn fail_cell(&self, cell: &mut RuntimeCell, message: impl Into<String>) -> AxonResult<()> {
        cell.fail(CellFailure {
            message: message.into(),
        })
    }

    /// 取消 cell。
    pub fn cancel_cell(
        &self,
        cell: &mut RuntimeCell,
        message: impl Into<String>,
    ) -> AxonResult<()> {
        cell.cancel(CancelReason {
            message: message.into(),
        })
    }
}
