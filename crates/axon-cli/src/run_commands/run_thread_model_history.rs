//! Thread 结构化历史到 runtime 模型消息的投影。

use std::collections::{BTreeMap, HashMap, HashSet};

use axon_agent_runtime::{
    AgentModelLocalFileRef, AgentModelLocalImageRef, AgentModelMessage, AgentRunToolOutputPolicy,
    AgentRunToolResultReceipt, ProviderToolCall, agent_run_tool_calls_message,
    agent_run_tool_result_messages,
};
use axon_session::{
    ThreadDetail, ThreadHistoryFileRef, ThreadHistoryImageRef, ThreadHistoryItem,
    ThreadHistoryStatus, Turn,
};

fn thread_history_json(detail: &ThreadDetail) -> Result<String, String> {
    model_messages_from_turns(&detail.turns)
}

pub(crate) fn thread_model_history_json(detail: &ThreadDetail) -> Result<String, String> {
    let Some(compaction) = detail.compacted_history.as_ref() else {
        return thread_history_json(detail);
    };
    let mut messages = serde_json::from_str::<Vec<serde_json::Value>>(&compaction.history_json)
        .map_err(|error| format!("压缩历史 JSON 无效：{error}"))?;
    let recent_turns = detail
        .turns
        .iter()
        .filter(|turn| turn.sequence > compaction.through_sequence);
    append_turn_model_messages(&mut messages, recent_turns);
    serde_json::to_string(&messages).map_err(|error| format!("历史序列化失败：{error}"))
}

fn model_messages_from_turns(turns: &[Turn]) -> Result<String, String> {
    let mut messages = Vec::new();
    append_turn_model_messages(&mut messages, turns.iter());
    serde_json::to_string(&messages).map_err(|error| format!("历史序列化失败：{error}"))
}

fn append_turn_model_messages<'a>(
    messages: &mut Vec<serde_json::Value>,
    turns: impl IntoIterator<Item = &'a Turn>,
) {
    for turn in turns {
        for message in model_messages_from_turn(turn) {
            if let Ok(value) = model_message_history_value(&message) {
                messages.push(value);
            }
        }
    }
}

fn model_message_history_value(
    message: &AgentModelMessage,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(message)
}

fn model_messages_from_turn(turn: &Turn) -> Vec<AgentModelMessage> {
    let batches = collect_tool_batches(turn);
    let mut emitted_batches = HashSet::new();
    let mut messages = Vec::new();
    for item in &turn.history_items {
        // 注入上下文按原序原字节回放：它决定跨轮消息前缀能否命中 Provider 缓存。
        if let ThreadHistoryItem::InjectedContext { message_json, .. } = item {
            if let Ok(message) = serde_json::from_str::<AgentModelMessage>(message_json) {
                messages.push(message);
            }
            continue;
        }
        if let Some(provider_turn_id) = item.provider_turn_id() {
            let Some(batch) = batches.get(&provider_turn_id) else {
                continue;
            };
            if batch.call_indices.is_empty() {
                if let ThreadHistoryItem::Reasoning { content, .. } = item
                    && !content.trim().is_empty()
                {
                    messages.push(AgentModelMessage::text("assistant", content));
                }
                continue;
            }
            if !emitted_batches.insert(provider_turn_id) {
                continue;
            }
            messages.extend(project_tool_batch(turn, batch));
            continue;
        }
        match item {
            ThreadHistoryItem::UserMessage {
                content,
                local_images,
                local_files,
                ..
            } if !content.trim().is_empty()
                || !local_images.is_empty()
                || !local_files.is_empty() =>
            {
                messages.push(user_message_from_history(
                    content,
                    local_images,
                    local_files,
                ));
            }
            ThreadHistoryItem::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                messages.push(AgentModelMessage::text("assistant", content));
            }
            ThreadHistoryItem::UserMessage { .. } | ThreadHistoryItem::AssistantMessage { .. } => {}
            ThreadHistoryItem::InjectedContext { .. } => {
                unreachable!("注入上下文已在进入批次匹配前按原序回放")
            }
            ThreadHistoryItem::Reasoning { .. }
            | ThreadHistoryItem::ToolCall { .. }
            | ThreadHistoryItem::ToolResult { .. } => {
                unreachable!("带 provider_turn_id 的结构化 item 必须进入工具批次路径")
            }
        }
    }
    messages
}

fn user_message_from_history(
    content: &str,
    images: &[ThreadHistoryImageRef],
    files: &[ThreadHistoryFileRef],
) -> AgentModelMessage {
    AgentModelMessage::text("user", content)
        .with_local_images(images.iter().map(model_image_ref).collect())
        .with_local_files(files.iter().map(model_file_ref).collect())
}

fn model_image_ref(image: &ThreadHistoryImageRef) -> AgentModelLocalImageRef {
    AgentModelLocalImageRef {
        path: image.path.clone(),
        mime_type: image.mime_type.clone(),
        byte_len: image.byte_len,
        sha256: image.sha256.clone(),
        archive_ref: image.archive_ref.clone(),
        source_ref: image.source_ref.clone(),
    }
}

fn model_file_ref(file: &ThreadHistoryFileRef) -> AgentModelLocalFileRef {
    AgentModelLocalFileRef {
        name: file.name.clone(),
        path: file.path.clone(),
        mime_type: file.mime_type.clone(),
        preview: file.preview.clone(),
        byte_len: file.byte_len,
        sha256: file.sha256.clone(),
        archive_ref: file.archive_ref.clone(),
        source_ref: file.source_ref.clone(),
    }
}

#[derive(Default)]
struct ToolBatch {
    call_indices: Vec<usize>,
    result_by_part_id: HashMap<String, usize>,
    reasoning_indices: Vec<usize>,
}

fn collect_tool_batches(turn: &Turn) -> BTreeMap<u64, ToolBatch> {
    let mut batches = BTreeMap::new();
    for (index, item) in turn.history_items.iter().enumerate() {
        match item {
            ThreadHistoryItem::ToolCall {
                provider_turn_id, ..
            } => batches
                .entry(*provider_turn_id)
                .or_insert_with(ToolBatch::default)
                .call_indices
                .push(index),
            ThreadHistoryItem::ToolResult {
                provider_turn_id,
                part_id,
                ..
            } => {
                batches
                    .entry(*provider_turn_id)
                    .or_insert_with(ToolBatch::default)
                    .result_by_part_id
                    .insert(part_id.clone(), index);
            }
            ThreadHistoryItem::Reasoning {
                provider_turn_id, ..
            } => batches
                .entry(*provider_turn_id)
                .or_insert_with(ToolBatch::default)
                .reasoning_indices
                .push(index),
            ThreadHistoryItem::UserMessage { .. }
            | ThreadHistoryItem::AssistantMessage { .. }
            | ThreadHistoryItem::InjectedContext { .. } => {}
        }
    }
    batches
}

fn project_tool_batch(turn: &Turn, batch: &ToolBatch) -> Vec<AgentModelMessage> {
    let calls = batch
        .call_indices
        .iter()
        .filter_map(|index| match &turn.history_items[*index] {
            ThreadHistoryItem::ToolCall {
                provider_call_id,
                tool_name,
                input,
                ..
            } => Some(ProviderToolCall::new(
                provider_call_id.clone(),
                tool_name.clone(),
                input.clone(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return Vec::new();
    }
    let reasoning = batch
        .reasoning_indices
        .iter()
        .filter_map(|index| match &turn.history_items[*index] {
            ThreadHistoryItem::Reasoning { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .filter(|content| !content.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let reasoning = (!reasoning.is_empty()).then_some(reasoning.as_str());
    let mut settled_calls = Vec::new();
    let mut settled_receipts = Vec::new();
    for (call_index, call) in batch.call_indices.iter().zip(calls.iter()) {
        let ThreadHistoryItem::ToolCall { part_id, .. } = &turn.history_items[*call_index] else {
            continue;
        };
        let Some(result_index) = batch.result_by_part_id.get(part_id) else {
            continue;
        };
        let Some(ThreadHistoryItem::ToolResult {
            receipt: Some(receipt),
            ..
        }) = turn.history_items.get(*result_index)
        else {
            continue;
        };
        settled_calls.push(call.clone());
        settled_receipts.push(AgentRunToolResultReceipt {
            provider_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            status: history_status_name(receipt.status),
            summary: receipt.summary.clone(),
            metadata: receipt.metadata.clone(),
            output_ref: receipt.output_ref.clone(),
            error_type: receipt.error_type.clone(),
            remote_trigger_allowed: receipt.remote_trigger_allowed,
            filesystem_write_performed: receipt.filesystem_write_performed,
        });
    }
    let mut projected = vec![agent_run_tool_calls_message(&calls, reasoning)];
    if settled_calls.is_empty() {
        return projected;
    }
    if let Ok(mut result_messages) = agent_run_tool_result_messages(
        &settled_calls,
        &settled_receipts,
        None,
        AgentRunToolOutputPolicy::default(),
    ) {
        result_messages.remove(0);
        projected.extend(result_messages);
    }
    projected
}

fn history_status_name(status: ThreadHistoryStatus) -> String {
    match status {
        ThreadHistoryStatus::InProgress => "in_progress",
        ThreadHistoryStatus::Completed => "completed",
        ThreadHistoryStatus::Failed => "failed",
        ThreadHistoryStatus::Interrupted => "interrupted",
        ThreadHistoryStatus::Rejected => "rejected",
    }
    .to_owned()
}
