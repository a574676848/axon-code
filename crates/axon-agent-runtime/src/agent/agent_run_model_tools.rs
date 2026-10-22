//! Runtime-owned provider-visible model tool policy and budget projection.

use crate::ProviderToolCall;
use crate::SearchToolExecutionReceipt;
use crate::SearchToolExecutionStatus;
use crate::ShellCommandBoundedStream;
use crate::ShellCommandExecutionReceipt;
use crate::ShellCommandExecutionStatus;
use crate::SubagentToolExecutionReceipt;
use crate::SubagentToolExecutionStatus;
pub use crate::agent_run_read_file_receipt::{
    agent_run_read_file_tool_receipt, agent_run_read_file_tool_receipt_summary,
};
#[path = "agent_run_model_tool_diagnostics.rs"]
pub(crate) mod agent_run_model_tool_diagnostics;
#[path = "agent_run_shell_receipt_metadata.rs"]
mod agent_run_shell_receipt_metadata;
use crate::GlobToolExecutionReceipt;
use crate::GlobToolExecutionStatus;
use crate::builtin_shell_tool::APPROX_CHARS_PER_TOKEN;
use crate::job_runtime::{JobId, JobOutput, JobStatus};
use agent_run_shell_receipt_metadata::{
    output_preview, shell_receipt_metadata, short_tool_name, stream_metadata,
};

/// 工具参数无效时的稳定回执错误类型。
pub const AGENT_RUN_TOOL_ERROR_TYPE_INVALID_ARGUMENTS: &str = "invalid_arguments";
/// 工具执行失败时的稳定回执错误类型。
pub const AGENT_RUN_TOOL_ERROR_TYPE_EXECUTION_FAILED: &str = "execution_failed";
/// MCP server 或 tool 当前不可用，Agent 应优先选择健康工具降级继续。
pub const AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE: &str = "mcp_unavailable";
/// 后台作业被终止时的稳定回执错误类型。
///
/// 与 `execution_failed` 区分：被终止的作业没有退出码，也不代表命令自身执行出错，
/// 回执必须让模型能分辨「失败」与「被终止」。
pub const AGENT_RUN_TOOL_ERROR_TYPE_KILLED: &str = "killed";
/// 执行前的准入裁决拒绝了这次任务；与 `execution_failed` 区分，
/// 否则父代理会把「任务未被接受」误读成「执行中出错」。
pub const AGENT_RUN_TOOL_ERROR_TYPE_REFUSED: &str = "refused";
/// 作业回执预览被输出预算截断时插入的省略标记。
const JOB_OUTPUT_OMITTED_MARKER: &str =
    "\n... output omitted; full output is in the artifact ...\n";

/// Runtime-owned projection for a local tool receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunToolReceiptProjection {
    /// Provider returned tool call id.
    pub provider_call_id: String,
    /// Provider-visible tool name.
    pub tool_name: String,
    /// Receipt status.
    pub status: String,
    /// Bounded execution summary.
    pub summary: String,
    /// Structured policy metadata.
    pub metadata: Option<serde_json::Value>,
    /// Stable error type.
    pub error_type: Option<String>,
    /// Whether remote trigger is allowed.
    pub remote_trigger_allowed: bool,
    /// Whether filesystem writes were performed.
    pub filesystem_write_performed: bool,
    /// 完整输出引用。
    pub output_ref: Option<axon_domain::OutputRef>,
}

/// Project a provider tool call into a generic blocked receipt.
#[must_use]
pub fn agent_run_blocked_tool_receipt(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
) -> AgentRunToolReceiptProjection {
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: "blocked".to_owned(),
        summary: summary.into(),
        metadata: None,
        output_ref: None,
        error_type: Some("blocked".to_owned()),
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    }
}

/// Project a provider tool call into a generic failed receipt.
#[must_use]
pub fn agent_run_failed_tool_receipt(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
) -> AgentRunToolReceiptProjection {
    agent_run_failed_tool_receipt_with_error_type(
        tool_call,
        summary,
        AGENT_RUN_TOOL_ERROR_TYPE_EXECUTION_FAILED,
    )
}

/// Project a provider tool call into a failed receipt with a stable error type.
#[must_use]
pub fn agent_run_failed_tool_receipt_with_error_type(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
    error_type: &'static str,
) -> AgentRunToolReceiptProjection {
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: "failed".to_owned(),
        summary: summary.into(),
        metadata: None,
        output_ref: None,
        error_type: Some(error_type.to_owned()),
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    }
}

/// Project a provider tool call into a generic completed receipt.
#[must_use]
pub fn agent_run_completed_tool_receipt(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
    metadata: Option<serde_json::Value>,
    filesystem_write_performed: bool,
) -> AgentRunToolReceiptProjection {
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: "completed".to_owned(),
        summary: summary.into(),
        metadata,
        output_ref: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed,
    }
}

/// Project a shell command execution receipt into the provider-visible tool receipt contract.
#[must_use]
pub fn agent_run_shell_tool_receipt(
    tool_call: &ProviderToolCall,
    receipt: &ShellCommandExecutionReceipt,
) -> AgentRunToolReceiptProjection {
    let status = shell_status_name(receipt.status);
    let summary = agent_run_shell_tool_receipt_summary(receipt);
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: status.to_owned(),
        summary: summary.clone(),
        metadata: Some(shell_receipt_metadata(tool_call, receipt, status, &summary)),
        output_ref: None,
        error_type: shell_error_type(receipt.status).map(str::to_owned),
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: false,
    }
}

/// Project a shell command execution receipt into a bounded provider-visible summary.
#[must_use]
pub fn agent_run_shell_tool_receipt_summary(receipt: &ShellCommandExecutionReceipt) -> String {
    format!(
        "shell command={} exit_code={} timed_out={} output_handler={} output_tokens={}/{} output_savings_pct={} stdout_truncated={} stdout_chars={}/{} stderr_truncated={} stderr_chars={}/{} full_output_available={} full_output_truncated={} full_output_failed={} stdout={} stderr={}",
        receipt.command.join(" "),
        receipt
            .exit_code
            .map_or_else(|| "none".to_owned(), |value| value.to_string()),
        receipt.timed_out,
        receipt.output_handler_used,
        receipt.filtered_output_tokens,
        receipt.original_output_tokens,
        receipt.output_savings_pct,
        receipt.stdout.truncated,
        receipt.stdout.retained_chars,
        receipt.stdout.original_chars,
        receipt.stderr.truncated,
        receipt.stderr.retained_chars,
        receipt.stderr.original_chars,
        receipt.full_output_path.is_some()
            && !receipt.artifact_truncated
            && !receipt.artifact_failed,
        receipt.artifact_truncated,
        receipt.artifact_failed,
        receipt.stdout.preview,
        receipt.stderr.preview
    )
}

/// Project a started background job into the provider-visible tool receipt contract.
///
/// INV-3：回执只承诺「作业已启动、当前仍在运行」。它不返回 `session_id`
/// （作业通道给 `job_id`），也不返回退出码：启动不是成功。
#[must_use]
pub fn agent_run_job_started_tool_receipt(
    tool_call: &ProviderToolCall,
    job_id: JobId,
) -> AgentRunToolReceiptProjection {
    let status = job_status_name(JobStatus::Running);
    let summary = format!("job start job_id={job_id} status={status}");
    let metadata = serde_json::json!({
        "kind": JOB_RECEIPT_KIND,
        "toolName": short_tool_name(tool_call.dispatch_name()),
        "status": status,
        "summary": summary,
        "jobId": job_id.to_string(),
        "jobStatus": status,
        "exitCode": serde_json::Value::Null,
    });
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: status.to_owned(),
        summary,
        metadata: Some(metadata),
        output_ref: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    }
}

/// Project a background job read into the provider-visible tool receipt contract.
///
/// INV-3：`status` 为 `running` 时不得暗示成功，`exit_code` 只在终态出现；
/// 预览被输出预算截断时同时给出 `fullOutputAvailable` 与归档路径。
#[must_use]
pub fn agent_run_job_output_tool_receipt(
    tool_call: &ProviderToolCall,
    output: &JobOutput,
    max_output_tokens: Option<usize>,
) -> AgentRunToolReceiptProjection {
    let (stdout_budget, stderr_budget) = job_output_char_budgets(
        max_output_tokens,
        output.stdout.original_chars,
        output.stderr.original_chars,
    );
    let stdout = bounded_job_stream(&output.stdout, stdout_budget);
    let stderr = bounded_job_stream(&output.stderr, stderr_budget);
    let status = job_status_name(output.status);
    let summary = agent_run_job_output_tool_receipt_summary(output, &stdout, &stderr);
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: status.to_owned(),
        summary: summary.clone(),
        metadata: Some(job_output_metadata(
            tool_call, output, &stdout, &stderr, status, &summary,
        )),
        output_ref: None,
        error_type: job_error_type(output.status).map(str::to_owned),
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    }
}

/// Project a job read into a bounded provider-visible summary.
#[must_use]
pub fn agent_run_job_output_tool_receipt_summary(
    output: &JobOutput,
    stdout: &ShellCommandBoundedStream,
    stderr: &ShellCommandBoundedStream,
) -> String {
    format!(
        "job output job_id={} status={} elapsed_ms={} exit_code={} stdout_truncated={} stdout_chars={}/{} stderr_truncated={} stderr_chars={}/{} full_output_available={} artifact_path={} stdout={} stderr={}",
        output.job_id,
        job_status_name(output.status),
        output.elapsed_ms,
        output
            .exit_code
            .map_or_else(|| "none".to_owned(), |value| value.to_string()),
        stdout.truncated,
        stdout.retained_chars,
        stdout.original_chars,
        stderr.truncated,
        stderr.retained_chars,
        stderr.original_chars,
        output.full_output_available,
        output.artifact_path.display(),
        stdout.preview,
        stderr.preview
    )
}

/// Project a completed `job_kill` into the provider-visible tool receipt contract.
#[must_use]
pub fn agent_run_job_kill_tool_receipt(
    tool_call: &ProviderToolCall,
    job_id: JobId,
    status: JobStatus,
) -> AgentRunToolReceiptProjection {
    let status_name = job_status_name(status);
    let summary = format!("job kill job_id={job_id} status={status_name}");
    let metadata = serde_json::json!({
        "kind": JOB_RECEIPT_KIND,
        "toolName": short_tool_name(tool_call.dispatch_name()),
        "status": status_name,
        "summary": summary,
        "jobId": job_id.to_string(),
        "jobStatus": status_name,
    });
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: status_name.to_owned(),
        summary,
        metadata: Some(metadata),
        output_ref: None,
        error_type: job_error_type(status).map(str::to_owned),
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    }
}

fn job_output_metadata(
    tool_call: &ProviderToolCall,
    output: &JobOutput,
    stdout: &ShellCommandBoundedStream,
    stderr: &ShellCommandBoundedStream,
    status: &str,
    summary: &str,
) -> serde_json::Value {
    serde_json::json!({
        "kind": JOB_RECEIPT_KIND,
        "toolName": short_tool_name(tool_call.dispatch_name()),
        "status": status,
        "summary": summary,
        "argumentsPreview": output.label,
        "outputPreview": job_output_preview(stdout, stderr),
        "jobId": output.job_id.to_string(),
        "elapsedMs": output.elapsed_ms,
        "exitCode": output.exit_code,
        "artifactPath": output.artifact_path.display().to_string(),
        "output": {
            "stdout": stream_metadata(stdout),
            "stderr": stream_metadata(stderr),
            "fullOutputAvailable": output.full_output_available,
        },
    })
}

/// 作业回执的稳定 `kind`；TUI 与 wire 投影按它区分作业与 shell 会话回执。
const JOB_RECEIPT_KIND: &str = "job";

fn job_status_name(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Running => "running",
        JobStatus::Completed => "completed",
        JobStatus::Refusal => "refusal",
        JobStatus::Failed => "failed",
        JobStatus::Killed => "killed",
        JobStatus::Timeout => "timeout",
        JobStatus::Unresponsive => "unresponsive",
    }
}

fn job_error_type(status: JobStatus) -> Option<&'static str> {
    match status {
        JobStatus::Running | JobStatus::Completed => None,
        JobStatus::Refusal => Some(AGENT_RUN_TOOL_ERROR_TYPE_REFUSED),
        JobStatus::Failed => Some(AGENT_RUN_TOOL_ERROR_TYPE_EXECUTION_FAILED),
        JobStatus::Killed | JobStatus::Timeout | JobStatus::Unresponsive => {
            Some(AGENT_RUN_TOOL_ERROR_TYPE_KILLED)
        }
    }
}

/// 把 token 预算折算为两条流的字符预算。
///
/// 与 shell 会话的分配口径一致：总量在预算内时不截断；超出时按两条流的原始
/// 字符数比例分配。作业的增量批次在作业运行时已按 head + tail 有界保留，
/// 这里只做二次收敛，超出的部分仍在归档中可读。
fn job_output_char_budgets(
    max_output_tokens: Option<usize>,
    stdout_chars: usize,
    stderr_chars: usize,
) -> (Option<usize>, Option<usize>) {
    let Some(max_output_tokens) = max_output_tokens else {
        return (None, None);
    };
    let budget = max_output_tokens.saturating_mul(APPROX_CHARS_PER_TOKEN);
    let total = stdout_chars.saturating_add(stderr_chars);
    if total <= budget || total == 0 {
        return (None, None);
    }
    let stdout_budget = budget.saturating_mul(stdout_chars) / total;
    (
        Some(stdout_budget),
        Some(budget.saturating_sub(stdout_budget)),
    )
}

/// 按字符预算收敛预览；预算内原样返回。
fn bounded_job_stream(
    stream: &ShellCommandBoundedStream,
    budget_chars: Option<usize>,
) -> ShellCommandBoundedStream {
    let Some(preview) = bounded_job_preview(&stream.preview, budget_chars) else {
        return stream.clone();
    };
    ShellCommandBoundedStream {
        retained_chars: preview.chars().count(),
        preview,
        truncated: true,
        original_chars: stream.original_chars,
        sha256: stream.sha256.clone(),
    }
}

/// 保留头尾并插入省略标记；已在预算内时返回 `None`，避免无谓改写。
fn bounded_job_preview(preview: &str, budget_chars: Option<usize>) -> Option<String> {
    let budget_chars = budget_chars?;
    let total_chars = preview.chars().count();
    if total_chars <= budget_chars {
        return None;
    }
    let marker_chars = JOB_OUTPUT_OMITTED_MARKER.chars().count();
    if budget_chars <= marker_chars {
        // 预算装不下省略标记时只保留头部：宁可少给内容，也不得超出预算。
        return Some(preview.chars().take(budget_chars).collect());
    }
    let content_budget = budget_chars - marker_chars;
    let head_chars = content_budget.div_ceil(2);
    let tail_chars = content_budget - head_chars;
    let head = preview.chars().take(head_chars).collect::<String>();
    let tail = preview
        .chars()
        .skip(total_chars - tail_chars)
        .collect::<String>();
    Some(format!("{head}{JOB_OUTPUT_OMITTED_MARKER}{tail}"))
}

fn job_output_preview(
    stdout: &ShellCommandBoundedStream,
    stderr: &ShellCommandBoundedStream,
) -> String {
    output_preview(&stdout.preview, &stderr.preview)
}

/// Project a search execution receipt into the provider-visible tool receipt contract.
#[must_use]
pub fn agent_run_search_tool_receipt(
    tool_call: &ProviderToolCall,
    receipt: &SearchToolExecutionReceipt,
) -> AgentRunToolReceiptProjection {
    let metadata = serde_json::json!({
        "kind": "search",
        "query": receipt.query,
        "root": receipt.root,
        "incomplete": receipt.incomplete,
        "page": {
            "fileOffset": receipt.file_offset,
            "nextFileOffset": receipt.next_file_offset,
        },
        "missingPaths": receipt.missing_paths,
        "binaryPaths": receipt.binary_paths,
        "unreadablePaths": receipt.unreadable_paths,
        "files": receipt.files.iter().map(search_file_metadata).collect::<Vec<_>>(),
    });
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: search_status_name(receipt.status).to_owned(),
        summary: agent_run_search_tool_receipt_summary(receipt),
        metadata: Some(metadata),
        output_ref: None,
        error_type: search_error_type(receipt.status).map(str::to_owned),
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: receipt.filesystem_write_performed,
    }
}

/// Project a search execution receipt into a bounded provider-visible summary.
#[must_use]
pub fn agent_run_search_tool_receipt_summary(receipt: &SearchToolExecutionReceipt) -> String {
    let header = format!(
        "search query={} files={} files_scanned={} next_file_offset={} incomplete={} missing={} binary={} unreadable={}",
        receipt.query,
        receipt.files.len(),
        receipt.files_scanned,
        receipt
            .next_file_offset
            .map_or_else(|| "end".to_owned(), |value| value.to_string()),
        receipt.incomplete,
        receipt.missing_paths.len(),
        receipt.binary_paths.len(),
        receipt.unreadable_paths.len(),
    );
    if receipt.files.is_empty() {
        return header;
    }
    let files = receipt
        .files
        .iter()
        .map(search_file_summary)
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header}\n{files}")
}

/// 将 glob 执行回执投影为 provider 可见工具回执。
#[must_use]
pub fn agent_run_glob_tool_receipt(
    tool_call: &ProviderToolCall,
    receipt: &GlobToolExecutionReceipt,
) -> AgentRunToolReceiptProjection {
    let metadata = serde_json::json!({
        "kind": "glob",
        "root": receipt.root,
        "truncated": receipt.truncated,
        "incomplete": receipt.incomplete,
        "page": {
            "offset": receipt.offset,
            "nextOffset": receipt.next_offset,
        },
        "missingPaths": receipt.missing_paths,
        "unreadablePaths": receipt.unreadable_paths,
        "entries": receipt.entries.iter().map(|entry| serde_json::json!({
            "path": entry.path,
            "isDir": entry.is_dir,
            "modifiedAtMs": entry.modified_at_ms,
        })).collect::<Vec<_>>(),
    });
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: glob_status_name(receipt.status).to_owned(),
        summary: agent_run_glob_tool_receipt_summary(receipt),
        metadata: Some(metadata),
        output_ref: None,
        error_type: glob_error_type(receipt.status).map(str::to_owned),
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: false,
    }
}

/// 将 glob 执行回执投影为有界 provider 可见摘要。
#[must_use]
pub fn agent_run_glob_tool_receipt_summary(receipt: &GlobToolExecutionReceipt) -> String {
    let header = format!(
        "glob root={} entries={} next_offset={} truncated={} incomplete={} missing={} unreadable={}",
        receipt.root,
        receipt.entries.len(),
        receipt
            .next_offset
            .map_or_else(|| "end".to_owned(), |value| value.to_string()),
        receipt.truncated,
        receipt.incomplete,
        receipt.missing_paths.len(),
        receipt.unreadable_paths.len(),
    );
    if receipt.entries.is_empty() {
        return header;
    }
    let entries = receipt
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header}\n{entries}")
}

/// Project a sub-agent tool execution receipt into the provider-visible contract.
#[must_use]
pub fn agent_run_subagent_tool_receipt(
    tool_call: &ProviderToolCall,
    receipt: &SubagentToolExecutionReceipt,
) -> AgentRunToolReceiptProjection {
    let mut metadata = serde_json::json!({
        "kind": "subagent",
        "payload": receipt.payload,
        "reasons": receipt.reasons,
    });
    if !receipt.model_messages.is_empty() {
        metadata["modelMessages"] = serde_json::json!(receipt.model_messages);
    }
    AgentRunToolReceiptProjection {
        provider_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        status: subagent_status_name(receipt.status).to_owned(),
        summary: receipt.bounded_summary.clone(),
        metadata: Some(metadata),
        output_ref: None,
        error_type: subagent_error_type(receipt.status).map(str::to_owned),
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: receipt.filesystem_write_performed,
    }
}

fn glob_error_type(status: GlobToolExecutionStatus) -> Option<&'static str> {
    match status {
        GlobToolExecutionStatus::Completed => None,
        GlobToolExecutionStatus::Blocked => Some("blocked"),
    }
}

fn glob_status_name(status: GlobToolExecutionStatus) -> &'static str {
    match status {
        GlobToolExecutionStatus::Completed => "completed",
        GlobToolExecutionStatus::Blocked => "blocked",
    }
}

fn search_error_type(status: SearchToolExecutionStatus) -> Option<&'static str> {
    match status {
        SearchToolExecutionStatus::Completed => None,
        SearchToolExecutionStatus::Blocked => Some("blocked"),
    }
}

fn search_status_name(status: SearchToolExecutionStatus) -> &'static str {
    match status {
        SearchToolExecutionStatus::Completed => "completed",
        SearchToolExecutionStatus::Blocked => "blocked",
    }
}

fn subagent_error_type(status: SubagentToolExecutionStatus) -> Option<&'static str> {
    match status {
        SubagentToolExecutionStatus::Completed => None,
        SubagentToolExecutionStatus::Blocked => Some("blocked"),
    }
}

fn subagent_status_name(status: SubagentToolExecutionStatus) -> &'static str {
    match status {
        SubagentToolExecutionStatus::Completed => "completed",
        SubagentToolExecutionStatus::Blocked => "blocked",
    }
}

fn shell_error_type(status: ShellCommandExecutionStatus) -> Option<&'static str> {
    match status {
        ShellCommandExecutionStatus::Running | ShellCommandExecutionStatus::Completed => None,
        ShellCommandExecutionStatus::Failed => Some("execution_failed"),
        ShellCommandExecutionStatus::Blocked => Some("blocked"),
    }
}

fn shell_status_name(status: ShellCommandExecutionStatus) -> &'static str {
    match status {
        ShellCommandExecutionStatus::Running => "running",
        ShellCommandExecutionStatus::Completed => "completed",
        ShellCommandExecutionStatus::Failed => "failed",
        ShellCommandExecutionStatus::Blocked => "blocked",
    }
}

fn search_file_metadata(file: &crate::SearchToolFile) -> serde_json::Value {
    serde_json::json!({
        "path": file.path,
        "matchedLineCount": file.matched_line_count,
        "matchesTruncated": file.matches_truncated,
        "hashlineTag": file.hashline_tag,
        "lines": file.lines.iter().map(|line| serde_json::json!({
            "lineNumber": line.line_number,
            "isMatch": line.is_match,
            "matchSpans": line.match_spans,
            "text": line.text,
        })).collect::<Vec<_>>(),
    })
}

fn search_file_summary(file: &crate::SearchToolFile) -> String {
    let tag = file.hashline_tag.as_deref().unwrap_or("none");
    let lines = file
        .lines
        .iter()
        .map(|line| {
            let marker = if line.is_match { ":" } else { "-" };
            format!("{}{}{} {}", line.line_number, marker, tag, line.text)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{} matches={} truncated={} hashline={}\n{}",
        file.path, file.matched_line_count, file.matches_truncated, tag, lines
    )
}
