use std::{fmt::Write as _, path::Path};

#[cfg(feature = "tui")]
use axon_tui_runtime::AgentPaneTurnEventStoreRuntime;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
    run_commands::run_thread_replay::{
        ReplayChainView, SessionTurnReplay, read_task_output, replay_session_chain,
        replay_session_turns,
    },
};

use super::resolve_session_target;

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
    chain: Vec<ReplayChainView>,
    turn_count: usize,
    turns: Vec<SessionTurnReplay>,
    optional_ui_turn_event_count: usize,
    tool_output: Option<SessionReplayToolOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionReplayToolOutput {
    provider_call_id: String,
    content: String,
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
    // 入参可能是会话清单里的车道 ID：先解析成 Thread，再回溯 parent 链表。
    let target = resolve_session_target(config_path, &command.session_id)?;
    let chain = replay_session_chain(config_path, &target.thread_id)?;
    let turns = replay_session_turns(config_path, &target.thread_id).unwrap_or_default();
    Ok(SessionReplayReport {
        status: "session_replayed",
        session_id: target.thread_id,
        chain_len: chain.len(),
        chain,
        turn_count: turns.len(),
        turns,
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

fn read_requested_tool_output(
    config_path: &Path,
    command: &SessionReplayCommand,
) -> Result<Option<SessionReplayToolOutput>, String> {
    let Some(provider_call_id) = command.provider_call_id.as_deref() else {
        return Ok(None);
    };
    let bytes = read_task_output(config_path, &command.session_id, provider_call_id)?;
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
            if report.turn_count > 0 {
                render_turns(&mut stdout, report);
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

fn render_turns(stdout: &mut String, report: &SessionReplayReport) {
    let _ = writeln!(stdout, "轮次回放：{} 轮", report.turn_count);
    for turn in &report.turns {
        let _ = writeln!(
            stdout,
            "  - seq={} status={} at={} turn_events={} tool_receipts={}",
            turn.sequence,
            turn.status,
            turn.recorded_at_ms,
            turn.turn_event_count,
            turn.tool_receipt_count
        );
        if let Some(notice) = turn.reflection_notice.as_deref() {
            let _ = writeln!(stdout, "    reflection_notice={notice}");
        }
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
