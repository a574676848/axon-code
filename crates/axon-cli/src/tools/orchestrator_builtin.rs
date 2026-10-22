//! 内置非文件类工具执行适配。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use axon_agent_runtime::{
    AgentModelMessage, AgentRuntimeNoteKind, MEMORY_SEARCH_DEFAULT_LIMIT,
    MEMORY_WRITE_TOOL_EVIDENCE_FIELD, MEMORY_WRITE_TOOL_KIND_FIELD, MEMORY_WRITE_TOOL_KINDS,
    MEMORY_WRITE_TOOL_SUMMARY_FIELD, MEMORY_WRITE_TOOL_SUPERSEDES_IDS_FIELD, ProviderToolCall,
    SearchToolRuntime, SessionNotesRuntime, UpdatePlanToolRuntime,
    agent_run_search_tool_input_from_provider_args, agent_run_search_tool_receipt,
};

use serde::Deserialize;

use crate::run_commands::{AgentMemorySearchInput, AgentMemoryWriteInput};

use super::{
    ToolOrchestratorContext, ToolReceipt, completed_tool_receipt, failed_tool_input_receipt,
    failed_tool_receipt, failed_tool_receipt_from_axon_error, now_ms, tool_receipt_from_runtime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenBudgetMeasurementSource {
    ProviderUsage,
    LocalEstimate,
}

#[derive(Debug, Clone)]
pub(super) struct TokenBudgetSnapshot {
    pub(super) owner_id: String,
    pub(super) rollover_limit_tokens: u64,
    pub(super) current_context_tokens: u64,
    pub(super) source: TokenBudgetMeasurementSource,
}

static TOKEN_BUDGET_SNAPSHOTS: OnceLock<Mutex<HashMap<String, TokenBudgetSnapshot>>> =
    OnceLock::new();
const SESSION_STORAGE_DIRECTORY_NAME: &str = "sessions";
const GET_CONTEXT_REMAINING_INPUT_GUIDANCE: &str =
    "请使用空参数调用 builtin.get_context_remaining。";
const NEW_CONTEXT_INPUT_GUIDANCE: &str = "请仅提供可选的 reason 文本，说明当前为何适合开启新窗口。";
const NEW_CONTEXT_REQUIRES_SAVED_STATE: &str = "换窗未执行：当前窗口尚未成功写入任务便签。请先调用 builtin.notes_write_file 或 builtin.notes_append_to_file，写入目标、关键决策、进展、经验教训与下一步动作，以及仍在处理的相关请求和重要工具动作的 window ID、item ID；便签写入成功后，再调用 builtin.new_context。";

pub(super) fn record_token_budget_snapshot(session_id: &str, snapshot: TokenBudgetSnapshot) {
    TOKEN_BUDGET_SNAPSHOTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(session_id.to_owned(), snapshot);
}

pub(super) fn clear_token_budget_snapshot(session_id: &str, owner_id: &str) {
    let Some(snapshots) = TOKEN_BUDGET_SNAPSHOTS.get() else {
        return;
    };
    let mut snapshots = snapshots
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if snapshots
        .get(session_id)
        .is_some_and(|snapshot| snapshot.owner_id == owner_id)
    {
        snapshots.remove(session_id);
    }
}
pub(super) fn execute_search(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let canonical_input = tool_call.canonical_input();
    let mut input =
        match agent_run_search_tool_input_from_provider_args(&context.cwd, &canonical_input) {
            Ok(input) => input,
            Err(message) => return failed_tool_input_receipt(tool_call, message),
        };
    input.snapshot_session = Some(context.workspace_files.clone());
    match SearchToolRuntime.execute(input) {
        Ok(receipt) => {
            tool_receipt_from_runtime(agent_run_search_tool_receipt(tool_call, &receipt))
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

pub(super) fn execute_update_plan(tool_call: &ProviderToolCall) -> ToolReceipt {
    let Some(value) = tool_call.input.as_json() else {
        return failed_tool_input_receipt(tool_call, "update_plan 参数必须是 JSON");
    };
    match UpdatePlanToolRuntime.execute(value) {
        Ok(receipt) => {
            let summary = format!(
                "计划已更新：{}/{} 步完成",
                receipt.completed_steps, receipt.total_steps
            );
            match serde_json::to_value(receipt) {
                Ok(metadata) => completed_tool_receipt(tool_call, summary, Some(metadata), false),
                Err(error) => {
                    failed_tool_receipt(tool_call, format!("update_plan 回执序列化失败：{error}"))
                }
            }
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

pub(super) fn execute_get_context_remaining(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    if let Err(message) = parse_tool_input::<GetContextRemainingInput>(
        tool_call,
        GET_CONTEXT_REMAINING_INPUT_GUIDANCE,
    ) {
        return failed_tool_input_receipt(tool_call, message);
    }
    let Some(snapshot) = TOKEN_BUDGET_SNAPSHOTS.get().and_then(|snapshots| {
        snapshots
            .lock()
            .ok()
            .and_then(|snapshots| snapshots.get(&context.thread_session_id).cloned())
    }) else {
        return failed_tool_receipt(
            tool_call,
            "当前记忆窗口预算尚未初始化，请在下一次模型响应后重试。",
        );
    };
    let tokens_left = snapshot
        .rollover_limit_tokens
        .saturating_sub(snapshot.current_context_tokens);
    let (source_label, source_code, estimated) = match snapshot.source {
        TokenBudgetMeasurementSource::ProviderUsage => ("Provider 实测", "provider_usage", false),
        TokenBudgetMeasurementSource::LocalEstimate => ("本地估算", "local_estimate", true),
    };
    completed_tool_receipt(
        tool_call,
        format!("当前记忆窗口距离自动换窗还剩 {tokens_left} tokens（{source_label}）。"),
        Some(serde_json::json!({
            "tokensLeft": tokens_left,
            "source": source_code,
            "estimated": estimated,
            "scope": "rollover_limit",
        })),
        false,
    )
}

pub(super) fn execute_new_context(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_tool_input::<NewContextInput>(tool_call, NEW_CONTEXT_INPUT_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    if SessionNotesRuntime::ensure_current_window_state_saved(&session_dir).is_err() {
        return failed_tool_receipt(tool_call, NEW_CONTEXT_REQUIRES_SAVED_STATE);
    }
    let reason = input
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("当前阶段已具备换窗条件");
    let marker = AgentModelMessage::runtime_note(
        AgentRuntimeNoteKind::ContextRolloverRequest,
        format!("{reason}。请在当前工具边界完成后开启新的记忆窗口。"),
    );
    completed_tool_receipt(
        tool_call,
        "换窗请求已记录。当前工具边界完成后会保存旧窗口，并从已记录的目标、事实、未决事项和下一步动作继续。",
        Some(serde_json::json!({
            "contextRolloverRequested": true,
            "modelMessages": [marker],
        })),
        false,
    )
}

pub(super) fn execute_memory_search(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let Some(config_path) = context.config_path.as_deref() else {
        return failed_tool_receipt(tool_call, "builtin.memory_search 缺少本地配置路径");
    };
    let value = match tool_call.input.as_json() {
        Some(value) => value,
        None => {
            return failed_tool_input_receipt(tool_call, "builtin.memory_search 参数必须是 JSON");
        }
    };
    let Some(query) = value
        .get("query")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
    else {
        return failed_tool_input_receipt(
            tool_call,
            "builtin.memory_search query 必须是非空字符串",
        );
    };
    let limit = match value.get("limit") {
        None => MEMORY_SEARCH_DEFAULT_LIMIT,
        Some(value) => {
            let Some(limit) = value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|limit| *limit > 0)
            else {
                return failed_tool_input_receipt(
                    tool_call,
                    "builtin.memory_search limit 必须是正整数",
                );
            };
            limit
        }
    };

    match context.memory_write_runtime.search(AgentMemorySearchInput {
        config_path,
        cwd: &context.cwd,
        query,
        limit,
        now_ms: now_ms(),
        timeout: context.timeout,
        is_root_agent: context.current_agent_path.is_root(),
    }) {
        Ok(summary) => completed_tool_receipt(tool_call, summary, None, false),
        Err(message) => failed_tool_receipt(tool_call, message),
    }
}

pub(super) fn execute_memory_write(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let Some(config_path) = context.config_path.as_deref() else {
        return failed_tool_receipt(tool_call, "builtin.memory_write 缺少本地配置路径");
    };
    let value = match tool_call.input.as_json() {
        Some(value) => value,
        None => {
            return failed_tool_input_receipt(tool_call, "builtin.memory_write 参数必须是 JSON");
        }
    };
    let Some(kind) = value
        .get(MEMORY_WRITE_TOOL_KIND_FIELD)
        .and_then(serde_json::Value::as_str)
        .filter(|kind| MEMORY_WRITE_TOOL_KINDS.contains(kind))
    else {
        return failed_tool_input_receipt(
            tool_call,
            "builtin.memory_write kind 必须是 profile、preference、experience、knowledge 或 procedure",
        );
    };
    let Some(summary) = value
        .get(MEMORY_WRITE_TOOL_SUMMARY_FIELD)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    else {
        return failed_tool_input_receipt(
            tool_call,
            "builtin.memory_write summary 必须是非空字符串",
        );
    };
    let Some(evidence) = value
        .get(MEMORY_WRITE_TOOL_EVIDENCE_FIELD)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|evidence| !evidence.is_empty())
    else {
        return failed_tool_input_receipt(
            tool_call,
            "builtin.memory_write evidence 必须是非空字符串",
        );
    };
    let supersedes_memory_ids =
        match memory_id_array_field(value, MEMORY_WRITE_TOOL_SUPERSEDES_IDS_FIELD) {
            Ok(memory_ids) => memory_ids,
            Err(message) => return failed_tool_input_receipt(tool_call, message),
        };

    match context.memory_write_runtime.trigger(AgentMemoryWriteInput {
        config_path,
        cwd: &context.cwd,
        session_id: context.session_id,
        cell_id: context.cell_id,
        kind,
        summary,
        evidence,
        supersedes_memory_ids: &supersedes_memory_ids,
        now_ms: now_ms(),
        is_root_agent: context.current_agent_path.is_root(),
    }) {
        Ok(report) => {
            let memory_id = report.record.as_ref().map(|record| record.id.0);
            let status = if report.record.is_some() {
                if report.merged_existing {
                    "已合并既有 Memory"
                } else {
                    "已写入 Memory"
                }
            } else {
                "Memory 未通过准入"
            };
            let repair_status = if report.needs_repair {
                "，派生投影需要后台修复"
            } else {
                ""
            };
            completed_tool_receipt(
                tool_call,
                format!("{status}{repair_status}：{:?}", report.admission.reason),
                Some(serde_json::json!({
                    "kind": "memory_write",
                    "memoryId": memory_id,
                    "mergedExisting": report.merged_existing,
                    "supersededMemoryIds": report
                        .superseded_memory_ids
                        .iter()
                        .map(|memory_id| memory_id.0)
                        .collect::<Vec<_>>(),
                    "supersessionTextIndexPruned": report.supersession_text_index_pruned,
                    "supersessionVectorProjectionPruned": report
                        .supersession_vector_projection_pruned,
                    "supersessionVectorIndexPruned": report.supersession_vector_index_pruned,
                    "admissionAccepted": report.observation.accepted,
                    "admissionReason": report.observation.reason,
                    "sqliteWritten": report.sqlite_written,
                    "textIndexWritten": report.text_index_written,
                    "vectorIndexWritten": report.vector_index_written,
                    "vectorProjectionWritten": report.vector_projection_written,
                    "pipelineStateWritten": report.pipeline_state_written,
                    "pipelineStatus": report.pipeline_status.as_str(),
                    "needsRepair": report.needs_repair,
                })),
                false,
            )
        }
        Err(message) => failed_tool_input_receipt(tool_call, message),
    }
}

fn memory_id_array_field(
    value: &serde_json::Value,
    field: &'static str,
) -> Result<Vec<u64>, String> {
    let Some(raw) = value.get(field) else {
        return Ok(Vec::new());
    };
    let values = raw
        .as_array()
        .map_or_else(|| std::slice::from_ref(raw), |values| values);
    let mut memory_ids = Vec::with_capacity(values.len());
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        let memory_id = parse_memory_id_value(value).ok_or_else(|| {
            format!("builtin.memory_write arguments 的 {field} 包含无效 Memory ID")
        })?;
        if !seen.insert(memory_id) {
            return Err(format!(
                "builtin.memory_write arguments 的 {field} 不能包含重复 Memory ID"
            ));
        }
        memory_ids.push(memory_id);
    }
    Ok(memory_ids)
}

fn parse_memory_id_value(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| {
            value.as_str().and_then(|text| {
                let text = text.trim();
                (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| text.parse::<u64>().ok())
                    .flatten()
            })
        })
        .filter(|memory_id| *memory_id > 0)
}

pub(super) fn current_session_directory(
    context: &ToolOrchestratorContext,
) -> Result<PathBuf, String> {
    session_directory_for_session_id(context, &context.thread_session_id)
}

pub(super) fn session_directory_for_session_id(
    context: &ToolOrchestratorContext,
    session_id: &str,
) -> Result<PathBuf, String> {
    let config_path = context
        .config_path
        .as_deref()
        .ok_or_else(|| "当前会话记录尚未就绪，请稍后重试。".to_owned())?;
    crate::run_commands::resolved_axon_root(config_path)
        .map(|root| root.join(SESSION_STORAGE_DIRECTORY_NAME).join(session_id))
        .map_err(|error| format!("当前会话记录暂时不可用，请稍后重试：{error}"))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetContextRemainingInput {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewContextInput {
    #[serde(default)]
    reason: Option<String>,
}

fn parse_tool_input<T>(tool_call: &ProviderToolCall, retry_guidance: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let Some(value) = tool_call.input.as_json() else {
        return Err(format!(
            "本次调用内容无法读取，工具尚未执行。{retry_guidance}"
        ));
    };
    serde_json::from_value(value.clone()).map_err(|_| {
        format!("本次调用内容不完整或包含不支持的内容，工具尚未执行。{retry_guidance}")
    })
}

#[cfg(test)]
#[path = "orchestrator_builtin_tests.rs"]
mod tests;
