use super::*;
use crate::model::{NextExecutionRequest, TurnExecutionError};
use axon_domain::{
    ConsumerCursor, DecisionEffect, DecisionId, GoalId, GoalPlan, GoalRecord, GoalStatus,
    RuntimeHostId, ScopeRef, SessionActionId, SessionActionStatus,
};
use axon_session::{
    CheckpointTelemetry, DecisionStore, ExecutionStore, GoalStore, RuntimeStore,
    SessionEventRecord, SqliteRuntimeStore,
};

struct NextExecutionExecutor;

impl TurnExecutor for NextExecutionExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: crate::application::goal_execution_report::verified_test_events(
                "next-task-session",
                "Goal ID 不变",
                "当前阶段已核验",
            ),
            next_execution: Some(NextExecutionRequest {
                goal_id: None,
                parent_execution_id: None,
                stage: Some("follow_up".to_owned()),
                transport_context_json: None,
                kind: "follow_up".to_owned(),
                priority: 1,
                dedupe_key: "next-execution".to_owned(),
                payload_json: serde_json::json!({"prompt":"核验后续阶段"}).to_string(),
                goal_revision: None,
                context_version: None,
            }),
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[derive(Clone, Copy)]
struct FixedClock(u64);

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn next_execution_inherits_goal_id_when_execution_id_differs() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("next-task-host").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("next-task-session").expect("session");
    store
        .create_session(&session_id, "cwd", 10)
        .expect("session");
    let goal_id = GoalId::new("next-task").expect("task");
    let mut task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "下一次执行继承 Task",
        GoalPlan {
            objective: "验证 Task 关联".to_owned(),
            steps: vec!["提交下一次 Execution".to_owned()],
        },
        vec!["Goal ID 不变".to_owned()],
        10,
    )
    .expect("task");
    task.transition(GoalStatus::Active, 11).expect("active");
    store.create_goal(&task).expect("save task");
    store
        .append_decision(
            &axon_domain::DecisionRecord {
                decision_id: DecisionId::new("next-task-allow").expect("decision"),
                scope: ScopeRef::global(),
                actor: "test".to_owned(),
                effect: DecisionEffect::Allow,
                revision: 1,
                expires_at_ms: None,
                created_at_ms: 12,
            },
            0,
        )
        .expect("allow");
    let action = SessionActionRecord {
        action_id: SessionActionId::new("current-execution").expect("action"),
        goal_id: Some(goal_id.clone()),
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 0,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: serde_json::json!({"prompt": "执行"}).to_string(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 13).expect("enqueue");
    let worker = ExecutionWorker::new(
        store.clone(),
        NextExecutionExecutor,
        NoopPublisher,
        FixedClock(20),
        host,
        session_id,
        100,
    );

    worker.pump_once().expect("Worker 执行");
    let next = store
        .get_execution(&axon_domain::ExecutionId::new("next-execution").expect("execution"))
        .expect("读取下一次 Execution")
        .expect("下一次 Execution 存在");
    assert_eq!(next.goal_id, goal_id);
}
