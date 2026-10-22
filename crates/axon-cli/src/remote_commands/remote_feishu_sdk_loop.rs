use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use axon_domain::SessionId;
use axon_jsonl::ShardedJsonlStore;
use axon_remote_feishu::{
    REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL, RemoteFeishuGatewayEventSourceWriterRuntime,
    RemoteFeishuGatewaySdkLoopAdapterRuntime, RemoteFeishuGatewaySdkLoopRunnerInput,
    RemoteFeishuGatewaySdkPayloadTextContext,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CliRunResult, OutputFormat, management_commands::cli_error};

use super::{RemoteCliContext, RemoteFeishuSdkLoopCommand, now_ms};

#[path = "remote_feishu_sdk_loop_callback.rs"]
mod remote_feishu_sdk_loop_callback;
#[path = "remote_feishu_sdk_loop_counters.rs"]
mod remote_feishu_sdk_loop_counters;
#[path = "remote_feishu_sdk_loop_payload.rs"]
mod remote_feishu_sdk_loop_payload;
#[path = "remote_feishu_sdk_loop_real.rs"]
mod remote_feishu_sdk_loop_real;

use remote_feishu_sdk_loop_payload::map_payload_lines;
use remote_feishu_sdk_loop_real::execute_real_sdk_loop;

const REMOTE_FEISHU_SDK_LOOP_CLIENT_ID: &str = "feishu-sdk-loop";
const REMOTE_FEISHU_SDK_LOOP_DEFAULT_INTERVAL_MS: u64 = 1_000;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuSdkLoopDaemonReport {
    pub status: &'static str,
    pub session_id: u64,
    pub transport: &'static str,
    pub daemon: bool,
    pub sdk_connection_mode: &'static str,
    pub sdk_connected: bool,
    pub websocket_url: String,
    pub app_id_configured: bool,
    pub app_secret_env_configured: bool,
    pub payload_source: Option<String>,
    pub event_source_file: String,
    pub interval_ms: u64,
    pub max_ticks: Option<usize>,
    pub max_events: Option<usize>,
    pub wait_ms: Option<u64>,
    pub stop_after_first_event: bool,
    pub completed_ticks: usize,
    pub empty_ticks: usize,
    pub acked_offset: u64,
    pub cursor_file: String,
    pub sdk_received_binary_frames: usize,
    pub sdk_received_data_frames: usize,
    pub sdk_skipped_data_frames: usize,
    pub input_payloads: usize,
    pub mapped_events: usize,
    pub appended_events: usize,
    pub sdk_sent_pings: usize,
    pub sdk_received_pongs: usize,
    pub received_acked_events: usize,
    pub received_ack_failed_events: usize,
    pub closure_gateway_input_events: usize,
    pub closure_agent_executed_events: usize,
    pub closure_reply_enqueued_events: usize,
    pub closure_session_reset_events: usize,
    pub closure_worker_delivered_count: usize,
    pub closure_feedback_completed_count: usize,
    pub closure_failed_events: usize,
    pub sdk_reconnect_attempts: usize,
    pub sdk_loop_errors: usize,
    pub network_send_allowed: bool,
    pub raw_payload_forward_allowed: bool,
    pub raw_payload_persisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteFeishuSdkLoopPayloadLine {
    kind: String,
    payload: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct RemoteFeishuSdkLoopCursorState {
    acked_offset: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PayloadFileSdkLoopStats {
    completed_ticks: usize,
    empty_ticks: usize,
    input_payloads: usize,
    mapped_events: usize,
    appended_events: usize,
}

struct PayloadFileSdkLoopReportInput<'a> {
    context: &'a RemoteCliContext,
    command: &'a RemoteFeishuSdkLoopCommand,
    event_source_file: &'a Path,
    interval_ms: u64,
    max_events: Option<usize>,
    payload_file: &'a Path,
    cursor_file: &'a Path,
    cursor: &'a RemoteFeishuSdkLoopCursorState,
    stats: PayloadFileSdkLoopStats,
}

pub(crate) fn run_remote_execute_feishu_sdk_loop(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteFeishuSdkLoopCommand,
) -> CliRunResult {
    run_remote_execute_feishu_sdk_loop_inner(format, context, command, None)
}

pub(crate) fn run_remote_execute_feishu_sdk_loop_with_cancellation(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteFeishuSdkLoopCommand,
    cancellation: Arc<std::sync::atomic::AtomicBool>,
) -> CliRunResult {
    run_remote_execute_feishu_sdk_loop_inner(format, context, command, Some(cancellation))
}

fn run_remote_execute_feishu_sdk_loop_inner(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteFeishuSdkLoopCommand,
    cancellation: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> CliRunResult {
    let report = match execute_feishu_sdk_loop_daemon(context, command, cancellation) {
        Ok(report) => report,
        Err(error) => return cli_error(error),
    };
    super::remote_render::render_remote_feishu_sdk_loop_daemon(format, &report)
}

fn execute_feishu_sdk_loop_daemon(
    context: &RemoteCliContext,
    command: &RemoteFeishuSdkLoopCommand,
    cancellation: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<RemoteFeishuSdkLoopDaemonReport, String> {
    validate_sdk_loop_config(context)?;
    let interval_ms = command
        .interval_ms
        .unwrap_or(REMOTE_FEISHU_SDK_LOOP_DEFAULT_INTERVAL_MS);
    if interval_ms == 0 {
        return Err("Feishu SDK loop daemon interval-ms 必须大于 0".to_owned());
    }
    let max_events = effective_max_events(command)?;
    let event_source_file = command.event_source_file.clone().unwrap_or_else(|| {
        RemoteFeishuGatewayEventSourceWriterRuntime::resolve_event_source_path(&context.root)
    });
    if command.connect_sdk {
        return execute_real_sdk_loop(
            context,
            command,
            &event_source_file,
            interval_ms,
            max_events,
            cancellation,
        );
    }
    let payload_file = command.payload_file.as_ref().ok_or_else(|| {
        "Feishu SDK loop daemon 未启用 --connect-sdk 时需要 --payload-file".to_owned()
    })?;
    let cursor_file = resolve_cursor_file(command, payload_file);
    let mut cursor = read_cursor_state(&cursor_file)?;
    let text_context = sdk_payload_text_context(context);
    let mut stats = PayloadFileSdkLoopStats::default();
    loop {
        if command
            .max_ticks
            .is_some_and(|max_ticks| stats.completed_ticks >= max_ticks)
        {
            break;
        }
        let (payloads, next_acked_offset) =
            read_payload_lines_after_cursor(payload_file, cursor.acked_offset)?;
        if payloads.is_empty() {
            if next_acked_offset != cursor.acked_offset {
                cursor.acked_offset = next_acked_offset;
                write_cursor_state(&cursor_file, &cursor)?;
            }
            stats.empty_ticks += 1;
            stats.completed_ticks += 1;
            sleep_if_continuous(command.max_ticks, interval_ms);
            continue;
        }
        let events = map_payload_lines(payloads, text_context.as_ref())?;
        let tick_mapped_events = events.len();
        let report = RemoteFeishuGatewaySdkLoopAdapterRuntime::run_events(
            RemoteFeishuGatewaySdkLoopRunnerInput {
                event_source_path: event_source_file.clone(),
                session_id: SessionId(command.session_id),
                client_id: REMOTE_FEISHU_SDK_LOOP_CLIENT_ID.to_owned(),
                enqueued_at_ms: now_ms(),
                events,
                media_download: None,
            },
        )
        .map_err(|error| format!("{error:?}"))?;
        cursor.acked_offset = next_acked_offset;
        write_cursor_state(&cursor_file, &cursor)?;
        stats.completed_ticks += 1;
        stats.input_payloads += report.input_events;
        stats.mapped_events += tick_mapped_events;
        stats.appended_events += report.appended_events;
        sleep_if_continuous(command.max_ticks, interval_ms);
    }
    Ok(payload_file_sdk_loop_report(
        &PayloadFileSdkLoopReportInput {
            context,
            command,
            event_source_file: &event_source_file,
            interval_ms,
            max_events,
            payload_file,
            cursor_file: &cursor_file,
            cursor: &cursor,
            stats,
        },
    ))
}

fn payload_file_sdk_loop_report(
    input: &PayloadFileSdkLoopReportInput<'_>,
) -> RemoteFeishuSdkLoopDaemonReport {
    RemoteFeishuSdkLoopDaemonReport {
        status: "remote_feishu_sdk_loop_daemon_ready",
        session_id: input.command.session_id,
        transport: "feishu",
        daemon: true,
        sdk_connection_mode: "payloadFileCallbackSource",
        sdk_connected: true,
        websocket_url: websocket_url(input.context),
        app_id_configured: !input.context.feishu_websocket.app_id.trim().is_empty(),
        app_secret_env_configured: !input
            .context
            .feishu_websocket
            .app_secret_env
            .trim()
            .is_empty(),
        payload_source: Some(input.payload_file.to_string_lossy().to_string()),
        event_source_file: input.event_source_file.to_string_lossy().to_string(),
        interval_ms: input.interval_ms,
        max_ticks: input.command.max_ticks,
        max_events: input.max_events,
        wait_ms: input.command.wait_ms,
        stop_after_first_event: input.command.stop_after_first_event,
        completed_ticks: input.stats.completed_ticks,
        empty_ticks: input.stats.empty_ticks,
        acked_offset: input.cursor.acked_offset,
        cursor_file: input.cursor_file.to_string_lossy().to_string(),
        sdk_received_binary_frames: 0,
        sdk_received_data_frames: 0,
        sdk_skipped_data_frames: 0,
        input_payloads: input.stats.input_payloads,
        mapped_events: input.stats.mapped_events,
        appended_events: input.stats.appended_events,
        sdk_sent_pings: 0,
        sdk_received_pongs: 0,
        received_acked_events: 0,
        received_ack_failed_events: 0,
        closure_gateway_input_events: 0,
        closure_agent_executed_events: 0,
        closure_reply_enqueued_events: 0,
        closure_session_reset_events: 0,
        closure_worker_delivered_count: 0,
        closure_feedback_completed_count: 0,
        closure_failed_events: 0,
        sdk_reconnect_attempts: 0,
        sdk_loop_errors: 0,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        raw_payload_persisted: false,
    }
}

fn validate_sdk_loop_config(context: &RemoteCliContext) -> Result<(), String> {
    if !context.feishu_websocket.enabled {
        return Err("Feishu SDK loop daemon 需要启用 remote.feishu_websocket.enabled".to_owned());
    }
    if context.feishu_websocket.app_id.trim().is_empty() {
        return Err("Feishu SDK loop daemon 缺少 remote.feishu_websocket.app_id".to_owned());
    }
    if context.feishu_websocket.app_secret_env.trim().is_empty() {
        return Err(
            "Feishu SDK loop daemon 缺少 remote.feishu_websocket.app_secret_env".to_owned(),
        );
    }
    Ok(())
}

fn effective_max_events(command: &RemoteFeishuSdkLoopCommand) -> Result<Option<usize>, String> {
    if command.max_events == Some(0) {
        return Err("Feishu SDK loop daemon max-events 必须大于 0".to_owned());
    }
    Ok(command.max_events)
}

fn read_payload_lines_after_cursor(
    path: &Path,
    acked_offset: u64,
) -> Result<(Vec<RemoteFeishuSdkLoopPayloadLine>, u64), String> {
    if acked_offset == 0 && !path.parent().is_some_and(|parent| parent.exists()) {
        return Ok((Vec::new(), 0));
    }
    let mut payloads = Vec::new();
    let lines = ShardedJsonlStore::new()
        .read_repaired_lines(path)
        .map_err(|error| format!("读取 Feishu SDK payload 文件失败: {error}"))?;
    if acked_offset > lines.len() as u64 {
        return Err("Feishu SDK cursor 行偏移超出 payload 长度".to_owned());
    }
    for (index, line) in lines
        .iter()
        .enumerate()
        .skip(usize::try_from(acked_offset).unwrap_or(usize::MAX))
    {
        let text = std::str::from_utf8(line)
            .map_err(|error| format!("Feishu SDK payload JSONL UTF-8 无效: {error}"))?;
        let payload = serde_json::from_str::<RemoteFeishuSdkLoopPayloadLine>(text)
            .map_err(|error| format!("Feishu SDK payload JSONL 行 {} 无效: {error}", index + 1))?;
        payloads.push(payload);
    }
    Ok((payloads, lines.len() as u64))
}

fn sdk_payload_text_context(
    context: &RemoteCliContext,
) -> Option<RemoteFeishuGatewaySdkPayloadTextContext> {
    let identity = super::remote_feishu_identity::resolve_feishu_bot_identity(context)?;
    Some(RemoteFeishuGatewaySdkPayloadTextContext::new(
        Some(identity.bot_open_id),
        Some(identity.bot_name),
    ))
}

fn sleep_if_continuous(max_ticks: Option<usize>, interval_ms: u64) {
    if max_ticks.is_none() {
        thread::sleep(Duration::from_millis(interval_ms));
    }
}

fn resolve_cursor_file(command: &RemoteFeishuSdkLoopCommand, payload_file: &Path) -> PathBuf {
    command
        .cursor_file
        .clone()
        .unwrap_or_else(|| payload_file.with_extension("cursor.json"))
}

fn resolve_app_secret(
    context: &RemoteCliContext,
    command: &RemoteFeishuSdkLoopCommand,
) -> Result<String, String> {
    if let Some(secret) = command
        .app_secret
        .as_ref()
        .filter(|secret| !secret.trim().is_empty())
    {
        return Ok(secret.clone());
    }
    context
        .feishu_app_secret()
        .ok_or_else(|| "Feishu SDK loop daemon 无法读取 app_secret 凭证".to_owned())
}

fn feishu_api_base_url_owned(context: &RemoteCliContext) -> Option<String> {
    (!context.feishu_websocket.api_base_url.trim().is_empty())
        .then_some(context.feishu_websocket.api_base_url.clone())
}

fn read_cursor_state(path: &PathBuf) -> Result<RemoteFeishuSdkLoopCursorState, String> {
    if !path.exists() {
        return Ok(RemoteFeishuSdkLoopCursorState::default());
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("读取 Feishu SDK loop cursor 失败: {error}"))?;
    serde_json::from_str::<RemoteFeishuSdkLoopCursorState>(&content)
        .map_err(|error| format!("Feishu SDK loop cursor JSON 无效: {error}"))
}

fn write_cursor_state(path: &Path, cursor: &RemoteFeishuSdkLoopCursorState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建 Feishu SDK loop cursor 目录失败: {error}"))?;
    }
    let content = serde_json::to_vec(cursor)
        .map_err(|error| format!("序列化 Feishu SDK loop cursor 失败: {error}"))?;
    let temporary_path = path.with_extension("tmp");
    let mut temporary = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary_path)
        .map_err(|error| format!("打开 Feishu SDK loop cursor 临时文件失败: {error}"))?;
    temporary
        .write_all(&content)
        .and_then(|()| temporary.sync_all())
        .map_err(|error| format!("写入 Feishu SDK loop cursor 临时文件失败: {error}"))?;
    drop(temporary);
    fs::rename(&temporary_path, path)
        .map_err(|error| format!("替换 Feishu SDK loop cursor 失败: {error}"))
}

fn websocket_url(context: &RemoteCliContext) -> String {
    if context.feishu_websocket.websocket_url.trim().is_empty() {
        REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL.to_owned()
    } else {
        context.feishu_websocket.websocket_url.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_cursor_reads_only_complete_records_after_line_offset() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("payloads.jsonl");
        let first = "{\"kind\":\"message\",\"payload\":{\"id\":1}}\n";
        let partial_second = "{\"kind\":\"message\",\"payload\":{\"id\":2}}";
        fs::write(
            ShardedJsonlStore::physical_shard_path(&path, 0),
            format!("{first}{partial_second}"),
        )
        .expect("写入 payload 文件");

        let (first_payloads, first_offset) =
            read_payload_lines_after_cursor(&path, 0).expect("读取完整 payload");

        assert_eq!(first_payloads.len(), 1);
        assert_eq!(first_payloads[0].payload["id"], 1);
        assert_eq!(first_offset, 1_u64);

        fs::write(
            ShardedJsonlStore::physical_shard_path(&path, 0),
            format!("{first}{{\"kind\":\"message\",\"payload\":{{\"id\":2}}}}\n"),
        )
        .expect("完成 payload 写入");
        let (second_payloads, second_offset) =
            read_payload_lines_after_cursor(&path, first_offset).expect("读取后续 payload");

        assert_eq!(second_payloads.len(), 1);
        assert_eq!(second_payloads[0].payload["id"], 2);
        assert_eq!(second_offset, 2_u64);
    }

    #[test]
    fn payload_cursor_rejects_obsolete_line_cursor() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("payload.cursor.json");
        fs::write(&path, r#"{"ackedLine":1}"#).expect("写入旧 cursor");

        let error = read_cursor_state(&path).expect_err("旧行号 cursor 必须被拒绝");

        assert!(error.starts_with("Feishu SDK loop cursor JSON 无效"));
    }
}
