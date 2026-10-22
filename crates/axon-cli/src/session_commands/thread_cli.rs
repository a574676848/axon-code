use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use axon_session::{
    ForkThreadCommand, JsonlThreadEventStore, ResumeThreadCommand, ThreadDetail, ThreadHistoryItem,
    ThreadListQuery, ThreadService, ThreadSummary, Turn,
};
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{bounded_session_limit, display_path, thread_event_issue_summaries};
use crate::runtime_paths::runtime_axon_root_path;

const SESSION_READ_PREVIEW_CHARS: usize = 240;

/// `axon debug session list` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionListCommand {
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
    /// 分页游标。
    #[arg(long)]
    pub cursor: Option<String>,
    /// 按工作目录过滤。
    #[arg(long)]
    pub cwd: Option<String>,
    /// 只返回已归档 thread。
    #[arg(long)]
    pub archived: bool,
}

/// `axon debug session show` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionShowCommand {
    /// 会话 ID。
    pub session_id: String,
}

/// `axon debug session read` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionReadCommand {
    /// Thread ID。
    pub thread_id: String,
}

/// `axon debug session resume` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionResumeCommand {
    /// Thread ID。
    pub thread_id: String,
}

/// `axon debug session fork` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionForkCommand {
    /// 来源 Thread ID。
    pub thread_id: String,
    /// 新 Thread ID；省略时自动生成。
    #[arg(long)]
    pub new_thread_id: Option<String>,
    /// 覆盖工作目录。
    #[arg(long)]
    pub cwd: Option<String>,
}

/// `axon debug session export` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionExportCommand {
    /// 会话 ID。
    pub session_id: String,
    /// 输出 JSON 文件；缺省时写 stdout。
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSummaryView {
    pub(crate) thread_id: String,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) forked_from_thread_id: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) cwd: String,
    pub(crate) turn_count: usize,
    pub(crate) prompt_sha256: String,
    pub(crate) response_sha256: String,
    pub(crate) prompt_chars: usize,
    pub(crate) response_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionListReport {
    status: &'static str,
    store_path: String,
    limit: usize,
    cursor: Option<String>,
    next_cursor: Option<String>,
    cwd: Option<String>,
    archived: bool,
    total_records: usize,
    unreadable_thread_shards: usize,
    warnings: Vec<String>,
    sessions: Vec<SessionSummaryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionShowReport {
    status: &'static str,
    store_path: String,
    session: SessionSummaryView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionReadReport {
    status: &'static str,
    store_path: String,
    thread: ThreadDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResumeReport {
    status: &'static str,
    thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionForkReport {
    status: &'static str,
    source_thread_id: String,
    new_thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionExportReport {
    status: &'static str,
    store_path: String,
    output_path: Option<String>,
    session: SessionSummaryView,
    raw_prompt_exported: bool,
    raw_response_exported: bool,
    provider_raw_payload_exported: bool,
}

pub(crate) fn run_session_list(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionListCommand,
) -> CliRunResult {
    let limit = match bounded_session_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let root = match runtime_axon_root_path(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let scan = match JsonlThreadEventStore.scan_all(&root) {
        Ok(scan) => scan,
        Err(error) => return cli_error(error),
    };
    let query = ThreadListQuery {
        limit,
        cursor: command.cursor.clone(),
        cwd: command.cwd.clone(),
        archived: command.archived.then_some(true),
    };
    let page = match thread_service().list_thread_page_from_records(&scan.records, &query) {
        Ok(page) => page,
        Err(error) => return cli_error(error),
    };
    let warnings = thread_event_issue_summaries(&scan.issues);
    let sessions = page.threads.iter().map(summary_view).collect();
    render_list(
        format,
        &SessionListReport {
            status: "session_ready",
            store_path: display_path(&JsonlThreadEventStore::path(&root)),
            limit,
            cursor: command.cursor.clone(),
            next_cursor: page.next_cursor,
            cwd: command.cwd.clone(),
            archived: command.archived,
            total_records: page.total_records,
            unreadable_thread_shards: scan.issues.len(),
            warnings,
            sessions,
        },
    )
}

pub(crate) fn run_session_show(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionShowCommand,
) -> CliRunResult {
    let root = match runtime_axon_root_path(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let detail = match thread_service().read_thread(&root, &command.session_id) {
        Ok(detail) => detail,
        Err(error) => return cli_error(error),
    };
    render_show(
        format,
        &SessionShowReport {
            status: "session_ready",
            store_path: display_path(&JsonlThreadEventStore::path(&root)),
            session: detail_view(&detail),
        },
    )
}

pub(crate) fn run_session_read(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionReadCommand,
) -> CliRunResult {
    let root = match runtime_axon_root_path(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let thread = match thread_service().read_thread(&root, &command.thread_id) {
        Ok(thread) => thread,
        Err(error) => return cli_error(error),
    };
    render_read(
        format,
        &SessionReadReport {
            status: "session_read",
            store_path: display_path(&JsonlThreadEventStore::path(&root)),
            thread,
        },
    )
}

pub(crate) fn run_session_resume(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionResumeCommand,
) -> CliRunResult {
    let root = match runtime_axon_root_path(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    if let Err(error) = thread_service().resume_thread(
        &root,
        &ResumeThreadCommand {
            thread_id: command.thread_id.clone(),
            recorded_at_ms: now_ms(),
        },
    ) {
        return cli_error(error);
    }
    if let Err(error) = crate::run_commands::select_rollout_session(config_path, &command.thread_id)
    {
        return cli_error(error);
    }
    render_resume(
        format,
        &SessionResumeReport {
            status: "session_resumed",
            thread_id: command.thread_id.clone(),
        },
    )
}

pub(crate) fn run_session_fork(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionForkCommand,
) -> CliRunResult {
    let root = match runtime_axon_root_path(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let service = thread_service();
    let source = match service.read_thread(&root, &command.thread_id) {
        Ok(source) => source,
        Err(error) => return cli_error(error),
    };
    let new_thread_id = command
        .new_thread_id
        .clone()
        .unwrap_or_else(|| format!("fork-{}-{}", std::process::id(), now_ms()));
    let mut settings = source.thread.settings.clone();
    if let Some(cwd) = command.cwd.as_deref() {
        cwd.clone_into(&mut settings.cwd);
    }
    if let Err(error) = service.fork_thread(
        &root,
        ForkThreadCommand {
            source_thread_id: command.thread_id.clone(),
            new_thread_id: new_thread_id.clone(),
            settings: Some(settings),
            ephemeral: false,
            recorded_at_ms: now_ms(),
        },
    ) {
        return cli_error(error);
    }
    render_fork(
        format,
        &SessionForkReport {
            status: "session_forked",
            source_thread_id: command.thread_id.clone(),
            new_thread_id,
        },
    )
}

pub(crate) fn run_session_export(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionExportCommand,
) -> CliRunResult {
    let root = match runtime_axon_root_path(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let detail = match thread_service().read_thread(&root, &command.session_id) {
        Ok(detail) => detail,
        Err(error) => return cli_error(error),
    };
    let report = SessionExportReport {
        status: "session_exported",
        store_path: display_path(&JsonlThreadEventStore::path(&root)),
        output_path: command.output.as_deref().map(display_path),
        session: detail_view(&detail),
        raw_prompt_exported: false,
        raw_response_exported: false,
        provider_raw_payload_exported: false,
    };
    if let Some(output) = command.output.as_deref()
        && let Err(error) = write_export_report(output, &report)
    {
        return cli_error(error);
    }
    render_export(format, &report)
}

pub(crate) fn session_summary_views_for_root(
    root: &Path,
    limit: usize,
) -> Result<(usize, Vec<SessionSummaryView>), String> {
    let page = thread_service().list_thread_page(
        root,
        &ThreadListQuery {
            limit,
            ..ThreadListQuery::default()
        },
    )?;
    let views = page.threads.iter().map(summary_view).collect();
    Ok((page.total_records, views))
}

fn render_list(format: OutputFormat, report: &SessionListReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.sessions.is_empty() {
                stdout.push_str("未找到 Axon thread 会话\n");
            } else {
                for session in &report.sessions {
                    let _ = writeln!(
                        stdout,
                        "{}\t{}\t{}\tturns={}",
                        session.thread_id, session.provider, session.model, session.turn_count
                    );
                }
            }
            let stderr = if report.warnings.is_empty() {
                String::new()
            } else {
                format!("{}\n", report.warnings.join("\n"))
            };
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr,
            }
        }
    }
}

fn render_show(format: OutputFormat, report: &SessionShowReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: render_show_text(report),
            stderr: String::new(),
        },
    }
}

fn render_show_text(report: &SessionShowReport) -> String {
    let stdout = format!(
        "Thread: {}\n- provider: {}\n- model: {}\n- turns: {}\n- prompt_sha256: {}\n- response_sha256: {}\n",
        report.session.thread_id,
        report.session.provider,
        report.session.model,
        report.session.turn_count,
        report.session.prompt_sha256,
        report.session.response_sha256,
    );
    stdout
}

fn render_read(format: OutputFormat, report: &SessionReadReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: render_read_text(report),
            stderr: String::new(),
        },
    }
}

fn render_read_text(report: &SessionReadReport) -> String {
    let thread = &report.thread.thread;
    let mut stdout = format!(
        "Thread: {}\n- name: {}\n- provider: {}\n- model: {}\n- cwd: {}\n- status: {:?}\n- source: {:?}\n- parent: {}\n- turns: {}\n- store: {}\n",
        thread.thread_id,
        thread.name.as_deref().unwrap_or("-"),
        thread.settings.provider,
        thread.settings.model,
        thread.settings.cwd,
        thread.status,
        thread.source,
        thread.parent_thread_id.as_deref().unwrap_or("-"),
        report.thread.turns.len(),
        report.store_path,
    );
    if let Some(compaction) = &report.thread.compacted_history {
        let _ = writeln!(
            stdout,
            "- compacted: before_tokens={} after_tokens={} through_sequence={}",
            compaction.before_tokens, compaction.after_tokens, compaction.through_sequence
        );
    }
    for turn in &report.thread.turns {
        stdout.push('\n');
        stdout.push_str(&render_turn_text(turn));
    }
    stdout
}

fn render_turn_text(turn: &Turn) -> String {
    let user = turn_user_content(turn);
    let assistant = turn_assistant_content(turn);
    let mut text = format!(
        "Turn {} · {:?}\n- turn_id: {}\n- started_at_ms: {}\n- completed_at_ms: {}\n- user_sha256: {}\n- assistant_sha256: {}\n- user_chars: {}\n- assistant_chars: {}\n",
        turn.sequence,
        turn.status,
        turn.turn_id,
        turn.started_at_ms,
        turn.completed_at_ms
            .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        super::sha256_hex(user),
        super::sha256_hex(assistant),
        user.chars().count(),
        assistant.chars().count(),
    );
    let _ = writeln!(
        text,
        "- user: {}",
        preview_text(user, SESSION_READ_PREVIEW_CHARS)
    );
    let _ = writeln!(
        text,
        "- assistant: {}",
        preview_text(assistant, SESSION_READ_PREVIEW_CHARS)
    );
    text
}

fn turn_user_content(turn: &Turn) -> &str {
    turn.history_items
        .iter()
        .find_map(|item| match item {
            ThreadHistoryItem::UserMessage { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .unwrap_or("")
}

fn turn_assistant_content(turn: &Turn) -> &str {
    turn.history_items
        .iter()
        .rev()
        .find_map(|item| match item {
            ThreadHistoryItem::AssistantMessage { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .unwrap_or("")
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = normalized.chars().take(max_chars).collect::<String>();
    if normalized.chars().count() > max_chars {
        preview.push_str("...");
    }
    if preview.is_empty() {
        "-".to_owned()
    } else {
        preview
    }
}

fn render_resume(format: OutputFormat, report: &SessionResumeReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!("已恢复 Thread: {}\n", report.thread_id),
            stderr: String::new(),
        },
    }
}

fn render_fork(format: OutputFormat, report: &SessionForkReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "已 fork Thread: {}\n- source: {}\n",
                report.new_thread_id, report.source_thread_id
            ),
            stderr: String::new(),
        },
    }
}

fn render_export(format: OutputFormat, report: &SessionExportReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "已导出 Thread 摘要: {}\n- prompt_sha256: {}\n- response_sha256: {}\n- output: {}\n",
                report.session.thread_id,
                report.session.prompt_sha256,
                report.session.response_sha256,
                report.output_path.as_deref().unwrap_or("-")
            ),
            stderr: String::new(),
        },
    }
}

fn write_export_report(path: &Path, report: &SessionExportReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "无法创建 session export 目录".to_owned())?;
    }
    let text =
        serde_json::to_string_pretty(report).map_err(|_| "session export 序列化失败".to_owned())?;
    fs::write(path, format!("{text}\n")).map_err(|_| "无法写入 session export 文件".to_owned())
}

fn detail_view(detail: &ThreadDetail) -> SessionSummaryView {
    let prompt = detail.turns.last().map_or("", turn_user_content);
    let response = detail.turns.last().map_or("", turn_assistant_content);
    SessionSummaryView {
        thread_id: detail.thread.thread_id.clone(),
        parent_thread_id: detail.thread.parent_thread_id.clone(),
        forked_from_thread_id: detail.thread.forked_from_thread_id.clone(),
        created_at_ms: detail.thread.created_at_ms,
        updated_at_ms: detail.thread.updated_at_ms,
        provider: detail.thread.settings.provider.clone(),
        model: detail.thread.settings.model.clone(),
        cwd: detail.thread.settings.cwd.clone(),
        turn_count: detail.turns.len(),
        prompt_sha256: super::sha256_hex(prompt),
        response_sha256: super::sha256_hex(response),
        prompt_chars: prompt.chars().count(),
        response_chars: response.chars().count(),
    }
}

fn summary_view(summary: &ThreadSummary) -> SessionSummaryView {
    SessionSummaryView {
        thread_id: summary.thread_id.clone(),
        parent_thread_id: summary.parent_thread_id.clone(),
        forked_from_thread_id: summary.forked_from_thread_id.clone(),
        created_at_ms: summary.created_at_ms,
        updated_at_ms: summary.updated_at_ms,
        provider: summary.provider.clone(),
        model: summary.model.clone(),
        cwd: summary.cwd.clone(),
        turn_count: summary.turn_count,
        prompt_sha256: super::sha256_hex(""),
        response_sha256: super::sha256_hex(""),
        prompt_chars: 0,
        response_chars: 0,
    }
}

fn thread_service() -> ThreadService<JsonlThreadEventStore> {
    ThreadService::new(JsonlThreadEventStore)
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis().min(u128::from(u64::MAX))).unwrap_or(u64::MAX)
        })
}
