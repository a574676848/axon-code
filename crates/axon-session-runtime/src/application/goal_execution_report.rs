//! 后台 Goal 的可选阶段续跑报告。

use serde::Deserialize;

#[cfg(test)]
const WIRE_EVENT_TYPE: &str = "wire_event";

/// 执行者可选声明下一阶段；当前 Execution 的成功由 Executor 返回状态决定。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GoalExecutionReport {
    #[serde(default)]
    pub continuation: Option<GoalStageContinuation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GoalStageContinuation {
    pub stage: String,
    pub prompt: String,
}

pub(super) fn parse(response: &str) -> Option<GoalExecutionReport> {
    serde_json::from_str(response.trim()).ok()
}

#[cfg(test)]
pub(crate) fn verified_test_events(
    session_id: &str,
    _criterion: &str,
    summary: &str,
) -> Vec<crate::model::RuntimeEvent> {
    vec![
        crate::model::RuntimeEvent {
            event_type: WIRE_EVENT_TYPE.to_owned(),
            payload_json: serde_json::json!({
                "schemaVersion": "axon.wire.v3", "eventId": "event-verified", "correlationId": "call-verified",
                "session": {"sessionId": session_id}, "sequence": 1, "recordedAtMs": 20,
                "kind": {"type": "toolResult", "payload": {
                    "partId": "part-verified", "toolCallId": "call-verified", "status": "completed", "summary": summary
                }}
            }).to_string(),
        },
        crate::model::RuntimeEvent {
            event_type: crate::model::TURN_COMPLETED_EVENT.to_owned(),
            payload_json: serde_json::json!({"response": serde_json::json!({
                "summary": summary
            }).to_string()}).to_string(),
        },
    ]
}
