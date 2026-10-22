//! `axon debug mailbox`：跨轮 mailbox 的消息与投递状态查询（合同 §5.3.2）。
//!
//! 该命令是**只读**观测面：不投递、不重试、不改状态。它与恢复装配共用同一份
//! `jarvis_agent_messages` 事实与同一套过滤口径，因此「观测到的」与「恢复时会用到的」
//! 不会出现两套真相。

use std::path::Path;

use axon_session::{
    AgentMailboxMessageRecord, AgentMailboxMessageState, AgentMailboxQuery, AgentMailboxStore,
    SqliteRuntimeStore,
};
use serde::Serialize;

use crate::management_commands::{cli_error, json_result};
use crate::{CliRunResult, DebugMailboxCommand, DebugMailboxState, OutputFormat};

/// 正文预览上限：查询是为了看投递状态，不需要把整条消息抄进终端。
const CONTENT_PREVIEW_CHARS: usize = 120;

/// 一条消息的查询投影。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MailboxMessageView {
    message_id: String,
    owner_id: String,
    author: String,
    recipient: String,
    kind: &'static str,
    state: &'static str,
    attempt_count: u32,
    retry_at_ms: Option<u64>,
    last_attempt_at_ms: Option<u64>,
    last_error: Option<String>,
    delivered_at_ms: Option<u64>,
    continuation_id: Option<String>,
    created_at_ms: u64,
    expires_at_ms: u64,
    content_preview: String,
}

impl MailboxMessageView {
    fn from_record(record: AgentMailboxMessageRecord) -> Self {
        Self {
            message_id: record.message_id,
            owner_id: record.owner_id,
            author: record.author,
            recipient: record.recipient,
            kind: record.kind.as_str(),
            state: record.state.as_str(),
            attempt_count: record.attempt_count,
            retry_at_ms: record.retry_at_ms,
            last_attempt_at_ms: record.last_attempt_at_ms,
            last_error: record.last_error,
            delivered_at_ms: record.delivered_at_ms,
            continuation_id: record.continuation_id,
            created_at_ms: record.created_at_ms,
            expires_at_ms: record.expires_at_ms,
            content_preview: preview(&record.content),
        }
    }
}

/// 查询报告。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MailboxReport {
    state_filter: &'static str,
    owner_filter: String,
    recipient_filter: String,
    limit: u32,
    count: usize,
    messages: Vec<MailboxMessageView>,
}

pub(crate) fn run_mailbox_debug(
    command: &DebugMailboxCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let store = match mailbox_store(config_path) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let report = match build_report(&store, command) {
        Ok(report) => report,
        Err(error) => return cli_error(error),
    };
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: render_text(&report),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&report),
    }
}

/// 查询并按命令参数投影；与打开库的步骤分离，便于用临时库做单元测试。
fn build_report(
    store: &SqliteRuntimeStore,
    command: &DebugMailboxCommand,
) -> Result<MailboxReport, String> {
    let query = AgentMailboxQuery {
        owner_id: command.owner.clone(),
        recipient: command.recipient.clone(),
        state: resolve_state(command.state),
        limit: command.limit,
    };
    let records = store
        .list_agent_messages(&query)
        .map_err(|error| format!("读取 mailbox 失败：{error}"))?;
    Ok(MailboxReport {
        state_filter: command.state.as_str(),
        owner_filter: command.owner.clone().unwrap_or_else(|| "不限".to_owned()),
        recipient_filter: command
            .recipient
            .clone()
            .unwrap_or_else(|| "不限".to_owned()),
        limit: command.limit,
        count: records.len(),
        messages: records
            .into_iter()
            .map(MailboxMessageView::from_record)
            .collect(),
    })
}

/// 打开共享 runtime 库；与 `axon run` / daemon 同源（`<axon_root>/data/runtime.sqlite3`）。
fn mailbox_store(config_path: Option<&Path>) -> Result<SqliteRuntimeStore, String> {
    let path = match config_path {
        Some(path) => path.to_path_buf(),
        None => axon_config::ConfigLoader::default_user_config_path()
            .map_err(|_| "无法解析用户配置路径".to_owned())?,
    };
    let data_root = crate::runtime_paths::runtime_axon_data_path_static(&path)?;
    Ok(SqliteRuntimeStore::from_root(data_root))
}

/// 把命令行取值解析为存储层状态；`Any` 表示不加状态条件。
fn resolve_state(state: DebugMailboxState) -> Option<AgentMailboxMessageState> {
    if state.is_any() {
        return None;
    }
    AgentMailboxMessageState::parse(state.as_str())
}

fn preview(content: &str) -> String {
    let head = content
        .chars()
        .take(CONTENT_PREVIEW_CHARS)
        .collect::<String>();
    if head.chars().count() < content.chars().count() {
        format!("{head}…")
    } else {
        head
    }
}

fn render_text(report: &MailboxReport) -> String {
    let mut stdout = format!(
        "mailbox: {} 条（state={}, owner={}, recipient={}, limit={}）\n",
        report.count,
        report.state_filter,
        report.owner_filter,
        report.recipient_filter,
        report.limit
    );
    if report.messages.is_empty() {
        stdout.push_str("无匹配消息\n");
        return stdout;
    }
    for message in &report.messages {
        stdout.push_str(&format!(
            "- {} {} {} {} -> {} attempts={} created={} expires={}\n",
            message.message_id,
            message.state,
            message.kind,
            message.author,
            message.recipient,
            message.attempt_count,
            message.created_at_ms,
            message.expires_at_ms
        ));
        if let Some(retry_at) = message.retry_at_ms {
            stdout.push_str(&format!("  retryAt={retry_at}\n"));
        }
        if let Some(error) = message.last_error.as_deref() {
            stdout.push_str(&format!("  lastError={error}\n"));
        }
        if let Some(delivered_at) = message.delivered_at_ms {
            stdout.push_str(&format!("  deliveredAt={delivered_at}\n"));
        }
        if let Some(continuation_id) = message.continuation_id.as_deref() {
            stdout.push_str(&format!("  continuation={continuation_id}\n"));
        }
        stdout.push_str(&format!("  正文：{}\n", message.content_preview));
    }
    stdout
}

#[cfg(test)]
#[path = "debug_mailbox_tests.rs"]
mod tests;
