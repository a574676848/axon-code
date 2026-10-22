//! 成功 turn 的证据验收、阶段推进和原子结算。

use super::{ExecutionWorker, ExecutionWorkerError, MAX_EXECUTION_ATTEMPTS};
use crate::model::{NextExecutionRequest, TURN_COMPLETED_EVENT, TurnExecutionOutcome};
use crate::ports::{EventPublisher, RuntimeClock, TurnExecutor};
use axon_domain::{
    ExecutionId, ExecutionStatus, GoalStatus, JarvisEvent, JarvisEventId, ResultErrorKind,
    ResultId, ResultRecord, SessionActionId, SessionActionStatus,
};
use axon_session::{CheckpointTelemetry, ExecutionSettlement, RuntimeStore, SessionActionRecord};

use axon_domain::GOAL_RESULT_SUMMARY_MAX_CHARS as MAX_RESULT_SUMMARY_CHARS;

impl<S, E, P, C> ExecutionWorker<S, E, P, C>
where
    S: RuntimeStore,
    E: TurnExecutor,
    P: EventPublisher,
    C: RuntimeClock,
{
    pub(super) fn commit_outcome(
        &self,
        action: &SessionActionRecord,
        outcome: TurnExecutionOutcome,
        checkpoint: &CheckpointTelemetry,
    ) -> Result<(), ExecutionWorkerError> {
        let now_ms = self.clock.now_ms();
        let mut next = outcome
            .next_execution
            .map(|request| next_action(action, request))
            .transpose()?;
        let events = outcome
            .events
            .into_iter()
            .map(|event| (event.event_type, event.payload_json))
            .collect::<Vec<_>>();
        let settlement = self.outcome_settlement(action, &events, &mut next, now_ms)?;
        let records = self.store.commit_action(
            &action.action_id,
            &self.session_id,
            &self.host,
            &events,
            next.as_ref(),
            settlement.as_ref(),
            checkpoint,
            now_ms,
        )?;
        for record in records {
            self.publisher
                .publish(&record, &record.cursor())
                .map_err(ExecutionWorkerError::Publish)?;
        }
        Ok(())
    }

    fn outcome_settlement(
        &self,
        action: &SessionActionRecord,
        events: &[(String, String)],
        next: &mut Option<SessionActionRecord>,
        now_ms: u64,
    ) -> Result<Option<ExecutionSettlement>, ExecutionWorkerError> {
        let id = ExecutionId::new(action.action_id.as_str())
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
        let Some(mut execution) = self
            .store
            .get_execution(&id)
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
        else {
            if action.goal_id.is_some() {
                return Err(ExecutionWorkerError::Gate(
                    "Goal Execution 不存在".to_owned(),
                ));
            }
            return Ok(None);
        };
        let Some(mut goal) = self
            .store
            .get_goal(&execution.goal_id)
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
        else {
            if action.goal_id.is_some() && execution.goal_id.as_str() != action.action_id.as_str() {
                return Err(ExecutionWorkerError::Gate("Goal 不存在".to_owned()));
            }
            return Ok(None);
        };
        if goal.status.is_terminal() {
            return Err(ExecutionWorkerError::Gate(
                "Goal 已进入终态，终止提交".to_owned(),
            ));
        }
        let lease = execution
            .lease
            .clone()
            .ok_or_else(|| ExecutionWorkerError::Gate("Execution 缺少结算 lease".to_owned()))?;
        let response = final_response(events);
        let receipts = successful_receipt_event_ids(events);
        let exhausted = next.is_some() && action.attempt >= MAX_EXECUTION_ATTEMPTS;
        let succeeded = !exhausted;
        goal.status = if exhausted {
            GoalStatus::Failed
        } else if next.is_some() {
            GoalStatus::Active
        } else {
            GoalStatus::Completed
        };
        goal.waiting_reason = None;
        if goal.status != GoalStatus::Active {
            *next = None;
        }
        let expected_goal_revision = goal.revision;
        goal.revision = goal
            .revision
            .checked_add(1)
            .ok_or_else(|| ExecutionWorkerError::Gate("Goal revision 溢出".to_owned()))?;
        goal.updated_at_ms = now_ms;
        let summary = if exhausted {
            "尝试多次未产生进展，自动终止".to_owned()
        } else {
            response.unwrap_or_else(|| format!("Execution {id} 完成"))
        };
        let mut artifact_refs = Vec::new();
        if summary.chars().nth(MAX_RESULT_SUMMARY_CHARS).is_some() {
            let path = self
                .store
                .save_goal_result_document(&action.session_id, &id, &summary)?;
            artifact_refs.push(path);
        }
        let result = ResultRecord {
            result_id: ResultId::new(format!("result-{id}"))
                .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?,
            goal_id: goal.goal_id.clone(),
            execution_id: id.clone(),
            succeeded,
            summary: truncate_summary(&summary),
            receipt_refs: receipts,
            artifact_refs,
            error_kind: (!succeeded).then_some(ResultErrorKind::Runtime),
            created_at_ms: now_ms,
        };
        execution.status = if succeeded {
            ExecutionStatus::Completed
        } else {
            ExecutionStatus::Failed
        };
        execution.updated_at_ms = now_ms;
        let event_type = if succeeded {
            "execution.completed"
        } else {
            "execution.failed"
        };
        let event = JarvisEvent {
            event_id: JarvisEventId::new(format!("event-outcome-{id}"))
                .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?,
            sequence: 0,
            event_type: event_type.to_owned(),
            idempotency_key: format!("{event_type}:{id}"),
            cause_event_id: None,
            payload_json: serde_json::to_string(&result)
                .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?,
            occurred_at_ms: now_ms,
        };
        Ok(Some(ExecutionSettlement {
            execution,
            owner: lease.owner,
            lease_token: lease.token,
            result,
            goal,
            expected_goal_revision,
            event,
        }))
    }
}

fn next_action(
    action: &SessionActionRecord,
    request: NextExecutionRequest,
) -> Result<SessionActionRecord, ExecutionWorkerError> {
    let mut payload: serde_json::Value = serde_json::from_str(&request.payload_json)
        .map_err(|error| ExecutionWorkerError::Execution(error.to_string()))?;
    if !payload.is_object() {
        return Err(ExecutionWorkerError::Execution(
            "续跑 payload 必须为 JSON 对象".to_owned(),
        ));
    }
    if payload
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|prompt| prompt.trim().is_empty())
    {
        return Err(ExecutionWorkerError::Execution(
            "续跑必须给出可执行的下一阶段指令".to_owned(),
        ));
    }
    let original: serde_json::Value = serde_json::from_str(&action.payload_json)
        .map_err(|error| ExecutionWorkerError::Execution(error.to_string()))?;
    let mut context = original
        .get("transportContext")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if action.goal_id.is_some() {
        let stage = request
            .stage
            .as_deref()
            .map(str::trim)
            .filter(|stage| !stage.is_empty())
            .ok_or_else(|| {
                ExecutionWorkerError::Execution("Goal 续跑必须提供阶段标识".to_owned())
            })?;
        let mut visited = context
            .get("visitedStages")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(current) = context.get("stage").and_then(serde_json::Value::as_str)
            && !visited.iter().any(|item| item.as_str() == Some(current))
        {
            visited.push(serde_json::json!(current));
        }
        if visited.iter().any(|item| item.as_str() == Some(stage)) {
            return Err(ExecutionWorkerError::Execution(
                "Goal 不允许重复已执行阶段".to_owned(),
            ));
        }
        context["visitedStages"] = serde_json::json!(visited);
    }
    if let Some(context) = context.as_object_mut() {
        context.insert("goalId".to_owned(), serde_json::json!(action.goal_id));
        context.insert(
            "parentExecutionId".to_owned(),
            serde_json::json!(action.action_id),
        );
        context.insert("stage".to_owned(), serde_json::json!(request.stage));
    }
    payload["transportContext"] = context;
    Ok(SessionActionRecord {
        action_id: SessionActionId::new(&request.dedupe_key)
            .map_err(|error| ExecutionWorkerError::Execution(error.to_string()))?,
        goal_id: action.goal_id.clone(),
        session_id: action.session_id.clone(),
        kind: request.kind,
        priority: request.priority,
        status: SessionActionStatus::Queued,
        dedupe_key: Some(request.dedupe_key),
        payload_json: payload.to_string(),
        goal_revision: action.goal_revision,
        context_version: action.context_version,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    })
}

fn final_response(events: &[(String, String)]) -> Option<String> {
    events
        .iter()
        .rev()
        .find(|(kind, _)| kind == TURN_COMPLETED_EVENT)
        .and_then(|(_, payload)| serde_json::from_str::<serde_json::Value>(payload).ok())
        .and_then(|payload| payload.get("response").cloned())
        .and_then(|response| {
            let text = response.as_str()?.to_owned();
            serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|report| {
                    report
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .or(Some(text))
        })
        .filter(|response| !response.trim().is_empty())
}

fn successful_receipt_event_ids(events: &[(String, String)]) -> Vec<String> {
    events
        .iter()
        .filter(|(kind, _)| kind == "wire_event")
        .filter_map(|(_, payload)| {
            let event =
                serde_json::from_str::<axon_protocol::wire::WireEventEnvelope>(payload).ok()?;
            match event.kind {
                axon_protocol::wire::WireEventKind::ToolResult(result)
                    if result.status == axon_protocol::wire::WireLifecycleStatus::Completed
                        && !result.tool_call_id.trim().is_empty() =>
                {
                    Some(event.event_id.0)
                }
                _ => None,
            }
        })
        .collect()
}

fn truncate_summary(text: &str) -> String {
    let mut chars = text.chars();
    let head = chars
        .by_ref()
        .take(MAX_RESULT_SUMMARY_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
