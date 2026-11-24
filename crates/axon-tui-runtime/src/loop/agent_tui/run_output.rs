//! Agent 运行结果到 TUI transcript 的结构化投射。

#[path = "run_output_hook_lifecycle.rs"]
mod run_output_hook_lifecycle;
#[path = "run_output_lifecycle.rs"]
mod run_output_lifecycle;
#[path = "run_output_models.rs"]
mod run_output_models;
#[path = "run_output_parts.rs"]
mod run_output_parts;
#[path = "run_output_runtime.rs"]
mod run_output_runtime;
#[path = "run_output_usage.rs"]
mod run_output_usage;

pub use run_output_hook_lifecycle::{
    AgentTuiHookOutputEntry, AgentTuiHookOutputKind, AgentTuiHookStatus, AgentTuiHookStatusView,
};
pub use run_output_lifecycle::{
    AgentTuiHistoryItem, AgentTuiHistoryLine, AgentTuiLifecycleLine, AgentTuiLifecyclePhase,
    AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput, AgentTuiPlanLine,
    AgentTuiPlanProgress, AgentTuiPlanStatus, AgentTuiPlanStep, AgentTuiReasoningLine,
    AgentTuiReviewLine, AgentTuiReviewStatus, AgentTuiStatusItem, AgentTuiStatusLine,
    AgentTuiToolLifecycleLine, AgentTuiToolLifecycleLineInput,
};
pub use run_output_models::{
    AgentTuiChannelAttempt, AgentTuiRunError, AgentTuiRunSessionInfo, AgentTuiToolKind,
    AgentTuiToolReceipt, AgentTuiToolReceiptDetails,
};
pub use run_output_parts::{
    AgentTuiMessagePart, AgentTuiMessagePartRole, AgentTuiReasoningPart, AgentTuiTranscriptPart,
    render_transcript_parts,
};
pub use run_output_runtime::{
    AgentTuiCancelPayload, AgentTuiInterruptPayload, AgentTuiRuntimeEvent, AgentTuiRuntimePayload,
    AgentTuiToolCompletionView, AgentTuiToolDiffSnapshotView, AgentTuiToolExecutionView,
    AgentTuiToolOutputChunk, AgentTuiToolProgressView, DaemonSubscriptionPhase,
    axon_cli_runtime_event, runtime_driver_event, tui_control_runtime_event, watcher_runtime_event,
};
pub use run_output_usage::{AgentTuiTokenUsage, AgentTuiUsageLine};

use super::run_output_diff::AgentTuiDiffLine;
use crate::surface::cell::RenderCellRecord;

/// TUI 运行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiRunOutput {
    lines: Vec<AgentTuiRunLine>,
}

/// TUI 运行结果中的结构化行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTuiRunLine {
    /// Assistant/User 文本输出。
    Message(AgentTuiMessagePart),
    /// 当前 run session 信息。
    Session(AgentTuiRunSessionInfo),
    /// 单个工具执行回执。
    ToolReceipt(AgentTuiToolReceipt),
    /// 工具生命周期事件。
    ToolLifecycle(AgentTuiToolLifecycleLine),
    /// 文件变更 Diff。
    Diff(AgentTuiDiffLine),
    /// Verify 停止产物。
    Review(AgentTuiReviewLine),
    /// 自修复循环时间线。
    Plan(AgentTuiPlanLine),
    /// Runtime turn 事件时间线。
    Status(AgentTuiStatusLine),
    /// 历史与上下文生命周期。
    History(AgentTuiHistoryLine),
    /// Hook 生命周期。
    Hook(AgentTuiHookStatusView),
    /// 命令反馈。
    Command(AgentTuiStatusLine),
    /// 模型生成生命周期状态。
    GenerationStatus(AgentTuiLifecycleLine),
    /// 模型 reasoning 输出。
    Reasoning(AgentTuiReasoningLine),
    /// 本轮 token usage。
    Usage(AgentTuiUsageLine),
    /// 错误输出。
    Error(AgentTuiRunError),
}

impl AgentTuiRunOutput {
    /// 创建空运行结果。
    #[must_use]
    pub fn empty() -> Self {
        Self { lines: Vec::new() }
    }

    /// 从单段 assistant 文本创建运行结果。
    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        let mut output = Self::empty();
        output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
            "assistant:local",
            text,
        )));
        output
    }

    /// 追加结构化行。
    pub fn push(&mut self, line: AgentTuiRunLine) {
        if !line.has_renderable_content() {
            return;
        }
        self.lines.push(line);
    }

    /// 是否没有任何可渲染内容。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// 按条件过滤保留结构化行。
    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&AgentTuiRunLine) -> bool,
    {
        self.lines.retain(f);
    }

    /// 读取结构化运行结果行。
    #[must_use]
    pub fn lines(&self) -> &[AgentTuiRunLine] {
        &self.lines
    }

    pub(in crate::r#loop::agent_tui) fn into_lines(self) -> Vec<AgentTuiRunLine> {
        self.lines
    }

    /// 汇总本次运行中出现的 token usage。
    #[must_use]
    pub fn token_usage_total(&self) -> AgentTuiTokenUsage {
        self.lines
            .iter()
            .filter_map(|line| match line {
                AgentTuiRunLine::Usage(usage) => Some(usage.usage),
                _ => None,
            })
            .fold(AgentTuiTokenUsage::default(), |mut total, usage| {
                total.add_assign(usage);
                total
            })
    }
}

impl AgentTuiRunLine {
    fn text(&self) -> &str {
        match self {
            Self::Message(message) => &message.text,
            Self::ToolLifecycle(tool) => &tool.tool_name,
            Self::Diff(diff) => &diff.title,
            Self::Review(review) => &review.subject,
            Self::Plan(plan) => &plan.title,
            Self::Status(status) => &status.title,
            Self::History(history) => &history.title,
            Self::Hook(hook) => &hook.name,
            Self::Command(command) => &command.title,
            Self::GenerationStatus(status) => &status.title,
            Self::Reasoning(reasoning) => &reasoning.text,
            Self::Error(error) => &error.message,
            Self::Session(info) => &info.session_id,
            Self::ToolReceipt(receipt) => &receipt.summary,
            Self::Usage(_) => "token usage",
        }
    }

    fn has_renderable_content(&self) -> bool {
        match self {
            Self::Usage(usage) => usage.usage.has_tokens(),
            Self::Session(info) => !info.session_id.trim().is_empty(),
            Self::ToolLifecycle(tool) => !tool.tool_name.trim().is_empty(),
            _ => !self.text().trim().is_empty(),
        }
    }

    pub(super) fn into_transcript_parts(self) -> Vec<AgentTuiTranscriptPart> {
        match self {
            Self::Message(message) => vec![AgentTuiTranscriptPart::Message(message)],
            Self::Session(_) => Vec::new(),
            Self::Usage(usage) => vec![AgentTuiTranscriptPart::Usage(usage)],
            Self::ToolReceipt(receipt) => vec![AgentTuiTranscriptPart::ToolReceipt(receipt)],
            Self::ToolLifecycle(tool) => vec![AgentTuiTranscriptPart::ToolLifecycle(tool)],
            Self::Diff(diff) => vec![AgentTuiTranscriptPart::Diff(diff)],
            Self::Review(review) => vec![AgentTuiTranscriptPart::Review(review)],
            Self::Plan(plan) => vec![AgentTuiTranscriptPart::Plan(plan)],
            Self::Status(status) => vec![AgentTuiTranscriptPart::Status(status)],
            Self::History(history) => vec![AgentTuiTranscriptPart::History(history)],
            Self::Hook(hook) => hook
                .should_persist_history_line()
                .then(|| AgentTuiTranscriptPart::HookHistory(hook.persistent_history_line()))
                .into_iter()
                .collect(),
            Self::Command(command) => vec![AgentTuiTranscriptPart::Command(command)],
            Self::GenerationStatus(status) => {
                vec![AgentTuiTranscriptPart::GenerationStatus(status)]
            }
            Self::Reasoning(reasoning) => vec![AgentTuiTranscriptPart::Reasoning(
                AgentTuiReasoningPart::completed(reasoning),
            )],
            Self::Error(error) => vec![AgentTuiTranscriptPart::Error(error)],
        }
    }

    pub(in crate::r#loop::agent_tui) fn render_cells(self) -> Vec<RenderCellRecord> {
        render_transcript_parts(self.into_transcript_parts())
    }

    pub(super) fn render_cell(self) -> Option<RenderCellRecord> {
        self.render_cells().into_iter().next()
    }
}
