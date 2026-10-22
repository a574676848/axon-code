//! rollout 事件模型与事件构造。

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::run_commands::run_session_store::RunSessionRecord;

/// rollout 事件记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolloutEventRecord {
    pub(crate) schema_version: u32,
    pub(crate) session_id: String,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) sequence: u32,
    pub(crate) event_type: RolloutEventType,
    pub(crate) recorded_at_ms: u64,
    pub(crate) payload: serde_json::Value,
}

/// rollout 事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RolloutEventType {
    SessionStarted,
    TurnEvents,
    ToolReceipts,
    ReasoningMessages,
    SessionCompleted,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RolloutEventAppendInput {
    pub(crate) session_id: String,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) event_type: RolloutEventType,
    pub(crate) recorded_at_ms: u64,
    pub(crate) payload: serde_json::Value,
}

pub(super) fn events_for_session(
    record: &RunSessionRecord,
) -> Result<Vec<RolloutEventRecord>, String> {
    let mut sequence = 0;
    let mut events = vec![rollout_event(
        record,
        sequence,
        RolloutEventType::SessionStarted,
        json!({
            "cwd": record.cwd,
            "provider": record.provider,
            "model": record.model,
            "selectedSkillName": record.selected_skill_name,
            "promptChars": record.prompt.chars().count(),
        }),
    )];
    if !record.tool_receipts.is_empty() {
        sequence += 1;
        events.push(rollout_event(
            record,
            sequence,
            RolloutEventType::ToolReceipts,
            json!({
                "toolReceiptCount": record.tool_receipts.len(),
                "outputRefs": record.tool_receipts.iter()
                    .filter_map(|receipt| receipt.output_ref.as_ref())
                    .collect::<Vec<_>>(),
            }),
        ));
    }
    if !record.turn_events.is_empty() {
        sequence += 1;
        events.push(rollout_event(
            record,
            sequence,
            RolloutEventType::TurnEvents,
            json!({
                "turnEventCount": record.turn_events.len(),
                "turnIds": record.turn_events.iter().map(|event| event.turn_id.0).collect::<Vec<_>>(),
            }),
        ));
    }
    if !record.reasoning_messages.is_empty() {
        sequence += 1;
        events.push(rollout_event(
            record,
            sequence,
            RolloutEventType::ReasoningMessages,
            json!({
                "reasoningMessageCount": record.reasoning_messages.len(),
                "reasoningIds": record.reasoning_messages.iter()
                    .map(|message| message.reasoning_id.clone())
                    .collect::<Vec<_>>(),
            }),
        ));
    }
    sequence += 1;
    events.push(rollout_event(
        record,
        sequence,
        RolloutEventType::SessionCompleted,
        json!({
            "responseChars": record.response.chars().count(),
            "hasUsage": record.usage.is_some(),
            "toolReceiptCount": record.tool_receipts.len(),
            "reasoningMessageCount": record.reasoning_messages.len(),
            "selectedSkillName": record.selected_skill_name,
        }),
    ));
    Ok(events)
}

fn rollout_event(
    record: &RunSessionRecord,
    sequence: u32,
    event_type: RolloutEventType,
    payload: serde_json::Value,
) -> RolloutEventRecord {
    RolloutEventRecord {
        schema_version: 1,
        session_id: record.session_id.clone(),
        parent_session_id: record.parent_session_id.clone(),
        sequence,
        event_type,
        recorded_at_ms: record.created_at_ms,
        payload,
    }
}
