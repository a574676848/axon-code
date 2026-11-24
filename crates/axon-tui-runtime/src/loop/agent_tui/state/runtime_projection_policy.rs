//! Runtime 事件到 TUI surface 的统一投影策略。

use crate::r#loop::agent_tui::run_output::{
    AgentTuiLifecyclePhase, AgentTuiLifecycleStatusView, AgentTuiRunLine,
};
use crate::surface::cell::{RenderCellData, RenderCellKind, RenderCellMode, RenderCellRecord};

const LIFECYCLE_KIND_STREAMING_STARTED: &str = "StreamingStarted";
const LIFECYCLE_KIND_STREAMING_STARTED_SNAKE: &str = "streaming_started";
const LIFECYCLE_KIND_GENERATION: &str = "Generation";
const LIFECYCLE_KIND_GENERATION_SNAKE: &str = "generation";
const LIFECYCLE_SUMMARY_GENERATING_REPLY: &str = "生成回复";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) enum RuntimeProjectionTarget {
    Transcript,
    LiveTail,
    DockStatus,
    Drop,
}

pub(in crate::r#loop::agent_tui) struct RuntimeProjectionPolicy;

impl RuntimeProjectionPolicy {
    pub(in crate::r#loop::agent_tui) fn lifecycle_status_target(
        view: &AgentTuiLifecycleStatusView,
        active_lines: &[&RenderCellRecord],
    ) -> RuntimeProjectionTarget {
        if generic_reply_status_is_covered_by_streaming(view, active_lines) {
            return RuntimeProjectionTarget::Drop;
        }
        match view.phase {
            AgentTuiLifecyclePhase::Streaming | AgentTuiLifecyclePhase::Generation => {
                RuntimeProjectionTarget::DockStatus
            }
            AgentTuiLifecyclePhase::Notice => RuntimeProjectionTarget::LiveTail,
            AgentTuiLifecyclePhase::Command => RuntimeProjectionTarget::Transcript,
            AgentTuiLifecyclePhase::Unknown => RuntimeProjectionTarget::Drop,
        }
    }

    pub(in crate::r#loop::agent_tui) fn lifecycle_cell_target(
        line: &RenderCellRecord,
    ) -> RuntimeProjectionTarget {
        match line.mode {
            RenderCellMode::Cell(RenderCellKind::GENERATION_STATUS) => {
                RuntimeProjectionTarget::DockStatus
            }
            RenderCellMode::Cell(RenderCellKind::STATUS) => RuntimeProjectionTarget::LiveTail,
            RenderCellMode::Cell(RenderCellKind::COMMAND) => RuntimeProjectionTarget::Transcript,
            RenderCellMode::Cell(_) | RenderCellMode::Line | RenderCellMode::Startup => {
                RuntimeProjectionTarget::Transcript
            }
        }
    }

    pub(in crate::r#loop::agent_tui) fn final_output_target(
        line: &AgentTuiRunLine,
    ) -> RuntimeProjectionTarget {
        match line {
            AgentTuiRunLine::GenerationStatus(_) | AgentTuiRunLine::Status(_) => {
                RuntimeProjectionTarget::DockStatus
            }
            AgentTuiRunLine::Hook(hook) if !hook.status.is_terminal() => {
                RuntimeProjectionTarget::Drop
            }
            _ => RuntimeProjectionTarget::Transcript,
        }
    }

    pub(in crate::r#loop::agent_tui) fn active_tail_allows(line: &RenderCellRecord) -> bool {
        matches!(
            Self::lifecycle_cell_target(line),
            RuntimeProjectionTarget::Transcript | RuntimeProjectionTarget::LiveTail
        )
    }
}

fn generic_reply_status_is_covered_by_streaming(
    view: &AgentTuiLifecycleStatusView,
    active_lines: &[&RenderCellRecord],
) -> bool {
    lifecycle_kind_is_generation(&view.kind)
        && view.summary == LIFECYCLE_SUMMARY_GENERATING_REPLY
        && active_lines.iter().any(|line| {
            matches!(
                &line.data,
                Some(RenderCellData::GenerationStatus(cell))
                    if cell.items.iter().any(|item| lifecycle_kind_is_streaming_started(&item.kind))
            )
        })
}

fn lifecycle_kind_is_generation(kind: &str) -> bool {
    matches!(
        kind,
        LIFECYCLE_KIND_GENERATION | LIFECYCLE_KIND_GENERATION_SNAKE
    )
}

fn lifecycle_kind_is_streaming_started(kind: &str) -> bool {
    matches!(
        kind,
        LIFECYCLE_KIND_STREAMING_STARTED | LIFECYCLE_KIND_STREAMING_STARTED_SNAKE
    )
}
