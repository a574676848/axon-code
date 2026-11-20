//! Goal 工具账本事件的构造与回执。
//!
//! 这些函数只把领域命令翻译成账本事件与结构化回执，不读运行态、不写存储，
//! 因此与目标工具的命令映射分开：命令映射关心「按可见性与版本执行哪条命令」，
//! 这里只关心「这次变更在账本上留下什么事实」。

use axon_domain::{GoalCreateCommand, GoalId, LedgerEvent, LedgerEventId, ScopeRef};
use serde_json::{Value, json};

use super::DaemonHostError;
use super::daemon_host_goal_tools::{GOAL_CREATED_EVENT_TYPE, GOAL_EVENT_ID_PREFIX};

/// 构造 GoalCreated Event；payload 只暴露 Goal 身份与目标，不泄露内部执行细节。
pub(super) fn goal_created_event(
    scope: &ScopeRef,
    command: &GoalCreateCommand,
    goal_id: &GoalId,
    now_ms: u64,
) -> Result<LedgerEvent, DaemonHostError> {
    let event_key = format!("{GOAL_EVENT_ID_PREFIX}{goal_id}");
    Ok(LedgerEvent {
        event_id: LedgerEventId::new(event_key.clone())
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
        sequence: 0,
        event_type: GOAL_CREATED_EVENT_TYPE.to_owned(),
        idempotency_key: event_key,
        cause_event_id: None,
        payload_json: serde_json::to_string(&json!({
            "goal_id": goal_id.as_str(),
            "scope": scope,
            "title": command.title,
            "create_execution": command.create_execution,
        }))
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
        occurred_at_ms: now_ms,
    })
}

/// 构造 GoalStarted Event；同一目标同一 revision 只落一条。
pub(super) fn goal_started_event(
    goal_id: &GoalId,
    revision: u64,
    now_ms: u64,
) -> Result<LedgerEvent, DaemonHostError> {
    let event_key = format!("goal.started:{}:{}", goal_id.as_str(), revision);
    Ok(LedgerEvent {
        event_id: LedgerEventId::new(format!("event-{event_key}"))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
        sequence: 0,
        event_type: "goal.started".to_owned(),
        idempotency_key: event_key,
        cause_event_id: None,
        payload_json: serde_json::json!({
            "goal_id": goal_id.as_str(),
            "revision": revision,
        })
        .to_string(),
        occurred_at_ms: now_ms,
    })
}

/// 目标写工具的统一回执：标识、状态、版本与是否新建。
pub(super) fn goal_receipt(tool: &str, goal: &axon_domain::GoalRecord, created: bool) -> Value {
    json!({
        "tool": tool,
        "goalId": goal.goal_id,
        "status": goal.status,
        "revision": goal.revision,
        "created": created,
    })
}

/// 目标服务的错误统一收敛成宿主工作错误。
pub(super) fn goal_service_error(error: impl std::fmt::Display) -> DaemonHostError {
    DaemonHostError::Worker(error.to_string())
}
