//! Agent TUI 真机 smoke 目标态契约。

use serde::Serialize;

#[cfg(test)]
const MIN_SMOKE_POINTS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiSmokeContractSnapshot {
    pub total_points: usize,
    pub required_points: usize,
    pub categories: Vec<AgentTuiSmokeCategorySnapshot>,
    pub points: Vec<AgentTuiSmokePoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiSmokeCategorySnapshot {
    pub category: &'static str,
    pub total_points: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiSmokePoint {
    pub id: &'static str,
    pub category: &'static str,
    pub target_entry: &'static str,
    pub evidence: &'static str,
    pub verification: &'static str,
    pub ability_id: Option<&'static str>,
}

#[must_use]
pub fn agent_tui_smoke_contract_snapshot() -> AgentTuiSmokeContractSnapshot {
    let points = smoke_points().to_vec();
    AgentTuiSmokeContractSnapshot {
        total_points: points.len(),
        required_points: required_smoke_point_ids().len(),
        categories: smoke_category_snapshots(&points),
        points,
    }
}

fn smoke_category_snapshots(points: &[AgentTuiSmokePoint]) -> Vec<AgentTuiSmokeCategorySnapshot> {
    SMOKE_CATEGORY_ORDER
        .iter()
        .filter_map(|category| {
            let total_points = points
                .iter()
                .filter(|point| point.category == *category)
                .count();
            (total_points > 0).then_some(AgentTuiSmokeCategorySnapshot {
                category,
                total_points,
            })
        })
        .collect()
}

pub(crate) fn required_smoke_point_ids() -> &'static [&'static str] {
    REQUIRED_SMOKE_POINT_IDS
}

pub(crate) fn smoke_points() -> &'static [AgentTuiSmokePoint] {
    SMOKE_POINTS
}

macro_rules! point {
    ($id:literal, $category:literal, $target:literal, $evidence:literal, $verification:literal, $ability:literal) => {
        AgentTuiSmokePoint {
            id: $id,
            category: $category,
            target_entry: $target,
            evidence: $evidence,
            verification: $verification,
            ability_id: Some($ability),
        }
    };
    ($id:literal, $category:literal, $target:literal, $evidence:literal, $verification:literal) => {
        AgentTuiSmokePoint {
            id: $id,
            category: $category,
            target_entry: $target,
            evidence: $evidence,
            verification: $verification,
            ability_id: None,
        }
    };
}

const SMOKE_CATEGORY_ORDER: &[&str] = &[
    "header",
    "diff_cell",
    "viewport_history",
    "resume_snapshot",
    "shortcut",
    "dock",
    "cell_visual",
    "terminal",
    "unicode_width",
    "output_backpressure",
    "runtime_contract",
    "error_presentation",
    "sub_state_invariant",
    "frame",
    "editing_mouse",
    "prompt_cache",
];

#[rustfmt::skip]
const REQUIRED_SMOKE_POINT_IDS: &[&str] = &[
    "header.width.80", "header.width.120", "header.width.160",
    "diff.file_path", "diff.action_type", "diff.line_stats", "diff.hunk_line", "diff.key_line",
    "history.detached_streaming", "history.jump_bottom", "history.jump_threshold",
    "resume.header_snapshot", "resume.viewport_cells", "resume.composer_initial_state", "resume.read_only_snapshot",
    "shortcut.esc_control",
    "dock.shortcut_help", "dock.tool_output_enter",
    "dock.skills", "dock.mcp_catalog", "dock.alt_enter",
    "resume.loading_cell", "resume.corrupt_inline_error", "resume.permission_inline_error",
    "cell.command.visual", "cell.diff.visual", "cell.message.visual", "cell.exec.visual", "cell.history.visual",
    "cell.hook.visual", "cell.mcp.visual", "cell.plan.visual", "cell.review.visual", "cell.status.visual",
    "cell.generation_status.visual", "cell.reasoning.visual", "cell.usage.visual", "cell.error.visual",
    "terminal.windows_terminal", "terminal.tmux", "terminal.screen", "terminal.ssh_slow_link",
    "unicode.cjk", "unicode.fullwidth_punctuation", "unicode.emoji", "unicode.zwj", "unicode.variation_selector",
    "unicode.skin_tone", "unicode.flag",
    "output.10mb", "output.100mb", "output.truncation_notice", "output.paged_artifact",
    "runtime.schema_mismatch", "runtime.capabilities", "runtime.steer_unsupported", "runtime.steer_rejected",
    "runtime.bounded_channel", "runtime.worker_panic",
    "error.provider_401", "error.provider_429", "error.provider_offline", "error.mcp_crash",
    "error.snapshot_corrupt", "error.snapshot_permission", "error.cell_payload_missing_field",
    "invariant.header_viewport_composer", "invariant.cell_selection_bounds", "invariant.dock_single_active",
    "invariant.runtime_correlation", "invariant.cache_key_boundary",
    "frame.thousand_schedule", "frame.no_duplicate_stable", "frame.latency_metric",
    "editing.file_completion", "editing.drag_select",
    "prompt_cache.dock", "prompt_cache.scroll", "prompt_cache.selection", "prompt_cache.animation_tick",
];

#[rustfmt::skip]
const SMOKE_POINTS: &[AgentTuiSmokePoint] = &[
    point!("header.width.80", "header", "HeaderBar 80 列裁剪", "widgets::header_bar width contract", "verify-tui-smoke: header width baseline", "system.elapsed"),
    point!("header.width.120", "header", "HeaderBar 120 列裁剪", "widgets::header_bar width contract", "verify-tui-smoke: header width baseline", "system.elapsed"),
    point!("header.width.160", "header", "HeaderBar 160 列裁剪", "widgets::header_bar width contract", "verify-tui-smoke: header width baseline", "system.lsp"),
    point!("diff.file_path", "diff_cell", "Diff cell 文件路径", "RenderCellKind::DIFF payload", "verify-tui-smoke: structured diff", "cell.diff"),
    point!("diff.action_type", "diff_cell", "Diff cell 变更类型", "WireEventKind::FileChange action", "verify-tui-smoke: structured diff", "runtime.wire.file_change"),
    point!("diff.line_stats", "diff_cell", "Diff cell 增删统计", "WireFileChangeEvent aggregate", "verify-tui-smoke: structured diff", "runtime.wire.file_change"),
    point!("diff.hunk_line", "diff_cell", "Diff cell hunk 行号", "WireFileChangeEvent hunks", "verify-tui-smoke: structured diff", "runtime.wire.file_change"),
    point!("diff.key_line", "diff_cell", "Diff cell 关键 diff 行", "WireFileChangeEvent diff lines", "verify-tui-smoke: structured diff", "runtime.wire.file_change"),
    point!("history.detached_streaming", "viewport_history", "用户上滚后 streaming 不抢焦点", "ViewportScrollState detached mode", "verify-tui-smoke: history scroll", "system.mouse_wheel"),
    point!("history.jump_bottom", "viewport_history", "jump-bottom 动作", "RenderViewport bottom attach", "verify-tui-smoke: history scroll", "system.mouse_wheel"),
    point!("history.jump_threshold", "viewport_history", "distance_from_bottom 阈值", "max(40, viewport_height) policy", "verify-tui-smoke: history scroll", "system.mouse_wheel"),
    point!("resume.header_snapshot", "resume_snapshot", "Header snapshot 恢复", "WireResumeSnapshot statusBar", "verify-tui-smoke: resume snapshot", "system.wire_resume_snapshot"),
    point!("resume.viewport_cells", "resume_snapshot", "RenderViewport cells 恢复", "WireResumeSnapshot timeline", "verify-tui-smoke: resume snapshot", "system.wire_resume_snapshot"),
    point!("resume.composer_initial_state", "resume_snapshot", "ComposerDock 初始态恢复", "WireResumeSnapshot inputPanel", "verify-tui-smoke: resume snapshot", "system.wire_resume_snapshot"),
    point!("resume.read_only_snapshot", "resume_snapshot", "只读目标态 snapshot", "wire snapshot only", "verify-tui-smoke: resume snapshot", "system.wire_resume_snapshot"),
    point!("shortcut.esc_control", "shortcut", "Esc 控制语义", "queued -> SubmitSteer; working -> InterruptRuntime; idle -> ClearDraft", "verify-tui-smoke: shortcut semantics", "system.clear"),
    point!("dock.shortcut_help", "dock", "? 打开 ShortcutHelpDock", "DockKind::ShortcutHelp", "verify-tui-smoke: dock workflow", "dock.shortcut_help"),
    point!("dock.tool_output_enter", "dock", "Ctrl+O 打开最近 ToolOutputViewerDock", "DockKind::ToolOutputViewer", "verify-tui-smoke: dock workflow", "dock.tool_output_viewer"),
    point!("dock.skills", "dock", "/skills 打开 SkillsDock", "DockKind::Skills", "verify-tui-smoke: dock workflow", "dock.skills"),
    point!("dock.mcp_catalog", "dock", "/mcp 展示 MCP server/tool 与失败列表", "RenderCellKind::MCP catalog", "verify-tui-smoke: MCP catalog workflow", "system.mcp_catalog"),
    point!("dock.alt_enter", "dock", "Alt+Enter 插入换行并保持 dock handoff", "UiAction::InsertNewline", "verify-tui-smoke: dock workflow", "input.newline"),
    point!("resume.loading_cell", "resume_snapshot", "resume loading cell 加载计数", "WireResumeSnapshot", "verify-tui-smoke: resume loading", "system.wire_resume_snapshot"),
    point!("resume.corrupt_inline_error", "resume_snapshot", "损坏 snapshot inline error cell", "WireRuntimeError", "verify-tui-smoke: resume errors", "system.wire_runtime_error"),
    point!("resume.permission_inline_error", "resume_snapshot", "权限不足 snapshot inline error cell", "WireRuntimeError", "verify-tui-smoke: resume errors", "system.wire_runtime_error"),
    point!("cell.command.visual", "cell_visual", "Command cell 80/120/160 视觉契约", "RenderCellKind::COMMAND", "verify-tui-smoke: cell visual matrix", "cell.command"),
    point!("cell.diff.visual", "cell_visual", "Diff cell 80/120/160 视觉契约", "RenderCellKind::DIFF", "verify-tui-smoke: cell visual matrix", "cell.diff"),
    point!("cell.message.visual", "cell_visual", "Message cell 80/120/160 视觉契约", "RenderCellKind::MESSAGE", "verify-tui-smoke: cell visual matrix", "cell.message"),
    point!("cell.exec.visual", "cell_visual", "Exec cell 80/120/160 视觉契约", "RenderCellKind::EXEC", "verify-tui-smoke: cell visual matrix", "cell.exec"),
    point!("cell.history.visual", "cell_visual", "History cell 80/120/160 视觉契约", "RenderCellKind::HISTORY", "verify-tui-smoke: cell visual matrix", "cell.history"),
    point!("cell.hook.visual", "cell_visual", "Hook cell 80/120/160 视觉契约", "RenderCellKind::HOOK", "verify-tui-smoke: cell visual matrix", "cell.hook"),
    point!("cell.mcp.visual", "cell_visual", "Mcp cell 80/120/160 视觉契约", "RenderCellKind::MCP", "verify-tui-smoke: cell visual matrix", "cell.mcp"),
    point!("cell.plan.visual", "cell_visual", "Plan cell 80/120/160 视觉契约", "RenderCellKind::PLAN", "verify-tui-smoke: cell visual matrix", "cell.plan"),
    point!("cell.review.visual", "cell_visual", "Review cell 80/120/160 视觉契约", "RenderCellKind::REVIEW", "verify-tui-smoke: cell visual matrix", "cell.review"),
    point!("cell.status.visual", "cell_visual", "Status cell 80/120/160 视觉契约", "RenderCellKind::STATUS", "verify-tui-smoke: cell visual matrix", "cell.status"),
    point!("cell.generation_status.visual", "cell_visual", "Generation status cell 80/120/160 视觉契约", "RenderCellKind::GENERATION_STATUS", "verify-tui-smoke: cell visual matrix", "cell.generation_status"),
    point!("cell.reasoning.visual", "cell_visual", "Reasoning cell 80/120/160 视觉契约", "RenderCellKind::REASONING", "verify-tui-smoke: cell visual matrix", "cell.reasoning"),
    point!("cell.usage.visual", "cell_visual", "Usage cell 80/120/160 视觉契约", "RenderCellKind::USAGE", "verify-tui-smoke: cell visual matrix", "cell.usage"),
    point!("cell.error.visual", "cell_visual", "Error cell 80/120/160 视觉契约", "RenderCellKind::ERROR", "verify-tui-smoke: cell visual matrix", "cell.error"),
    point!("terminal.windows_terminal", "terminal", "Windows Terminal fixture", "TerminalCompatibilityPolicy", "verify-tui-smoke: terminal fixture", "system.drag_select_text"),
    point!("terminal.tmux", "terminal", "tmux fixture", "TerminalCompatibilityPolicy", "verify-tui-smoke: terminal fixture", "system.drag_select_text"),
    point!("terminal.screen", "terminal", "screen fixture", "TerminalCompatibilityPolicy", "verify-tui-smoke: terminal fixture", "system.drag_select_text"),
    point!("terminal.ssh_slow_link", "terminal", "ssh slow link fixture", "TerminalCompatibilityPolicy", "verify-tui-smoke: terminal fixture", "system.drag_select_text"),
    point!("unicode.cjk", "unicode_width", "中文显示宽度", "unicode_width_contract_tests", "verify-tui-smoke: unicode width", "cell.message"),
    point!("unicode.fullwidth_punctuation", "unicode_width", "全角标点显示宽度", "unicode_width_contract_tests", "verify-tui-smoke: unicode width", "cell.message"),
    point!("unicode.emoji", "unicode_width", "Emoji 显示宽度", "unicode_width_contract_tests", "verify-tui-smoke: unicode width", "cell.message"),
    point!("unicode.zwj", "unicode_width", "ZWJ emoji 显示宽度", "unicode_width_contract_tests", "verify-tui-smoke: unicode width", "cell.message"),
    point!("unicode.variation_selector", "unicode_width", "variation selector 显示宽度", "unicode_width_contract_tests", "verify-tui-smoke: unicode width", "cell.message"),
    point!("unicode.skin_tone", "unicode_width", "skin tone emoji 显示宽度", "unicode_width_contract_tests", "verify-tui-smoke: unicode width", "cell.message"),
    point!("unicode.flag", "unicode_width", "flag emoji 显示宽度", "unicode_width_contract_tests", "verify-tui-smoke: unicode width", "cell.message"),
    point!("output.10mb", "output_backpressure", "10MB tool output 首帧预算", "WireEventKind::ToolResult output policy", "verify-tui-smoke: output backpressure", "runtime.wire.tool_result"),
    point!("output.100mb", "output_backpressure", "100MB streaming log 非线性增长", "WireEventKind::ToolResult bounded stream", "verify-tui-smoke: output backpressure", "runtime.wire.tool_result"),
    point!("output.truncation_notice", "output_backpressure", "输出截断提示", "ToolOutputViewerDock truncation notice", "verify-tui-smoke: output backpressure", "dock.tool_output_viewer"),
    point!("output.paged_artifact", "output_backpressure", "分页加载和 artifact 入口", "ToolOutputViewerDock paged artifact", "verify-tui-smoke: output backpressure", "dock.tool_output_viewer"),
    point!("runtime.schema_mismatch", "runtime_contract", "adapter schema mismatch", "RuntimeAdapterContract::validate", "verify-tui-smoke: runtime contract"),
    point!("runtime.capabilities", "runtime_contract", "adapter capabilities", "RuntimeAdapterContract capabilities", "verify-tui-smoke: runtime contract"),
    point!("runtime.steer_unsupported", "runtime_contract", "steer unsupported", "AgentTuiSteerResult::Unsupported", "verify-tui-smoke: runtime contract", "input.queued_working"),
    point!("runtime.steer_rejected", "runtime_contract", "steer rejected", "AgentTuiSteerResult::Rejected", "verify-tui-smoke: runtime contract", "queue.steer_now"),
    point!("runtime.bounded_channel", "runtime_contract", "bounded channel", "RuntimeSubmissionDriver backpressure", "verify-tui-smoke: runtime contract"),
    point!("runtime.worker_panic", "runtime_contract", "worker panic 转 error cell", "RuntimeSubmissionDriver panic guard", "verify-tui-smoke: runtime contract", "cell.error"),
    point!("error.provider_401", "error_presentation", "Provider 401 error cell", "TuiError projection", "verify-tui-smoke: error presentation", "cell.error"),
    point!("error.provider_429", "error_presentation", "Provider 429 error cell", "TuiError projection", "verify-tui-smoke: error presentation", "cell.error"),
    point!("error.provider_offline", "error_presentation", "Provider offline error cell", "TuiError projection", "verify-tui-smoke: error presentation", "cell.error"),
    point!("error.mcp_crash", "error_presentation", "MCP crash error cell", "MCP runtime error projection", "verify-tui-smoke: error presentation", "cell.mcp"),
    point!("error.snapshot_corrupt", "error_presentation", "snapshot corrupt error cell", "WireRuntimeError", "verify-tui-smoke: error presentation", "system.wire_runtime_error"),
    point!("error.snapshot_permission", "error_presentation", "snapshot permission error cell", "WireRuntimeError", "verify-tui-smoke: error presentation", "system.wire_runtime_error"),
    point!("error.cell_payload_missing_field", "error_presentation", "cell payload 缺字段 error cell", "CellPayload validation", "verify-tui-smoke: error presentation", "cell.error"),
    point!("invariant.header_viewport_composer", "sub_state_invariant", "三区 sub-state 不变量", "state dump invariant", "verify-tui-smoke: sub-state invariant"),
    point!("invariant.cell_selection_bounds", "sub_state_invariant", "cell selection bounds", "state dump invariant", "verify-tui-smoke: sub-state invariant"),
    point!("invariant.dock_single_active", "sub_state_invariant", "dock single active", "state dump invariant", "verify-tui-smoke: sub-state invariant", "dock.shortcut_help"),
    point!("invariant.runtime_correlation", "sub_state_invariant", "runtime correlation invariant", "runtime event envelope", "verify-tui-smoke: sub-state invariant"),
    point!("invariant.cache_key_boundary", "sub_state_invariant", "cell cache key boundary", "CellCacheKey contract", "verify-tui-smoke: sub-state invariant"),
    point!("frame.thousand_schedule", "frame", "1000 次 schedule 合并", "FrameScheduler coalescing", "verify-tui-smoke: frame idempotency"),
    point!("frame.no_duplicate_stable", "frame", "稳定内容不重复渲染", "FrameRenderSignature", "verify-tui-smoke: frame idempotency"),
    point!("frame.latency_metric", "frame", "frame latency 指标", "TuiTelemetrySnapshot", "verify-tui-smoke: frame idempotency"),
    point!("editing.file_completion", "editing_mouse", "@file/@image 路径补全", "ComposerPopup completion", "verify-tui-smoke: editing and mouse", "system.file_completion"),
    point!("editing.drag_select", "editing_mouse", "拖拽选择可见文本并释放复制", "UiAction::VisibleTextSelectionDrag", "verify-tui-smoke: editing and mouse", "system.drag_select_text"),
    point!("prompt_cache.dock", "prompt_cache", "dock 状态不改变 prompt cache anchor", "prompt_cache_boundary_test", "verify-tui-smoke: prompt cache boundary"),
    point!("prompt_cache.scroll", "prompt_cache", "scroll 状态不改变 prompt cache anchor", "prompt_cache_boundary_test", "verify-tui-smoke: prompt cache boundary"),
    point!("prompt_cache.selection", "prompt_cache", "selection 状态不改变 prompt cache anchor", "prompt_cache_boundary_test", "verify-tui-smoke: prompt cache boundary"),
    point!("prompt_cache.animation_tick", "prompt_cache", "animation tick 不改变 prompt cache anchor", "prompt_cache_boundary_test", "verify-tui-smoke: prompt cache boundary"),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::ability_matrix::agent_tui_ability_entries;
    use super::*;

    #[test]
    fn smoke_contract_covers_required_target_state_points() {
        let snapshot = agent_tui_smoke_contract_snapshot();
        assert!(snapshot.total_points >= MIN_SMOKE_POINTS);
        assert_eq!(snapshot.points.len(), snapshot.total_points);
        assert_eq!(snapshot.required_points, REQUIRED_SMOKE_POINT_IDS.len());

        let mut ids = HashSet::new();
        for point in &snapshot.points {
            assert!(ids.insert(point.id), "重复 smoke point：{}", point.id);
            assert!(
                !point.category.trim().is_empty(),
                "{} 缺少 category",
                point.id
            );
            assert!(
                !point.target_entry.trim().is_empty(),
                "{} 缺少 target_entry",
                point.id
            );
            assert!(
                !point.evidence.trim().is_empty(),
                "{} 缺少 evidence",
                point.id
            );
            assert!(
                !point.verification.trim().is_empty(),
                "{} 缺少 verification",
                point.id
            );
        }

        for required_id in required_smoke_point_ids() {
            assert!(
                ids.contains(required_id),
                "缺少必备 smoke point：{required_id}"
            );
        }
    }

    #[test]
    fn smoke_contract_links_to_existing_ability_ids() {
        let ability_ids = agent_tui_ability_entries()
            .iter()
            .map(|ability| ability.id)
            .collect::<HashSet<_>>();

        for point in smoke_points() {
            if let Some(ability_id) = point.ability_id {
                assert!(
                    ability_ids.contains(ability_id),
                    "{} 指向不存在的 ability_id：{}",
                    point.id,
                    ability_id
                );
            }
        }
    }
}
