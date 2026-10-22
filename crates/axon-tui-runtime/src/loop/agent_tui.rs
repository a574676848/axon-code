//! Axon Agent TUI 全屏交互入口 - 基于 ratatui 的产品化 UI。
//!
//! 架构：HeaderBar + RenderViewport + ComposerDock 三段 AppShell。

use std::{
    io, thread,
    time::{Duration, Instant},
};

use crossterm::event;

pub(super) const MOUSE_SCROLL_STEP_ROWS: u16 = 3;

#[path = "agent_tui/ability_matrix.rs"]
mod ability_matrix;
#[cfg(test)]
#[path = "agent_tui/agent_tui_error_tests.rs"]
mod agent_tui_error_tests;
#[path = "agent_tui/assistant_stream_registry.rs"]
mod assistant_stream_registry;
#[path = "agent_tui/attachments.rs"]
pub(crate) mod attachments;
#[path = "agent_tui/bottom_view.rs"]
pub(crate) mod bottom_view;
#[path = "agent_tui/catalog_loader.rs"]
mod catalog_loader;
#[path = "agent_tui/clipboard.rs"]
mod clipboard;
#[path = "agent_tui/commands.rs"]
pub(crate) mod commands;
#[path = "agent_tui/completion_inbox.rs"]
mod completion_inbox;
#[path = "agent_tui/composer_popup.rs"]
pub(crate) mod composer_popup;
#[path = "agent_tui/custom_terminal.rs"]
mod custom_terminal;
#[cfg(test)]
#[path = "agent_tui/custom_terminal_tests.rs"]
mod custom_terminal_tests;
#[path = "agent_tui/display_units.rs"]
pub(crate) mod display_units;
#[path = "agent_tui/draft_elements.rs"]
mod draft_elements;
#[path = "agent_tui/draft_state.rs"]
pub(crate) mod draft_state;
#[path = "agent_tui/draft_text.rs"]
pub(crate) mod draft_text;
#[path = "agent_tui/duration_format.rs"]
pub(crate) mod duration_format;
#[path = "agent_tui/error_projection.rs"]
mod error_projection;
#[cfg(test)]
pub(crate) use error_projection::run_error_cell as test_run_error_cell;
#[path = "agent_tui/event_handlers.rs"]
mod event_handlers;
#[cfg(test)]
#[path = "agent_tui/event_handlers_autoscroll_tests.rs"]
mod event_handlers_autoscroll_tests;
#[cfg(test)]
#[path = "agent_tui/event_handlers_history_navigation_tests.rs"]
mod event_handlers_history_navigation_tests;
#[cfg(test)]
#[path = "agent_tui/event_handlers_mouse_tests.rs"]
mod event_handlers_mouse_tests;
#[cfg(test)]
#[path = "agent_tui/event_handlers_question_tests.rs"]
mod event_handlers_question_tests;
#[cfg(test)]
#[path = "agent_tui/event_handlers_runtime_tests.rs"]
mod event_handlers_runtime_tests;
#[cfg(test)]
#[path = "agent_tui/event_handlers_tests.rs"]
mod event_handlers_tests;
#[path = "agent_tui/external_editor.rs"]
mod external_editor;
#[path = "agent_tui/frame_scheduler.rs"]
mod frame_scheduler;
#[path = "agent_tui/history_search.rs"]
pub(crate) mod history_search;
#[path = "agent_tui/history_store.rs"]
mod history_store;
#[path = "agent_tui/input.rs"]
pub(crate) mod input;
mod input_contract;
#[cfg(test)]
#[path = "agent_tui/input_result_tests.rs"]
mod input_result_tests;
#[path = "agent_tui/large_paste.rs"]
mod large_paste;
#[path = "agent_tui/launch.rs"]
pub(crate) mod launch;
#[path = "agent_tui/live_status.rs"]
pub(crate) mod live_status;
#[path = "agent_tui/mcp_login_loader.rs"]
mod mcp_login_loader;
#[path = "agent_tui/notification.rs"]
mod notification;
#[path = "agent_tui/output_dock.rs"]
pub(crate) mod output_dock;
#[path = "agent_tui/paste_burst.rs"]
mod paste_burst;
#[path = "agent_tui/paste_input.rs"]
mod paste_input;
#[path = "agent_tui/provider_activity.rs"]
mod provider_activity;
#[path = "agent_tui/render.rs"]
mod render;
#[path = "agent_tui/render_toast.rs"]
mod render_toast;
#[path = "agent_tui/render_transcript.rs"]
mod render_transcript;
#[path = "agent_tui/run_output.rs"]
mod run_output;
#[path = "agent_tui/run_output_diff.rs"]
mod run_output_diff;
#[cfg(test)]
#[path = "agent_tui/run_output_tests.rs"]
mod run_output_tests;
#[path = "agent_tui/runtime_bridge.rs"]
mod runtime_bridge;
#[path = "agent_tui/runtime_contract.rs"]
mod runtime_contract;
#[path = "agent_tui/runtime_identity.rs"]
pub(crate) mod runtime_identity;
#[path = "agent_tui/session_generation.rs"]
mod session_generation;
#[path = "agent_tui/session_summary.rs"]
pub(crate) mod session_summary;
#[path = "agent_tui/shortcut_help.rs"]
pub(crate) mod shortcut_help;
#[path = "agent_tui/shortcuts.rs"]
mod shortcuts;
#[path = "agent_tui/smoke_contract.rs"]
mod smoke_contract;
#[path = "agent_tui/startup_transcript.rs"]
mod startup_transcript;
#[path = "agent_tui/state.rs"]
pub(crate) mod state;
#[cfg(test)]
#[path = "agent_tui/state_tests.rs"]
mod state_tests;
#[path = "agent_tui/streaming/mod.rs"]
pub(crate) mod streaming;
#[path = "agent_tui/submission.rs"]
mod submission;
#[path = "agent_tui/terminal_session.rs"]
mod terminal_session;
#[path = "agent_tui/terminal_sync.rs"]
mod terminal_sync;
#[path = "agent_tui/terminal_title.rs"]
mod terminal_title;
#[path = "agent_tui/tmp_cleanup.rs"]
mod tmp_cleanup;
#[path = "agent_tui/tool_display_projection.rs"]
pub(crate) mod tool_display_projection;
#[path = "agent_tui/tool_output_projection.rs"]
pub(crate) mod tool_output_projection;
#[path = "agent_tui/transcript_view_navigation.rs"]
mod transcript_view_navigation;
#[path = "agent_tui/tui_event.rs"]
pub(crate) mod tui_event;
#[path = "agent_tui/user_question_receipt.rs"]
mod user_question_receipt;
#[cfg(test)]
#[path = "agent_tui/wire_event_test_support.rs"]
pub(crate) mod wire_event_test_support;

use crate::surface::{
    error_model::TuiError,
    runtime_adapter::{RuntimeError, RuntimeEventEmitter},
};
pub use ability_matrix::{
    AgentTuiAbilityCategorySnapshot, AgentTuiAbilityEntry, AgentTuiAbilityMatrixSnapshot,
};
pub use catalog_loader::AgentTuiCatalogLoader;
pub use completion_inbox::{
    AgentTuiCompletionInbox, AgentTuiCompletionInboxClient, AgentTuiCompletionNotification,
};
use event_handlers::{TuiEventOutcome, handle_tui_event};
use frame_scheduler::{FrameRequestReason, FrameScheduler};
pub use input::{
    AgentTuiInputElement, AgentTuiInputElementKind, AgentTuiInputResult, AgentTuiQueuedAction,
};
pub(crate) use input_contract::{
    AGENT_TUI_PROMPT_INDICATOR, TUI_HINT_IDLE, agent_tui_prompt_width,
};
use input_contract::{TUI_HINT_COMMAND, TUI_HINT_DONE, TUI_HINT_WORKING};
pub use launch::{
    AgentTuiCatalog, AgentTuiCatalogLoadSnapshot, AgentTuiCatalogLoadState,
    AgentTuiConfigDiagnostic, AgentTuiConfigDiagnosticStatus, AgentTuiConfigStatus,
    AgentTuiLaunchReport, AgentTuiLaunchRequest, AgentTuiMcpServerCatalogItem,
    AgentTuiModelCatalogItem, AgentTuiRunOverrides, AgentTuiSessionCatalogItem,
    AgentTuiSettingsChange, AgentTuiSkillStateChange, AgentTuiWorkspaceStatus,
    launch_agent_tui_report,
};
pub use mcp_login_loader::{
    AgentTuiMcpAuthAction, AgentTuiMcpAuthLoader, AgentTuiMcpAuthOutcome, AgentTuiMcpAuthRequest,
};
use render::render_tui;
pub use run_output::{
    AgentTuiCancelPayload, AgentTuiChannelAttempt, AgentTuiHistoryItem, AgentTuiHistoryLine,
    AgentTuiHookOutputEntry, AgentTuiHookOutputKind, AgentTuiHookStatus, AgentTuiHookStatusView,
    AgentTuiInterruptPayload, AgentTuiLifecycleLine, AgentTuiLifecyclePhase,
    AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput, AgentTuiMessagePart,
    AgentTuiMessagePartRole, AgentTuiPlanLine, AgentTuiPlanProgress, AgentTuiPlanStatus,
    AgentTuiPlanStep, AgentTuiReasoningLine, AgentTuiReviewLine, AgentTuiReviewStatus,
    AgentTuiRunError, AgentTuiRunLine, AgentTuiRunOutput, AgentTuiRunSessionInfo,
    AgentTuiRuntimeEvent, AgentTuiRuntimePayload, AgentTuiStatusItem, AgentTuiStatusLine,
    AgentTuiTokenUsage, AgentTuiToolCompletionView, AgentTuiToolDiffSnapshotView,
    AgentTuiToolExecutionView, AgentTuiToolKind, AgentTuiToolLifecycleLine,
    AgentTuiToolLifecycleLineInput, AgentTuiToolOutputChunk, AgentTuiToolProgressView,
    AgentTuiToolReceipt, AgentTuiToolReceiptDetails, AgentTuiUsageLine, DaemonSubscriptionPhase,
    axon_cli_runtime_event, runtime_driver_event, tui_control_runtime_event, watcher_runtime_event,
};
pub use run_output_diff::{AgentTuiDiffLine, file_change_from_parts};
pub use runtime_contract::{
    AgentTuiActiveTurn, AgentTuiCancelToken, AgentTuiRuntimeAdapter, AgentTuiSteerResult,
    AgentTuiSubmitFailure,
};
pub use session_summary::AgentTuiSessionSummary;
use state::AgentTuiState;
pub use state::{AgentTuiQuestionAnswer, AgentTuiSubmit};
use submission::RuntimeSubmissionDriver;
use terminal_session::{AgentTerminal, AgentTerminalSession};
use terminal_title::{TerminalTitleManager, finish_stable_terminal_title};
pub use tmp_cleanup::{cleanup_agent_tui_tmp_dir, ensure_agent_tui_tmp_cleanup_started};
pub use tool_display_projection::ToolDisplayProjection;

/// Agent TUI 启动成功状态。
pub const AGENT_TUI_STATUS_STARTED: &str = "agent_tui_started";
/// Agent TUI 正常退出状态。
pub const AGENT_TUI_STATUS_EXITED: &str = "agent_tui_exited";
/// Agent TUI 已提交任务状态。
pub const AGENT_TUI_STATUS_SUBMITTED: &str = "agent_tui_submitted";

const AGENT_TUI_KIND: &str = "agent";
const AGENT_TUI_FOCUS_TARGET: &str = "agent_composer";
const AGENT_TUI_PANEL_TITLE: &str = "Axon Agent";
const AGENT_TUI_TERMINAL_IO_EXIT_CODE: u8 = 2;

pub(crate) const AXON_VERSION: &str = env!("CARGO_PKG_VERSION");
const AGENT_TUI_INPUT_DRAIN_BUDGET: usize = 32;
const AGENT_TUI_POLL_TIMEOUT: Duration = Duration::from_millis(8);
const TERMINAL_TITLE_IDLE_REFRESH_INTERVAL: Duration = Duration::from_hours(1);

/// Agent TUI 运行错误。
#[derive(Debug)]
pub enum AgentTuiError {
    /// 终端 I/O 错误。
    Io(io::Error),
}

/// 运行带完整 runtime adapter 和设置变更处理器的 agent-first 全屏 TUI。
///
/// # Errors
///
/// 终端 raw mode、事件读取或绘制失败时返回错误。
pub fn run_agent_tui_cli_with_runtime_adapter_and_settings<A, S>(
    request: &AgentTuiLaunchRequest,
    adapter: A,
    on_settings_change: S,
) -> Result<AgentTuiLaunchReport, AgentTuiError>
where
    A: AgentTuiRuntimeAdapter,
    S: FnMut(AgentTuiSettingsChange),
{
    let mut terminal_session = AgentTerminalSession::start()?;
    let mut state = AgentTuiState::new(request);
    let active_turn = adapter.load_active_turn(request);
    if active_turn.is_none()
        && let Some(history) = adapter.load_initial_history(request)
    {
        state.restore_historical_turn(history);
    }
    if let Err(error) = adapter.contract().validate() {
        state.push_runtime_error(error);
    }
    let catalog_loader = None;
    let mcp_auth_loader = None;
    let result = run_agent_tui_loop(
        terminal_session.terminal_mut(),
        &mut state,
        request,
        adapter,
        catalog_loader,
        mcp_auth_loader,
        on_settings_change,
        active_turn,
    );
    terminal_session.restore()?;
    finish_stable_terminal_title(&state.terminal_title_context()).map_err(AgentTuiError::Io)?;
    result?;

    Ok(AgentTuiLaunchReport {
        status: if state.composer.submitted_prompt.is_some() {
            AGENT_TUI_STATUS_SUBMITTED
        } else {
            AGENT_TUI_STATUS_EXITED
        },
        submitted_prompt: state.composer.submitted_prompt.clone(),
        session_usage: state.session_usage(),
        session_summary: state.session_summary(),
        telemetry: state.telemetry_snapshot(),
        message: "Axon Agent TUI 已关闭".to_owned(),
        ..launch_agent_tui_report(request)
    })
}

fn run_agent_tui_loop<F>(
    terminal: &mut AgentTerminal,
    state: &mut AgentTuiState,
    request: &AgentTuiLaunchRequest,
    adapter: F,
    mut catalog_loader: Option<AgentTuiCatalogLoader>,
    mut mcp_auth_loader: Option<AgentTuiMcpAuthLoader>,
    mut on_settings_change: impl FnMut(AgentTuiSettingsChange),
    active_turn: Option<AgentTuiActiveTurn>,
) -> Result<(), AgentTuiError>
where
    F: AgentTuiRuntimeAdapter,
{
    let frame_scheduler = FrameScheduler::start().map_err(AgentTuiError::Io)?;
    frame_scheduler.request_frame(FrameRequestReason::InitialMount, state);
    let mut terminal_title = TerminalTitleManager::default();
    let mut runtime_driver = RuntimeSubmissionDriver::new(adapter.clone());
    state.attach_mailbox_telemetry(runtime_driver.mailbox_telemetry());
    if let Some(active_turn) = active_turn {
        let submission = state.submit_plain(active_turn.prompt.clone());
        state.start_runtime_submit(&submission);
        runtime_driver.start_resume(submission, active_turn, request.clone());
    }
    // 收件箱在 TUI 启动即注册 presence：它与 Feishu 等其他可用渠道并行接收完成通知，
    // 线程内轮询，主循环只做展示与回执。
    let mut completion_inbox = adapter.start_completion_inbox(request);
    let mut next_title_refresh = Instant::now();
    let mut last_render_signature = None;
    let mut last_transcript_signature = None;
    let mut startup_prewarm_started = false;
    'tui: loop {
        let mut input_handled = false;
        for _ in 0..AGENT_TUI_INPUT_DRAIN_BUDGET {
            if !event::poll(Duration::ZERO).map_err(AgentTuiError::Io)? {
                break;
            }
            match handle_tui_event(terminal, state, &mut runtime_driver)? {
                TuiEventOutcome::Ignored => {}
                TuiEventOutcome::Handled { needs_redraw } => {
                    apply_settings_changes(state, &mut on_settings_change);
                    schedule_if_redraw_needed(&frame_scheduler, state, needs_redraw);
                    input_handled = true;
                }
                TuiEventOutcome::Exit => break 'tui,
            }
        }

        let now = Instant::now();
        if runtime_driver.tick(state)? {
            frame_scheduler.request_frame(FrameRequestReason::RuntimeProgress, state);
        }
        if state.tick(now) {
            frame_scheduler.request_frame(FrameRequestReason::StateTick, state);
        }
        if poll_catalog_loader(&adapter, state, &mut catalog_loader) {
            frame_scheduler.request_frame(FrameRequestReason::StateTick, state);
        }
        if poll_mcp_auth_loader(&adapter, state, &mut mcp_auth_loader) {
            frame_scheduler.request_frame(FrameRequestReason::StateTick, state);
        }
        if poll_completion_inbox(
            state,
            &mut completion_inbox,
            &mut runtime_driver,
            &adapter,
            request,
        ) {
            frame_scheduler.request_frame(FrameRequestReason::StateTick, state);
        }
        if state.flush_paste_burst_if_due(now) {
            frame_scheduler.request_frame(FrameRequestReason::PasteBurstFlush, state);
        }
        if state.maintain_cell_history_budget() {
            frame_scheduler.request_frame(FrameRequestReason::StateTick, state);
        }

        // FrameScheduler 已经是唯一帧率权威，收到信号后立即绘制最新状态。
        if frame_scheduler.try_take_frame(state) {
            state.prepare_cold_bottom_mount();
            let render_signature = state.frame_render_signature();
            next_title_refresh = refresh_terminal_title(&mut terminal_title, state, now)?;
            if last_render_signature == Some(render_signature) {
                state.record_stable_duplicate_frame();
            } else {
                let draw_started_at = Instant::now();
                let transcript_signature =
                    draw_synchronized_frame(terminal, state, last_transcript_signature)?;
                if !startup_prewarm_started {
                    start_first_frame_background_work(
                        &adapter,
                        request,
                        &mut startup_prewarm_started,
                        &mut catalog_loader,
                    );
                }
                state.record_frame_draw(draw_started_at.elapsed());
                last_render_signature = Some(render_signature);
                last_transcript_signature = Some(transcript_signature);
                if state.complete_cold_bottom_mount_frame() {
                    frame_scheduler.request_frame(FrameRequestReason::StateTick, state);
                }
            }
        } else if now >= next_title_refresh {
            // 空闲时也要刷新 title 等独立信号。
            next_title_refresh = refresh_terminal_title(&mut terminal_title, state, now)?;
        }

        if input_handled {
            continue;
        }
        if !event::poll(Duration::ZERO).map_err(AgentTuiError::Io)? {
            thread::park_timeout(AGENT_TUI_POLL_TIMEOUT);
            continue;
        }

        match handle_tui_event(terminal, state, &mut runtime_driver)? {
            TuiEventOutcome::Ignored => {}
            TuiEventOutcome::Handled { needs_redraw } => {
                apply_settings_changes(state, &mut on_settings_change);
                schedule_if_redraw_needed(&frame_scheduler, state, needs_redraw);
            }
            TuiEventOutcome::Exit => break 'tui,
        }
    }
    terminal_title
        .finish(&state.terminal_title_context())
        .map_err(AgentTuiError::Io)?;
    Ok(())
}

fn refresh_terminal_title(
    terminal_title: &mut TerminalTitleManager,
    state: &AgentTuiState,
    now: Instant,
) -> Result<Instant, AgentTuiError> {
    let interval = terminal_title
        .refresh(&state.terminal_title_context(), now)
        .map_err(AgentTuiError::Io)?
        .unwrap_or(TERMINAL_TITLE_IDLE_REFRESH_INTERVAL);
    Ok(now + interval)
}

fn start_first_frame_background_work<F>(
    adapter: &F,
    request: &AgentTuiLaunchRequest,
    startup_prewarm_started: &mut bool,
    catalog_loader: &mut Option<AgentTuiCatalogLoader>,
) where
    F: AgentTuiRuntimeAdapter,
{
    adapter.start_session_prewarm(request);
    *catalog_loader = adapter.start_catalog_loader(request, false);
    *startup_prewarm_started = true;
}

fn poll_catalog_loader<F>(
    adapter: &F,
    state: &mut AgentTuiState,
    catalog_loader: &mut Option<AgentTuiCatalogLoader>,
) -> bool
where
    F: AgentTuiRuntimeAdapter,
{
    if let Some(loader) = catalog_loader.as_mut()
        && let Some(snapshot) = loader.poll()
    {
        state.apply_catalog_load_snapshot(snapshot);
        *catalog_loader = None;
        return true;
    }
    if catalog_loader.is_some() {
        return false;
    }
    let Some(force_mcp_refresh) = state.take_catalog_reload_request() else {
        return false;
    };
    if let Some(loader) =
        adapter.start_catalog_loader(&state.catalog_reload_launch_request(), force_mcp_refresh)
    {
        *catalog_loader = Some(loader);
    } else {
        state.fail_catalog_reload("当前 runtime adapter 未提供 catalog loader");
    }
    true
}

/// 轮询 MCP OAuth 后台任务：先收集结果，再按状态请求启动新任务。
fn poll_mcp_auth_loader<F>(
    adapter: &F,
    state: &mut AgentTuiState,
    mcp_auth_loader: &mut Option<AgentTuiMcpAuthLoader>,
) -> bool
where
    F: AgentTuiRuntimeAdapter,
{
    if let Some(loader) = mcp_auth_loader.as_mut()
        && let Some(outcome) = loader.poll()
    {
        state.apply_mcp_auth_outcome(outcome);
        *mcp_auth_loader = None;
        return true;
    }
    if mcp_auth_loader.is_some() {
        return false;
    }
    let Some(request) = state.take_mcp_auth_request() else {
        return false;
    };
    match adapter.start_mcp_auth(&state.catalog_reload_launch_request(), &request) {
        Some(loader) => *mcp_auth_loader = Some(loader),
        None => state.fail_mcp_auth_request(&request, "当前 runtime adapter 未提供 MCP 授权任务"),
    }
    true
}

fn schedule_if_redraw_needed(
    frame_scheduler: &FrameScheduler,
    state: &mut AgentTuiState,
    needs_redraw: bool,
) {
    if needs_redraw {
        frame_scheduler.request_frame(FrameRequestReason::UiAction, state);
    }
}

/// 展示完成通知收件箱里的新消息，并在展示之后回执确认，随后接上通知驱动的主 turn。
///
/// 顺序不可颠倒：只有已经写入 transcript 的通知才允许回执，否则等于对用户没看到的
/// 完成消息做静默确认。
fn poll_completion_inbox<F: AgentTuiRuntimeAdapter>(
    state: &mut AgentTuiState,
    completion_inbox: &mut Option<AgentTuiCompletionInbox>,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    adapter: &F,
    request: &AgentTuiLaunchRequest,
) -> bool {
    let Some(inbox) = completion_inbox.as_mut() else {
        return false;
    };
    let mut changed = false;
    let mut drained = 0_usize;
    for notification in inbox.drain() {
        state.push_completion_notification(&notification.content);
        inbox.acknowledge(&notification.notification_id);
        drained += 1;
        changed = true;
    }
    if let Some(failure) = inbox.take_failure() {
        state.push_completion_inbox_failure(&failure);
        changed = true;
    }
    if drained > 0 {
        changed |= resume_notice_driven_turn(state, runtime_driver, adapter, request);
    }
    changed
}

/// 通知到达后的主 turn 自动继续（合同 §5.1.5 TUI 面）。
///
/// daemon 在派发完成通知时已提交持久 steer Interaction，主 turn 因此会自行继续；
/// TUI 侧要做的**不是再造一轮**，而是接上这一轮：空闲会话无需用户输入即继续，
/// 用户也不必手动回车才能看到模型对通知的响应。
///
/// 已有活跃提交时说明 TUI 正跟着这一轮，直接返回；没有活跃轮次时也不另起一轮，
/// 避免与 daemon 的持久投递状态机重复消费同一条通知。
fn resume_notice_driven_turn<F: AgentTuiRuntimeAdapter>(
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    adapter: &F,
    request: &AgentTuiLaunchRequest,
) -> bool {
    if runtime_driver.is_active() {
        return false;
    }
    let Some(active_turn) = adapter.load_active_turn(request) else {
        return false;
    };
    let submission = state.submit_plain(active_turn.prompt.clone());
    state.start_runtime_submit(&submission);
    runtime_driver.start_resume(submission, active_turn, request.clone());
    true
}

fn draw_synchronized_frame(
    terminal: &mut AgentTerminal,
    state: &mut AgentTuiState,
    previous_transcript_signature: Option<u64>,
) -> Result<u64, AgentTuiError> {
    terminal
        .begin_synchronized_update()
        .map_err(AgentTuiError::Io)?;
    let draw_result = draw_agent_frame(terminal, state, previous_transcript_signature);
    let end_result = terminal
        .end_synchronized_update()
        .map_err(AgentTuiError::Io);
    match (draw_result, end_result) {
        (Ok(signature), Ok(())) => Ok(signature),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
    }
}

fn draw_agent_frame(
    terminal: &mut AgentTerminal,
    state: &mut AgentTuiState,
    previous_transcript_signature: Option<u64>,
) -> Result<u64, AgentTuiError> {
    let terminal_size = terminal.size().map_err(AgentTuiError::Io)?;
    state.sync_stream_width(terminal_size.width, Instant::now());
    state.set_frame_size(terminal_size);
    terminal
        .update_fullscreen_viewport()
        .map_err(AgentTuiError::Io)?;
    let transcript_signature = state.transcript_render_signature();
    let render_transcript = previous_transcript_signature != Some(transcript_signature);
    let draw_result = if render_transcript {
        terminal.draw(|frame| render_tui(frame, state))
    } else {
        terminal.draw_preserving_previous(|frame| {
            render::render_tui_with_transcript(frame, state, false)
        })
    };
    draw_result.map_err(AgentTuiError::Io)?;
    state
        .visible_text_selection
        .update_snapshot(terminal.visible_text_snapshot());
    Ok(transcript_signature)
}

fn apply_settings_changes(
    state: &mut AgentTuiState,
    on_settings_change: &mut impl FnMut(AgentTuiSettingsChange),
) {
    for change in state.take_settings_changes() {
        on_settings_change(change);
    }
}

impl AgentTuiError {
    /// 用户可读错误消息。
    #[must_use]
    pub fn message(&self) -> String {
        self.to_tui_error().message
    }

    /// 进程退出码。
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Io(_) => AGENT_TUI_TERMINAL_IO_EXIT_CODE,
        }
    }

    /// 投影为统一 TUI 错误模型。
    #[must_use]
    pub fn to_tui_error(&self) -> TuiError {
        match self {
            Self::Io(error) => TuiError::terminal_io(format!("Agent TUI 终端 I/O 失败：{error}")),
        }
    }
}
