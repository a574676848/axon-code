//! CLI/TUI 到 Daemon Primary Turn 的执行编排与输出渲染。

use std::{collections::HashMap, io::Write, path::Path};

#[cfg(test)]
use std::time::{Duration, Instant};

use axon_daemon::primary_session_id_for_interaction;
use axon_domain::InteractionRecord;
#[cfg(test)]
use axon_domain::InteractionStatus;
use axon_protocol::wire::{
    WireAssistantMessage, WireEventEnvelope, WireEventKind, WireTurnLifecycle,
};
#[cfg(test)]
use axon_session::SessionEventRecord;
use serde::Serialize;

#[cfg(test)]
use super::daemon_event_cursor::ensure_assistant_event;
#[cfg(test)]
use super::daemon_event_cursor::{TURN_COMPLETED_EVENT, WIRE_EVENT, collect_events};
pub(crate) use super::daemon_event_subscription_client::DaemonSubscriptionPhase;
pub(crate) use super::daemon_event_subscription_client::DaemonTurnObservation;
use super::daemon_event_subscription_client::{
    DaemonTurnProjection, DaemonTurnState, wait_for_daemon_turn,
};
#[cfg(test)]
use super::daemon_event_subscription_client::{
    SubscriptionControl, SubscriptionMessage, completed_or_status, handle_subscription_message,
};
pub(super) use super::daemon_interaction_submission::{DAEMON_CLIENT_ID, send_command};
#[cfg(test)]
pub(super) use super::daemon_interaction_submission::{daemon_request, response_body};
#[cfg(test)]
use super::daemon_subscription_recovery::ensure_wait_can_continue;
use super::{OutputFormat, submit_daemon_interaction};
use crate::{CliRunResult, runtime_paths::resolve_daemon_endpoint};

use super::run_compaction_projection::{
    COMPACTION_TEXT_PREFIX, is_compaction_kind, is_text_projectable_kind,
};

const LIVE_OUTPUT_NEWLINE: &[u8] = b"\n";
pub(super) const DAEMON_RUN_FAILED_EVENT: &str = "run_failed";

pub(crate) use super::daemon_history_loader::{
    LatestCompletedTurn, load_active_interaction, load_active_interaction_for_workspace,
};

pub(crate) fn load_latest_completed_turn_for_workspace(
    cwd: &Path,
    config_path: Option<&Path>,
    channel: &str,
) -> Result<Option<LatestCompletedTurn>, String> {
    super::daemon_history_loader::load_latest_completed_turn_for_workspace(
        cwd,
        config_path,
        channel,
    )
}

/// 请求 Daemon 实时重探 MCP 能力：清除健康负缓存并重建该 Workspace 的启动快照。
///
/// 这是 TUI `/mcp` 的探测入口：Daemon 侧完成后，恢复的 MCP 工具会在下一轮 turn
/// 进入 Provider 工具表，而不是只刷新前端目录。
pub(crate) fn refresh_daemon_mcp_capabilities(
    cwd: &Path,
    config_path: Option<&Path>,
) -> Result<serde_json::Value, String> {
    let endpoint =
        resolve_daemon_endpoint(crate::runtime_paths::DEFAULT_DAEMON_ENDPOINT, config_path)?;
    send_command(
        &endpoint,
        axon_protocol::DaemonCommand::RefreshMcpCapabilities {
            cwd: cwd.to_string_lossy().into_owned(),
        },
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonRunReport<'a> {
    session_id: &'a str,
    content: &'a str,
    wire_events: &'a [WireEventEnvelope],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonRunFailureReport<'a> {
    event: &'static str,
    error: &'a str,
    /// 已积累的 durable 事件：TUI 实时邮箱只淘汰可重建的展示增量，
    /// 被淘汰的增量只能由终态按 durable 顺序完整重放补齐。
    #[serde(default, skip_serializing_if = "wire_events_are_empty")]
    wire_events: &'a [WireEventEnvelope],
}

fn wire_events_are_empty(events: &[WireEventEnvelope]) -> bool {
    events.is_empty()
}

/// Daemon 回合失败：错误必须与**已积累的 durable 事件**一起返回。
///
/// TUI 的实时邮箱只保证展示增量可淘汰；被淘汰的增量只能由终态按 durable 顺序
/// 完整重放补齐。失败路径丢弃 `wire_events` 会让正文留下无法察觉的缺口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DaemonTurnFailure {
    message: String,
    wire_events: Vec<WireEventEnvelope>,
}

impl DaemonTurnFailure {
    /// 尚未取得任何 durable 累积时只携带失败原因。
    fn without_events(message: String) -> Self {
        Self {
            message,
            wire_events: Vec::new(),
        }
    }

    /// 携带已积累的 durable 事件。
    fn with_events(message: String, wire_events: Vec<WireEventEnvelope>) -> Self {
        Self {
            message,
            wire_events,
        }
    }

    /// 与成功报告共用 `wireEvents` 契约的失败报告文本。
    pub(crate) fn to_report_json(&self) -> String {
        serde_json::to_string(&DaemonRunFailureReport {
            event: DAEMON_RUN_FAILED_EVENT,
            error: &self.message,
            wire_events: &self.wire_events,
        })
        .unwrap_or_else(|error| {
            axon_logger::error(&format!("序列化 Daemon 失败报告失败：{error}"));
            serde_json::json!({
                "event": DAEMON_RUN_FAILED_EVENT,
                "error": self.message,
            })
            .to_string()
        })
    }
}

/// 把一次自然语言 Interaction 提交给 Daemon 并等待 durable 终态。
pub(crate) fn run_daemon_interaction(
    interaction: InteractionRecord,
    cwd: &Path,
    config_path: Option<&Path>,
    output: OutputFormat,
    timeout_seconds: Option<u64>,
    is_cancelled: &dyn Fn() -> bool,
    on_wire_event: Option<&mut dyn FnMut(WireEventEnvelope)>,
    live_writer: Option<&mut dyn Write>,
) -> CliRunResult {
    let mut live_output = DaemonTextLiveOutput::new(live_writer);
    let mut on_wire_event = on_wire_event;
    let mut observe = |event: WireEventEnvelope| {
        live_output.observe(&event);
        if let Some(observer) = on_wire_event.as_deref_mut() {
            observer(event);
        }
    };
    let mut turn_observer = move |observation: DaemonTurnObservation| {
        if let DaemonTurnObservation::Wire(event) = observation {
            observe(event);
        }
    };
    let result = execute_daemon_interaction(
        interaction,
        cwd,
        config_path,
        timeout_seconds,
        is_cancelled,
        Some(&mut turn_observer),
    );
    live_output.finish();
    if let Some(error) = live_output.error {
        return render_failure(&error, output);
    }
    match result {
        Ok(projection) => render_projection(&projection, output, live_output.emitted),
        Err(error) => render_failure(&error, output),
    }
}

pub(crate) fn resume_daemon_interaction(
    interaction: InteractionRecord,
    config_path: Option<&Path>,
    is_cancelled: &dyn Fn() -> bool,
    on_event: Option<&mut dyn FnMut(DaemonTurnObservation)>,
) -> Result<DaemonTurnProjection, DaemonTurnFailure> {
    let endpoint =
        resolve_daemon_endpoint(crate::runtime_paths::DEFAULT_DAEMON_ENDPOINT, config_path)
            .map_err(DaemonTurnFailure::without_events)?;
    let deadline = None;
    let mut state = DaemonTurnState::new(interaction.interaction_id.as_str().to_owned(), on_event);
    let session_id = primary_session_id_for_interaction(&interaction)
        .map_err(DaemonTurnFailure::without_events)?;
    let response = wait_for_daemon_turn(
        &endpoint,
        &interaction,
        session_id.as_str(),
        deadline,
        is_cancelled,
        &mut state,
    )
    .map_err(|message| {
        // 失败同样交出已积累的 durable 事件，供 TUI 终态全量重放补齐被淘汰的增量。
        DaemonTurnFailure::with_events(message, std::mem::take(&mut state.wire_events))
    })?;
    Ok(DaemonTurnProjection {
        session_id: session_id.to_string(),
        response,
        wire_events: state.wire_events,
    })
}

pub(crate) fn execute_daemon_interaction(
    interaction: InteractionRecord,
    cwd: &Path,
    config_path: Option<&Path>,
    timeout_seconds: Option<u64>,
    is_cancelled: &dyn Fn() -> bool,
    on_event: Option<&mut dyn FnMut(DaemonTurnObservation)>,
) -> Result<DaemonTurnProjection, String> {
    let (session_id, primary_interaction_id) =
        submit_daemon_interaction(&interaction, cwd, config_path)?;
    let endpoint =
        resolve_daemon_endpoint(crate::runtime_paths::DEFAULT_DAEMON_ENDPOINT, config_path)?;
    // 前台等待**不受** `--timeout` 约束：该参数是 provider 流空闲超时
    // （见 `runtime::constants::provider_stream_idle_timeout`），把它同时当作整体等待上限
    // 会让「健康但耗时超过该值」的回合被误报为 `run_failed`（实测 61.5s 的回合在
    // `--timeout 60` 下被判定失败，而 Daemon 内该回合 `status=completed`）。
    // 等待只由取消信号与 Daemon 自身的终态/不可达判定结束。
    let _ = timeout_seconds;
    let deadline = None;
    let observed_interaction = InteractionRecord {
        interaction_id: primary_interaction_id.clone(),
        ..interaction
    };
    let mut state = DaemonTurnState::new(primary_interaction_id.as_str().to_owned(), on_event);
    let response = wait_for_daemon_turn(
        &endpoint,
        &observed_interaction,
        session_id.as_str(),
        deadline,
        is_cancelled,
        &mut state,
    )?;
    Ok(DaemonTurnProjection {
        session_id: session_id.to_string(),
        response,
        wire_events: state.wire_events,
    })
}

fn render_projection(
    projection: &DaemonTurnProjection,
    output: OutputFormat,
    live_text_emitted: bool,
) -> CliRunResult {
    match render_projection_output(projection, output, live_text_emitted) {
        Ok(stdout) => CliRunResult {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        },
        Err(error) => CliRunResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: error,
        },
    }
}

fn render_failure(error: &str, output: OutputFormat) -> CliRunResult {
    match output {
        OutputFormat::Text => CliRunResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: error.to_owned(),
        },
        OutputFormat::Json => match serde_json::to_string(&DaemonRunFailureReport {
            event: DAEMON_RUN_FAILED_EVENT,
            error,
            wire_events: &[],
        }) {
            Ok(json) => CliRunResult {
                exit_code: 1,
                stdout: format!("{json}\n"),
                stderr: String::new(),
            },
            Err(serialization_error) => CliRunResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("序列化 Daemon Primary Turn 失败输出失败：{serialization_error}"),
            },
        },
    }
}

fn render_projection_output(
    projection: &DaemonTurnProjection,
    output: OutputFormat,
    live_text_emitted: bool,
) -> Result<String, String> {
    match output {
        OutputFormat::Text if live_text_emitted => Ok(String::new()),
        OutputFormat::Text => Ok(format!("{}\n", projection.response)),
        OutputFormat::Json => serde_json::to_string(&DaemonRunReport {
            session_id: &projection.session_id,
            content: &projection.response,
            wire_events: &projection.wire_events,
        })
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("序列化 Daemon Primary Turn 输出失败：{error}")),
    }
}

struct DaemonTextLiveOutput<'a> {
    writer: Option<&'a mut dyn Write>,
    assistant_contents: HashMap<String, String>,
    latest_message_id: Option<String>,
    latest_content: String,
    emitted: bool,
    line_open: bool,
    error: Option<String>,
}

impl<'a> DaemonTextLiveOutput<'a> {
    fn new(writer: Option<&'a mut dyn Write>) -> Self {
        Self {
            writer,
            assistant_contents: HashMap::new(),
            latest_message_id: None,
            latest_content: String::new(),
            emitted: false,
            line_open: false,
            error: None,
        }
    }

    fn observe(&mut self, event: &WireEventEnvelope) {
        match &event.kind {
            WireEventKind::AssistantMessage(message) => self.observe_assistant_message(message),
            WireEventKind::TurnLifecycle(lifecycle) => {
                self.observe_compaction_lifecycle(lifecycle);
            }
            _ => {}
        }
    }

    /// 压缩行必须落在 stdout：用户对「历史被裁剪、checkpoint 建立」不能零感知。
    fn observe_compaction_lifecycle(&mut self, lifecycle: &WireTurnLifecycle) {
        let Some(kind) = lifecycle.kind.as_deref() else {
            return;
        };
        if !is_compaction_kind(kind) || !is_text_projectable_kind(kind) {
            return;
        }
        let summary = lifecycle.summary.as_deref().unwrap_or_default().trim();
        if summary.is_empty() {
            return;
        }
        self.write_standalone_line(&format!("{COMPACTION_TEXT_PREFIX}{summary}"));
    }

    /// 独立整行输出；必要时先闭合上一行未结束的 assistant 正文。
    fn write_standalone_line(&mut self, line: &str) {
        let mut payload = String::new();
        if self.line_open {
            payload.push('\n');
        }
        payload.push_str(line);
        payload.push('\n');
        let Some(writer) = self.writer.as_deref_mut() else {
            return;
        };
        if let Err(error) = writer.write_all(payload.as_bytes()) {
            self.error
                .get_or_insert_with(|| format!("实时压缩投影输出失败：{error}"));
            return;
        }
        if let Err(error) = writer.flush() {
            self.error
                .get_or_insert_with(|| format!("刷新实时压缩投影输出失败：{error}"));
            return;
        }
        self.line_open = false;
    }

    fn observe_assistant_message(&mut self, message: &WireAssistantMessage) {
        if message.content.is_empty() {
            return;
        }
        let message_id = if message.message_id.trim().is_empty() {
            message.part_id.clone()
        } else {
            message.message_id.clone()
        };
        let previous = self
            .assistant_contents
            .get(&message_id)
            .cloned()
            .unwrap_or_else(|| self.latest_content.clone());
        let output = if message.delta {
            let content = self
                .assistant_contents
                .entry(message_id.clone())
                .or_default();
            if self.latest_message_id.as_deref() != Some(message_id.as_str()) {
                content.clear();
            }
            content.push_str(&message.content);
            self.latest_content = content.clone();
            self.latest_message_id = Some(message_id);
            message.content.clone()
        } else {
            let output = if previous == message.content {
                String::new()
            } else if let Some(suffix) = message.content.strip_prefix(&previous) {
                suffix.to_owned()
            } else {
                message.content.clone()
            };
            self.assistant_contents
                .insert(message_id.clone(), message.content.clone());
            self.latest_content = message.content.clone();
            self.latest_message_id = Some(message_id);
            output
        };
        if output.is_empty() {
            return;
        }
        let Some(writer) = self.writer.as_deref_mut() else {
            return;
        };
        if let Err(error) = writer.write_all(output.as_bytes()) {
            self.error
                .get_or_insert_with(|| format!("实时 assistant 输出失败：{error}"));
            return;
        }
        if let Err(error) = writer.flush() {
            self.error
                .get_or_insert_with(|| format!("刷新实时 assistant 输出失败：{error}"));
            return;
        }
        self.line_open = !output.ends_with('\n');
        self.emitted = true;
    }

    fn finish(&mut self) {
        if !self.line_open {
            return;
        }
        let Some(writer) = self.writer.as_deref_mut() else {
            return;
        };
        if let Err(error) = writer.write_all(LIVE_OUTPUT_NEWLINE) {
            self.error
                .get_or_insert_with(|| format!("结束实时 assistant 输出失败：{error}"));
            return;
        }
        if let Err(error) = writer.flush() {
            self.error
                .get_or_insert_with(|| format!("刷新实时 assistant 输出失败：{error}"));
        }
    }
}

#[cfg(test)]
#[path = "daemon_interaction_client_tests.rs"]
mod tests;
