use super::*;
use crate::model::TurnExecutionError;
use axon_domain::{
    ConsumerCursor, DecisionEffect, DecisionId, GoalId, GoalPlan, GoalRecord, GoalStatus,
    RuntimeHostId, ScopeRef,
};
use axon_session::{
    CheckpointTelemetry, DecisionStore, GoalStore, RuntimeStore, SessionEventRecord,
    SqliteRuntimeStore,
};

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

fn action(session_id: &DaemonSessionId, action_id: &str, prompt: &str) -> SessionActionRecord {
    SessionActionRecord {
        action_id: SessionActionId::new(action_id).expect("action"),
        goal_id: None,
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 0,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: serde_json::json!({"prompt": prompt}).to_string(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    }
}

#[test]
fn no_progress_without_receipts_terminates_task_as_failed() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("worker-host-np").expect("host"),
            1,
            None,
            10,
            1_000,
        )
        .expect("claim");
    let session_id = DaemonSessionId::new("worker-session-np").expect("session");
    let goal_id = GoalId::new("task-no-prog").expect("goal_id");
    let task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "空转无进展任务",
        GoalPlan {
            objective: "多次尝试但无成果".to_owned(),
            steps: vec!["步骤 1".to_owned()],
        },
        vec!["仅验证执行逻辑".to_owned()],
        10,
    )
    .expect("task");
    store.create_goal(&task).expect("create_goal");
    store
        .create_session(&session_id, "cwd", 11)
        .expect("create_session");
    store
        .append_decision(
            &axon_domain::DecisionRecord {
                decision_id: DecisionId::new("allow-np-1").expect("decision"),
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
    let mut action = action(&session_id, "action-np-1", "尝试执行");
    action.goal_id = Some(goal_id.clone());
    action.attempt = MAX_EXECUTION_ATTEMPTS;
    store.enqueue_action(&action, 13).expect("enqueue");

    #[derive(Clone)]
    struct LoopExecutor;
    impl TurnExecutor for LoopExecutor {
        fn execute(
            &self,
            _request: ActionExecutionRequest,
        ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
            Ok(TurnExecutionOutcome {
                events: Vec::new(),
                next_execution: Some(crate::model::NextExecutionRequest {
                    goal_id: None,
                    parent_execution_id: None,
                    stage: Some("retry".to_owned()),
                    transport_context_json: None,
                    kind: "user_input".to_owned(),
                    priority: 0,
                    dedupe_key: "loop-next".to_owned(),
                    payload_json: serde_json::json!({"prompt":"继续核验"}).to_string(),
                    goal_revision: None,
                    context_version: None,
                }),
                checkpoint: CheckpointTelemetry::default(),
            })
        }
    }

    let worker = ExecutionWorker::new(
        store.clone(),
        LoopExecutor,
        NoopPublisher,
        FixedClock(20),
        host,
        session_id,
        100,
    );
    assert_eq!(worker.pump_once().expect("pump"), Some(action.action_id));

    let updated_goal = store
        .get_goal(&goal_id)
        .expect("get_goal")
        .expect("goal exists");
    assert_eq!(updated_goal.status, GoalStatus::Failed);
    let unread_results =
        axon_session::ResultStore::list_unread_results(&store, &ScopeRef::global(), 10)
            .expect("list_unread");
    assert_eq!(unread_results.len(), 1);
    assert!(!unread_results[0].succeeded);
    assert!(unread_results[0].summary.contains("尝试多次未产生进展"));
}

/// 成功 Execution 的结果摘要必须承载执行者最终结论。
#[test]
fn successful_execution_summary_prefers_final_response_over_decision_objective() {
    const FINAL_REPORT: &str = "已执行 Start-Sleep -Seconds 30，实际耗时 30.025 秒";
    const OBJECTIVE: &str = "在后台执行命令 Start-Sleep -Seconds 30，记录时间并报告实际耗时";

    #[derive(Clone, Copy)]
    struct ReportExecutor;
    impl TurnExecutor for ReportExecutor {
        fn execute(
            &self,
            _request: ActionExecutionRequest,
        ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
            Ok(TurnExecutionOutcome {
                events: crate::application::goal_execution_report::verified_test_events(
                    "summary-session",
                    "报告实际耗时",
                    FINAL_REPORT,
                ),
                next_execution: None,
                checkpoint: CheckpointTelemetry::default(),
            })
        }
    }

    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("summary-host").expect("host"),
            1,
            None,
            10,
            1_000,
        )
        .expect("claim");
    let session_id = DaemonSessionId::new("summary-session").expect("session");
    store
        .create_session(&session_id, "cwd", 10)
        .expect("session");
    let goal_id = GoalId::new("task-summary").expect("task");
    let task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "后台休眠",
        GoalPlan {
            objective: OBJECTIVE.to_owned(),
            steps: vec!["执行 Start-Sleep".to_owned()],
        },
        vec!["报告实际耗时".to_owned()],
        10,
    )
    .expect("task");
    store.create_goal(&task).expect("create_goal");
    store
        .append_decision(
            &axon_domain::DecisionRecord {
                decision_id: DecisionId::new("allow-summary-1").expect("decision"),
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
    let mut queued = action(&session_id, "action-summary", OBJECTIVE);
    queued.goal_id = Some(goal_id);
    store.enqueue_action(&queued, 13).expect("enqueue");
    let worker = ExecutionWorker::new(
        store.clone(),
        ReportExecutor,
        NoopPublisher,
        FixedClock(20),
        host,
        session_id,
        100,
    );
    assert_eq!(worker.pump_once().expect("pump"), Some(queued.action_id));

    let unread_results =
        axon_session::ResultStore::list_unread_results(&store, &ScopeRef::global(), 10)
            .expect("list_unread");
    assert_eq!(unread_results.len(), 1);
    assert!(unread_results[0].succeeded);
    assert_eq!(unread_results[0].summary, FINAL_REPORT);
}
