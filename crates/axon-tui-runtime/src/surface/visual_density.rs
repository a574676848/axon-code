//! TUI 视觉密度与噪声预算事实源。
//!
//! 这里只定义产品级视觉规则：cell 间距、14 类 cell 排版、动画预算与廉价感 guard。
//! Widget 负责具体绘制，RenderPolicy 负责状态到颜色角色的选择。

use crate::surface::{
    cell::{RENDER_CELL_KIND_STARTUP, ROLE_USER, RenderCell, RenderCellKind},
    display_width::DisplayWidthText,
};

/// 二级行（详情/输出/参数）的统一缩进前缀。
pub(crate) const DETAIL_PREFIX: &str = "  └ ";
/// 二级行换行后的延续缩进。
pub(crate) const DETAIL_CONTINUATION: &str = "    ";
/// 三级行（嵌套列表、补丁 diff 子行）的统一缩进。
pub(crate) const NESTED_PREFIX: &str = "    ";
/// 中性 cell 标题前缀（命令/历史/工具/计划/Review 等已结束单元）。
pub(crate) const NEUTRAL_PREFIX: &str = "  • ";
/// 中性 cell 标题延续缩进。
pub(crate) const NEUTRAL_CONTINUATION: &str = "    ";
/// 详情列表项分隔（状态摘要、错误明细、命令项）。
pub(crate) const BULLET_PREFIX: &str = "  · ";

/// 不同状态对应的 cell 标题前缀。
pub(crate) const STATUS_RUNNING_PREFIX: &str = "  ◌ ";
pub(crate) const STATUS_FAILED_PREFIX: &str = "  ✗ ";
pub(crate) const STATUS_PENDING_PREFIX: &str = "  □ ";
pub(crate) const STATUS_SKIPPED_PREFIX: &str = "  - ";
pub(crate) const STATUS_APPROVED_PREFIX: &str = "  ✓ ";

const TURN_SEPARATOR_ROWS: u16 = 2;
const STRUCTURED_CELL_BOUNDARY_ROWS: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptWrapMode {
    Adaptive,
    PreserveUrl,
    PreservePreformatted,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptCellLayoutSpec {
    pub(crate) kind: RenderCellKind,
    pub(crate) prefix: &'static str,
    pub(crate) continuation: &'static str,
    pub(crate) detail_prefix: &'static str,
    pub(crate) summary_height: u16,
    pub(crate) expanded_height: Option<u16>,
    pub(crate) wrap_mode: TranscriptWrapMode,
}

pub(crate) const TRANSCRIPT_CELL_LAYOUT_SPECS: [TranscriptCellLayoutSpec; 16] = [
    spec(
        RenderCellKind::COMMAND,
        NEUTRAL_PREFIX,
        NEUTRAL_CONTINUATION,
        BULLET_PREFIX,
        1,
        None,
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::DIFF,
        NEUTRAL_PREFIX,
        NEUTRAL_CONTINUATION,
        NESTED_PREFIX,
        1,
        None,
        TranscriptWrapMode::Diff,
    ),
    spec(
        RenderCellKind::MESSAGE,
        "",
        "",
        DETAIL_PREFIX,
        1,
        None,
        TranscriptWrapMode::PreserveUrl,
    ),
    spec(
        RenderCellKind::EXEC,
        NEUTRAL_PREFIX,
        NEUTRAL_CONTINUATION,
        DETAIL_PREFIX,
        1,
        None,
        TranscriptWrapMode::PreservePreformatted,
    ),
    spec(
        RenderCellKind::HISTORY,
        NEUTRAL_PREFIX,
        NEUTRAL_CONTINUATION,
        BULLET_PREFIX,
        1,
        Some(6),
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::HOOK,
        NEUTRAL_PREFIX,
        NEUTRAL_CONTINUATION,
        DETAIL_PREFIX,
        1,
        None,
        TranscriptWrapMode::PreservePreformatted,
    ),
    spec(
        RenderCellKind::MCP,
        NEUTRAL_PREFIX,
        NEUTRAL_CONTINUATION,
        BULLET_PREFIX,
        1,
        Some(8),
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::PLAN,
        STATUS_PENDING_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        None,
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::REVIEW,
        STATUS_RUNNING_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        None,
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::STATUS,
        BULLET_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        Some(5),
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::GENERATION_STATUS,
        STATUS_RUNNING_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        Some(4),
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::REASONING,
        STATUS_RUNNING_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        Some(4),
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::TURN_SUMMARY,
        "",
        "",
        "",
        1,
        Some(1),
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::USAGE,
        BULLET_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        Some(3),
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::ERROR,
        STATUS_FAILED_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        None,
        TranscriptWrapMode::Adaptive,
    ),
    spec(
        RenderCellKind::QUESTION,
        STATUS_PENDING_PREFIX,
        DETAIL_CONTINUATION,
        BULLET_PREFIX,
        1,
        None,
        TranscriptWrapMode::Adaptive,
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VisualNoiseBudget {
    pub(crate) turn_separator_rows: u16,
    pub(crate) high_emphasis_cells_per_viewport: u16,
    pub(crate) icon_only_for_state_change: bool,
    pub(crate) full_border_only_for_focus_states: bool,
}

pub(crate) const VISUAL_NOISE_BUDGET: VisualNoiseBudget = VisualNoiseBudget {
    turn_separator_rows: TURN_SEPARATOR_ROWS,
    high_emphasis_cells_per_viewport: 2,
    icon_only_for_state_change: true,
    full_border_only_for_focus_states: true,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptSpacingClass {
    Startup,
    User,
    Narrative,
    Structured,
}

pub(crate) fn cell_spacing_rows(
    previous: Option<&dyn RenderCell>,
    current: &dyn RenderCell,
) -> u16 {
    let Some(previous) = previous else {
        return 0;
    };
    if spacing_class(previous) == TranscriptSpacingClass::Startup
        || spacing_class(current) == TranscriptSpacingClass::Startup
    {
        return 0;
    }
    if spacing_class(current) == TranscriptSpacingClass::User {
        return VISUAL_NOISE_BUDGET.turn_separator_rows;
    }
    if spacing_class(previous) == TranscriptSpacingClass::Narrative
        && spacing_class(current) == TranscriptSpacingClass::Narrative
        && same_cell_kind(previous, current)
    {
        return 0;
    }
    STRUCTURED_CELL_BOUNDARY_ROWS
}

pub(crate) fn cell_layout_spec(kind: RenderCellKind) -> TranscriptCellLayoutSpec {
    // Invariant: TRANSCRIPT_CELL_LAYOUT_SPECS has one layout row for every RenderCellKind variant.
    TRANSCRIPT_CELL_LAYOUT_SPECS
        .iter()
        .copied()
        .find(|spec| spec.kind == kind)
        .expect("每个 RenderCellKind 必须有 transcript layout spec")
}

pub(crate) fn validate_cell_layout_contract(kind: RenderCellKind) -> TranscriptCellLayoutSpec {
    let spec = cell_layout_spec(kind);
    debug_assert!(
        spec.summary_height >= 1,
        "cell summary height 必须至少 1 行"
    );
    debug_assert!(
        spec.expanded_height
            .is_none_or(|height| height >= spec.summary_height),
        "expanded height 不得小于 summary height"
    );
    debug_assert_eq!(
        DisplayWidthText::width(spec.prefix),
        DisplayWidthText::width(spec.continuation),
        "cell title prefix 与 continuation 必须等宽"
    );
    spec
}

const fn spec(
    kind: RenderCellKind,
    prefix: &'static str,
    continuation: &'static str,
    detail_prefix: &'static str,
    summary_height: u16,
    expanded_height: Option<u16>,
    wrap_mode: TranscriptWrapMode,
) -> TranscriptCellLayoutSpec {
    TranscriptCellLayoutSpec {
        kind,
        prefix,
        continuation,
        detail_prefix,
        summary_height,
        expanded_height,
        wrap_mode,
    }
}

fn spacing_class(cell: &dyn RenderCell) -> TranscriptSpacingClass {
    if cell.kind() == RENDER_CELL_KIND_STARTUP {
        return TranscriptSpacingClass::Startup;
    }
    if cell.summary().role == ROLE_USER {
        return TranscriptSpacingClass::User;
    }
    if cell.kind() == RenderCellKind::MESSAGE.as_str() {
        return TranscriptSpacingClass::Narrative;
    }
    TranscriptSpacingClass::Structured
}

fn same_cell_kind(previous: &dyn RenderCell, current: &dyn RenderCell) -> bool {
    previous.kind() == current.kind()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::cell::RenderCellRecord;

    #[test]
    fn density_spacing_separates_structured_cells_but_keeps_narrative_compact() {
        let tool =
            RenderCellRecord::mcp_with_data(crate::surface::cell_event_data::AgentRenderMcpCell {
                id: "mcp:test:density".to_owned(),
                title: "MCP".to_owned(),
                empty_text: "无".to_owned(),
                source: crate::surface::cell_event_data::AgentRenderMcpCellSource::ToolCall,
                items: Vec::new(),
            });
        let exec = RenderCellRecord::exec_with_data(
            crate::surface::cell_event_data::AgentRenderExecCell::running_with_id(
                "exec:test:call",
                "shell",
                "call",
                None,
            ),
            crate::surface::cell::RenderCellAction::Running,
        );
        let reasoning =
            RenderCellRecord::reasoning_with_data(crate::surface::cell::RenderReasoningCell {
                id: "reasoning:test:density".to_owned(),
                text: "分析路径".to_owned(),
                done: false,
                elapsed_ms: None,
            });
        let answer = RenderCellRecord::agent_message("assistant:test:density", "完成");
        let follow_up =
            RenderCellRecord::agent_message("assistant:test:density-follow-up", "继续补充");
        let user = RenderCellRecord::user_with_data("继续");

        assert_eq!(cell_spacing_rows(Some(&tool), &exec), 1);
        assert_eq!(cell_spacing_rows(Some(&reasoning), &exec), 1);
        assert_eq!(cell_spacing_rows(Some(&exec), &reasoning), 1);
        assert_eq!(cell_spacing_rows(Some(&reasoning), &answer), 1);
        assert_eq!(cell_spacing_rows(Some(&reasoning), &reasoning), 1);
        assert_eq!(cell_spacing_rows(Some(&user), &tool), 1);
        assert_eq!(cell_spacing_rows(Some(&exec), &answer), 1);
        assert_eq!(cell_spacing_rows(Some(&answer), &follow_up), 0);
        assert_eq!(cell_spacing_rows(Some(&answer), &user), 2);
    }

    #[test]
    fn visual_noise_budget_keeps_animation_and_emphasis_bounded() {
        assert!(crate::surface::animation::TranscriptMotionTokens::FULL.stays_within_tui_budget());
    }

    #[test]
    fn detail_prefix_and_continuation_share_visible_width() {
        assert_eq!(
            DisplayWidthText::width(DETAIL_PREFIX),
            DisplayWidthText::width(DETAIL_CONTINUATION),
        );
        assert_eq!(
            DisplayWidthText::width(NEUTRAL_PREFIX),
            DisplayWidthText::width(NEUTRAL_CONTINUATION),
        );
    }
}
