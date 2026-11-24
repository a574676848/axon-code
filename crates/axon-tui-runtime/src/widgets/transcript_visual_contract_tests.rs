//! Transcript 视觉契约测试。

use std::{collections::BTreeMap, path::PathBuf};

use axon_domain::{UserQuestionItem, UserQuestionOption};
use axon_protocol::diff::FileChange;
use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

use crate::{
    app_shell::style_tokens::{
        ACCENT_MINT, TEXT_MUTED, TEXT_SECONDARY, TerminalPaletteDepth, TranscriptColorRole,
        transcript_palette_for_depth,
    },
    r#loop::agent_tui::{AgentTuiLifecyclePhase, AgentTuiTokenUsage},
    surface::{
        animation::{AXON_ACTIVITY_LABEL, TranscriptMotionTokens, axon_logo_breath_highlight},
        cell::{
            RenderCellKind, RenderCellRecord, RenderCommandCell, RenderGenerationStatusCell,
            RenderLifecycleItem, RenderReasoningCell, RenderStatusCell, RenderStatusItem,
        },
        cell_diff_data::AgentRenderDiffCell,
        cell_event_data::{
            AgentRenderErrorCell, AgentRenderExecCell, AgentRenderHistoryCell,
            AgentRenderHistoryItem, AgentRenderHookCell, AgentRenderHookItem,
            AgentRenderLifecycleStatus, AgentRenderMcpCell, AgentRenderMcpCellSource,
            AgentRenderMcpItem, AgentRenderUsageCell,
        },
        cell_plan_data::{
            AgentRenderPlanCell, AgentRenderPlanSourceKind, AgentRenderPlanStatus,
            AgentRenderPlanStep,
        },
        cell_question_data::AgentRenderQuestionCell,
        cell_review_data::{
            AgentRenderReviewCell, AgentRenderReviewDetail, AgentRenderReviewDetailKind,
            AgentRenderReviewStatus,
        },
        cell_turn_summary_data::AgentRenderTurnSummaryCell,
        error_model::TuiError,
        visual_density::{
            TRANSCRIPT_CELL_LAYOUT_SPECS, TranscriptWrapMode, VISUAL_NOISE_BUDGET,
            cell_layout_spec, cell_spacing_rows,
        },
    },
    widgets::{
        TranscriptRenderGateway, terminal_hyperlinks::visible_lines,
        transcript_style::policy_cell_visual,
    },
};

const CONTRACT_WIDTHS: [u16; 3] = [80, 120, 160];

#[test]
fn cell_specs_are_complete_and_width_stable() {
    assert_eq!(TRANSCRIPT_CELL_LAYOUT_SPECS.len(), 16);
    for line in representative_lines() {
        let RenderCellKind::MESSAGE = kind_of(&line) else {
            validate_layout_spec(kind_of(&line));
            continue;
        };
        validate_layout_spec(RenderCellKind::MESSAGE);
    }
}

#[test]
fn representative_cells_render_without_visible_overflow() {
    for line in representative_lines() {
        for width in CONTRACT_WIDTHS {
            let lines = TranscriptRenderGateway::render_cell_hyperlink_lines(&line, true, width);
            assert!(!lines.is_empty(), "{:?} 应产生可见渲染行", kind_of(&line));
            for rendered in visible_lines(&lines) {
                let visible = rendered
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                assert!(
                    UnicodeWidthStr::width(visible.as_str()) <= usize::from(width),
                    "{:?} 在 {width} 列下溢出：{visible}",
                    kind_of(&line)
                );
            }
        }
    }
}

#[test]
fn semantic_roles_have_readable_palette_fallbacks() {
    let roles = [
        TranscriptColorRole::User,
        TranscriptColorRole::Assistant,
        TranscriptColorRole::ToolRunning,
        TranscriptColorRole::ToolCompleted,
        TranscriptColorRole::ToolFailed,
        TranscriptColorRole::DiffAdded,
        TranscriptColorRole::DiffRemoved,
        TranscriptColorRole::DiffHunk,
        TranscriptColorRole::Error,
        TranscriptColorRole::Interrupted,
        TranscriptColorRole::Accent,
        TranscriptColorRole::Muted,
        TranscriptColorRole::Accent,
        TranscriptColorRole::Loading,
        TranscriptColorRole::ToolInput,
        TranscriptColorRole::SubagentActive,
        TranscriptColorRole::InlineHighlight,
        TranscriptColorRole::McpIdentifier,
    ];
    for depth in [
        TerminalPaletteDepth::NoColor,
        TerminalPaletteDepth::Ansi16,
        TerminalPaletteDepth::Ansi256,
        TerminalPaletteDepth::TrueColor,
    ] {
        let palette = transcript_palette_for_depth(depth);
        for role in roles {
            if matches!(depth, TerminalPaletteDepth::NoColor)
                || (role == TranscriptColorRole::Assistant
                    && !matches!(depth, TerminalPaletteDepth::TrueColor))
            {
                continue;
            }
            assert_ne!(palette.color(role), Color::Reset);
        }
    }
}

#[test]
fn codex_palette_uses_core_ansi_semantic_colors() {
    let palette = transcript_palette_for_depth(TerminalPaletteDepth::Ansi16);

    assert_eq!(palette.color(TranscriptColorRole::User), Color::LightGreen);
    assert_eq!(
        palette.color(TranscriptColorRole::ToolRunning),
        Color::LightGreen
    );
    assert_eq!(
        palette.color(TranscriptColorRole::Accent),
        Color::LightGreen
    );
    assert_eq!(
        palette.color(TranscriptColorRole::Loading),
        Color::LightGreen
    );
    assert_eq!(
        palette.color(TranscriptColorRole::ToolCompleted),
        Color::Green
    );
    assert_eq!(palette.color(TranscriptColorRole::DiffAdded), Color::Green);
    assert_eq!(palette.color(TranscriptColorRole::ToolFailed), Color::Red);
    assert_eq!(
        palette.color(TranscriptColorRole::Interrupted),
        Color::Yellow
    );
    assert_eq!(palette.color(TranscriptColorRole::DiffRemoved), Color::Red);
    assert_eq!(
        palette.color(TranscriptColorRole::Accent),
        Color::LightGreen
    );
    assert_eq!(
        palette.color(TranscriptColorRole::DiffHunk),
        Color::DarkGray
    );
    assert_eq!(palette.color(TranscriptColorRole::ToolInput), Color::Gray);
    assert_eq!(
        palette.color(TranscriptColorRole::SubagentActive),
        Color::Magenta
    );
    assert_eq!(
        palette.color(TranscriptColorRole::InlineHighlight),
        Color::LightGreen
    );
    assert_eq!(
        palette.color(TranscriptColorRole::McpIdentifier),
        Color::LightGreen
    );
    assert_eq!(palette.color(TranscriptColorRole::Assistant), Color::Gray);
}

#[test]
fn true_color_palette_uses_focused_six_color_hierarchy() {
    let palette = transcript_palette_for_depth(TerminalPaletteDepth::TrueColor);

    assert_eq!(
        palette.color(TranscriptColorRole::ToolInput),
        TEXT_SECONDARY
    );
    assert_eq!(
        palette.color(TranscriptColorRole::SubagentActive),
        Color::Magenta
    );
    assert_eq!(
        palette.color(TranscriptColorRole::InlineHighlight),
        ACCENT_MINT
    );
    assert_eq!(
        palette.color(TranscriptColorRole::McpIdentifier),
        ACCENT_MINT
    );
    assert_eq!(palette.color(TranscriptColorRole::DiffHunk), TEXT_MUTED);
}

#[test]
fn interrupt_palette_uses_requested_orange_with_terminal_fallbacks() {
    assert_eq!(
        transcript_palette_for_depth(TerminalPaletteDepth::TrueColor)
            .color(TranscriptColorRole::Interrupted),
        Color::Rgb(217, 133, 94)
    );
    assert_eq!(
        transcript_palette_for_depth(TerminalPaletteDepth::Ansi256)
            .color(TranscriptColorRole::Interrupted),
        Color::Indexed(173)
    );
    assert_eq!(
        transcript_palette_for_depth(TerminalPaletteDepth::Ansi16)
            .color(TranscriptColorRole::Interrupted),
        Color::Yellow
    );
    assert_eq!(
        transcript_palette_for_depth(TerminalPaletteDepth::NoColor)
            .color(TranscriptColorRole::Interrupted),
        Color::Reset
    );
}

#[test]
fn no_color_palette_uses_only_uncolored_foreground_roles() {
    let palette = transcript_palette_for_depth(TerminalPaletteDepth::NoColor);
    for role in [
        TranscriptColorRole::User,
        TranscriptColorRole::Assistant,
        TranscriptColorRole::ToolRunning,
        TranscriptColorRole::ToolCompleted,
        TranscriptColorRole::ToolFailed,
        TranscriptColorRole::DiffAdded,
        TranscriptColorRole::DiffRemoved,
        TranscriptColorRole::DiffHunk,
        TranscriptColorRole::Error,
        TranscriptColorRole::Accent,
        TranscriptColorRole::Notice,
        TranscriptColorRole::Warning,
        TranscriptColorRole::Muted,
        TranscriptColorRole::Accent,
        TranscriptColorRole::Loading,
        TranscriptColorRole::ToolInput,
        TranscriptColorRole::SubagentActive,
        TranscriptColorRole::InlineHighlight,
        TranscriptColorRole::McpIdentifier,
    ] {
        assert!(
            matches!(palette.color(role), Color::Reset | Color::Gray),
            "no-color palette 不应为 {role:?} 选择彩色 foreground"
        );
    }
}

#[test]
fn cell_visuals_resolve_through_render_policy_color_roles() {
    for line in representative_lines() {
        let decision = crate::surface::render_policy::RenderPolicy::decide(&line);
        let visual = policy_cell_visual(decision);
        assert!(visual.title_style.fg.is_some());
        assert!(visual.action_text_style.fg.is_some());
        assert!(visual.detail_text_style.fg.is_some());
        assert_eq!(visual.detail_prefix_style.fg, visual.detail_prefix_style.fg);
    }
}

#[test]
fn animation_cadence_is_declared_in_one_place() {
    assert_eq!(AXON_ACTIVITY_LABEL, "⚡ Axon");
    assert!(axon_logo_breath_highlight(0));
    assert!(!axon_logo_breath_highlight(2));
    assert!(TranscriptMotionTokens::FULL.stays_within_tui_budget());
}

#[test]
fn motion_tokens_define_full_and_reduced_animation_budget() {
    assert!(TranscriptMotionTokens::FULL.stays_within_tui_budget());
    assert!(TranscriptMotionTokens::REDUCED.stays_within_tui_budget());
    assert!(TranscriptMotionTokens::FULL.dock > TranscriptMotionTokens::REDUCED.dock);
    assert!(
        TranscriptMotionTokens::FULL.cell_mutation > TranscriptMotionTokens::REDUCED.cell_mutation
    );
    assert!(TranscriptMotionTokens::FULL.jump_bottom > TranscriptMotionTokens::REDUCED.jump_bottom);
    assert!(TranscriptMotionTokens::REDUCED.dock.is_zero());
    assert!(TranscriptMotionTokens::REDUCED.cell_mutation.is_zero());
    assert!(TranscriptMotionTokens::REDUCED.jump_bottom.is_zero());
}

#[test]
fn density_policy_keeps_transcript_quiet_and_scannable() {
    let tool = RenderCellRecord::mcp_with_data(AgentRenderMcpCell {
        id: "mcp:test:noise-budget".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无调用".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: Vec::new(),
    });
    let exec = RenderCellRecord::exec_with_data(
        AgentRenderExecCell::running_with_id("exec:test:tool-1", "shell", "tool-1", None),
        crate::surface::cell::RenderCellAction::Running,
    );
    let answer = RenderCellRecord::agent_message("assistant:test:noise-budget", "工具完成后的回答");
    let user = RenderCellRecord::user_with_data("继续");

    assert_eq!(cell_spacing_rows(Some(&tool), &exec), 1);
    assert_eq!(cell_spacing_rows(Some(&exec), &answer), 1);
    assert_eq!(cell_spacing_rows(Some(&answer), &user), 2);
    assert_eq!(VISUAL_NOISE_BUDGET.turn_separator_rows, 2);
}

#[test]
fn density_policy_separates_distinct_non_tool_cell_types() {
    let answer = RenderCellRecord::agent_message("assistant:test:distinct-spacing", "完成");
    let usage = usage_line();

    assert_eq!(cell_spacing_rows(Some(&answer), &usage), 1);
}

#[test]
fn wrap_modes_are_assigned_by_content_shape() {
    assert_eq!(
        cell_layout_spec(RenderCellKind::DIFF).wrap_mode,
        TranscriptWrapMode::Diff
    );
    assert_eq!(
        cell_layout_spec(RenderCellKind::MESSAGE).wrap_mode,
        TranscriptWrapMode::PreserveUrl
    );
    assert_eq!(
        cell_layout_spec(RenderCellKind::EXEC).wrap_mode,
        TranscriptWrapMode::PreservePreformatted
    );
}

fn validate_layout_spec(kind: RenderCellKind) {
    let spec = cell_layout_spec(kind);
    assert_eq!(spec.kind, kind);
    assert!(spec.summary_height >= 1);
    assert!(
        spec.expanded_height
            .is_none_or(|height| height >= spec.summary_height)
    );
    for width in CONTRACT_WIDTHS {
        assert!(UnicodeWidthStr::width(spec.prefix) < usize::from(width));
        assert!(UnicodeWidthStr::width(spec.continuation) < usize::from(width));
        assert!(UnicodeWidthStr::width(spec.detail_prefix) < usize::from(width));
    }
}

fn kind_of(line: &RenderCellRecord) -> RenderCellKind {
    let crate::surface::cell::RenderCellMode::Cell(kind) = line.mode else {
        panic!("视觉契约只接收结构化 cell")
    };
    kind
}

fn representative_lines() -> Vec<RenderCellRecord> {
    vec![
        command_line(),
        RenderCellRecord::file_change_with_data(diff_cell()),
        message_line(),
        exec_line(),
        history_line(),
        hook_line(),
        mcp_line(),
        plan_line(),
        review_line(),
        status_line(),
        generation_status_line(),
        reasoning_line(),
        turn_summary_line(),
        usage_line(),
        error_line(),
        question_line(),
    ]
}

fn command_line() -> RenderCellRecord {
    RenderCellRecord::command_with_data(RenderCommandCell::new(
        "test:visual:command",
        "命令",
        vec![status_item(
            "model",
            "gpt-5 中文宽度",
            AgentTuiLifecyclePhase::Command,
        )],
    ))
}

fn message_line() -> RenderCellRecord {
    RenderCellRecord::agent_message(
        "assistant:test:visual",
        "这是包含中文、URL https://example.com/a/very/long/path 和摘要的回复。",
    )
}

fn exec_line() -> RenderCellRecord {
    let mut cell = AgentRenderExecCell::running_with_id(
        "exec:test:visual",
        "builtin.shell_command",
        "tool-1",
        Some("cargo check -p axon-tui-runtime --lib".to_owned()),
    );
    cell.output_preview = Some("正在检查 crates/axon-tui-runtime/src/widgets".to_owned());
    cell.duration_ms = Some(1_250);
    RenderCellRecord::exec_with_data(cell, crate::surface::cell::RenderCellAction::Running)
}

fn history_line() -> RenderCellRecord {
    RenderCellRecord::history_with_data(AgentRenderHistoryCell {
        id: "history:test:visual".to_owned(),
        title: "历史".to_owned(),
        empty_text: "暂无历史".to_owned(),
        items: vec![AgentRenderHistoryItem {
            name: "session-中文路径".to_owned(),
            kind_label: Some("会话".to_owned()),
            status_label: Some("进行中".to_owned()),
        }],
    })
}

fn hook_line() -> RenderCellRecord {
    RenderCellRecord::hook_with_data(AgentRenderHookCell {
        id: "hook:test:visual".to_owned(),
        title: "Hook".to_owned(),
        items: vec![AgentRenderHookItem {
            run_id: "hook-1".to_owned(),
            name: "quality_gate".to_owned(),
            status: AgentRenderLifecycleStatus::Running,
            indicator: Some(AXON_ACTIVITY_LABEL.to_owned()),
            status_message: Some("格式化检查中".to_owned()),
            entries: Vec::new(),
        }],
    })
}

fn mcp_line() -> RenderCellRecord {
    RenderCellRecord::mcp_with_data(AgentRenderMcpCell {
        id: "mcp:test:visual".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无调用".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![{
            let mut item = AgentRenderMcpItem::catalog(
                "gitnexus",
                Some("query".to_owned()),
                AgentRenderLifecycleStatus::Running,
                None,
            );
            item.provider_call_id = Some("mcp-1".to_owned());
            item.arguments_preview = Some("文本渲染规范".to_owned());
            item.output_preview = Some("命中 transcript_style".to_owned());
            item
        }],
    })
}

fn plan_line() -> RenderCellRecord {
    RenderCellRecord::plan_with_data(AgentRenderPlanCell {
        id: "test:visual:plan".to_owned(),
        title: "计划".to_owned(),
        explanation: None,
        steps: vec![AgentRenderPlanStep {
            id: None,
            status: AgentRenderPlanStatus::Running,
            summary: "检查状态".to_owned(),
        }],
        source: "# 计划\n\n- [~] 检查状态".to_owned(),
        source_kind: AgentRenderPlanSourceKind::Stream,
    })
}

fn review_line() -> RenderCellRecord {
    RenderCellRecord::review_with_data(AgentRenderReviewCell {
        id: "test:visual:review".to_owned(),
        subject: "验证".to_owned(),
        status: Some(AgentRenderReviewStatus::Running),
        details: vec![AgentRenderReviewDetail {
            kind: AgentRenderReviewDetailKind::Message,
            text: "等待结果".to_owned(),
        }],
    })
}

fn status_line() -> RenderCellRecord {
    RenderCellRecord::status_with_data(RenderStatusCell {
        id: "test:visual:status".to_owned(),
        title: "状态".to_owned(),
        items: vec![status_item(
            "runtime",
            "流式输出中",
            AgentTuiLifecyclePhase::Notice,
        )],
    })
}

fn generation_status_line() -> RenderCellRecord {
    RenderCellRecord::generation_status_with_data(RenderGenerationStatusCell {
        id: "test:generation:visual-contract".to_owned(),
        title: "生成".to_owned(),
        items: vec![RenderLifecycleItem {
            id: "test:generation:visual-contract:item".to_owned(),
            kind: "StreamingStarted".to_owned(),
            summary: "等待首字输出".to_owned(),
            phase: AgentTuiLifecyclePhase::Generation,
            indicator: None,
            tool_name: None,
            duration_ms: None,
            attempt_index: None,
        }],
    })
}

fn reasoning_line() -> RenderCellRecord {
    RenderCellRecord::reasoning_with_data(RenderReasoningCell {
        id: "reason-1".to_owned(),
        text: "分析输入并选择执行路径".to_owned(),
        done: true,
        elapsed_ms: Some(1_250),
    })
}

fn usage_line() -> RenderCellRecord {
    RenderCellRecord::usage_with_data(AgentRenderUsageCell {
        id: "usage:test:visual".to_owned(),
        title: "Token".to_owned(),
        total_tokens: 1234,
        total: "总计 1.23K".to_owned(),
        input_tokens: 900,
        input: "输入 900".to_owned(),
        cache_read_input_tokens: 500,
        cache_read: "缓存读取 500".to_owned(),
        cache_write_input_tokens: 0,
        cache_write: "缓存写入 0".to_owned(),
        output_tokens: 334,
        output: "输出 334".to_owned(),
        reasoning_output_tokens: 0,
        reasoning: "推理 0".to_owned(),
    })
}

fn turn_summary_line() -> RenderCellRecord {
    RenderCellRecord::turn_summary_with_data(AgentRenderTurnSummaryCell::new(
        "turn-summary:test:visual",
        Some(65_000),
        Some(AgentTuiTokenUsage::new(5_113, 9_700, 300, 104, 38)),
    ))
}

fn error_line() -> RenderCellRecord {
    let error = TuiError::runtime("cargo check 失败");
    RenderCellRecord::error_with_data(AgentRenderErrorCell {
        id: "error:test:visual".to_owned(),
        title: "验证失败".to_owned(),
        message: "cargo check 失败".to_owned(),
        details: vec!["src/widgets/transcript.rs: 行宽超过预算".to_owned()],
        next_step: Some("修正后重跑 focused check".to_owned()),
        severity: error.severity,
        action: error.action,
        cell_kind: error.cell_kind,
    })
}

fn question_line() -> RenderCellRecord {
    RenderCellRecord::question_with_data(AgentRenderQuestionCell::new(
        "question:test:visual",
        "call-question-visual",
        "pending",
        vec![UserQuestionItem {
            id: "q1".to_owned(),
            question: "是否继续部署到生产环境？".to_owned(),
            header: Some("部署确认".to_owned()),
            detail: None,
            options: vec![
                UserQuestionOption {
                    label: "继续部署".to_owned(),
                    description: Some("直接发布当前版本".to_owned()),
                },
                UserQuestionOption {
                    label: "暂停观察".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        Vec::new(),
        None,
        1_700_000_000_000,
    ))
}

fn status_item(kind: &str, summary: &str, phase: AgentTuiLifecyclePhase) -> RenderStatusItem {
    RenderStatusItem {
        kind: kind.to_owned(),
        summary: summary.to_owned(),
        phase: Some(phase),
        indicator: None,
        tool_name: None,
        duration_ms: None,
        turn_id: None,
        attempt_index: None,
    }
}

fn diff_cell() -> AgentRenderDiffCell {
    let mut changes = BTreeMap::new();
    changes.insert(
        PathBuf::from("crates/axon-tui-runtime/src/widgets/transcript_style.rs"),
        FileChange::Update {
            unified_diff: "@@ -1,2 +1,2 @@\n-old line\n+new line".to_owned(),
            move_path: None,
        },
    );
    AgentRenderDiffCell {
        id: "diff:test:visual-contract".to_owned(),
        title: "文件修改".to_owned(),
        changes,
        diagnostics: BTreeMap::new(),
        text_writes_applied: 1,
        renames_applied: 0,
        truncated: false,
        truncation_boundary: None,
    }
}
