//! Thread 结构化历史到 runtime 模型消息的投影。

use std::collections::{BTreeMap, HashMap, HashSet};

use axon_agent_runtime::{
    AgentModelLocalFileRef, AgentModelLocalImageRef, AgentModelMessage, AgentRunToolOutputPolicy,
    AgentRunToolResultReceipt, ProviderToolCall, agent_run_tool_calls_message,
    agent_run_tool_result_messages,
};
use axon_session::{
    ThreadCompaction, ThreadDetail, ThreadHistoryFileRef, ThreadHistoryImageRef, ThreadHistoryItem,
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
    // `through_sequence` 是**首个仍然可见的轮次**：换窗那轮自己留在窗口里（它的回复在换窗
    // 之后才产生，压缩摘要里不可能有），此前的轮次已经并入摘要。因此边界取包含。
    let through_sequence = effective_through_sequence(detail, compaction);
    let recent_turns = detail
        .turns
        .iter()
        .filter(|turn| turn.sequence >= through_sequence);
    let recent = collect_turn_model_messages(recent_turns);
    // 换窗可能发生在多步 turn 中途：那一轮的 user 消息已经落过 thread，交接摘要的尾部
    // 也带了同一条输入。拼接前剔除重复的一条，否则重启恢复后模型会把同一句话看两次。
    let recent = drop_repeated_leading_user(&messages, recent);
    messages.extend(
        recent
            .into_iter()
            .filter_map(|message| model_message_history_value(&message).ok()),
    );
    serde_json::to_string(&messages).map_err(|error| format!("历史序列化失败：{error}"))
}

/// 解析真正生效的窗口边界，修正两类会让历史装配失真的快照。
///
/// 一是 0：可能是这次换窗就发生在第一轮（那时它是唯一可见轮次，0 正确），也可能是旧写入
/// 把起跑占位轮次的 0 当成了边界。仅凭快照字段分不出来，但换窗时刻本身是证据：换窗那轮
/// 在换窗之前就已起跑（`started_at_ms <= recorded_at_ms`），它必须留在窗口里，更早的轮次
/// 已经并入摘要。按这个口径反推既不误伤合法的 0，也不会把已归档轮次整段装回模型视图
/// （真机现场：710 条约 1.05M tokens，而 Provider 实测只有 143K）。
///
/// 二是边界越过全部轮次：边界按「本轮收口后会拿到的轮序」写入，若那轮最终没有收口
/// （被中断、进程被杀、或换窗后直接结束），这个号码从未落到任何轮次上。照它过滤会把整条
/// 历史清空，只剩上一个窗口的 checkpoint，模型于是丢掉最近所有动作、把已经做过的事重做
/// 一遍（真机现场：边界 40 而最大轮序 39，可见轮次 0 条）。这种情况下回退到最新已收口轮，
/// 宁可多带一轮，也不让窗口交接到空的历史上。
fn effective_through_sequence(detail: &ThreadDetail, compaction: &ThreadCompaction) -> u64 {
    let declared = compaction.through_sequence;
    let boundary = if declared > 0 {
        declared
    } else {
        let rebuilt = detail
            .turns
            .iter()
            .filter(|turn| turn.started_at_ms <= compaction.recorded_at_ms)
            .map(|turn| turn.sequence)
            .max()
            .unwrap_or(0);
        if rebuilt > 0 {
            axon_logger::warning(&format!(
                "窗口轮转边界为 0 但换窗时已有更早轮次，已按换窗那轮的轮序 {rebuilt} 修正：thread={} turns={}",
                detail.thread.thread_id,
                detail.turns.len()
            ));
        }
        rebuilt
    };
    if boundary == 0 || detail.turns.is_empty() {
        return boundary;
    }
    if detail.turns.iter().any(|turn| turn.sequence >= boundary) {
        return boundary;
    }
    let fallback = detail
        .turns
        .iter()
        .map(|turn| turn.sequence)
        .max()
        .unwrap_or(0);
    axon_logger::warning(&format!(
        "窗口轮转边界 {boundary} 越过全部轮次，已回退到最新轮 {fallback}：换窗那轮没有收口，thread={} turns={}",
        detail.thread.thread_id,
        detail.turns.len()
    ));
    fallback
}

/// 收集若干轮次投影出的模型消息，不写入外部缓冲。
fn collect_turn_model_messages<'a>(
    turns: impl IntoIterator<Item = &'a Turn>,
) -> Vec<AgentModelMessage> {
    let mut messages = Vec::new();
    for turn in turns {
        messages.extend(model_messages_from_turn(turn));
    }
    messages
}

/// 丢弃续接部分里与已装配历史末条 user 重复的那一条。
///
/// 只比较纯文本 user 内容：多模态、空内容或与末条不同的提问一律不动，宁可保留一次
/// 重复也不误删用户输入。
fn drop_repeated_leading_user(
    previous: &[serde_json::Value],
    recent: Vec<AgentModelMessage>,
) -> Vec<AgentModelMessage> {
    let Some(first) = recent.first() else {
        return recent;
    };
    if first.role != "user" {
        return recent;
    }
    let Ok(current) = model_message_history_value(first) else {
        return recent;
    };
    let Some(text) = current
        .get("content")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())
    else {
        return recent;
    };
    let last_user_text = previous
        .iter()
        .rev()
        .find(|value| value.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        .and_then(|value| value.get("content"))
        .and_then(serde_json::Value::as_str);
    if last_user_text != Some(text) {
        return recent;
    }
    recent.into_iter().skip(1).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn user_value(text: &str) -> serde_json::Value {
        serde_json::json!({"role": "user", "content": text})
    }

    /// 多步 turn 中途换窗后，续接部分的首条提问与摘要尾部同文时只保留一条。
    #[test]
    fn repeated_leading_user_is_dropped_once() {
        let previous = vec![user_value("同一句提问")];
        let recent = vec![
            AgentModelMessage::text("user", "同一句提问"),
            AgentModelMessage::text("assistant", "回答"),
        ];
        let kept = drop_repeated_leading_user(&previous, recent);
        assert_eq!(kept.len(), 1, "重复的用户提问必须只留一条");
        assert_eq!(kept[0].role, "assistant");
    }

    /// 不同提问、非 user 开头、或空内容都不判重，宁可保留也不误删用户输入。
    #[test]
    fn non_matching_leading_message_is_kept() {
        let previous = vec![user_value("另一句提问")];
        let different = vec![AgentModelMessage::text("user", "新提问")];
        assert_eq!(
            drop_repeated_leading_user(&previous, different).len(),
            1,
            "内容不同的提问不得被丢弃"
        );

        let assistant_first = vec![AgentModelMessage::text("assistant", "继续")];
        assert_eq!(
            drop_repeated_leading_user(&previous, assistant_first).len(),
            1,
            "首条不是 user 时不得丢弃"
        );

        let empty = vec![AgentModelMessage::text("user", "")];
        assert_eq!(
            drop_repeated_leading_user(&[user_value("")], empty).len(),
            1,
            "空内容不得参与判重"
        );
    }
}
