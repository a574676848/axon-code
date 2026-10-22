//! RunSessionRecord 到结构化 Thread 历史 item 的投影。

use std::borrow::Cow;

use axon_domain::ToolCallInput;
use axon_session::{
    ThreadHistoryFileRef, ThreadHistoryImageRef, ThreadHistoryItem, ThreadHistoryReceipt,
    ThreadHistoryStatus,
};
use serde_json::Value;

use super::{run_execution::RunToolReceipt, run_session_store::RunSessionRecord};

/// TUI 使用的借用式历史投影视图。
#[derive(Debug, Clone)]
pub(super) enum HistoryItemView<'a> {
    /// 当轮注入 Provider 的非会话上下文；按原字节回放，不进入 transcript。
    Injected {
        sequence: u64,
        part_id: String,
        message_json: String,
    },
    User {
        sequence: u64,
        part_id: String,
        content: &'a str,
        local_images: &'a [ThreadHistoryImageRef],
        local_files: &'a [ThreadHistoryFileRef],
    },
    Assistant {
        sequence: u64,
        part_id: String,
        content: &'a str,
    },
    Reasoning {
        sequence: u64,
        provider_turn_id: u64,
        part_id: String,
        reasoning_id: &'a str,
        content: &'a str,
    },
    ToolCall {
        sequence: u64,
        provider_turn_id: u64,
        part_id: String,
        tool_call_id: Cow<'a, str>,
        operation_id: String,
        provider_call_id: &'a str,
        tool_name: &'a str,
        input: &'a ToolCallInput,
        status: ThreadHistoryStatus,
        provider_metadata: Option<&'a Value>,
    },
    ToolResult {
        sequence: u64,
        provider_turn_id: u64,
        part_id: &'a str,
        tool_call_id: Cow<'a, str>,
        operation_id: String,
        provider_call_id: &'a str,
        tool_name: &'a str,
        content: &'a str,
        status: ThreadHistoryStatus,
        run_receipt: Option<&'a RunToolReceipt>,
        provider_metadata: Option<&'a Value>,
    },
}

impl HistoryItemView<'_> {
    fn into_owned(self) -> ThreadHistoryItem {
        match self {
            Self::Injected {
                sequence,
                part_id,
                message_json,
            } => ThreadHistoryItem::InjectedContext {
                sequence,
                part_id,
                message_json,
            },
            Self::User {
                sequence,
                part_id,
                content,
                local_images,
                local_files,
            } => ThreadHistoryItem::UserMessage {
                sequence,
                part_id,
                content: content.to_owned(),
                local_images: local_images.to_vec(),
                local_files: local_files.to_vec(),
                provider_metadata: None,
            },
            Self::Assistant {
                sequence,
                part_id,
                content,
            } => ThreadHistoryItem::AssistantMessage {
                sequence,
                part_id,
                content: content.to_owned(),
                provider_metadata: None,
            },
            Self::Reasoning {
                sequence,
                provider_turn_id,
                part_id,
                reasoning_id,
                content,
            } => ThreadHistoryItem::Reasoning {
                sequence,
                provider_turn_id,
                part_id,
                reasoning_id: reasoning_id.to_owned(),
                content: content.to_owned(),
                provider_metadata: Some(serde_json::json!({"reasoningId": reasoning_id})),
            },
            Self::ToolCall {
                sequence,
                provider_turn_id,
                part_id,
                tool_call_id,
                operation_id,
                provider_call_id,
                tool_name,
                input,
                status,
                provider_metadata,
            } => ThreadHistoryItem::ToolCall {
                sequence,
                provider_turn_id,
                part_id,
                tool_call_id: tool_call_id.into_owned(),
                operation_id,
                provider_call_id: provider_call_id.to_owned(),
                tool_name: tool_name.to_owned(),
                input: input.clone(),
                status,
                provider_metadata: provider_metadata.cloned(),
            },
            Self::ToolResult {
                sequence,
                provider_turn_id,
                part_id,
                tool_call_id,
                operation_id,
                provider_call_id,
                tool_name,
                content,
                status,
                run_receipt,
                provider_metadata,
                ..
            } => ThreadHistoryItem::ToolResult {
                sequence,
                provider_turn_id,
                part_id: part_id.to_owned(),
                tool_call_id: tool_call_id.into_owned(),
                operation_id,
                provider_call_id: provider_call_id.to_owned(),
                tool_name: tool_name.to_owned(),
                content: content.to_owned(),
                status,
                receipt: run_receipt.map(|receipt| Box::new(thread_history_receipt(receipt))),
                provider_metadata: provider_metadata.cloned(),
            },
        }
    }
}

pub(super) fn history_items_from_record(
    record: &RunSessionRecord,
    _turn_sequence: u64,
) -> Vec<ThreadHistoryItem> {
    let mut items = Vec::new();
    visit_history_item_views_from_record(record, |item| items.push(item.into_owned()));
    items
}

pub(super) fn visit_history_item_views_from_record(
    record: &RunSessionRecord,
    mut visit: impl FnMut(HistoryItemView<'_>),
) {
    let mut sequence = 0_u64;
    for (index, message) in record.injected_context_messages.iter().enumerate() {
        let Ok(message_json) = serde_json::to_string(message) else {
            // 序列化失败时宁可丢弃该注入项，也不能写入半截 JSON 破坏后续回放。
            continue;
        };
        visit(HistoryItemView::Injected {
            sequence,
            part_id: format!("context:{}:{index}", record.assistant_part_id),
            message_json,
        });
        sequence = sequence.saturating_add(1);
    }
    visit(HistoryItemView::User {
        sequence,
        part_id: format!("user:{}:{}", record.session_id, record.created_at_ms),
        content: &record.prompt,
        local_images: &record.local_images,
        local_files: &record.local_files,
    });
    sequence = sequence.saturating_add(1);

    for (index, tool_call) in record.tool_calls.iter().enumerate() {
        let started_event = tool_call_started_event(record, index);
        let Some(provider_turn_id) = started_event.map(|event| event.turn_id.0) else {
            // 无法确认 provider turn 时不伪造结构化批次 0。
            continue;
        };
        let receipt = tool_receipt_for_call(record, index, started_event);
        let part_id = receipt
            .map(|value| value.part_id.clone())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                started_event
                    .map(|event| event.part_id.clone())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| {
                crate::tool_orchestrator::tool_execution_part_id(
                    provider_turn_id,
                    &tool_call.id,
                    index,
                )
            });
        let tool_call_id = receipt.map_or_else(
            || Cow::Owned(index.saturating_add(1).to_string()),
            |value| Cow::Owned(value.tool_call_id.to_string()),
        );
        let operation_id = crate::turn_execution_ledger::persisted_operation_id(
            &record.session_id,
            &part_id,
            tool_call,
        );
        let status = receipt.map_or(ThreadHistoryStatus::InProgress, |value| {
            history_status(&value.status)
        });
        let provider_metadata = receipt
            .and_then(|value| value.metadata.as_ref())
            .or_else(|| started_event.and_then(|event| event.metadata.as_ref()));
        visit(HistoryItemView::ToolCall {
            sequence,
            provider_turn_id,
            part_id,
            tool_call_id,
            operation_id: operation_id.clone(),
            provider_call_id: &tool_call.id,
            tool_name: &tool_call.name,
            input: &tool_call.input,
            status,
            provider_metadata,
        });
        sequence = sequence.saturating_add(1);
        if let Some(receipt) = receipt {
            visit(HistoryItemView::ToolResult {
                sequence,
                provider_turn_id,
                part_id: &receipt.part_id,
                tool_call_id: Cow::Owned(receipt.tool_call_id.to_string()),
                operation_id: operation_id.clone(),
                provider_call_id: &receipt.provider_call_id,
                tool_name: &receipt.tool_name,
                content: &receipt.summary,
                status,
                run_receipt: Some(receipt),
                provider_metadata: receipt.metadata.as_ref(),
            });
            sequence = sequence.saturating_add(1);
        }
    }
    for reasoning in &record.reasoning_messages {
        let Some(provider_turn_id) = reasoning_provider_turn_id(&reasoning.reasoning_id) else {
            // 无法解析 provider reasoning turn 时不归入未知批次。
            continue;
        };
        visit(HistoryItemView::Reasoning {
            sequence,
            provider_turn_id,
            part_id: reasoning.part_id.clone(),
            reasoning_id: &reasoning.reasoning_id,
            content: &reasoning.content,
        });
        sequence = sequence.saturating_add(1);
    }
    if !record.response.trim().is_empty() {
        visit(HistoryItemView::Assistant {
            sequence,
            part_id: record.assistant_part_id.clone(),
            content: &record.response,
        });
    }
}

pub(super) fn record_has_unsettled_tool_call(record: &RunSessionRecord) -> bool {
    record.tool_calls.iter().enumerate().any(|(index, _)| {
        let started_event = tool_call_started_event(record, index);
        tool_receipt_for_call(record, index, started_event).is_none()
    })
}

fn history_status(status: &str) -> ThreadHistoryStatus {
    match status {
        "completed" => ThreadHistoryStatus::Completed,
        "interrupted" | "cancelled" => ThreadHistoryStatus::Interrupted,
        "blocked" | "rejected" => ThreadHistoryStatus::Rejected,
        "in_progress" | "inProgress" | "running" => ThreadHistoryStatus::InProgress,
        _ => ThreadHistoryStatus::Failed,
    }
}

fn reasoning_provider_turn_id(reasoning_id: &str) -> Option<u64> {
    let turn_id = reasoning_id
        .rsplit_once(':')
        .and_then(|(_, value)| value.parse::<u64>().ok())?;
    (turn_id != crate::tool_orchestrator::NON_PROVIDER_TURN_ID
        && reasoning_id == axon_agent_runtime::agent_run_provider_reasoning_id(turn_id))
    .then_some(turn_id)
}

fn thread_history_receipt(receipt: &RunToolReceipt) -> ThreadHistoryReceipt {
    ThreadHistoryReceipt {
        status: history_status(&receipt.status),
        summary: receipt.summary.clone(),
        metadata: receipt.metadata.clone(),
        output_ref: receipt.output_ref.clone(),
        error_type: receipt.error_type.clone(),
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: receipt.filesystem_write_performed,
        duration_ms: receipt.duration_ms,
    }
}

fn tool_call_started_event(
    record: &RunSessionRecord,
    dispatch_order: usize,
) -> Option<&axon_agent_runtime::AgentTurnEvent> {
    record
        .turn_events
        .iter()
        .filter(|event| event.kind == axon_agent_runtime::AgentTurnEventKind::ToolExecutionStarted)
        .nth(dispatch_order)
}

fn tool_receipt_for_call<'a>(
    record: &'a RunSessionRecord,
    dispatch_order: usize,
    started_event: Option<&axon_agent_runtime::AgentTurnEvent>,
) -> Option<&'a RunToolReceipt> {
    let Some(part_id) = started_event
        .map(|event| event.part_id.as_str())
        .filter(|value| !value.is_empty())
    else {
        return record.tool_receipts.get(dispatch_order);
    };
    record
        .tool_receipts
        .iter()
        .find(|receipt| receipt.part_id == part_id)
}
