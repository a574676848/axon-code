//! `RuntimeCell` 领域状态机。

#[path = "cell/events.rs"]
mod events;
#[path = "cell/state_machine.rs"]
mod state_machine;

pub use events::{
    ApplyExecutionStatus, BlockReason, CancelReason, CellClosureState, CellFailure, CellResult,
    McpServerStartupStatus, OutputDrainState, PausedCellState, RuntimeCellEvent, RuntimeCellState,
};

use state_machine::RuntimeCellStateMachine;

use crate::{
    AttachmentId, AxonError, AxonResult, ByteChunkId, CellId, ExecutionRequestId, ModelProfileId,
    PaneId, PermissionSnapshotId, ToolCallId,
};

/// Agent turn 的可审计状态对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCell {
    /// Cell ID。
    pub id: CellId,
    /// 绑定 pane。
    pub pane_id: PaneId,
    /// 工作目录。
    pub cwd: String,
    /// 模型路由。
    pub model_profile_id: ModelProfileId,
    /// 权限快照。
    pub permission_snapshot_id: PermissionSnapshotId,
    /// 当前状态。
    pub state: RuntimeCellState,
    /// 输出排空状态。
    pub output_drain: OutputDrainState,
    /// 关闭状态。
    pub closure: CellClosureState,
    /// 事件日志。
    pub events: Vec<RuntimeCellEvent>,
}

/// `RuntimeCell` 可查询快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCellSnapshot {
    /// Cell ID。
    pub id: CellId,
    /// 绑定 pane。
    pub pane_id: PaneId,
    /// 工作目录。
    pub cwd: String,
    /// 模型路由。
    pub model_profile_id: ModelProfileId,
    /// 权限快照。
    pub permission_snapshot_id: PermissionSnapshotId,
    /// 当前状态。
    pub state: RuntimeCellState,
    /// 输出排空状态。
    pub output_drain: OutputDrainState,
    /// 关闭状态。
    pub closure: CellClosureState,
    /// 事件数量。
    pub event_count: usize,
}

impl RuntimeCell {
    /// 创建新的 `RuntimeCell`。
    #[must_use]
    pub fn new(
        id: CellId,
        pane_id: PaneId,
        cwd: impl Into<String>,
        model_profile_id: ModelProfileId,
        permission_snapshot_id: PermissionSnapshotId,
    ) -> Self {
        Self {
            id,
            pane_id,
            cwd: cwd.into(),
            model_profile_id,
            permission_snapshot_id,
            state: RuntimeCellState::Draft,
            output_drain: OutputDrainState::Drained,
            closure: CellClosureState::Open,
            events: vec![RuntimeCellEvent::Created { cell_id: id }],
        }
    }

    /// 开始运行。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Draft` 或 `Queued` 状态时返回错误。
    pub fn start(&mut self) -> AxonResult<()> {
        self.state = RuntimeCellStateMachine::start(&self.state)?;
        self.events
            .push(RuntimeCellEvent::Started { cell_id: self.id });
        Ok(())
    }

    /// 等待工具完成。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Running` 状态时返回错误。
    pub fn wait_for_tool(&mut self, tool_call_id: ToolCallId) -> AxonResult<()> {
        self.state = RuntimeCellStateMachine::wait_for_tool(&self.state, tool_call_id)?;
        self.events.push(RuntimeCellEvent::ToolCallStarted {
            cell_id: self.id,
            tool_call_id,
        });
        Ok(())
    }

    /// 工具完成后恢复运行。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `WaitingTool` 状态时返回错误。
    pub fn tool_completed(&mut self) -> AxonResult<()> {
        let (next_state, tool_call_id) = RuntimeCellStateMachine::complete_tool(&self.state)?;
        self.state = next_state;
        self.events.push(RuntimeCellEvent::ToolCallCompleted {
            cell_id: self.id,
            tool_call_id,
        });
        Ok(())
    }

    /// 记录工具调用增量。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于同一工具调用的 `WaitingTool` 状态时返回错误。
    pub fn tool_delta(
        &mut self,
        tool_call_id: ToolCallId,
        summary: impl Into<String>,
    ) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_tool_call(
            &self.state,
            tool_call_id,
            "只有匹配的 WaitingTool cell 能记录工具增量",
        )?;

        self.events.push(RuntimeCellEvent::ToolCallDelta {
            cell_id: self.id,
            tool_call_id,
            summary: summary.into(),
        });
        Ok(())
    }

    /// 记录视觉理解输出投影。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Running` 状态，或附件列表为空时返回错误。
    pub fn project_vision_understanding(
        &mut self,
        model_id: ModelProfileId,
        attachment_ids: Vec<AttachmentId>,
        audit_summary: impl Into<String>,
    ) -> AxonResult<()> {
        self.ensure_running("只有 Running cell 能记录视觉理解输出")?;
        if attachment_ids.is_empty() {
            return Err(AxonError::cell_invalid_transition(
                "视觉理解输出必须绑定至少一个附件引用",
            ));
        }

        self.events
            .push(RuntimeCellEvent::VisionUnderstandingProjected {
                cell_id: self.id,
                model_id,
                attachment_ids,
                audit_summary: audit_summary.into(),
            });
        Ok(())
    }

    /// 暂停 cell。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于可恢复状态，或已经处于终态时返回错误。
    pub fn pause(&mut self) -> AxonResult<()> {
        let (next_state, previous_state) = RuntimeCellStateMachine::pause(&self.state)?;
        self.state = next_state;
        self.events.push(RuntimeCellEvent::Paused {
            cell_id: self.id,
            previous_state,
        });
        Ok(())
    }

    /// 记录 MCP server startup 状态投影。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Running` 状态，或 MCP 配置引用为空时返回错误。
    pub fn project_mcp_server_startup(
        &mut self,
        server_id: impl Into<String>,
        config_ref: impl Into<String>,
        status: McpServerStartupStatus,
        audit_summary: impl Into<String>,
    ) -> AxonResult<()> {
        self.ensure_running("只有 Running cell 能记录 MCP server startup 状态")?;
        let config_ref = config_ref.into();
        if config_ref.trim().is_empty() {
            return Err(AxonError::cell_invalid_transition(
                "MCP server startup 必须绑定配置引用",
            ));
        }

        self.events
            .push(RuntimeCellEvent::McpServerStartupProjected {
                cell_id: self.id,
                server_id: server_id.into(),
                config_ref,
                status,
                audit_summary: audit_summary.into(),
            });
        Ok(())
    }

    /// 记录 Agent 模型回答投影。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Running` 状态、预览为空或来源引用为空时返回错误。
    pub fn project_agent_response(
        &mut self,
        preview: impl Into<String>,
        full_content_chars: usize,
        truncated: bool,
        source_ref: impl Into<String>,
    ) -> AxonResult<()> {
        self.ensure_running("只有 Running cell 能记录 Agent 模型回答")?;
        let preview = preview.into();
        let source_ref = source_ref.into();
        if preview.trim().is_empty() {
            return Err(AxonError::cell_invalid_transition(
                "Agent 模型回答预览不能为空",
            ));
        }
        if source_ref.trim().is_empty() {
            return Err(AxonError::cell_invalid_transition(
                "Agent 模型回答必须绑定来源引用",
            ));
        }
        self.events.push(RuntimeCellEvent::AgentResponseProjected {
            cell_id: self.id,
            preview,
            full_content_chars,
            truncated,
            source_ref,
        });
        Ok(())
    }

    /// 记录 Agent Pane apply 执行结果投影。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Running` 状态，或目标路径/摘要/来源引用为空时返回错误。
    pub fn project_apply_execution(
        &mut self,
        path: impl Into<String>,
        execution_request_id: ExecutionRequestId,
        status: ApplyExecutionStatus,
        audit_summary: impl Into<String>,
        source_ref: impl Into<String>,
    ) -> AxonResult<()> {
        self.ensure_running("只有 Running cell 能记录 Agent Pane apply 执行结果")?;
        let path = path.into();
        let audit_summary = audit_summary.into();
        let source_ref = source_ref.into();
        if path.trim().is_empty() {
            return Err(AxonError::cell_invalid_transition(
                "Agent Pane apply 执行结果必须绑定目标路径",
            ));
        }
        if audit_summary.trim().is_empty() {
            return Err(AxonError::cell_invalid_transition(
                "Agent Pane apply 执行结果摘要不能为空",
            ));
        }
        if source_ref.trim().is_empty() {
            return Err(AxonError::cell_invalid_transition(
                "Agent Pane apply 执行结果必须绑定来源引用",
            ));
        }
        self.events.push(RuntimeCellEvent::ApplyExecutionProjected {
            cell_id: self.id,
            path,
            execution_request_id,
            status,
            audit_summary,
            source_ref,
        });
        Ok(())
    }

    /// 恢复暂停的 cell。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Paused` 状态时返回错误。
    pub fn resume(&mut self) -> AxonResult<()> {
        let (next_state, resumed_state) = RuntimeCellStateMachine::resume(&self.state)?;
        self.state = next_state;
        self.events.push(RuntimeCellEvent::Resumed {
            cell_id: self.id,
            resumed_state,
        });
        Ok(())
    }

    /// 标记终端输出待排空。
    ///
    /// # Errors
    ///
    /// 当 cell 已经处于完成、失败或取消终态时返回错误。
    pub fn output_pending(&mut self, last_chunk_id: ByteChunkId) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_not_final(
            &self.state,
            "终态 RuntimeCell 不能进入输出待排空状态",
        )?;
        if matches!(self.closure, CellClosureState::Closed { .. }) {
            return Err(AxonError::cell_invalid_transition(
                "Closed RuntimeCell 不能重新进入输出待排空状态",
            ));
        }
        self.output_drain = OutputDrainState::Pending { last_chunk_id };
        self.events.push(RuntimeCellEvent::OutputDrainPending {
            cell_id: self.id,
            last_chunk_id,
        });
        Ok(())
    }

    /// 标记终端输出已排空。
    ///
    /// # Errors
    ///
    /// 当 cell 已经处于完成、失败或取消终态时返回错误。
    pub fn output_drained(&mut self, last_chunk_id: ByteChunkId) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_not_final(
            &self.state,
            "终态 RuntimeCell 不能更新输出排空状态",
        )?;
        self.output_drain = OutputDrainState::Drained;
        self.events.push(RuntimeCellEvent::OutputDrained {
            cell_id: self.id,
            last_chunk_id,
        });
        Ok(())
    }

    /// 标记 cell 输出关闭。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Running`、输出未排空或已经关闭时返回错误。
    pub fn close(&mut self, last_chunk_id: Option<ByteChunkId>) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_not_final(&self.state, "终态 RuntimeCell 不能再次关闭")?;
        RuntimeCellStateMachine::ensure_running(
            &self.state,
            "RuntimeCell 只能在 Running 状态关闭",
        )?;
        if !matches!(self.output_drain, OutputDrainState::Drained) {
            return Err(AxonError::cell_invalid_transition(
                "RuntimeCell 只能在输出排空后关闭",
            ));
        }
        if matches!(self.closure, CellClosureState::Closed { .. }) {
            return Err(AxonError::cell_invalid_transition(
                "RuntimeCell 不能重复关闭",
            ));
        }
        self.closure = CellClosureState::Closed { last_chunk_id };
        self.events.push(RuntimeCellEvent::Closed {
            cell_id: self.id,
            last_chunk_id,
        });
        Ok(())
    }

    /// 记录工具调用失败并失败 cell。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于同一工具调用的 `WaitingTool` 状态时返回错误。
    pub fn tool_failed(
        &mut self,
        tool_call_id: ToolCallId,
        failure: CellFailure,
    ) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_tool_call(
            &self.state,
            tool_call_id,
            "只有匹配的 WaitingTool cell 能记录工具失败",
        )?;

        self.events.push(RuntimeCellEvent::ToolCallFailed {
            cell_id: self.id,
            tool_call_id,
            message: failure.message.clone(),
        });
        self.state = RuntimeCellState::Failed(failure);
        self.events
            .push(RuntimeCellEvent::Failed { cell_id: self.id });
        Ok(())
    }

    /// 阻塞 cell。
    ///
    /// # Errors
    ///
    /// 当 cell 已经处于完成、失败或取消终态时返回错误。
    pub fn block(&mut self, reason: BlockReason) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_not_final(&self.state, "终态 RuntimeCell 不能再次阻塞")?;
        self.state = RuntimeCellState::Blocked(reason.clone());
        self.events.push(RuntimeCellEvent::Blocked {
            cell_id: self.id,
            reason,
        });
        Ok(())
    }

    /// 完成 cell。
    ///
    /// # Errors
    ///
    /// 当 cell 不处于 `Running` 状态，或输出仍未排空时返回错误。
    pub fn complete(&mut self, result: CellResult) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_running(
            &self.state,
            "RuntimeCell 只能在 Running 且无 pending 工作时完成",
        )?;
        if !matches!(self.output_drain, OutputDrainState::Drained) {
            return Err(AxonError::cell_invalid_transition(
                "RuntimeCell 只能在输出排空后完成",
            ));
        }
        if !matches!(self.closure, CellClosureState::Closed { .. }) {
            return Err(AxonError::cell_invalid_transition(
                "RuntimeCell 只能在关闭后完成",
            ));
        }
        self.state = RuntimeCellState::Completed(result);
        self.events
            .push(RuntimeCellEvent::Completed { cell_id: self.id });
        Ok(())
    }

    /// 失败 cell。
    ///
    /// # Errors
    ///
    /// 当 cell 已经处于完成、失败或取消终态时返回错误。
    pub fn fail(&mut self, failure: CellFailure) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_not_final(&self.state, "终态 RuntimeCell 不能再次失败")?;
        self.state = RuntimeCellState::Failed(failure);
        self.events
            .push(RuntimeCellEvent::Failed { cell_id: self.id });
        Ok(())
    }

    /// 取消 cell。
    ///
    /// # Errors
    ///
    /// 当 cell 已经处于完成、失败或取消终态时返回错误。
    pub fn cancel(&mut self, reason: CancelReason) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_not_final(&self.state, "终态 RuntimeCell 不能再次取消")?;
        self.state = RuntimeCellState::Cancelled(reason);
        self.events
            .push(RuntimeCellEvent::Cancelled { cell_id: self.id });
        Ok(())
    }

    /// 返回可查询快照。
    #[must_use]
    pub fn snapshot(&self) -> RuntimeCellSnapshot {
        RuntimeCellSnapshot {
            id: self.id,
            pane_id: self.pane_id,
            cwd: self.cwd.clone(),
            model_profile_id: self.model_profile_id,
            permission_snapshot_id: self.permission_snapshot_id,
            state: self.state.clone(),
            output_drain: self.output_drain.clone(),
            closure: self.closure.clone(),
            event_count: self.events.len(),
        }
    }

    fn ensure_running(&self, message: &'static str) -> AxonResult<()> {
        RuntimeCellStateMachine::ensure_running(&self.state, message)
    }
}
