//! CLI 工具执行账本的 session 级生命周期管理。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axon_agent_runtime::{ProviderToolCall, ToolLedgerDecision, TurnExecutionLedger};
use axon_session::{ThreadDetail, ThreadHistoryItem, ThreadHistoryStatus};

use crate::tool_orchestrator::{ToolExecutionRequest, ToolReceipt};

type SharedToolLedger = Arc<Mutex<TurnExecutionLedger<ToolReceipt>>>;
const IN_FLIGHT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SESSION_LEDGERS: usize = 256;

static SESSION_LEDGERS: LazyLock<Mutex<HashMap<String, SharedToolLedger>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn begin(
    session_id: &str,
    request: &ToolExecutionRequest,
    workspace_revision: u64,
    turn_id: &str,
) -> ToolLedgerDecision<ToolReceipt> {
    let ledger = ledger_for(session_id);
    let fingerprint = operation_fingerprint(request);
    let operation = operation_id(session_id, &request.part_id, &fingerprint);
    let execution_key = format!("{turn_id}:{}:{fingerprint}", request.part_id);
    let started = Instant::now();
    loop {
        let decision = ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin_tool(
                execution_key.clone(),
                operation.clone(),
                fingerprint.clone(),
                workspace_revision,
                turn_id,
            );
        if !matches!(decision, ToolLedgerDecision::InFlight) {
            return decision;
        }
        if started.elapsed() >= IN_FLIGHT_WAIT_TIMEOUT {
            return ToolLedgerDecision::RejectUnchanged(
                "相同工具操作仍在执行，等待超时，已阻止重复调用".to_owned(),
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(crate) fn settle(
    session_id: &str,
    request: &ToolExecutionRequest,
    receipt: ToolReceipt,
    turn_id: &str,
) {
    let ledger = ledger_for(session_id);
    let mut ledger = ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fingerprint = operation_fingerprint(request);
    ledger.settle_tool(
        format!("{turn_id}:{}:{fingerprint}", request.part_id),
        operation_id(session_id, &request.part_id, &fingerprint),
        receipt,
    );
}

fn ledger_for(session_id: &str) -> SharedToolLedger {
    let mut ledgers = SESSION_LEDGERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !ledgers.contains_key(session_id)
        && ledgers.len() >= MAX_SESSION_LEDGERS
        && let Some(oldest) = ledgers.keys().next().cloned()
    {
        ledgers.remove(&oldest);
    }
    Arc::clone(
        ledgers
            .entry(session_id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(TurnExecutionLedger::default()))),
    )
}

fn operation_id(session_id: &str, part_id: &str, fingerprint: &str) -> String {
    format!("{session_id}:{part_id}:{fingerprint}")
}

pub(crate) fn persisted_operation_id(
    session_id: &str,
    part_id: &str,
    tool_call: &ProviderToolCall,
) -> String {
    operation_id(
        session_id,
        part_id,
        &format!(
            "{}:{}",
            tool_call.dispatch_name(),
            canonical_json(&tool_call.canonical_input())
        ),
    )
}

fn operation_fingerprint(request: &ToolExecutionRequest) -> String {
    let arguments = canonical_json(&request.tool_call.canonical_input());
    format!("{}:{arguments}", request.tool_call.dispatch_name())
}

fn canonical_json(arguments: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return arguments.trim().to_owned();
    };
    sort_json(&mut value);
    serde_json::to_string(&value).unwrap_or_else(|_| arguments.trim().to_owned())
}

fn sort_json(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(object) = value {
        let source = std::mem::take(object);
        let mut entries = source.into_iter().collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (_, child) in &mut entries {
            sort_json(child);
        }
        object.extend(entries);
    } else if let serde_json::Value::Array(values) = value {
        values.iter_mut().for_each(sort_json);
    }
}

pub(crate) fn turn_id(context: &crate::tool_orchestrator::ToolOrchestratorContext) -> String {
    format!(
        "{}:messages:{}",
        context.thread_session_id,
        context.model_messages.len()
    )
}

pub(crate) fn scope_id(context: &crate::tool_orchestrator::ToolOrchestratorContext) -> String {
    context.thread_session_id.clone()
}

pub(crate) fn restore_thread_history(session_id: &str, detail: &ThreadDetail) {
    let ledger = ledger_for(session_id);
    let mut ledger = ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for item in detail.turns.iter().flat_map(|turn| &turn.history_items) {
        match item {
            ThreadHistoryItem::ToolCall {
                operation_id,
                status: ThreadHistoryStatus::InProgress,
                ..
            } => ledger.restore_in_flight(operation_id.clone()),
            ThreadHistoryItem::ToolResult {
                tool_call_id,
                operation_id,
                provider_call_id,
                tool_name,
                content,
                receipt: Some(receipt),
                ..
            } => ledger.restore_settled_tool(
                tool_call_id.clone(),
                operation_id.clone(),
                ToolReceipt {
                    tool_call_id: tool_call_id.parse().unwrap_or_default(),
                    part_id: item.part_id().to_owned(),
                    provider_call_id: provider_call_id.clone(),
                    tool_name: tool_name.clone(),
                    status: history_status_name(receipt.status).to_owned(),
                    summary: content.clone(),
                    metadata: receipt.metadata.clone(),
                    error_type: receipt.error_type.clone(),
                    remote_trigger_allowed: receipt.remote_trigger_allowed,
                    filesystem_write_performed: receipt.filesystem_write_performed,
                    output_ref: receipt.output_ref.clone(),
                    duration_ms: None,
                },
            ),
            _ => {}
        }
    }
}

fn history_status_name(status: ThreadHistoryStatus) -> &'static str {
    match status {
        ThreadHistoryStatus::InProgress => "in_progress",
        ThreadHistoryStatus::Completed => "completed",
        ThreadHistoryStatus::Failed => "failed",
        ThreadHistoryStatus::Interrupted => "interrupted",
        ThreadHistoryStatus::Rejected => "rejected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_orchestrator::ToolKind;
    use axon_domain::{
        ExecutionRequestId, OutputArchiveReference, OutputRef, OutputRefId, ToolCallId,
        ToolExecutionPermission,
    };
    use axon_session::{
        Thread, ThreadDetail, ThreadHistoryItem, ThreadHistoryReceipt, ThreadHistoryStatus,
        ThreadSettings, ThreadStatus, Turn, TurnStatus,
    };
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_nested_objects() {
        assert_eq!(
            canonical_json(r#"{"b":1,"a":{"d":2,"c":3}}"#),
            r#"{"a":{"c":3,"d":2},"b":1}"#
        );
        assert_eq!(canonical_json(&json!({"a":1}).to_string()), r#"{"a":1}"#);
    }

    #[test]
    fn restore_thread_history_reuses_settled_receipt_and_blocks_in_flight_call() {
        let running_session = "restore-running-session";
        let running_call = ProviderToolCall::new(
            "call-running",
            "builtin.read_file",
            r#"{"path":"README.md"}"#,
        );
        let running_operation =
            persisted_operation_id(running_session, "running-part", &running_call);
        let settled_session = "restore-settled-session";
        let settled_call =
            ProviderToolCall::new("call-settled", "mcp.search", r#"{"query":"axon"}"#);
        let settled_operation =
            persisted_operation_id(settled_session, "settled-part", &settled_call);
        let expected_output_ref = OutputRef {
            id: OutputRefId(9),
            byte_len: 12,
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
            source_trace: "tool://mcp/search".to_owned(),
            preview: "命中".to_owned(),
            archive_ref: Some(OutputArchiveReference {
                relative_path: "archive/settled.log".to_owned(),
            }),
        };
        let detail = ThreadDetail {
            thread: Thread {
                thread_id: settled_session.to_owned(),
                session_root_id: settled_session.to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                name: None,
                status: ThreadStatus::Idle,
                source: axon_session::ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: "E:/fixture".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 1,
                updated_at_ms: 1,
                archived_at_ms: None,
            },
            turns: vec![Turn {
                turn_id: "turn-1".to_owned(),
                sequence: 0,
                status: TurnStatus::Completed,
                history_items: vec![
                    ThreadHistoryItem::ToolCall {
                        sequence: 0,
                        provider_turn_id: 1,
                        part_id: "running-part".to_owned(),
                        tool_call_id: "91".to_owned(),
                        operation_id: running_operation.clone(),
                        provider_call_id: running_call.id.clone(),
                        tool_name: running_call.name.clone(),
                        input: running_call.input.clone(),
                        status: ThreadHistoryStatus::InProgress,
                        provider_metadata: None,
                    },
                    ThreadHistoryItem::ToolResult {
                        sequence: 1,
                        provider_turn_id: 1,
                        part_id: "settled-part".to_owned(),
                        tool_call_id: "92".to_owned(),
                        operation_id: settled_operation,
                        provider_call_id: settled_call.id.clone(),
                        tool_name: settled_call.name.clone(),
                        content: "命中".to_owned(),
                        status: ThreadHistoryStatus::Completed,
                        receipt: Some(Box::new(ThreadHistoryReceipt {
                            status: ThreadHistoryStatus::Completed,
                            summary: "命中".to_owned(),
                            metadata: Some(json!({"source":"fixture"})),
                            output_ref: Some(expected_output_ref.clone()),
                            error_type: None,
                            remote_trigger_allowed: false,
                            filesystem_write_performed: false,
                            duration_ms: Some(3),
                        })),
                        provider_metadata: None,
                    },
                ],
                token_usage_json: None,
                started_at_ms: 1,
                completed_at_ms: Some(1),
                interrupted_at_ms: None,
                failure: None,
            }],
            compacted_history: None,
        };

        restore_thread_history(running_session, &detail);
        restore_thread_history(settled_session, &detail);

        let running_request = ToolExecutionRequest {
            tool_call: running_call,
            tool_call_id: ToolCallId(91),
            execution_request_id: ExecutionRequestId(1),
            part_id: "running-part".to_owned(),
            kind: ToolKind::ReadFile,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: ToolExecutionPermission::Allowed,
        };
        let running_fingerprint = operation_fingerprint(&running_request);
        assert_eq!(
            ledger_for(running_session)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .begin_tool(
                    "resume-call",
                    running_operation,
                    running_fingerprint,
                    1,
                    "resume-turn",
                ),
            ToolLedgerDecision::InFlight
        );

        let settled_request = ToolExecutionRequest {
            tool_call: settled_call,
            tool_call_id: ToolCallId(92),
            execution_request_id: ExecutionRequestId(2),
            part_id: "settled-part".to_owned(),
            kind: ToolKind::Search,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: ToolExecutionPermission::Allowed,
        };
        let decision = begin(settled_session, &settled_request, 1, "resume-turn");
        let ToolLedgerDecision::Reuse(receipt) = decision else {
            panic!("恢复后的 settled tool 必须直接复用回执");
        };
        assert_eq!(receipt.provider_call_id, "call-settled");
        assert_eq!(receipt.summary, "命中");
        assert_eq!(receipt.output_ref, Some(expected_output_ref));
    }
}
