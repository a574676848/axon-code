//! Agent provider streaming 回答聚合。
//!
//! 该模块只消费 provider 已归一化后的文本 delta，不接触 provider 原始事件或网络连接。

use axon_domain::{AxonError, AxonResult, RuntimeCell};

use crate::{
    AgentPaneProjection, AgentPaneProjectionInput, AgentResponseProjection, RuntimeCellRuntime,
};

/// Agent streaming 回答来源引用前缀。
pub const AGENT_RESPONSE_STREAM_SOURCE_PREFIX: &str = "agent-response-stream://cell/";
/// Agent streaming 回答投影原因。
pub const AGENT_RESPONSE_STREAM_REASON_PROJECTED: &str =
    "agent_response_stream:projected_to_runtime_cell";
/// Agent streaming 回答远控关闭原因。
pub const AGENT_RESPONSE_STREAM_REASON_REMOTE_CLOSED: &str =
    "agent_response_stream:remote_full_response_closed";
/// Agent streaming 活跃预览最大字符数。
pub const AGENT_RESPONSE_STREAM_ACTIVE_PREVIEW_CHARS: usize = 240;

/// Provider streaming 文本 delta。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponseStreamDelta {
    /// Delta 序号。
    pub sequence: u64,
    /// 文本片段。
    pub content_delta: String,
}

/// Agent streaming 回答请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponseStreamRequest {
    /// 已处于 Running 的 RuntimeCell。
    pub cell: RuntimeCell,
    /// Agent Pane 基础投影输入。
    pub pane_input: AgentPaneProjectionInput,
    /// 已归一化文本 delta。
    pub deltas: Vec<AgentResponseStreamDelta>,
}

/// Agent streaming 回答投影报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponseStreamReport {
    /// 更新后的 RuntimeCell。
    pub cell: RuntimeCell,
    /// 有界回答投影。
    pub response: AgentResponseProjection,
    /// 带聊天卡片的 Agent Pane 投影。
    pub pane_projection: AgentPaneProjection,
    /// 聚合的 delta 数量。
    pub delta_count: usize,
    /// 完整回答字符数。
    pub full_content_chars: usize,
    /// 可渲染活跃预览。
    pub active_preview: String,
    /// 活跃预览是否被截断。
    pub active_preview_truncated: bool,
    /// 是否允许远控查看完整回答。
    pub remote_full_response_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Agent streaming 回答 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentResponseStreamRuntime;

impl AgentResponseStreamRuntime {
    /// 聚合 provider delta，并写入 RuntimeCell / Agent Pane 投影。
    pub fn project(
        &self,
        request: AgentResponseStreamRequest,
    ) -> AxonResult<AgentResponseStreamReport> {
        let content = aggregate_deltas(&request.deltas)?;
        let (active_preview, active_preview_truncated) =
            bounded_tail(&content, AGENT_RESPONSE_STREAM_ACTIVE_PREVIEW_CHARS);
        let mut cell = request.cell;
        let source_ref = format!("{AGENT_RESPONSE_STREAM_SOURCE_PREFIX}{}", cell.id.0);
        let response = AgentResponseProjection::from_content(cell.id.0, &content, source_ref);
        RuntimeCellRuntime::default().project_agent_response(&mut cell, &response)?;

        let mut pane_input = request.pane_input;
        pane_input.runtime_cell = Some(cell.snapshot());
        pane_input.response = Some(response.clone());
        let pane_projection = AgentPaneProjection::from_input(pane_input);

        Ok(AgentResponseStreamReport {
            cell,
            response,
            pane_projection,
            delta_count: request.deltas.len(),
            full_content_chars: content.chars().count(),
            active_preview,
            active_preview_truncated,
            remote_full_response_allowed: false,
            reasons: vec![
                AGENT_RESPONSE_STREAM_REASON_PROJECTED.to_owned(),
                AGENT_RESPONSE_STREAM_REASON_REMOTE_CLOSED.to_owned(),
            ],
        })
    }
}

fn aggregate_deltas(deltas: &[AgentResponseStreamDelta]) -> AxonResult<String> {
    if deltas.is_empty() {
        return Err(AxonError::EmptyInput);
    }

    let mut expected = None;
    let mut content = String::new();
    for delta in deltas {
        if delta.content_delta.is_empty() {
            return Err(AxonError::EmptyInput);
        }
        if let Some(expected_sequence) = expected
            && delta.sequence != expected_sequence
        {
            return Err(AxonError::runtime_failure(
                "Agent streaming delta sequence 不连续",
            ));
        }
        expected = Some(delta.sequence + 1);
        content.push_str(&delta.content_delta);
    }

    if content.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    Ok(content)
}

fn bounded_tail(content: &str, char_limit: usize) -> (String, bool) {
    let total_chars = content.chars().count();
    if total_chars <= char_limit {
        return (content.to_owned(), false);
    }
    let tail_reversed = content.chars().rev().take(char_limit).collect::<String>();
    (tail_reversed.chars().rev().collect(), true)
}
