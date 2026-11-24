//! Transcript part 事实模型与 cell 投影。

use super::{
    AgentTuiHistoryLine, AgentTuiLifecycleLine, AgentTuiLifecyclePhase, AgentTuiReasoningLine,
    AgentTuiReviewLine, AgentTuiRunError, AgentTuiStatusLine, AgentTuiToolLifecycleLine,
    AgentTuiToolReceipt, AgentTuiUsageLine,
};
use crate::{
    r#loop::agent_tui::{
        error_projection::run_error_cell, run_output_diff::AgentTuiDiffLine, tool_output_projection,
    },
    surface::{
        cell::{
            RenderCellRecord, RenderCommandCell, RenderGenerationStatusCell, RenderReasoningCell,
            RenderStatusCell,
        },
        cell_diff_data::AgentRenderDiffCell,
        cell_event_data::{AgentRenderHistoryCell, AgentRenderUsageCell},
        cell_plan_data::AgentRenderPlanCell,
        cell_review_data::AgentRenderReviewCell,
    },
};

const UNKNOWN_PART_ID: &str = "unknown";

/// TUI transcript 可渲染 part。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTuiTranscriptPart {
    /// Assistant/User 文本。
    Message(AgentTuiMessagePart),
    /// 工具执行结果。
    ToolReceipt(AgentTuiToolReceipt),
    /// 工具生命周期事件。
    ToolLifecycle(AgentTuiToolLifecycleLine),
    /// 文件变更 Diff。
    Diff(AgentTuiDiffLine),
    /// Verify 停止产物。
    Review(AgentTuiReviewLine),
    /// 自修复循环时间线。
    Plan(super::AgentTuiPlanLine),
    /// Runtime 状态。
    Status(AgentTuiStatusLine),
    /// 历史与上下文生命周期。
    History(AgentTuiHistoryLine),
    /// Hook 历史条目。
    HookHistory(AgentTuiHistoryLine),
    /// 命令反馈。
    Command(AgentTuiStatusLine),
    /// 模型生成生命周期状态。
    GenerationStatus(AgentTuiLifecycleLine),
    /// 模型 reasoning 输出。
    Reasoning(AgentTuiReasoningPart),
    /// Token usage。
    Usage(AgentTuiUsageLine),
    /// 错误输出。
    Error(AgentTuiRunError),
}

/// transcript 文本 part。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiMessagePart {
    /// 稳定 message part id。
    pub id: String,
    /// 文本角色。
    pub role: AgentTuiMessagePartRole,
    /// 文本内容。
    pub text: String,
}

/// transcript 文本角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiMessagePartRole {
    /// Assistant 输出。
    Assistant,
    /// User 输入。
    User,
}

/// transcript reasoning part。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiReasoningPart {
    /// reasoning id。
    pub id: String,
    /// reasoning 文本。
    pub text: String,
    /// 是否已结束。
    pub done: bool,
}

/// opencode 风格的 part 分组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTuiTranscriptPartGroup {
    /// 单个独立 part。
    Part {
        /// 稳定 group key。
        key: String,
        /// part 内容。
        part: AgentTuiTranscriptPart,
    },
    /// 连续上下文 part 分组。
    Context {
        /// 稳定 group key。
        key: String,
        /// 上下文 part 列表。
        parts: Vec<AgentTuiTranscriptPart>,
    },
}

impl AgentTuiMessagePart {
    /// 创建 assistant 文本 part。
    #[must_use]
    pub fn assistant(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role: AgentTuiMessagePartRole::Assistant,
            text: text.into(),
        }
    }

    /// 创建 user 文本 part。
    ///
    /// 用户消息正文里可能带结构化问答回执块（作答落库时的机器编码）。它在任何
    /// 界面上都不是可见正文，因此在唯一的用户文本入口处按编码器口径剥掉，
    /// 各条投递路径不必各自记得处理。
    #[must_use]
    pub fn user(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role: AgentTuiMessagePartRole::User,
            text: crate::r#loop::agent_tui::user_question_receipt::strip_question_receipt_block(
                &text.into(),
            ),
        }
    }
}

impl AgentTuiReasoningPart {
    /// 创建 reasoning part。
    #[must_use]
    pub fn new(id: impl Into<String>, text: impl Into<String>, done: bool) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            done,
        }
    }

    /// 从 run output reasoning 行创建已完成 part。
    #[must_use]
    pub fn completed(line: AgentTuiReasoningLine) -> Self {
        Self::new(line.id, line.text, true)
    }
}

impl AgentTuiTranscriptPart {
    /// 是否可进入 transcript 渲染。
    #[must_use]
    pub fn renderable(&self) -> bool {
        match self {
            Self::Usage(usage) => usage.usage.has_tokens(),
            Self::ToolLifecycle(tool) => !tool.tool_name.trim().is_empty(),
            Self::Message(message) => !message.text.trim().is_empty(),
            Self::Reasoning(reasoning) => !reasoning.text.trim().is_empty(),
            Self::History(history) | Self::HookHistory(history) => !history.title.trim().is_empty(),
            Self::Diff(diff) => !diff.title.trim().is_empty(),
            Self::Review(review) => !review.subject.trim().is_empty(),
            Self::Plan(plan) => !plan.title.trim().is_empty(),
            Self::Status(status) | Self::Command(status) => !status.title.trim().is_empty(),
            Self::GenerationStatus(status) => !status.title.trim().is_empty(),
            Self::ToolReceipt(receipt) => !receipt.summary.trim().is_empty(),
            Self::Error(error) => !error.message.trim().is_empty(),
        }
    }

    fn context_groupable(&self) -> bool {
        matches!(self, Self::History(_) | Self::HookHistory(_))
    }

    fn part_id(&self) -> String {
        match self {
            Self::Message(message) => stable_part_id(&message.id),
            Self::ToolReceipt(receipt) => stable_part_id(&receipt.id),
            Self::ToolLifecycle(tool) => stable_part_id(&tool.id),
            Self::Diff(diff) => stable_part_id(&diff.id),
            Self::Review(review) => stable_part_id(&review.id),
            Self::Plan(plan) => stable_part_id(&plan.id),
            Self::Status(status) => stable_part_id(&status.id),
            Self::History(history) => stable_part_id(&history.id),
            Self::HookHistory(history) => stable_part_id(&history.id),
            Self::Command(command) => stable_part_id(&command.id),
            Self::GenerationStatus(status) => stable_part_id(&status.id),
            Self::Reasoning(reasoning) => stable_part_id(&reasoning.id),
            Self::Usage(usage) => stable_part_id(&usage.id),
            Self::Error(error) => stable_part_id(&error.id),
        }
    }

    fn group_key(&self) -> String {
        format!("part:{}", self.part_id())
    }

    fn context_group_key(&self) -> String {
        format!("context:{}", self.part_id())
    }

    fn render_lines(self) -> Vec<RenderCellRecord> {
        match self {
            Self::Diff(diff) => diff_render_line(diff).into_iter().collect(),
            other => other.render_line().into_iter().collect(),
        }
    }

    fn render_line(self) -> Option<RenderCellRecord> {
        match self {
            Self::Message(message) => match message.role {
                AgentTuiMessagePartRole::Assistant => {
                    let text = axon_memory::project_memory_citation_text(&message.text);
                    (!text.trim().is_empty())
                        .then(|| RenderCellRecord::agent_message(message.id, text))
                }
                AgentTuiMessagePartRole::User => {
                    Some(RenderCellRecord::user_message(message.id, message.text))
                }
            },
            Self::Usage(usage) => Some(RenderCellRecord::usage_with_data(
                AgentRenderUsageCell::from_usage_line(&usage),
            )),
            Self::ToolReceipt(receipt) => Some(tool_output_projection::tool_result_line(&receipt)),
            Self::ToolLifecycle(tool) => Some(tool_output_projection::tool_lifecycle_line(&tool)),
            Self::Diff(diff) => diff_render_line(diff),
            Self::Review(review) => Some(RenderCellRecord::review_with_data(
                AgentRenderReviewCell::from_tui_line(&review),
            )),
            Self::Plan(plan) => Some(RenderCellRecord::plan_with_data(
                AgentRenderPlanCell::from_tui_line(&plan),
            )),
            Self::Status(status) => status_render_line(status),
            Self::History(history) | Self::HookHistory(history) => {
                Some(RenderCellRecord::history_with_data(
                    AgentRenderHistoryCell::from_tui_line(&history),
                ))
            }
            Self::Command(command) => Some(RenderCellRecord::command_with_data(
                RenderCommandCell::from_tui_status(&command),
            )),
            Self::GenerationStatus(status) => Some(RenderCellRecord::generation_status_with_data(
                RenderGenerationStatusCell::from_lifecycle_line(&status),
            )),
            Self::Reasoning(reasoning) => {
                Some(RenderCellRecord::reasoning_with_data(RenderReasoningCell {
                    id: reasoning.id,
                    text: reasoning.text,
                    done: reasoning.done,
                    elapsed_ms: None,
                }))
            }
            Self::Error(error) => Some(RenderCellRecord::error_with_data(run_error_cell(&error))),
        }
    }
}

impl AgentTuiTranscriptPartGroup {
    fn render_lines(self) -> Vec<RenderCellRecord> {
        match self {
            Self::Part { part, .. } => part.render_lines(),
            Self::Context { parts, .. } => parts
                .into_iter()
                .flat_map(AgentTuiTranscriptPart::render_lines)
                .collect(),
        }
    }
}

/// 过滤并分组 transcript parts。
#[must_use]
pub fn group_transcript_parts(
    parts: impl IntoIterator<Item = AgentTuiTranscriptPart>,
) -> Vec<AgentTuiTranscriptPartGroup> {
    let renderable = parts
        .into_iter()
        .filter(AgentTuiTranscriptPart::renderable)
        .collect::<Vec<_>>();
    let mut groups = Vec::new();
    let mut context_start_key = None::<String>;
    let mut context_parts = Vec::new();

    for part in renderable {
        if part.context_groupable() {
            if context_parts.is_empty() {
                context_start_key = Some(part.context_group_key());
            }
            context_parts.push(part);
            continue;
        }
        flush_context_group(&mut groups, &mut context_start_key, &mut context_parts);
        groups.push(AgentTuiTranscriptPartGroup::Part {
            key: part.group_key(),
            part,
        });
    }
    flush_context_group(&mut groups, &mut context_start_key, &mut context_parts);
    groups
}

/// 将 part 管线渲染为 transcript cells。
#[must_use]
pub fn render_transcript_parts(
    parts: impl IntoIterator<Item = AgentTuiTranscriptPart>,
) -> Vec<RenderCellRecord> {
    group_transcript_parts(parts)
        .into_iter()
        .flat_map(AgentTuiTranscriptPartGroup::render_lines)
        .collect()
}

fn flush_context_group(
    groups: &mut Vec<AgentTuiTranscriptPartGroup>,
    context_start_key: &mut Option<String>,
    context_parts: &mut Vec<AgentTuiTranscriptPart>,
) {
    if context_parts.is_empty() {
        return;
    }
    groups.push(AgentTuiTranscriptPartGroup::Context {
        key: context_start_key
            .take()
            .unwrap_or_else(|| "context:unknown".to_owned()),
        parts: std::mem::take(context_parts),
    });
}

fn diff_render_line(diff: AgentTuiDiffLine) -> Option<RenderCellRecord> {
    let mut cell = AgentRenderDiffCell::from_tui_line(&diff);
    cell.changes.retain(|path, change| {
        let (added, removed) = crate::surface::cell_diff_data::line_stats(change);
        let is_rename = matches!(
            change,
            axon_protocol::diff::FileChange::Update {
                move_path: Some(_),
                ..
            }
        );
        let has_diagnostics = cell
            .diagnostics
            .get(path)
            .is_some_and(|diagnostics| !diagnostics.is_empty());
        added > 0 || removed > 0 || is_rename || has_diagnostics
    });
    (!cell.changes.is_empty()).then(|| RenderCellRecord::file_change_with_data(cell))
}

fn status_render_line(status: AgentTuiStatusLine) -> Option<RenderCellRecord> {
    let phase = status.items.first()?.phase?;
    match phase {
        AgentTuiLifecyclePhase::Command => Some(RenderCellRecord::command_with_data(
            RenderCommandCell::from_tui_status(&status),
        )),
        AgentTuiLifecyclePhase::Notice => Some(RenderCellRecord::status_with_data(
            RenderStatusCell::from_tui_status(&status),
        )),
        AgentTuiLifecyclePhase::Streaming
        | AgentTuiLifecyclePhase::Generation
        | AgentTuiLifecyclePhase::Unknown => None,
    }
}

fn stable_part_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        UNKNOWN_PART_ID.to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
#[path = "run_output_parts_identity_tests.rs"]
mod identity_tests;

#[cfg(test)]
mod tests {
    use super::super::{
        AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput, AgentTuiStatusItem,
    };
    use super::*;
    use crate::surface::cell::{RenderCellData, RenderCellKind, RenderCellMode};

    #[test]
    fn grouping_keeps_adjacent_context_parts_together() {
        let groups = group_transcript_parts([
            AgentTuiTranscriptPart::History(AgentTuiHistoryLine::new(
                "记忆",
                vec![super::super::AgentTuiHistoryItem::new(
                    "命中偏好",
                    Some("Profile".to_owned()),
                    Some("完成".to_owned()),
                )],
            )),
            AgentTuiTranscriptPart::HookHistory(AgentTuiHistoryLine::new(
                "Hook",
                vec![super::super::AgentTuiHistoryItem::new(
                    "已同步",
                    Some("PostToolUse".to_owned()),
                    Some("完成".to_owned()),
                )],
            )),
            AgentTuiTranscriptPart::Reasoning(AgentTuiReasoningPart::new(
                "reasoning-1",
                "分析路径",
                true,
            )),
        ]);

        assert_eq!(groups.len(), 2);
        assert!(matches!(
            &groups[0],
            AgentTuiTranscriptPartGroup::Context { key, parts }
                if key == "context:history:test:记忆" && parts.len() == 2
        ));
        assert!(matches!(
            &groups[1],
            AgentTuiTranscriptPartGroup::Part {
                key,
                part: AgentTuiTranscriptPart::Reasoning(_),
            }
                if key == "part:reasoning-1"
        ));
    }

    #[test]
    fn render_transcript_parts_filters_empty_text_and_keeps_reasoning_cell() {
        let cells = render_transcript_parts([
            AgentTuiTranscriptPart::Message(AgentTuiMessagePart::assistant(
                "assistant:test",
                "   ",
            )),
            AgentTuiTranscriptPart::Reasoning(AgentTuiReasoningPart::new(
                "reasoning-1",
                "分析路径",
                true,
            )),
        ]);

        assert_eq!(cells.len(), 1);
        assert!(matches!(
            cells[0].mode,
            RenderCellMode::Cell(RenderCellKind::REASONING)
        ));
        assert!(matches!(
            &cells[0].data,
            Some(RenderCellData::Reasoning(cell))
                if cell.id == "reasoning-1" && cell.text == "分析路径" && cell.done
        ));
    }

    #[test]
    fn render_transcript_parts_hides_assistant_memory_citation_markup() {
        let cells = render_transcript_parts([AgentTuiTranscriptPart::Message(
            AgentTuiMessagePart::assistant(
                "assistant:test",
                "结论<oai-mem-citation><citation_entries>memory:7</citation_entries></oai-mem-citation>结束",
            ),
        )]);

        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].text, "结论结束");
    }

    #[test]
    fn render_transcript_parts_skips_citation_only_assistant_message() {
        let cells = render_transcript_parts([AgentTuiTranscriptPart::Message(
            AgentTuiMessagePart::assistant(
                "assistant:test",
                "<oai-mem-citation><citation_entries>memory:7</citation_entries></oai-mem-citation>",
            ),
        )]);

        assert!(cells.is_empty());
    }

    #[test]
    fn render_transcript_parts_skips_noop_write_diff() {
        let changes = std::collections::BTreeMap::from([(
            std::path::PathBuf::from("src/lib.rs"),
            axon_protocol::diff::FileChange::Update {
                unified_diff: String::new(),
                move_path: None,
            },
        )]);

        let cells = render_transcript_parts([AgentTuiTranscriptPart::Diff(
            AgentTuiDiffLine::with_id("diff:noop", "文件变更", changes, 0, 0),
        )]);

        assert!(cells.is_empty());
    }

    #[test]
    fn render_transcript_parts_keeps_meaningful_write_and_rename_diffs() {
        let write = axon_protocol::diff::FileChange::Update {
            unified_diff: "@@ -1 +1 @@\n-old\n+new".to_owned(),
            move_path: None,
        };
        let rename = axon_protocol::diff::FileChange::Update {
            unified_diff: String::new(),
            move_path: Some(std::path::PathBuf::from("src/new.rs")),
        };

        for (id, change) in [("diff:write", write), ("diff:rename", rename)] {
            let changes = std::collections::BTreeMap::from([(
                std::path::PathBuf::from("src/lib.rs"),
                change,
            )]);
            let cells = render_transcript_parts([AgentTuiTranscriptPart::Diff(
                AgentTuiDiffLine::with_id(id, "文件变更", changes, 0, 0),
            )]);

            assert_eq!(cells.len(), 1, "{id} should remain visible");
            assert!(matches!(
                cells[0].mode,
                RenderCellMode::Cell(RenderCellKind::DIFF)
            ));
        }
    }

    #[test]
    fn grouping_uses_reasoning_part_id_as_stable_key() {
        let groups = group_transcript_parts([
            AgentTuiTranscriptPart::Message(AgentTuiMessagePart::assistant("assistant-1", "回答")),
            AgentTuiTranscriptPart::Reasoning(AgentTuiReasoningPart::new(
                "reasoning-1",
                "分析",
                false,
            )),
        ]);

        assert!(matches!(
            &groups[1],
            AgentTuiTranscriptPartGroup::Part { key, .. }
                if key == "part:reasoning-1"
        ));
    }

    #[test]
    fn grouping_separates_reasoning_from_generation_status() {
        let groups = group_transcript_parts([
            AgentTuiTranscriptPart::GenerationStatus(AgentTuiLifecycleLine::with_id(
                "generation:streaming:1",
                "生成回复中",
                vec![AgentTuiLifecycleStatusView::with_id(
                    AgentTuiLifecycleStatusViewInput {
                        id: "lifecycle:event:1".to_owned(),
                        title: "生成回复中".to_owned(),
                        kind: "StreamingStarted".to_owned(),
                        summary: "cpa/gpt-5.4".to_owned(),
                        phase: "streaming".to_owned(),
                        tool_name: None,
                        duration_ms: None,
                        attempt_index: Some(1),
                    },
                )],
            )),
            AgentTuiTranscriptPart::Reasoning(AgentTuiReasoningPart::new(
                "reasoning-1",
                "模型内部推理摘要",
                false,
            )),
        ]);

        assert_eq!(groups.len(), 2);
        assert!(matches!(
            &groups[0],
            AgentTuiTranscriptPartGroup::Part { key, part }
                if key == "part:generation:streaming:1"
                    && matches!(part, AgentTuiTranscriptPart::GenerationStatus(_))
        ));
        assert!(matches!(
            &groups[1],
            AgentTuiTranscriptPartGroup::Part { key, part }
                if key == "part:reasoning-1"
                    && matches!(part, AgentTuiTranscriptPart::Reasoning(_))
        ));
    }

    #[test]
    fn grouping_uses_plan_and_review_ids_as_stable_part_keys() {
        let groups = group_transcript_parts([
            AgentTuiTranscriptPart::Plan(super::super::AgentTuiPlanLine::with_id(
                "plan:event:1",
                "计划",
                None,
                vec![super::super::AgentTuiPlanStep::new(
                    "running", "检查", None, 0, 0,
                )],
            )),
            AgentTuiTranscriptPart::Review(AgentTuiReviewLine::with_id(
                "review:event:1",
                "running",
                "验证",
                vec!["cargo check".to_owned()],
            )),
        ]);

        assert!(matches!(
            &groups[0],
            AgentTuiTranscriptPartGroup::Part { key, .. }
                if key == "part:plan:event:1"
        ));
        assert!(matches!(
            &groups[1],
            AgentTuiTranscriptPartGroup::Part { key, .. }
                if key == "part:review:event:1"
        ));
    }

    #[test]
    fn grouping_uses_status_and_command_ids_as_stable_part_keys() {
        let status_item = AgentTuiStatusItem::new(
            "notice",
            "准备完成",
            Some(1),
            Some("notice".to_owned()),
            None,
            None,
        );
        let command_item = AgentTuiStatusItem::new(
            "command",
            "执行命令",
            Some(1),
            Some("command".to_owned()),
            Some("cargo check".to_owned()),
            None,
        );
        let groups = group_transcript_parts([
            AgentTuiTranscriptPart::Status(AgentTuiStatusLine::with_id(
                "status:wire:1",
                "状态",
                vec![status_item],
            )),
            AgentTuiTranscriptPart::Command(AgentTuiStatusLine::with_id(
                "command:wire:1",
                "命令",
                vec![command_item],
            )),
        ]);

        assert!(matches!(
            &groups[0],
            AgentTuiTranscriptPartGroup::Part { key, .. }
                if key == "part:status:wire:1"
        ));
        assert!(matches!(
            &groups[1],
            AgentTuiTranscriptPartGroup::Part { key, .. }
                if key == "part:command:wire:1"
        ));
    }

    #[test]
    fn grouping_uses_history_usage_and_error_ids_as_stable_part_keys() {
        let groups = group_transcript_parts([
            AgentTuiTranscriptPart::History(AgentTuiHistoryLine::with_id(
                "history:wire:1",
                "历史",
                vec![super::super::AgentTuiHistoryItem::new(
                    "上下文压缩",
                    Some("上下文".to_owned()),
                    Some("完成".to_owned()),
                )],
            )),
            AgentTuiTranscriptPart::Usage(AgentTuiUsageLine::with_id(
                "usage:wire:1",
                super::super::AgentTuiTokenUsage::new(10, 0, 0, 5, 0),
            )),
            AgentTuiTranscriptPart::Error(AgentTuiRunError::message_with_id(
                "error:wire:1",
                "provider failed",
            )),
        ]);

        assert!(matches!(
            &groups[0],
            AgentTuiTranscriptPartGroup::Context { key, .. }
                if key == "context:history:wire:1"
        ));
        assert!(matches!(
            &groups[1],
            AgentTuiTranscriptPartGroup::Part { key, .. }
                if key == "part:usage:wire:1"
        ));
        assert!(matches!(
            &groups[2],
            AgentTuiTranscriptPartGroup::Part { key, .. }
                if key == "part:error:wire:1"
        ));
    }
}
