use super::*;
use crate::model::{RuntimeEvent, SessionExecutionContext, TurnExecutionError, TurnFailureKind};
use axon_domain::{
    ConsumerCursor, DecisionEffect, DecisionId, GoalId, GoalPlan, GoalRecord, GoalStatus,
    RuntimeHostId, ScopeRef,
};
use axon_session::{
    CheckpointTelemetry, DecisionStore, GoalStore, SessionEventRecord, SessionRuntimeRecord,
    SqliteRuntimeStore,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct RecordingExecutor {
    requests: Arc<Mutex<Vec<ActionExecutionRequest>>>,
}

struct VerifiedExecutor;

impl TurnExecutor for VerifiedExecutor {
    fn execute(
        &self,
        request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: crate::application::goal_execution_report::verified_test_events(
                request.action.session_id.as_str(),
                "产生结果",
                "已核验结果",
            ),
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

impl TurnExecutor for RecordingExecutor {
    fn execute(
        &self,
        request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request);
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: "turn_completed".to_owned(),
                payload_json: "{}".to_owned(),
            }],
            next_execution: None,
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

#[derive(Clone, Default)]
struct RecordingPublisher {
    events: Arc<Mutex<Vec<SessionEventRecord>>>,
}

impl EventPublisher for RecordingPublisher {
    fn publish(&self, event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.clone());
        Ok(())
    }
}

#[test]
fn reset_discards_old_action_and_executes_next_action_with_new_context() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("worker-host").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("worker-session").expect("session");
    let original = store
        .create_session(&session_id, "old-cwd", 10)
        .expect("session");
    store
        .enqueue_action(&action(&session_id, "old-action", "旧请求"), 11)
        .expect("旧 Action 入队");

    let reset = store
        .reset_session(&session_id, "new-cwd", &host, 20)
        .expect("reset");
    let submitted = store
        .enqueue_action(&action(&session_id, "new-action", "新请求"), 21)
        .expect("新 Action 入队");
    let executor = RecordingExecutor::default();
    let worker = ExecutionWorker::new(
        store,
        executor.clone(),
        NoopPublisher,
        FixedClock(30),
        host,
        session_id,
        100,
    );

    let executed = worker.pump_once().expect("Worker 执行");
    assert_eq!(executed, Some(submitted.action_id));
    let requests = executor
        .requests
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(requests.len(), 1);
    assert_execution_context(&requests[0].context, &reset);
    assert_ne!(reset.run_session_id, original.run_session_id);
}

#[test]
fn ledger_gate_blocks_paused_task_and_denied_decision() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("gate-host").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let paused_session = DaemonSessionId::new("paused-session").expect("session");
    store
        .create_session(&paused_session, "cwd", 10)
        .expect("session");
    let paused_goal_id = GoalId::new("paused-action").expect("task");
    let mut paused_task = GoalRecord::new(
        paused_goal_id,
        ScopeRef::global(),
        "paused",
        GoalPlan {
            objective: "paused".to_owned(),
            steps: vec!["wait".to_owned()],
        },
        vec!["resume".to_owned()],
        10,
    )
    .expect("task");
    paused_task
        .transition(GoalStatus::Active, 11)
        .expect("active");
    paused_task
        .transition(GoalStatus::Paused, 12)
        .expect("paused");
    store.create_goal(&paused_task).expect("save task");
    let paused_action = action(&paused_session, "paused-action", "暂停");
    store.enqueue_action(&paused_action, 13).expect("enqueue");
    let paused_executor = RecordingExecutor::default();
    let paused_worker = ExecutionWorker::new(
        store.clone(),
        paused_executor.clone(),
        NoopPublisher,
        FixedClock(20),
        host.clone(),
        paused_session,
        100,
    );
    paused_worker.pump_once().expect("gate");
    assert!(
        paused_executor
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );

    let denied_session = DaemonSessionId::new("denied-session").expect("session");
    store
        .create_session(&denied_session, "cwd", 10)
        .expect("session");
    let denied_goal_id = GoalId::new("denied-action").expect("task");
    let mut denied_task = GoalRecord::new(
        denied_goal_id,
        ScopeRef::global(),
        "denied",
        GoalPlan {
            objective: "denied".to_owned(),
            steps: vec!["stop".to_owned()],
        },
        vec!["none".to_owned()],
        10,
    )
    .expect("task");
    denied_task
        .transition(GoalStatus::Active, 11)
        .expect("active");
    store.create_goal(&denied_task).expect("save task");
    store
        .append_decision(
            &axon_domain::DecisionRecord {
                decision_id: DecisionId::new("deny-1").expect("decision"),
                scope: ScopeRef::global(),
                actor: "test".to_owned(),
                effect: DecisionEffect::Deny,
                revision: 1,
                expires_at_ms: None,
                created_at_ms: 12,
            },
            0,
        )
        .expect("deny");
    let denied_action = action(&denied_session, "denied-action", "拒绝");
    store.enqueue_action(&denied_action, 13).expect("enqueue");
    let denied_executor = RecordingExecutor::default();
    let denied_worker = ExecutionWorker::new(
        store,
        denied_executor.clone(),
        NoopPublisher,
        FixedClock(20),
        host,
        denied_session,
        100,
    );
    denied_worker.pump_once().expect("gate");
    assert!(
        denied_executor
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
}

#[test]
fn ledger_gate_uses_persisted_goal_id_when_execution_id_differs() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("gate-distinct-id-host").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("gate-distinct-id-session").expect("session");
    store
        .create_session(&session_id, "cwd", 10)
        .expect("session");
    let goal_id = GoalId::new("real-task-id").expect("task");
    let mut task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "真实 Task",
        GoalPlan {
            objective: "验证执行关联".to_owned(),
            steps: vec!["运行 Worker".to_owned()],
        },
        vec!["执行一次".to_owned()],
        10,
    )
    .expect("task");
    task.transition(GoalStatus::Active, 11).expect("active");
    store.create_goal(&task).expect("save task");
    store
        .append_decision(
            &axon_domain::DecisionRecord {
                decision_id: DecisionId::new("allow-distinct-id").expect("decision"),
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
    let mut action = action(&session_id, "execution-id", "执行");
    action.goal_id = Some(goal_id);
    store.enqueue_action(&action, 13).expect("enqueue");
    let executor = RecordingExecutor::default();
    let worker = ExecutionWorker::new(
        store,
        executor.clone(),
        NoopPublisher,
        FixedClock(20),
        host,
        session_id,
        100,
    );

    worker.pump_once().expect("Worker 执行");
    assert_eq!(
        executor
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );
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
        payload_json: serde_json::json!({
            "prompt": prompt
        })
        .to_string(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    }
}

fn assert_execution_context(actual: &SessionExecutionContext, runtime: &SessionRuntimeRecord) {
    assert_eq!(actual.run_session_id, runtime.run_session_id);
    assert_eq!(actual.cwd, runtime.cwd);
    assert_eq!(actual.context_version, runtime.context_version);
}

#[test]
fn failure_kinds_are_structured_not_text_sniffed() {
    assert_eq!(
        TurnExecutionError::permanent("Action payload 解析失败：unknown field").kind(),
        TurnFailureKind::Permanent
    );
    assert_eq!(
        TurnExecutionError::retryable("Provider 网络请求超时").kind(),
        TurnFailureKind::Retryable
    );
    assert_eq!(
        TurnExecutionError::external_outcome_unknown("Provider 连接中断，外部结果未知").kind(),
        TurnFailureKind::ExternalOutcomeUnknown
    );
    assert!(TurnFailureKind::Permanent.is_conclusive());
    assert!(TurnFailureKind::ExternalOutcomeUnknown.is_conclusive());
    assert!(!TurnFailureKind::Retryable.is_conclusive());
}

#[test]
fn conclusive_failure_without_task_is_published_after_durable_append() {
    #[derive(Clone, Copy)]
    struct FailingExecutor;

    impl TurnExecutor for FailingExecutor {
        fn execute(
            &self,
            _request: ActionExecutionRequest,
        ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
            Err(TurnExecutionError::permanent("无法执行"))
        }
    }

    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("failure-host").expect("host"),
            1,
            None,
            10,
            1_000,
        )
        .expect("claim");
    let session_id = DaemonSessionId::new("failure-session").expect("session");
    store
        .create_session(&session_id, "cwd", 10)
        .expect("session");
    let action = action(&session_id, "failure-action", "触发永久失败");
    store.enqueue_action(&action, 11).expect("enqueue");
    let publisher = RecordingPublisher::default();
    let worker = ExecutionWorker::new(
        store.clone(),
        FailingExecutor,
        publisher.clone(),
        FixedClock(20),
        host,
        session_id.clone(),
        100,
    );

    assert!(worker.pump_once().is_err());

    let durable = store.replay_events(&session_id, 0, 10).expect("replay");
    let published = publisher
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(durable.len(), 1);
    assert_eq!(published.as_slice(), durable.as_slice());
    assert_eq!(durable[0].event_type, "turn_failed");
}

#[test]
fn execution_completion_persists_result_record_and_completes_task() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("commit-host").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("commit-session").expect("session");
    store
        .create_session(&session_id, "cwd", 10)
        .expect("session");
    let goal_id = GoalId::new("task-completion-1").expect("task");
    let task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "完成任务",
        GoalPlan {
            objective: "执行并完成".to_owned(),
            steps: vec!["第一步".to_owned()],
        },
        vec!["产生结果".to_owned()],
        10,
    )
    .expect("task");
    store.create_goal(&task).expect("save task");
    store
        .append_decision(
            &axon_domain::DecisionRecord {
                decision_id: DecisionId::new("allow-commit-1").expect("decision"),
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
    let mut action = action(&session_id, "action-commit-1", "执行完成");
    action.goal_id = Some(goal_id.clone());
    store.enqueue_action(&action, 13).expect("enqueue");
    let executor = VerifiedExecutor;
    let worker = ExecutionWorker::new(
        store.clone(),
        executor,
        NoopPublisher,
        FixedClock(20),
        host,
        session_id,
        100,
    );

    let executed = worker.pump_once().expect("pump");
    assert_eq!(executed, Some(action.action_id));

    // 验证 Goal 状态迁移为 Completed
    let updated_task = store
        .get_goal(&goal_id)
        .expect("get_goal")
        .expect("task exists");
    assert_eq!(updated_task.status, GoalStatus::Completed);

    // 验证 ResultRecord 已被写入
    let unread_results =
        axon_session::ResultStore::list_unread_results(&store, &ScopeRef::global(), 10)
            .expect("list_unread");
    assert_eq!(unread_results.len(), 1);
    assert_eq!(unread_results[0].goal_id, goal_id);
    assert!(unread_results[0].succeeded);
}

#[test]
fn executor_success_completes_goal_without_extra_artifact_gate() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("worker-host-art").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("worker-session-art").expect("session");
    let goal_id = GoalId::new("task-artifact-req").expect("goal_id");
    let task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "需要产物的任务",
        GoalPlan {
            objective: "生成代码产物".to_owned(),
            steps: vec!["步骤 1".to_owned()],
        },
        vec!["必须提供 artifact 产物交付".to_owned()],
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
                decision_id: DecisionId::new("allow-artifact-1").expect("decision"),
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
    let mut action = action(&session_id, "action-artifact-1", "执行但不产生产物");
    action.goal_id = Some(goal_id.clone());
    store.enqueue_action(&action, 13).expect("enqueue");
    let executor = RecordingExecutor::default();
    let worker = ExecutionWorker::new(
        store.clone(),
        executor,
        NoopPublisher,
        FixedClock(20),
        host,
        session_id,
        100,
    );

    let executed = worker.pump_once().expect("pump");
    assert_eq!(executed, Some(action.action_id));

    let updated_goal = store
        .get_goal(&goal_id)
        .expect("get_goal")
        .expect("goal exists");
    assert_eq!(updated_goal.status, GoalStatus::Completed);

    let unread_results =
        axon_session::ResultStore::list_unread_results(&store, &ScopeRef::global(), 10)
            .expect("list_unread");
    assert_eq!(unread_results.len(), 1);
    assert!(unread_results[0].succeeded);
}

#[test]
fn cancelled_task_aborts_commit_and_emits_cancelled_event() {
    use axon_session::LedgerEventStore;
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("worker-host-cancel").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("worker-session-cancel").expect("session");
    let goal_id = GoalId::new("task-cancelled-race").expect("goal_id");
    let mut task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "即将取消的任务",
        GoalPlan {
            objective: "执行中取消".to_owned(),
            steps: vec!["步骤 1".to_owned()],
        },
        vec!["无需产物".to_owned()],
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
                decision_id: DecisionId::new("allow-cancelled-1").expect("decision"),
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
    let mut action = action(&session_id, "action-cancelled-1", "执行中取消");
    action.goal_id = Some(goal_id.clone());
    store.enqueue_action(&action, 13).expect("enqueue");

    // 模拟外部并发取消 Task
    task.status = GoalStatus::Cancelled;
    task.revision += 1;
    store.update_goal(&task, 0).expect("cancel task externally");

    let executor = RecordingExecutor::default();
    let worker = ExecutionWorker::new(
        store.clone(),
        executor,
        NoopPublisher,
        FixedClock(20),
        host,
        session_id,
        100,
    );

    // Gate 拦截取消，不执行或回滚迟到提交
    let executed = worker.pump_once().expect("pump");
    assert_eq!(executed, Some(action.action_id));

    let updated_task = store
        .get_goal(&goal_id)
        .expect("get_goal")
        .expect("task exists");
    assert_eq!(updated_task.status, GoalStatus::Cancelled);

    // 验证事件流中有 cancelled 记录
    let events = store
        .replay_ledger_events(0, 100)
        .expect("replay_ledger_events");
    assert!(events.iter().any(|e| e.event_type == "execution.cancelled"));
}
