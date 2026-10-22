use std::{fmt::Write as _, path::Path};

#[cfg(feature = "tui")]
use axon_tui_runtime::AgentPaneTurnEventStoreRuntime;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

/// `axon debug session replay` 参数。
#[derive(Debug, Clone, clap::Args, PartialEq, Eq)]
pub struct SessionReplayCommand {
    /// 起始会话 ID；沿 parent 回溯整条 resume 链。
    pub session_id: String,
    /// 显式读取指定 provider tool-call 的完整归档输出。
    #[arg(long)]
    pub provider_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionReplayReport {
    status: &'static str,
    session_id: String,
    chain_len: usize,
    chain: Vec<crate::run_commands::RolloutReplayView>,
    event_count: usize,
    event_types: Vec<String>,
    event_summaries: Vec<SessionReplayEventSummary>,
    optional_ui_turn_event_count: usize,
    tool_output: Option<SessionReplayToolOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionReplayToolOutput {
    provider_call_id: String,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionReplayEventSummary {
    sequence: u32,
    event_type: String,
    recorded_at_ms: u64,
    source: Option<String>,
    turn_event_count: usize,
    tool_receipt_count: usize,
    memory_write_status: Option<String>,
}

pub(super) fn run_session_replay(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionReplayCommand,
) -> CliRunResult {
    match build_session_replay_report(config_path, command) {
        Ok(report) => render_replay(format, &report),
        Err(error) => cli_error(error),
    }
}

fn build_session_replay_report(
    config_path: &Path,
    command: &SessionReplayCommand,
) -> Result<SessionReplayReport, String> {
    validate_session_replay_command(command)?;
    let events = crate::run_commands::replay_rollout_events(config_path, &command.session_id)
        .unwrap_or_default();
    let chain = replay_chain_or_events(config_path, &command.session_id, !events.is_empty())?;
    let event_types = events
        .iter()
        .map(|event| format!("{:?}", event.event_type))
        .collect();
    let event_summaries = events.iter().map(session_replay_event_summary).collect();
    Ok(SessionReplayReport {
        status: "session_replayed",
        session_id: command.session_id.clone(),
        chain_len: chain.len(),
        chain,
        event_count: events.len(),
        event_types,
        event_summaries,
        optional_ui_turn_event_count: read_optional_ui_turn_events(config_path),
        tool_output: read_requested_tool_output(config_path, command)?,
    })
}

fn validate_session_replay_command(command: &SessionReplayCommand) -> Result<(), String> {
    if command.session_id.trim().is_empty() {
        return Err("session id 不能为空".to_owned());
    }
    if command
        .provider_call_id
        .as_deref()
        .is_some_and(|provider_call_id| provider_call_id.trim().is_empty())
    {
        return Err("provider call id 不能为空".to_owned());
    }
    Ok(())
}

fn replay_chain_or_events(
    config_path: &Path,
    session_id: &str,
    has_events: bool,
) -> Result<Vec<crate::run_commands::RolloutReplayView>, String> {
    match crate::run_commands::replay_session_chain(config_path, session_id) {
        Ok(chain) => Ok(chain),
        Err(_) if has_events => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_requested_tool_output(
    config_path: &Path,
    command: &SessionReplayCommand,
) -> Result<Option<SessionReplayToolOutput>, String> {
    let Some(provider_call_id) = command.provider_call_id.as_deref() else {
        return Ok(None);
    };
    let bytes = crate::run_commands::read_rollout_tool_output(
        config_path,
        &command.session_id,
        provider_call_id,
    )?;
    let content = String::from_utf8(bytes)
        .map_err(|_| "工具归档不是 UTF-8 文本，无法通过 replay 渲染".to_owned())?;
    Ok(Some(SessionReplayToolOutput {
        provider_call_id: provider_call_id.to_owned(),
        content,
    }))
}

fn render_replay(format: OutputFormat, report: &SessionReplayReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "resume 链 {}（共 {} 段）：",
                report.session_id, report.chain_len
            );
            if report.event_count > 0 {
                render_rollout_events(&mut stdout, report);
            }
            if let Some(tool_output) = report.tool_output.as_ref() {
                let _ = writeln!(stdout, "工具输出 {}：", tool_output.provider_call_id);
                let _ = writeln!(stdout, "{}", tool_output.content);
            }
            for view in &report.chain {
                let _ = writeln!(
                    stdout,
                    "  #{} {}\t{}\t{}\tprompt={}字\tresponse={}字",
                    view.depth,
                    view.session_id,
                    view.provider,
                    view.model,
                    view.prompt_chars,
                    view.response_chars
                );
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

fn render_rollout_events(stdout: &mut String, report: &SessionReplayReport) {
    let _ = writeln!(
        stdout,
        "事件级 rollout：{} 条（{}）",
        report.event_count,
        report.event_types.join(", ")
    );
    for event in &report.event_summaries {
        let _ = writeln!(
            stdout,
            "  - seq={} type={} source={} turn_events={}",
            event.sequence,
            event.event_type,
            event.source.as_deref().unwrap_or("-"),
            event.turn_event_count
        );
        if event.tool_receipt_count > 0 {
            let _ = writeln!(stdout, "    tool_receipts={}", event.tool_receipt_count);
        }
        if let Some(memory_write_status) = event.memory_write_status.as_deref() {
            let _ = writeln!(stdout, "    memory_write_status={memory_write_status}");
        }
    }
}

fn session_replay_event_summary(
    event: &crate::run_commands::RolloutEventRecord,
) -> SessionReplayEventSummary {
    let source = event
        .payload
        .get("source")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let turn_event_count = event
        .payload
        .get("turnEventCount")
        .and_then(serde_json::Value::as_u64)
        .map(|value| usize::try_from(value).unwrap_or(usize::MAX))
        .or_else(|| {
            event
                .payload
                .get("turnEvents")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0);
    let tool_receipt_count = event
        .payload
        .get("toolReceiptCount")
        .and_then(serde_json::Value::as_u64)
        .map(|value| usize::try_from(value).unwrap_or(usize::MAX))
        .or_else(|| {
            event
                .payload
                .get("toolReceipts")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0);
    let memory_write_status = event
        .payload
        .get("writeDecision")
        .or_else(|| event.payload.get("status"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    SessionReplayEventSummary {
        sequence: event.sequence,
        event_type: format!("{:?}", event.event_type),
        recorded_at_ms: event.recorded_at_ms,
        source,
        turn_event_count,
        tool_receipt_count,
        memory_write_status,
    }
}

fn read_optional_ui_turn_events(config_path: &Path) -> usize {
    #[cfg(not(feature = "tui"))]
    {
        let _ = config_path;
        0
    }
    #[cfg(feature = "tui")]
    {
        let Ok(axon_root) = crate::runtime_paths::runtime_axon_root_path(config_path) else {
            return 0;
        };
        AgentPaneTurnEventStoreRuntime::read_records(
            AgentPaneTurnEventStoreRuntime::resolve_store_path(axon_root),
        )
        .map_or(0, |records| {
            records.iter().map(|record| record.turn_events.len()).sum()
        })
    }
}
