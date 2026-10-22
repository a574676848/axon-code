//! TUI 渲染逻辑 - AppShell 三段布局。
//!
//! 三段式：
//!   ┌─────────────────────────────────────┐
//!   │   HeaderBar（会话 / 模型 / 状态）   │
//!   ├─────────────────────────────────────┤
//!   │   RenderViewport（流式响应 + 工具） │
//!   │                                     │
//!   ├─────────────────────────────────────┤
//!   │   ComposerDock（输入区）            │
//!   └─────────────────────────────────────┘
//! HeaderBar 吸顶，ComposerDock 吸底，RenderViewport 独立填充中间区域。

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

#[cfg(test)]
pub(super) use super::render_transcript::refresh_main_viewport_bounds;
pub(super) use super::render_transcript::render_active_transcript;
use super::{
    custom_terminal::Frame,
    render_toast::render_toast,
    state::composer_prompt_width,
    state::{AgentRuntimeStatus, AgentTuiState},
};
use crate::app_shell::{
    composer_dock::{
        COMPOSER_DOCK_SEPARATOR_HEIGHT, ComposerDockDraft, ComposerDockDraftElement,
        ComposerDockDraftElementKind, ComposerDockPanel, ComposerDockViewModel, WorkingStatusTone,
        WorkingStatusViewModel,
    },
    frame_view_model::AppShellFrameViewModel,
    header_bar::{HeaderBarMetrics, HeaderBarStatus, HeaderViewModel},
    layout::layout_app_shell,
    style_tokens::TEXT_MUTED,
};
use crate::r#loop::agent_tui::provider_activity::ProviderActivityTone;
use crate::r#loop::agent_tui::runtime_identity::{effective_model_identity, provider_model_route};
use crate::widgets::{ComposerDock, DockRenderGateway, HeaderBar};

const SECTION_DIVIDER_STYLE: Style = Style::new().fg(TEXT_MUTED);

/// TUI 主渲染函数。
pub(super) fn render_tui(frame: &mut Frame, state: &mut AgentTuiState) {
    render_tui_with_transcript(frame, state, true);
}

/// 绘制 TUI，并在 transcript 状态未变化时复用上一帧正文。
pub(super) fn render_tui_with_transcript(
    frame: &mut Frame,
    state: &mut AgentTuiState,
    render_transcript: bool,
) {
    let area = frame.area();
    let header = header_bar(state);
    let header_height = header.required_height(area.width);
    let layout = layout_app_shell(area, state.composer_height(), header_height);

    frame.render_widget(header, layout.header);
    render_section_divider(frame, layout.header.bottom(), layout.viewport.width);
    if render_transcript {
        render_active_transcript(frame, state, layout.viewport);
    }
    let composer_area = render_bottom_composer(frame, state, layout.composer);

    render_visible_text_selection(frame, state);
    render_toast(frame, state, layout.composer);
    render_cursor(frame, state, composer_area);
}

pub(super) fn header_bar_height(state: &AgentTuiState, width: u16) -> u16 {
    header_bar(state).required_height(width)
}

fn header_bar(state: &AgentTuiState) -> HeaderBar<'_> {
    let view_model = app_shell_frame_view_model(state);
    HeaderBar {
        view_model: HeaderViewModel {
            config_status: view_model.header.config_status,
            overrides: view_model.header.overrides,
            metrics: view_model.header.metrics.clone(),
            models: view_model.header.models,
        },
    }
}

fn render_section_divider(frame: &mut Frame, y: u16, width: u16) {
    if width == 0 {
        return;
    }
    let divider = "─".repeat(width as usize);
    frame
        .buffer_mut()
        .set_string(0, y, divider, SECTION_DIVIDER_STYLE);
}

fn render_bottom_composer(frame: &mut Frame, state: &mut AgentTuiState, area: Rect) -> Rect {
    let view_model = composer_dock_view_model(state);
    let composer_area = DockRenderGateway::partition(&view_model, area).input();
    frame.render_widget(ComposerDock { view_model }, area);
    composer_area
}

fn render_visible_text_selection(frame: &mut Frame, state: &AgentTuiState) {
    let Some(range) = state.visible_text_selection.active_range() else {
        return;
    };
    let area = frame.area();
    if area.is_empty() {
        return;
    }
    let highlight = Style::new().add_modifier(Modifier::REVERSED);
    let start_row = range.start_row.max(area.top());
    let end_row = range.end_row.min(area.bottom().saturating_sub(1));
    if start_row > end_row {
        return;
    }
    for row in start_row..=end_row {
        let start_column = if row == range.start_row {
            range.start_column
        } else {
            area.left()
        };
        let end_column = if row == range.end_row {
            range.end_column
        } else {
            area.right().saturating_sub(1)
        };
        let start_column = start_column.max(area.left());
        let end_column = end_column.min(area.right().saturating_sub(1));
        if start_column > end_column {
            continue;
        }
        for column in start_column..=end_column {
            if let Some(cell) = frame.buffer_mut().cell_mut((column, row)) {
                cell.set_style(highlight);
            }
        }
    }
}

fn render_cursor(frame: &mut Frame, state: &AgentTuiState, composer_area: Rect) {
    let (_cursor_row, cursor_col) = state.cursor_row_col();
    let cursor_x = cursor_x_position(composer_area.x, cursor_col);
    let visible_row = state
        .visible_cursor_row()
        .min(state.visible_draft_height().saturating_sub(1));
    let command_height = state.command_suggestion_panel_height();
    let queued_height = state.queued_submit_preview_height();
    // `composer_area` 是 dock 分区里的输入区，它的 y 已经把上方面板高度与工作状态条
    // 计进去了（见 widgets/dock_render_gateway.rs 的 partition）。这里再补一次面板
    // 高度会让光标整体下移一个面板高度，因此只累加输入区内部的行偏移。
    let cursor_y = composer_area.y
        + COMPOSER_DOCK_SEPARATOR_HEIGHT
        + command_height
        + queued_height
        + visible_row;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn composer_dock_view_model(state: &AgentTuiState) -> ComposerDockViewModel<'_> {
    let queued_preview = state.queued_submit_preview();
    let command_suggestions_open = state.command_suggestions_open();
    let draft_height = state.visible_draft_height();
    let (suggestion_title, suggestions, popup_items, selected_suggestion) =
        if let Some(popup) = state.composer.composer_popup.as_ref() {
            (
                popup.title(),
                [].as_slice(),
                popup.items.as_slice(),
                popup.selected,
            )
        } else if command_suggestions_open {
            (
                "Commands",
                state.composer.command_suggestions.as_slice(),
                [].as_slice(),
                state.composer.selected_command_suggestion,
            )
        } else {
            ("", [].as_slice(), [].as_slice(), 0)
        };
    let draft_elements = state
        .composer
        .draft
        .element_snapshots()
        .into_iter()
        .map(|snapshot| ComposerDockDraftElement {
            range: snapshot.range,
            kind: match snapshot.visual {
                super::draft_state::DraftElementVisualKind::Mention(sigil) => {
                    ComposerDockDraftElementKind::Mention(sigil)
                }
                super::draft_state::DraftElementVisualKind::File => {
                    ComposerDockDraftElementKind::File
                }
                super::draft_state::DraftElementVisualKind::Image => {
                    ComposerDockDraftElementKind::Image
                }
            },
        })
        .collect::<Vec<_>>();
    let panel = state.bottom_view().and_then(|view| {
        let lifecycle = state.bottom_view_lifecycle()?;
        Some(ComposerDockPanel::bottom_view(view, lifecycle))
    });
    ComposerDockViewModel {
        draft: ComposerDockDraft {
            text: state.composer.draft.text(),
            elements: draft_elements,
        },
        command_suggestions: suggestions,
        popup_items,
        suggestion_title,
        selected_command_suggestion: selected_suggestion,
        queued_preview,
        working_status: working_status_view_model(state),
        draft_scroll_offset: state.draft_scroll_offset(draft_height),
        panel,
        subagent_activity: state.subagent_activity_snapshot(std::time::Instant::now()),
    }
}

fn working_status_view_model(state: &AgentTuiState) -> Option<WorkingStatusViewModel> {
    if state.bridge.runtime_status != AgentRuntimeStatus::Working {
        return None;
    }
    let elapsed = state.display_elapsed(std::time::Instant::now());
    let queued = state.has_queued_follow_up_message();
    if let Some(activity) = state.bridge.provider_activity.current() {
        return Some(WorkingStatusViewModel {
            primary: activity.label.clone(),
            prefix: activity.prefix,
            tone: match activity.tone {
                ProviderActivityTone::Active => WorkingStatusTone::Active,
                ProviderActivityTone::Success => WorkingStatusTone::Success,
                ProviderActivityTone::Error => WorkingStatusTone::Error,
            },
            animated: activity.animated,
            elapsed: activity.show_turn_elapsed.then_some(elapsed).flatten(),
            queued,
        });
    }
    let identity = effective_model_identity(
        &state.header.config_status,
        &state.header.overrides,
        &state.header.catalog.models,
    );
    let route = provider_model_route(&identity.provider, &identity.model);
    Some(WorkingStatusViewModel::generating(route, elapsed, queued))
}

/// 计算光标的屏幕 X 位置。
fn cursor_x_position(area_x: u16, cursor_col: u16) -> u16 {
    area_x
        .saturating_add(composer_prompt_width())
        .saturating_add(cursor_col)
}

fn app_shell_frame_view_model(state: &AgentTuiState) -> AppShellFrameViewModel<'_> {
    AppShellFrameViewModel {
        header: HeaderViewModel {
            config_status: &state.header.config_status,
            overrides: &state.header.overrides,
            metrics: header_bar_metrics(state),
            models: state.models(),
        },
    }
}

fn header_bar_metrics(state: &AgentTuiState) -> HeaderBarMetrics {
    let status = derive_session_status(state);
    // 首字与速率按整个会话重算：只看最近一轮会把早期慢轮次抹掉。
    let generation = state.bridge.session_generation.metrics();
    HeaderBarMetrics {
        token_usage: state.last_turn_usage(),
        plan_progress: state.plan_progress(),
        lsp_active_count: state.lsp_active_count(),
        elapsed: (!matches!(status, HeaderBarStatus::Working))
            .then(|| state.display_elapsed(std::time::Instant::now()))
            .flatten(),
        first_token_avg_ms: generation.first_token_avg_ms,
        generation_tps_x10: generation.generation_tps_x10,
        status,
        cwd: state.header.cwd.clone(),
        session_id: state
            .header
            .session_summary
            .session_id
            .clone()
            .or_else(|| state.header.overrides.session.clone()),
        continue_last: state.header.overrides.continue_last,
        replay_snapshot: state.header.replay_snapshot,
        spinner: state.spinner(),
        animation_tick: state.animation_tick(),
    }
}

fn derive_session_status(state: &AgentTuiState) -> HeaderBarStatus {
    if state.bridge.runtime_status == AgentRuntimeStatus::Working {
        HeaderBarStatus::Working
    } else {
        HeaderBarStatus::Ready
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, layout::Rect};

    use crate::app_shell::layout::{HEADER_BAR_HEIGHT, SECTION_DIVIDER_HEIGHT, layout_app_shell};
    use crate::r#loop::agent_tui::state::viewport_state::TranscriptViewBoundsCause;
    use crate::r#loop::agent_tui::{
        AgentTuiRunOverrides, AgentTuiSubmit, custom_terminal::Terminal,
        input::AgentTuiInputResult, run_output::DaemonSubscriptionPhase, state::AgentTuiState,
        state_tests::launch_request,
    };

    #[test]
    fn working_status_renders_in_composer_dock_not_header() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.header.config_status =
            crate::r#loop::agent_tui::AgentTuiConfigStatus::new("cpa", "gpt-5.4", "已认证");
        state.start_runtime_submit(&test_submit("当前任务"));
        state.replace_draft("下一轮".to_owned());
        assert!(state.queue_draft_for_next_turn());
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new_for_test(backend, 24).expect("测试终端应可创建");

        terminal
            .draw(|frame| super::render_tui(frame, &mut state))
            .expect("TUI 渲染不应失败");

        let buffer = terminal.backend().buffer();
        let header_text = rows_text(buffer, 0, 2);
        let dock_top = buffer.area.height.saturating_sub(state.composer_height());
        let dock_text = rows_text(buffer, dock_top, buffer.area.height);
        let spacing_row = row_text(buffer, dock_top);
        let status_row = row_text(buffer, dock_top.saturating_add(1));
        let input_divider_row = row_text(
            buffer,
            dock_top
                .saturating_add(2)
                .saturating_add(state.queued_submit_preview_height()),
        );
        let normalized_header = header_text.split_whitespace().collect::<String>();
        let normalized_dock = dock_text.split_whitespace().collect::<String>();
        let normalized_status_row = status_row.split_whitespace().collect::<String>();
        assert!(!normalized_header.contains("Esc中断"));
        assert!(!normalized_header.contains("耗时:"));
        assert!(normalized_dock.contains("cpa/gpt-5.4正在连接"));
        assert!(normalized_dock.contains("已有待发送消息·按ESC立即发送"));
        assert!(spacing_row.trim().is_empty());
        assert!(normalized_status_row.contains("cpa/gpt-5.4正在连接"));
        assert!(input_divider_row.starts_with("─"));
        assert!(!input_divider_row.contains("正在连接"));
    }

    #[test]
    fn layout_keeps_header_top_and_composer_bottom() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = layout_app_shell(area, 3, HEADER_BAR_HEIGHT);

        assert_eq!(layout.header.y, area.y);
        assert_eq!(layout.header.height, HEADER_BAR_HEIGHT);
        assert_eq!(
            layout.viewport.y,
            layout.header.bottom() + SECTION_DIVIDER_HEIGHT
        );
        assert_eq!(layout.viewport.bottom(), layout.composer.y);
        assert_eq!(layout.composer.y + layout.composer.height, area.bottom());
    }

    #[test]
    fn header_bar_is_top_status_row() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.header.cwd = "E:/work".to_owned();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new_for_test(backend, 24).expect("测试终端应可创建");

        terminal
            .draw(|frame| super::render_tui(frame, &mut state))
            .expect("TUI 渲染不应失败");

        let buffer = terminal.backend().buffer();
        let first_row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect();
        let second_row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 1)].symbol())
            .collect();
        assert!(!first_row.contains("就绪"));
        assert!(!first_row.trim().is_empty());
        assert!(second_row.starts_with("─"));
    }

    #[test]
    fn main_viewport_resize_restores_previous_top_anchor() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.cells.transcript.replace_with_records(vec![
            crate::surface::cell::RenderCellRecord::system("短行 0"),
            crate::surface::cell::RenderCellRecord::system("短行 1"),
            crate::surface::cell::RenderCellRecord::system(
                "目标行第一段\n目标行第二段\n目标行第三段",
            ),
        ]);
        state.cells.transcript.extend((3..80).map(|index| {
            crate::surface::cell::RenderCellRecord::system(format!("尾部行 {index}"))
        }));
        let target_id = state.cells.transcript[2].render_id();
        state.viewport.restore_transcript_view_bounds(
            TranscriptViewBoundsCause::ContentAppended,
            20,
            5,
            None,
            None,
        );
        state.viewport.detach_transcript_view_at_offset(12);
        state.viewport.transcript_view_top_anchor =
            Some(crate::surface::viewport_anchor::ScrollAnchor {
                cell_id: target_id,
                row_offset_in_cell: 1,
            });

        super::refresh_main_viewport_bounds(&mut state, Rect::new(0, 0, 80, 8));

        assert!(state.viewport.transcript_view_offset_rows > 0);
        assert_eq!(
            state.viewport.transcript_view_top_anchor,
            Some(crate::surface::viewport_anchor::ScrollAnchor {
                cell_id: target_id,
                row_offset_in_cell: 1,
            })
        );
    }

    #[test]
    fn repeated_scroll_up_advances_after_viewport_reflow() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.set_frame_size(ratatui::layout::Size::new(80, 24));
        state.cells.transcript.clear();
        state.cells.transcript.extend((0..80).map(|index| {
            crate::surface::cell::RenderCellRecord::system(format!(
                "历史行 {index}\n历史行 {index} 详情"
            ))
        }));
        let area = layout_app_shell(
            Rect::new(0, 0, 80, 24),
            state.composer_height(),
            super::header_bar_height(&state, 80),
        )
        .viewport;

        super::refresh_main_viewport_bounds(&mut state, area);
        assert!(state.viewport.transcript_view_top_anchor.is_some());

        state.scroll_transcript_view_up();
        let first_offset = state.viewport.transcript_view_offset_rows;
        super::refresh_main_viewport_bounds(&mut state, area);
        assert_eq!(state.viewport.transcript_view_offset_rows, first_offset);

        state.scroll_transcript_view_up();
        let second_offset = state.viewport.transcript_view_offset_rows;
        super::refresh_main_viewport_bounds(&mut state, area);

        assert!(second_offset > first_offset);
        assert_eq!(state.viewport.transcript_view_offset_rows, second_offset);
    }

    #[test]
    fn visible_text_selection_highlights_across_app_shell_regions() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.cells.transcript.replace_with_records(vec![
            crate::surface::cell::RenderCellRecord::system("渲染区输出"),
        ]);
        state.replace_draft("输入区草稿".to_owned());
        state.visible_text_selection.begin(0, 0);
        assert!(state.visible_text_selection.update(6, 4));
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new_for_test(backend, 10).expect("测试终端应可创建");

        terminal
            .draw(|frame| super::render_tui(frame, &mut state))
            .expect("TUI 渲染不应失败");

        let buffer = terminal.backend().buffer();
        assert!(
            buffer[(0, 0)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "header 应被可见选区反白"
        );
        assert!(
            buffer[(0, 3)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "渲染区应被可见选区反白"
        );
        assert!(
            buffer[(0, 6)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "输入区应被可见选区反白"
        );
    }

    #[test]
    fn composer_bottom_divider_is_terminal_bottom_boundary() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.replace_draft("输入区草稿".to_owned());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new_for_test(backend, 24).expect("测试终端应可创建");

        terminal
            .draw(|frame| super::render_tui(frame, &mut state))
            .expect("TUI 渲染不应失败");

        let buffer = terminal.backend().buffer();
        assert_eq!(row_text(buffer, buffer.area.height - 1), "─".repeat(80));
    }

    #[test]
    fn subscription_recovery_is_not_projected_to_default_working_status() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.start_runtime_submit(&test_submit("订阅恢复"));
        state.bridge.provider_activity.push(
            axon_protocol::wire::WireProviderStatus {
                provider: "cpa".to_owned(),
                model: "gpt-5.4".to_owned(),
                phase: axon_protocol::wire::WireProviderStatusPhase::Generating,
                transport: axon_protocol::wire::WireProviderTransport::Http,
                attempt_index: 1,
                http_status: None,
                error_message: None,
                next_provider: None,
                next_model: None,
            },
            std::time::Instant::now(),
        );
        state.apply_subscription_phase(DaemonSubscriptionPhase::Reconnecting);
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new_for_test(backend, 24).expect("测试终端应可创建");

        terminal
            .draw(|frame| super::render_tui(frame, &mut state))
            .expect("TUI 渲染不应失败");

        let buffer = terminal.backend().buffer();
        let dock_top = buffer.area.height.saturating_sub(state.composer_height());
        let dock_text = rows_text(buffer, dock_top, buffer.area.height);
        let normalized_dock = dock_text.split_whitespace().collect::<String>();
        assert!(!normalized_dock.contains("daemon:reconnecting"));
        assert!(!normalized_dock.contains("订阅恢复"));
        assert!(normalized_dock.contains("cpa/gpt-5.4生成回复中"));
    }

    #[test]
    fn provider_activity_stays_primary_during_subscription_recovery() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));
        state.start_runtime_submit(&test_submit("订阅恢复"));
        state.bridge.provider_activity.push(
            axon_protocol::wire::WireProviderStatus {
                provider: "cpa".to_owned(),
                model: "gpt-5.4".to_owned(),
                phase: axon_protocol::wire::WireProviderStatusPhase::Generating,
                transport: axon_protocol::wire::WireProviderTransport::Http,
                attempt_index: 1,
                http_status: None,
                error_message: None,
                next_provider: None,
                next_model: None,
            },
            std::time::Instant::now(),
        );
        state.apply_subscription_phase(DaemonSubscriptionPhase::Reconnecting);
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new_for_test(backend, 24).expect("测试终端应可创建");

        terminal
            .draw(|frame| super::render_tui(frame, &mut state))
            .expect("TUI 渲染不应失败");

        let buffer = terminal.backend().buffer();
        let dock_top = buffer.area.height.saturating_sub(state.composer_height());
        let dock_text = rows_text(buffer, dock_top, buffer.area.height);
        let normalized_dock = dock_text.split_whitespace().collect::<String>();
        assert!(normalized_dock.contains("cpa/gpt-5.4生成回复中"));
        assert!(!normalized_dock.contains("订阅恢复"));
    }

    fn rows_text(buffer: &ratatui::buffer::Buffer, from: u16, to: u16) -> String {
        let mut text = String::new();
        for y in from..to {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn row_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn test_submit(text: &str) -> AgentTuiSubmit {
        AgentTuiSubmit {
            input: AgentTuiInputResult::Submit {
                text: text.to_owned(),
                elements: Vec::new(),
            },
            overrides: AgentTuiRunOverrides::default(),
            cwd: axon_test_support::test_temp_root(),
            question_answer: None,
        }
    }
}
