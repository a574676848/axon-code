//! RenderCell 行为策略。

use crate::surface::{
    cell::{
        ROLE_ERROR, RenderCell, RenderCellAction, RenderCellData, RenderCellKind, RenderCellMode,
        is_primary_runtime_cell_kind,
    },
    terminal_capabilities::TerminalCompatibilityPolicy,
};

const LONG_SUMMARY_TEXT_CHARS: usize = 240;
const LONG_DIFF_FILE_COUNT: usize = 4;
const RETRY_LIFECYCLE_KIND_PROVIDER_RETRY: &str = "ProviderRetry";
const RETRY_LIFECYCLE_KIND_PROVIDER_RETRY_SNAKE: &str = "provider_retry";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPolicySummaryMode {
    Full,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderPolicyDecision {
    pub(crate) summary_mode: RenderPolicySummaryMode,
    pub(crate) expanded_by_default: bool,
    pub(crate) selectable: bool,
    pub(crate) copyable: bool,
    pub(crate) openable: bool,
    pub(crate) history_persisted: bool,
    pub(crate) active_tail: bool,
    pub(crate) open_in_diff_viewer: bool,
    pub(crate) hyperlink: RenderPolicyHyperlink,
    pub(crate) copy_action: RenderPolicyCopyAction,
    pub(crate) mouse_action: RenderPolicyMouseAction,
    pub(crate) visual_roles: RenderPolicyVisualRoles,
}

impl RenderPolicyDecision {
    pub(crate) const fn render_expanded(self, selected: bool) -> bool {
        self.expanded_by_default || (selected && self.openable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPolicyHyperlink {
    Osc8,
    PlainText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPolicyCopyAction {
    SystemClipboard,
    FailSilent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPolicyMouseAction {
    Enabled,
    KeyboardOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderPolicyVisualRoles {
    pub(crate) title: RenderPolicyColorRole,
    pub(crate) body: RenderPolicyColorRole,
    pub(crate) detail: RenderPolicyColorRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPolicyColorRole {
    User,
    Assistant,
    ToolRunning,
    ToolCompleted,
    ToolFailed,
    DiffAdded,
    DiffHunk,
    Error,
    Muted,
    Accent,
    Loading,
}

pub(crate) struct RenderPolicy;

impl RenderPolicy {
    pub(crate) fn decide(cell: &dyn RenderCell) -> RenderPolicyDecision {
        Self::decide_with_terminal(cell, TerminalCompatibilityPolicy::current())
    }

    pub(crate) fn decide_with_terminal(
        cell: &dyn RenderCell,
        terminal: TerminalCompatibilityPolicy,
    ) -> RenderPolicyDecision {
        if let Some(record) = cell
            .as_any()
            .downcast_ref::<crate::surface::cell::RenderCellRecord>()
        {
            return Self::decide_record(record, terminal);
        }

        let summary = cell.summary();
        Self::decide_parts(
            &summary.mode,
            summary.action,
            summary.role,
            &summary.text,
            None,
            cell.is_stream_continuation(),
            terminal,
        )
    }

    fn decide_record(
        record: &crate::surface::cell::RenderCellRecord,
        terminal: TerminalCompatibilityPolicy,
    ) -> RenderPolicyDecision {
        Self::decide_parts(
            &record.mode,
            record.action,
            record.role,
            &record.text,
            record.data.as_ref(),
            record.is_stream_continuation(),
            terminal,
        )
    }

    fn decide_parts(
        mode: &RenderCellMode,
        action: RenderCellAction,
        role: &'static str,
        text: &str,
        data: Option<&RenderCellData>,
        active_tail: bool,
        terminal: TerminalCompatibilityPolicy,
    ) -> RenderPolicyDecision {
        let kind = cell_kind(mode);
        let summary_mode = summary_mode(kind, text, data);
        let selectable = selectable(kind);
        let copyable = copyable(kind, data);
        let open_in_diff_viewer = open_in_diff_viewer(kind, terminal);
        RenderPolicyDecision {
            summary_mode,
            expanded_by_default: expanded_by_default(kind, action, role),
            selectable,
            copyable,
            openable: openable(kind, data),
            history_persisted: history_persisted(kind, active_tail, data),
            active_tail,
            open_in_diff_viewer,
            hyperlink: if terminal.supports_osc8_hyperlinks() {
                RenderPolicyHyperlink::Osc8
            } else {
                RenderPolicyHyperlink::PlainText
            },
            copy_action: if terminal.supports_osc52_clipboard() {
                RenderPolicyCopyAction::SystemClipboard
            } else {
                RenderPolicyCopyAction::FailSilent
            },
            mouse_action: if terminal.supports_mouse() {
                RenderPolicyMouseAction::Enabled
            } else {
                RenderPolicyMouseAction::KeyboardOnly
            },
            visual_roles: visual_roles(kind, action, role),
        }
    }
}

fn cell_kind(mode: &RenderCellMode) -> Option<RenderCellKind> {
    match mode {
        RenderCellMode::Cell(kind) => Some(*kind),
        RenderCellMode::Line | RenderCellMode::Startup => None,
    }
}

fn summary_mode(
    kind: Option<RenderCellKind>,
    text: &str,
    data: Option<&RenderCellData>,
) -> RenderPolicySummaryMode {
    if matches!(
        kind,
        Some(RenderCellKind::EXEC | RenderCellKind::MCP | RenderCellKind::DIFF)
    ) && is_long_cell(text, data)
    {
        RenderPolicySummaryMode::Summary
    } else {
        RenderPolicySummaryMode::Full
    }
}

fn is_long_cell(text: &str, data: Option<&RenderCellData>) -> bool {
    if text.chars().count() > LONG_SUMMARY_TEXT_CHARS {
        return true;
    }
    match data {
        Some(RenderCellData::Exec(cell)) => cell
            .output_preview
            .as_deref()
            .is_some_and(|output| output.chars().count() > LONG_SUMMARY_TEXT_CHARS),
        Some(RenderCellData::Mcp(cell)) => cell.items.iter().any(|item| {
            item.output_preview
                .as_deref()
                .is_some_and(|output| output.chars().count() > LONG_SUMMARY_TEXT_CHARS)
        }),
        Some(RenderCellData::Diff(cell)) => cell.changes.len() > LONG_DIFF_FILE_COUNT,
        _ => false,
    }
}

fn expanded_by_default(
    kind: Option<RenderCellKind>,
    action: RenderCellAction,
    role: &'static str,
) -> bool {
    if role == ROLE_ERROR {
        return true;
    }
    matches!((kind, action), (Some(RenderCellKind::ERROR), _))
}

fn selectable(kind: Option<RenderCellKind>) -> bool {
    kind.is_some_and(is_primary_runtime_cell_kind)
}

fn copyable(kind: Option<RenderCellKind>, data: Option<&RenderCellData>) -> bool {
    match kind {
        Some(RenderCellKind::EXEC) => matches!(data, Some(RenderCellData::Exec(_))),
        Some(RenderCellKind::MCP) => matches!(data, Some(RenderCellData::Mcp(_))),
        Some(RenderCellKind::TURN_SUMMARY) => false,
        _ => true,
    }
}

fn openable(kind: Option<RenderCellKind>, data: Option<&RenderCellData>) -> bool {
    match kind {
        Some(RenderCellKind::EXEC) => matches!(data, Some(RenderCellData::Exec(_))),
        Some(RenderCellKind::MCP) => matches!(data, Some(RenderCellData::Mcp(_))),
        Some(RenderCellKind::DIFF) => {
            matches!(data, Some(RenderCellData::Diff(cell)) if cell.has_diff_content())
        }
        Some(RenderCellKind::REASONING) => matches!(data, Some(RenderCellData::Reasoning(_))),
        _ => false,
    }
}

fn history_persisted(
    kind: Option<RenderCellKind>,
    active_tail: bool,
    data: Option<&RenderCellData>,
) -> bool {
    if active_tail {
        return false;
    }
    match kind {
        Some(RenderCellKind::STATUS) => false,
        Some(RenderCellKind::GENERATION_STATUS) => generation_status_history_persisted(data),
        _ => true,
    }
}

fn generation_status_history_persisted(data: Option<&RenderCellData>) -> bool {
    let Some(RenderCellData::GenerationStatus(cell)) = data else {
        return false;
    };
    cell.items.iter().any(|item| {
        matches!(
            item.kind.as_str(),
            RETRY_LIFECYCLE_KIND_PROVIDER_RETRY | RETRY_LIFECYCLE_KIND_PROVIDER_RETRY_SNAKE
        )
    })
}

fn open_in_diff_viewer(
    kind: Option<RenderCellKind>,
    terminal: TerminalCompatibilityPolicy,
) -> bool {
    matches!(kind, Some(RenderCellKind::DIFF)) && terminal.supports_diff_viewer()
}

fn visual_roles(
    kind: Option<RenderCellKind>,
    action: RenderCellAction,
    role: &'static str,
) -> RenderPolicyVisualRoles {
    let title = if role == ROLE_ERROR {
        RenderPolicyColorRole::Error
    } else {
        action_color_role(action)
    };
    RenderPolicyVisualRoles {
        title,
        body: RenderPolicyColorRole::Assistant,
        detail: kind_color_role(kind),
    }
}

fn action_color_role(action: RenderCellAction) -> RenderPolicyColorRole {
    match action {
        RenderCellAction::Read | RenderCellAction::Mcp | RenderCellAction::Status => {
            RenderPolicyColorRole::Accent
        }
        RenderCellAction::List
        | RenderCellAction::Search
        | RenderCellAction::Searched
        | RenderCellAction::Command
        | RenderCellAction::History
        | RenderCellAction::Usage => RenderPolicyColorRole::DiffHunk,
        RenderCellAction::TurnSummary => RenderPolicyColorRole::Muted,
        RenderCellAction::Patched | RenderCellAction::Review | RenderCellAction::Plan => {
            RenderPolicyColorRole::ToolCompleted
        }
        RenderCellAction::Error => RenderPolicyColorRole::ToolFailed,
        RenderCellAction::Running
        | RenderCellAction::Waiting
        | RenderCellAction::Called
        | RenderCellAction::Hook => RenderPolicyColorRole::ToolRunning,
        RenderCellAction::GenerationStatus
        | RenderCellAction::Reasoning
        | RenderCellAction::Answer
        | RenderCellAction::Run
        | RenderCellAction::Ran => RenderPolicyColorRole::Loading,
        RenderCellAction::User => RenderPolicyColorRole::User,
    }
}

fn kind_color_role(kind: Option<RenderCellKind>) -> RenderPolicyColorRole {
    match kind {
        Some(RenderCellKind::COMMAND | RenderCellKind::HISTORY | RenderCellKind::USAGE) => {
            RenderPolicyColorRole::DiffHunk
        }
        Some(RenderCellKind::DIFF) => RenderPolicyColorRole::DiffAdded,
        Some(RenderCellKind::MESSAGE) => RenderPolicyColorRole::Assistant,
        Some(RenderCellKind::EXEC | RenderCellKind::HOOK) => RenderPolicyColorRole::ToolRunning,
        Some(RenderCellKind::MCP | RenderCellKind::STATUS) => RenderPolicyColorRole::Accent,
        Some(RenderCellKind::PLAN) => RenderPolicyColorRole::ToolCompleted,
        Some(
            RenderCellKind::REVIEW | RenderCellKind::GENERATION_STATUS | RenderCellKind::REASONING,
        ) => RenderPolicyColorRole::Loading,
        Some(RenderCellKind::ERROR) => RenderPolicyColorRole::Error,
        Some(_) | None => RenderPolicyColorRole::Muted,
    }
}

#[cfg(test)]
mod tests {
    use super::{RenderPolicy, RenderPolicyCopyAction, RenderPolicySummaryMode};
    use crate::AgentTuiLifecyclePhase;
    use crate::surface::{
        cell::{
            RenderCellAction, RenderCellRecord, RenderGenerationStatusCell, RenderLifecycleItem,
        },
        cell_diff_data::AgentRenderDiffCell,
        cell_event_data::{AgentRenderExecCell, AgentRenderExecStatus},
        terminal_capabilities::{TerminalCompatibilityPolicy, TerminalEnvironment},
    };
    use axon_protocol::diff::FileChange;

    #[test]
    fn long_exec_defaults_to_summary_but_remains_openable_and_copyable() {
        let mut exec =
            AgentRenderExecCell::running_with_id("exec:test:call-1", "shell", "call-1", None);
        exec.status = AgentRenderExecStatus::Output;
        exec.output_preview = Some("x".repeat(300));
        let cell = RenderCellRecord::exec_with_data(exec, RenderCellAction::Run);

        let decision = RenderPolicy::decide(&cell);

        assert_eq!(decision.summary_mode, RenderPolicySummaryMode::Summary);
        assert!(decision.selectable);
        assert!(decision.copyable);
        assert!(decision.openable);
        assert!(!decision.active_tail);
    }

    #[test]
    fn streaming_agent_is_active_tail_and_not_history_persisted() {
        let cell = RenderCellRecord::streaming_agent_message("assistant:test:streaming", "partial");

        let decision = RenderPolicy::decide(&cell);

        assert!(decision.active_tail);
        assert!(!decision.history_persisted);
    }

    #[test]
    fn retry_generation_status_is_history_persisted() {
        let cell = RenderCellRecord::generation_status_with_data(RenderGenerationStatusCell {
            id: "test:generation:retry-policy".to_owned(),
            title: "生成".to_owned(),
            items: vec![RenderLifecycleItem {
                id: "test:generation:retry-policy:item".to_owned(),
                kind: "ProviderRetry".to_owned(),
                summary: "连接失败，正在切换渠道".to_owned(),
                phase: AgentTuiLifecyclePhase::Generation,
                indicator: None,
                tool_name: Some("gpt-5.4".to_owned()),
                duration_ms: None,
                attempt_index: Some(2),
            }],
        });

        let decision = RenderPolicy::decide(&cell);

        assert!(decision.history_persisted);
    }

    #[test]
    fn terminal_degradation_is_part_of_policy() {
        let terminal = TerminalCompatibilityPolicy::from_environment(
            &TerminalEnvironment::from_pairs([("TMUX", "active")]),
        );
        let decision = RenderPolicy::decide_with_terminal(
            &RenderCellRecord::agent_message("assistant:test:plain", "plain"),
            terminal,
        );

        assert_eq!(decision.copy_action, RenderPolicyCopyAction::FailSilent);
        assert!(!matches!(
            decision.hyperlink,
            super::RenderPolicyHyperlink::Osc8
        ));
    }

    #[test]
    fn diff_viewer_openability_falls_back_to_copyable_when_terminal_lacks_viewer() {
        let cell = RenderCellRecord::file_change_with_data(AgentRenderDiffCell {
            id: "diff:test:render-policy".to_owned(),
            title: "修改".to_owned(),
            changes: std::collections::BTreeMap::from([(
                std::path::PathBuf::from("src/lib.rs"),
                FileChange::Add {
                    content: "pub fn value() {}\n".to_owned(),
                },
            )]),
            diagnostics: std::collections::BTreeMap::new(),
            text_writes_applied: 0,
            renames_applied: 0,
            truncated: false,
            truncation_boundary: None,
        });
        let terminal = TerminalCompatibilityPolicy::from_environment(
            &TerminalEnvironment::from_pairs([("TERM", "xterm-256color")]),
        );

        let decision = RenderPolicy::decide_with_terminal(&cell, terminal);

        assert!(!decision.open_in_diff_viewer);
        assert!(decision.openable);
        assert!(decision.copyable);
    }
}
