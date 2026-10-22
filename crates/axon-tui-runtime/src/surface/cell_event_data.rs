//! Transcript 事件类 cell 的结构化数据模型。

use crate::{AgentTuiToolKind, ToolDisplayProjection};

use super::tool_display_helpers::{mcp_catalog_raw_name, mcp_catalog_target, mcp_tool_parts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderExecCell {
    pub(crate) id: String,
    pub(crate) tool_name: String,
    pub(crate) display_label: String,
    pub(crate) display_target: Option<String>,
    pub(crate) raw_tool_name: String,
    pub(crate) display_detail: Option<String>,
    pub(crate) status: AgentRenderExecStatus,
    pub(crate) provider_call_id: Option<String>,
    pub(crate) arguments_preview: Option<String>,
    pub(crate) output_preview: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderExecCompletion {
    pub(crate) id: String,
    pub(crate) tool_name: String,
    pub(crate) status: String,
    pub(crate) provider_call_id: String,
    pub(crate) summary: Option<String>,
    pub(crate) arguments_preview: Option<String>,
    pub(crate) output_preview: Option<String>,
    pub(crate) duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderExecStatus {
    Running,
    Completed,
    Failed,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderUsageCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) total_tokens: u64,
    pub(crate) total: String,
    pub(crate) input_tokens: u64,
    pub(crate) input: String,
    pub(crate) cache_read_input_tokens: u64,
    pub(crate) cache_read: String,
    pub(crate) cache_write_input_tokens: u64,
    pub(crate) cache_write: String,
    pub(crate) output_tokens: u64,
    pub(crate) output: String,
    pub(crate) reasoning_output_tokens: u64,
    pub(crate) reasoning: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderHookCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) items: Vec<AgentRenderHookItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderHookItem {
    pub(crate) run_id: String,
    pub(crate) name: String,
    pub(crate) status: AgentRenderLifecycleStatus,
    pub(crate) indicator: Option<String>,
    pub(crate) status_message: Option<String>,
    pub(crate) entries: Vec<AgentRenderHookOutputEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderHookOutputEntry {
    pub(crate) kind: AgentRenderHookOutputKind,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderHookOutputKind {
    Feedback,
    Warning,
    Stop,
    Context,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderMcpCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) empty_text: String,
    pub(crate) source: AgentRenderMcpCellSource,
    pub(crate) items: Vec<AgentRenderMcpItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentRenderMcpStatus {
    id: String,
    tool_name: String,
    status: String,
    provider_call_id: Option<String>,
    arguments_preview: Option<String>,
    output_preview: Option<String>,
    summary: Option<String>,
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderMcpCellSource {
    ToolCall,
    Catalog,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderMcpItem {
    pub(crate) provider_call_id: Option<String>,
    pub(crate) server: String,
    pub(crate) tool: Option<String>,
    pub(crate) display_label: String,
    pub(crate) display_target: Option<String>,
    pub(crate) raw_tool_name: String,
    pub(crate) display_detail: Option<String>,
    pub(crate) status: AgentRenderLifecycleStatus,
    pub(crate) arguments_preview: Option<String>,
    pub(crate) output_preview: Option<String>,
    pub(crate) summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderHistoryCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) empty_text: String,
    pub(crate) items: Vec<AgentRenderHistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderHistoryItem {
    pub(crate) name: String,
    pub(crate) kind_label: Option<String>,
    pub(crate) status_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderLifecycleStatus {
    Completed,
    Failed,
    Running,
    Pending,
    Blocked,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderErrorCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) details: Vec<String>,
    pub(crate) next_step: Option<String>,
    pub(crate) severity: crate::surface::error_model::TuiErrorSeverity,
    pub(crate) action: crate::surface::error_model::TuiErrorAction,
    pub(crate) cell_kind: crate::surface::error_model::TuiErrorCellKind,
}

impl AgentRenderErrorCell {
    pub(crate) fn plain_text_fallback(&self) -> String {
        std::iter::once(self.title.as_str())
            .chain(std::iter::once(self.message.as_str()))
            .chain(self.details.iter().map(String::as_str))
            .chain(self.next_step.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl AgentRenderExecCell {
    pub(crate) fn running_with_id(
        id: impl Into<String>,
        tool_name: impl Into<String>,
        provider_call_id: impl Into<String>,
        arguments_preview: Option<String>,
    ) -> Self {
        let tool_name = tool_name.into();
        let projection = ToolDisplayProjection::from_tool(
            &tool_name,
            AgentTuiToolKind::from_tool_name(&tool_name),
            None,
        );
        Self {
            id: id.into(),
            tool_name,
            display_label: projection.label,
            display_target: projection.target,
            raw_tool_name: projection.raw_name,
            display_detail: projection.detail,
            status: AgentRenderExecStatus::Running,
            provider_call_id: Some(provider_call_id.into()),
            arguments_preview,
            output_preview: None,
            summary: None,
            duration_ms: None,
        }
    }

    pub(crate) fn output_with_id(
        id: impl Into<String>,
        tool_name: impl Into<String>,
        provider_call_id: impl Into<String>,
        output_preview: Option<String>,
    ) -> Self {
        let tool_name = tool_name.into();
        let projection = ToolDisplayProjection::from_tool(
            &tool_name,
            AgentTuiToolKind::from_tool_name(&tool_name),
            None,
        );
        Self {
            id: id.into(),
            tool_name,
            display_label: projection.label,
            display_target: projection.target,
            raw_tool_name: projection.raw_name,
            display_detail: projection.detail,
            status: AgentRenderExecStatus::Output,
            provider_call_id: Some(provider_call_id.into()),
            arguments_preview: None,
            output_preview,
            summary: None,
            duration_ms: None,
        }
    }

    pub(crate) fn completed_with_id(input: AgentRenderExecCompletion) -> Self {
        let tool_name = input.tool_name;
        let projection = ToolDisplayProjection::from_tool(
            &tool_name,
            AgentTuiToolKind::from_tool_name(&tool_name),
            None,
        );
        Self {
            id: input.id,
            tool_name,
            display_label: projection.label,
            display_target: projection.target,
            raw_tool_name: projection.raw_name,
            display_detail: projection.detail,
            status: AgentRenderExecStatus::from_raw(&input.status),
            provider_call_id: Some(input.provider_call_id),
            arguments_preview: input.arguments_preview,
            output_preview: input.output_preview,
            summary: input.summary,
            duration_ms: input.duration_ms,
        }
    }

    pub(crate) fn merge_output_preview(&mut self, output_preview: String) {
        self.status = AgentRenderExecStatus::Output;
        self.output_preview = Some(output_preview);
    }

    pub(crate) fn merge_progress_preview(&mut self, progress_preview: String) {
        self.status = AgentRenderExecStatus::Running;
        self.output_preview = Some(progress_preview);
    }

    pub(crate) fn fill_missing_previews_from(&mut self, previous: &Self) {
        if self.arguments_preview.is_none() {
            self.arguments_preview = previous.arguments_preview.clone();
        }
        if self.output_preview.is_none() {
            self.output_preview = previous.output_preview.clone();
        }
    }

    pub(crate) fn set_display_projection(&mut self, projection: ToolDisplayProjection) {
        self.display_label = projection.label;
        self.display_target = projection.target;
        self.raw_tool_name = projection.raw_name;
        self.display_detail = projection.detail;
    }

    pub(crate) fn set_duration_ms(&mut self, duration_ms: Option<u64>) {
        self.duration_ms = duration_ms;
    }
}

impl AgentRenderExecStatus {
    pub(crate) fn from_raw(status: &str) -> Self {
        match status {
            "completed" | "success" => Self::Completed,
            "failed" | "error" | "blocked" => Self::Failed,
            "aborted" | "cancelled" | "canceled" => Self::Failed,
            "output" => Self::Output,
            _ => Self::Running,
        }
    }

    #[cfg(test)]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Running => "运行中",
            Self::Completed => "完成",
            Self::Failed => "失败",
            Self::Output => "输出",
        }
    }
}

impl AgentRenderUsageCell {
    pub(crate) fn from_usage_line(line: &crate::AgentTuiUsageLine) -> Self {
        Self::from_usage(line.id.clone(), line.usage)
    }

    pub(crate) fn from_usage(id: impl Into<String>, usage: crate::AgentTuiTokenUsage) -> Self {
        Self {
            id: id.into(),
            title: "Token".to_owned(),
            total_tokens: usage.total_tokens(),
            total: usage.render_total(),
            input_tokens: usage.input_tokens,
            input: usage.render_input(),
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_read: usage.render_cache_read_input(),
            cache_write_input_tokens: usage.cache_write_input_tokens,
            cache_write: usage.render_cache_write_input(),
            output_tokens: usage.output_tokens,
            output: usage.render_output(),
            reasoning_output_tokens: usage.reasoning_output_tokens,
            reasoning: usage.render_reasoning_output(),
        }
    }
}

impl AgentRenderHookCell {
    pub(crate) fn from_tui_status(view: &crate::AgentTuiHookStatusView) -> Self {
        Self {
            id: format!("hook:{}", view.run_id),
            title: view.name.clone(),
            items: vec![AgentRenderHookItem {
                run_id: view.run_id.clone(),
                name: view.name.clone(),
                status: AgentRenderLifecycleStatus::from_tui_hook_status(view.status),
                indicator: None,
                status_message: view.status_message.clone(),
                entries: view
                    .entries
                    .iter()
                    .map(AgentRenderHookOutputEntry::from_tui_entry)
                    .collect(),
            }],
        }
    }

    pub(crate) fn merge_from(&mut self, next: Self) {
        self.id = "hook:runtime".to_owned();
        self.title = "Hook".to_owned();
        for item in next.items {
            self.upsert_item(item);
        }
    }

    pub(crate) fn remove_item(&mut self, run_id: &str) {
        self.items.retain(|item| item.run_id != run_id);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn upsert_item(&mut self, item: AgentRenderHookItem) {
        if let Some(existing) = self
            .items
            .iter_mut()
            .find(|candidate| candidate.run_id == item.run_id)
        {
            *existing = item;
            return;
        }
        self.items.push(item);
    }
}

impl AgentRenderHookOutputEntry {
    fn from_tui_entry(entry: &crate::AgentTuiHookOutputEntry) -> Self {
        Self {
            kind: AgentRenderHookOutputKind::from_tui_kind(entry.kind),
            text: entry.text.clone(),
        }
    }
}

impl AgentRenderHookOutputKind {
    const fn from_tui_kind(kind: crate::AgentTuiHookOutputKind) -> Self {
        match kind {
            crate::AgentTuiHookOutputKind::Feedback => Self::Feedback,
            crate::AgentTuiHookOutputKind::Warning => Self::Warning,
            crate::AgentTuiHookOutputKind::Stop => Self::Stop,
            crate::AgentTuiHookOutputKind::Context => Self::Context,
            crate::AgentTuiHookOutputKind::Error => Self::Error,
        }
    }
}

impl AgentRenderMcpCell {
    pub(crate) fn from_tool_execution(view: &crate::AgentTuiToolExecutionView) -> Self {
        Self::from_tool_status(AgentRenderMcpStatus {
            id: view.part_id.clone(),
            tool_name: view.tool_name.clone(),
            status: "running".to_owned(),
            provider_call_id: Some(view.provider_call_id.clone()),
            arguments_preview: Some(view.arguments_preview.clone()),
            output_preview: None,
            summary: None,
            metadata: None,
        })
    }

    pub(crate) fn from_tool_receipt(
        receipt: &crate::AgentTuiToolReceipt,
        summary: Option<String>,
        arguments_preview: Option<String>,
        output_preview: Option<String>,
    ) -> Self {
        Self::from_tool_status(AgentRenderMcpStatus {
            id: receipt.id.clone(),
            tool_name: receipt.tool_name.clone(),
            status: receipt.status.clone(),
            provider_call_id: Some(receipt.provider_call_id.clone()),
            arguments_preview,
            output_preview,
            summary,
            metadata: receipt.metadata.clone(),
        })
    }

    pub(crate) fn from_tool_output_chunk(
        chunk: &crate::AgentTuiToolOutputChunk,
        output_preview: String,
    ) -> Self {
        Self::from_tool_status(AgentRenderMcpStatus {
            id: chunk.part_id.clone(),
            tool_name: chunk.tool_name.clone(),
            status: "running".to_owned(),
            provider_call_id: Some(chunk.provider_call_id.clone()),
            arguments_preview: None,
            output_preview: Some(output_preview),
            summary: None,
            metadata: None,
        })
    }

    pub(crate) fn from_tool_progress(
        progress: &crate::AgentTuiToolProgressView,
        output_preview: String,
    ) -> Self {
        Self::from_tool_status(AgentRenderMcpStatus {
            id: progress.part_id.clone(),
            tool_name: progress.tool_name.clone(),
            status: "running".to_owned(),
            provider_call_id: Some(progress.provider_call_id.clone()),
            arguments_preview: None,
            output_preview: Some(output_preview),
            summary: None,
            metadata: None,
        })
    }

    fn from_tool_status(input: AgentRenderMcpStatus) -> Self {
        let (server, tool) = mcp_tool_parts(&input.tool_name);
        let projection = ToolDisplayProjection::from_tool(
            &input.tool_name,
            AgentTuiToolKind::Mcp,
            input.metadata.as_ref(),
        );
        Self {
            id: input.id,
            title: "MCP".to_owned(),
            empty_text: "无 MCP 调用".to_owned(),
            source: AgentRenderMcpCellSource::ToolCall,
            items: vec![AgentRenderMcpItem {
                provider_call_id: input.provider_call_id,
                server,
                tool,
                display_label: projection.label,
                display_target: projection.target,
                raw_tool_name: projection.raw_name,
                display_detail: projection.detail,
                status: AgentRenderLifecycleStatus::from_raw(&input.status),
                arguments_preview: input.arguments_preview,
                output_preview: input.output_preview,
                summary: input.summary,
            }],
        }
    }

    pub(crate) fn from_catalog_items(
        id: impl Into<String>,
        title: impl Into<String>,
        items: &[AgentRenderMcpItem],
        empty_text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            empty_text: empty_text.into(),
            source: AgentRenderMcpCellSource::Catalog,
            items: items.to_vec(),
        }
    }

    pub(crate) fn merge_output_preview(&mut self, output_preview: String) {
        let Some(item) = self.items.first_mut() else {
            return;
        };
        item.status = AgentRenderLifecycleStatus::Running;
        item.output_preview = Some(output_preview);
    }

    pub(crate) fn fill_missing_previews_from(&mut self, previous: &Self) {
        let (Some(item), Some(previous)) = (self.items.first_mut(), previous.items.first()) else {
            return;
        };
        if item.arguments_preview.is_none() {
            item.arguments_preview
                .clone_from(&previous.arguments_preview);
        }
        if item.output_preview.is_none() {
            item.output_preview.clone_from(&previous.output_preview);
        }
    }

    pub(crate) fn is_catalog_inventory(&self) -> bool {
        matches!(self.source, AgentRenderMcpCellSource::Catalog)
    }
}

impl AgentRenderMcpItem {
    pub(crate) fn catalog(
        server: impl Into<String>,
        tool: Option<String>,
        status: AgentRenderLifecycleStatus,
        summary: Option<String>,
    ) -> Self {
        let server = server.into();
        Self {
            provider_call_id: None,
            display_label: "MCP".to_owned(),
            display_target: mcp_catalog_target(&server, tool.as_deref()),
            raw_tool_name: mcp_catalog_raw_name(&server, tool.as_deref()),
            display_detail: None,
            server,
            tool,
            status,
            arguments_preview: None,
            output_preview: None,
            summary,
        }
    }

    pub(crate) fn from_catalog_value(value: &str) -> Self {
        let (server, tool) = value
            .split_once('/')
            .map_or((value, None), |(server, tool)| {
                (server, (!tool.trim().is_empty()).then_some(tool.trim()))
            });
        Self::catalog(
            server.trim(),
            tool.map(ToOwned::to_owned),
            AgentRenderLifecycleStatus::Completed,
            None,
        )
    }
}

impl AgentRenderHistoryCell {
    pub(crate) fn from_tui_line(line: &crate::AgentTuiHistoryLine) -> Self {
        Self {
            id: line.id.clone(),
            title: line.title.clone(),
            empty_text: "无历史".to_owned(),
            items: line
                .items
                .iter()
                .map(AgentRenderHistoryItem::from_tui_item)
                .collect(),
        }
    }

    pub(crate) fn from_catalog_items(
        id: impl Into<String>,
        title: impl Into<String>,
        item_kind: &str,
        items: &[String],
        empty_text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            empty_text: empty_text.into(),
            items: items
                .iter()
                .map(|item| AgentRenderHistoryItem::from_catalog_value(item_kind, item))
                .collect(),
        }
    }
}

impl AgentRenderHistoryItem {
    pub(crate) fn local(
        name: impl Into<String>,
        kind_label: impl Into<String>,
        status_label: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind_label: Some(kind_label.into()),
            status_label,
        }
    }

    fn from_catalog_value(kind: &str, value: &String) -> Self {
        Self {
            name: value.to_owned(),
            kind_label: history_kind_label(kind.to_owned()),
            status_label: None,
        }
    }

    fn from_tui_item(item: &crate::AgentTuiHistoryItem) -> Self {
        Self {
            name: item.name.clone(),
            kind_label: item.kind_label.clone(),
            status_label: item.status_label.clone(),
        }
    }
}

impl AgentRenderLifecycleStatus {
    pub(crate) fn from_raw(status: &str) -> Self {
        match status {
            "completed" | "success" | "loaded" => Self::Completed,
            "failed" | "error" => Self::Failed,
            "running" | "calling" | "active" => Self::Running,
            "pending" | "queued" => Self::Pending,
            "blocked" => Self::Blocked,
            "stopped" | "interrupted" | "cancelled" | "canceled" => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    fn from_tui_hook_status(status: crate::AgentTuiHookStatus) -> Self {
        match status {
            crate::AgentTuiHookStatus::Completed => Self::Completed,
            crate::AgentTuiHookStatus::Failed => Self::Failed,
            crate::AgentTuiHookStatus::Running => Self::Running,
            crate::AgentTuiHookStatus::Pending => Self::Pending,
            crate::AgentTuiHookStatus::Blocked => Self::Blocked,
            crate::AgentTuiHookStatus::Stopped => Self::Stopped,
            crate::AgentTuiHookStatus::Unknown => Self::Unknown,
        }
    }
}

fn history_kind_label(kind: String) -> Option<String> {
    match kind.as_str() {
        "history" | "History" => None,
        "Skill" | "skill" => Some("技能".to_owned()),
        "Session" | "session" => Some("会话".to_owned()),
        "model" => Some("模型".to_owned()),
        value => Some(value.to_owned()),
    }
}
