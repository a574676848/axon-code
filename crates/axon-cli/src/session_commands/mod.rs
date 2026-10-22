use std::collections::BTreeSet;
use std::path::Path;

use axon_crypto::sha256_hex;
use clap::{Args, Subcommand};
use serde::Serialize;

use axon_session::{
    JsonlThreadEventStore, ThreadEvent, ThreadEventReadIssue, ThreadEventStore as _,
    ThreadListQuery, ThreadService,
};

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result, resolved_config_path},
    runtime_paths::runtime_axon_root_path,
};

mod output;
mod replay;
mod thread_cli;

use output::{SessionOutputCommand, run_session_output};
use replay::{SessionReplayCommand, run_session_replay};
pub(crate) use thread_cli::session_summary_views_for_root;
use thread_cli::{
    SessionExportCommand, SessionForkCommand, SessionListCommand, SessionReadCommand,
    SessionResumeCommand, SessionShowCommand, run_session_export, run_session_fork,
    run_session_list, run_session_read, run_session_resume, run_session_show,
};

const SESSION_CLI_DEFAULT_LIMIT: usize = 20;
const SESSION_CLI_MAX_LIMIT: usize = 100;
const SESSION_DIAGNOSTIC_ISSUE_PREVIEW_LIMIT: usize = 3;

/// `axon debug session` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionCommand {
    /// session 子命令。
    #[command(subcommand)]
    pub command: Option<SessionSubcommand>,
}

/// `axon debug session` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum SessionSubcommand {
    /// 列出统一 Thread 会话。
    List(SessionListCommand),
    /// 查看单个 Thread 摘要。
    Show(SessionShowCommand),
    /// 读取单个 Thread 详情。
    Read(SessionReadCommand),
    /// 恢复 Thread，让后续 run 继续追加到同一会话。
    Resume(SessionResumeCommand),
    /// Fork 一个新 Thread。
    Fork(SessionForkCommand),
    /// 删除旧 run 会话记录。
    Delete(SessionDeleteCommand),
    /// 导出单个会话的 ref-only 摘要。
    Export(SessionExportCommand),
    /// 回放 resume 链：沿 parent 回溯并按时间正序输出整条会话链。
    Replay(SessionReplayCommand),
    /// 读取 tool output 归档内容。
    Output(SessionOutputCommand),
    /// 只读诊断 Thread 事件流与会话树。
    Doctor(SessionDoctorCommand),
}

/// `axon debug session doctor` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionDoctorCommand {
    /// 显式请求修复；当前诊断命令默认只读，避免隐式改写事件流。
    #[arg(long)]
    pub repair: bool,
}

/// `axon debug session delete` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionDeleteCommand {
    /// 会话 ID。
    pub session_id: String,
}

pub(crate) fn run_session(
    command: &SessionCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let subcommand =
        command
            .command
            .as_ref()
            .unwrap_or(&SessionSubcommand::List(SessionListCommand {
                limit: Some(SESSION_CLI_DEFAULT_LIMIT),
                cursor: None,
                cwd: None,
                archived: false,
            }));
    match subcommand {
        SessionSubcommand::List(command) => run_session_list(format, &path, command),
        SessionSubcommand::Show(command) => run_session_show(format, &path, command),
        SessionSubcommand::Read(command) => run_session_read(format, &path, command),
        SessionSubcommand::Resume(command) => run_session_resume(format, &path, command),
        SessionSubcommand::Fork(command) => run_session_fork(format, &path, command),
        SessionSubcommand::Delete(command) => run_session_delete(format, &path, command),
        SessionSubcommand::Export(command) => run_session_export(format, &path, command),
        SessionSubcommand::Replay(command) => run_session_replay(format, &path, command),
        SessionSubcommand::Output(command) => run_session_output(format, command),
        SessionSubcommand::Doctor(command) => run_session_doctor(format, &path, command),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDeleteReport {
    status: &'static str,
    store_path: String,
    session_id: String,
    deleted_events: usize,
    remaining_events: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDoctorReport {
    status: &'static str,
    store_path: String,
    event_count: usize,
    thread_count: usize,
    unreadable_thread_shards: usize,
    orphan_parent_count: usize,
    orphan_fork_count: usize,
    repair_requested: bool,
    repair_performed: bool,
    raw_prompt_exported: bool,
    warnings: Vec<String>,
    checks: Vec<SessionDoctorCheckView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDoctorCheckView {
    name: &'static str,
    status: &'static str,
    summary: String,
}

fn run_session_delete(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionDeleteCommand,
) -> CliRunResult {
    if command.session_id.trim().is_empty() {
        return cli_error("session id 不能为空");
    }
    let root = match runtime_axon_root_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let store = JsonlThreadEventStore;
    let records = match store.read_all(&root) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let before = records.len();
    let kept = records
        .into_iter()
        .filter(|record| record.thread_id != command.session_id)
        .collect::<Vec<_>>();
    let deleted_events = before.saturating_sub(kept.len());
    if deleted_events == 0 {
        return cli_error(format!("未找到会话：{}", command.session_id));
    }
    if let Err(error) =
        JsonlThreadEventStore.rewrite_thread_with_records(&root, &command.session_id, &kept)
    {
        return cli_error(error);
    }
    let report = SessionDeleteReport {
        status: "session_deleted",
        store_path: display_path(&JsonlThreadEventStore::path(&root)),
        session_id: command.session_id.clone(),
        deleted_events,
        remaining_events: kept.len(),
    };
    render_delete(format, &report)
}

fn run_session_doctor(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionDoctorCommand,
) -> CliRunResult {
    if command.repair {
        return cli_error("session doctor --repair 当前未启用；请先运行只读诊断并人工确认");
    }
    let root = match runtime_axon_root_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let store = JsonlThreadEventStore;
    let scan = match store.scan_all(&root) {
        Ok(scan) => scan,
        Err(error) => return cli_error(error),
    };
    let projection = match ThreadService::new(JsonlThreadEventStore).list_thread_page_from_records(
        &scan.records,
        &ThreadListQuery {
            limit: SESSION_CLI_MAX_LIMIT,
            ..ThreadListQuery::default()
        },
    ) {
        Ok(page) => page.threads,
        Err(error) => return cli_error(error),
    };
    let records = &scan.records;
    let started = records
        .iter()
        .filter_map(|record| match record.event {
            ThreadEvent::ThreadStarted { .. } => Some(record.thread_id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let orphan_parent_count = records
        .iter()
        .filter(|record| match &record.event {
            ThreadEvent::ThreadStarted {
                parent_thread_id: Some(parent),
                ..
            } => !started.contains(parent.as_str()),
            _ => false,
        })
        .count();
    let orphan_fork_count = records
        .iter()
        .filter(|record| match &record.event {
            ThreadEvent::ThreadStarted {
                forked_from_thread_id: Some(source),
                ..
            } => !started.contains(source.as_str()),
            _ => false,
        })
        .count();
    let warnings = thread_event_issue_summaries(&scan.issues);
    let event_store_ok = scan.issues.is_empty();
    let mut checks = vec![
        SessionDoctorCheckView {
            name: "event_store",
            status: if event_store_ok { "ok" } else { "failed" },
            summary: format!(
                "可读 records={} unreadable_shards={}",
                records.len(),
                scan.issues.len()
            ),
        },
        SessionDoctorCheckView {
            name: "projection",
            status: "ok",
            summary: format!("可读 shard 投影可重建，threads={}", projection.len()),
        },
    ];
    checks.push(SessionDoctorCheckView {
        name: "thread_tree",
        status: if orphan_parent_count == 0 && orphan_fork_count == 0 {
            "ok"
        } else {
            "failed"
        },
        summary: format!("orphan_parent={orphan_parent_count} orphan_fork={orphan_fork_count}"),
    });
    let failed = checks.iter().any(|check| check.status != "ok");
    let report = SessionDoctorReport {
        status: if failed {
            "session_doctor_failed"
        } else {
            "session_doctor_ok"
        },
        store_path: display_path(&JsonlThreadEventStore::path(&root)),
        event_count: records.len(),
        thread_count: projection.len(),
        unreadable_thread_shards: scan.issues.len(),
        orphan_parent_count,
        orphan_fork_count,
        repair_requested: command.repair,
        repair_performed: false,
        raw_prompt_exported: false,
        warnings,
        checks,
    };
    render_doctor(format, &report)
}

pub(super) fn thread_event_issue_summaries(issues: &[ThreadEventReadIssue]) -> Vec<String> {
    let mut warnings = issues
        .iter()
        .take(SESSION_DIAGNOSTIC_ISSUE_PREVIEW_LIMIT)
        .map(|issue| format!("thread shard {}: {}", issue.shard, issue.error))
        .collect::<Vec<_>>();
    let remaining = issues
        .len()
        .saturating_sub(SESSION_DIAGNOSTIC_ISSUE_PREVIEW_LIMIT);
    if remaining > 0 {
        warnings.push(format!("另有 {remaining} 个不可读 thread shard"));
    }
    warnings
}

fn render_delete(format: OutputFormat, report: &SessionDeleteReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "已删除会话: {}\n- deleted_events: {}\n- remaining_events: {}\n",
                report.session_id, report.deleted_events, report.remaining_events
            ),
            stderr: String::new(),
        },
    }
}

fn render_doctor(format: OutputFormat, report: &SessionDoctorReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = format!("Axon session doctor: {}\n", report.status);
            for check in &report.checks {
                let _ = std::fmt::Write::write_fmt(
                    &mut stdout,
                    format_args!("- {}: {} {}\n", check.name, check.status, check.summary),
                );
            }
            CliRunResult {
                exit_code: if report.status == "session_doctor_ok" {
                    0
                } else {
                    2
                },
                stdout,
                stderr: String::new(),
            }
        }
    }
}

pub(crate) fn bounded_session_limit(limit: Option<usize>) -> Result<usize, &'static str> {
    let limit = limit.unwrap_or(SESSION_CLI_DEFAULT_LIMIT);
    if limit == 0 || limit > SESSION_CLI_MAX_LIMIT {
        return Err("session limit 必须在 1..=100 范围内");
    }
    Ok(limit)
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}
