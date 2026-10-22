//! Execution Worker 失败与 uncertain 结算的回归测试。

use super::*;
use crate::model::TurnExecutionError;
use axon_domain::{
    ConsumerCursor, DecisionEffect, DecisionId, DecisionRecord, ExecutionStatus, GoalPlan,
    GoalRecord, GoalStatus, ResultErrorKind, RuntimeHostId, ScopeRef,
};
use axon_session::{
    DecisionStore, ExecutionStore, GoalStore, ResultStore, RuntimeStore, SessionEventRecord,
    SqliteRuntimeStore,
};
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Clone, Copy)]
struct FixedClock(u64);

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

#[derive(Clone, Default)]
struct RecordingPublisher {
    published: Arc<Mutex<Vec<String>>>,
}

impl RecordingPublisher {
    fn published(&self) -> Vec<String> {
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl EventPublisher for RecordingPublisher {
    fn publish(&self, event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event.event_type.clone());
        Ok(())
    }
}

#[derive(Clone)]
struct FailingExecutor {
    failure: TurnExecutionError,
    calls: Arc<Mutex<u32>>,
}

impl FailingExecutor {
    fn new(failure: TurnExecutionError) -> Self {
        Self {
            failure,
            calls: Arc::new(Mutex::new(0)),
        }
    }

    fn calls(&self) -> u32 {
        *self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TurnExecutor for FailingExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        *self.calls.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Err(self.failure.clone())
    }
}

struct FailureFixture {
    store: SqliteRuntimeStore,
    host: DaemonHostRecord,
    session_id: DaemonSessionId,
    goal_id: GoalId,
    execution_id: ExecutionId,
    // 字段顺序即析构顺序：临时目录必须最后析构，`store` 先释放 SQLite 句柄，目录才删得掉。
    _directory: axon_test_support::TestTempDir,
}

fn failure_fixture(prefix: &str) -> FailureFixture {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new(format!("{prefix}-host")).expect("host"),
            1,
            None,
            10,
            10_000,
        )
        .expect("claim");
    let session_id = DaemonSessionId::new(format!("{prefix}-session")).expect("session");
    store
        .create_session(&session_id, "cwd", 10)
        .expect("session");
    let goal_id = GoalId::new(format!("{prefix}-task")).expect("task");
    let mut task = GoalRecord::new(
        goal_id.clone(),
        ScopeRef::global(),
        "失败结算任务",
        GoalPlan {
            objective: "验证失败结算".to_owned(),
            steps: vec!["执行一次".to_owned()],
        },
        vec!["产出结论".to_owned()],
        10,
    )
    .expect("task");
    task.transition(GoalStatus::Active, 11).expect("active");
    store.create_goal(&task).expect("save task");
    store
        .append_decision(
            &DecisionRecord {
                decision_id: DecisionId::new(format!("{prefix}-allow")).expect("decision"),
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
    let execution_id = ExecutionId::new(format!("{prefix}-execution")).expect("execution");
    let action = SessionActionRecord {
        action_id: SessionActionId::new(execution_id.as_str()).expect("action"),
        goal_id: Some(goal_id.clone()),
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 100,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: serde_json::json!({"prompt": "执行失败路径"}).to_string(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 13).expect("enqueue");
    FailureFixture {
        store,
        host,
        session_id,
        goal_id,
        execution_id,
        _directory: directory,
    }
}

fn run_failure(
    fixture: &FailureFixture,
    publisher: &RecordingPublisher,
    failure: TurnExecutionError,
) -> FailingExecutor {
    let executor = FailingExecutor::new(failure);
    let worker = ExecutionWorker::new(
        fixture.store.clone(),
        executor.clone(),
        publisher.clone(),
        FixedClock(20),
        fixture.host.clone(),
        fixture.session_id.clone(),
        1_000,
    );
    worker.pump_once().expect_err("失败必须向上暴露");
    executor
}

#[test]
fn permanent_failure_commits_failed_execution_task_and_result() {
    let fixture = failure_fixture("permanent");
    let publisher = RecordingPublisher::default();
    let executor = run_failure(
        &fixture,
        &publisher,
        TurnExecutionError::permanent("Provider 拒绝执行该请求"),
    );
    assert_eq!(executor.calls(), 1);
    assert_eq!(publisher.published(), vec!["turn_failed".to_owned()]);

    let execution = fixture
        .store
        .get_execution(&fixture.execution_id)
        .expect("get_execution")
        .expect("execution");
    assert_eq!(execution.status, ExecutionStatus::Failed);
    assert!(execution.lease.is_none());

    let task = fixture
        .store
        .get_goal(&fixture.goal_id)
        .expect("get_goal")
        .expect("task");
    assert_eq!(task.status, GoalStatus::Failed);

    let results = fixture
        .store
        .list_unread_results(&ScopeRef::global(), 10)
        .expect("list_unread_results");
    assert_eq!(results.len(), 1);
    assert!(!results[0].succeeded);
    assert_eq!(results[0].error_kind, Some(ResultErrorKind::Runtime));
    assert_eq!(results[0].execution_id, fixture.execution_id);
}

#[test]
fn external_outcome_unknown_enters_uncertain_and_never_auto_retries() {
    let fixture = failure_fixture("uncertain");
    let publisher = RecordingPublisher::default();
    let executor = run_failure(
        &fixture,
        &publisher,
        TurnExecutionError::external_outcome_unknown("外部消息已发送但连接中断，结果未知"),
    );
    assert_eq!(executor.calls(), 1);
    assert_eq!(publisher.published(), vec!["turn_uncertain".to_owned()]);

    let execution = fixture
        .store
        .get_execution(&fixture.execution_id)
        .expect("get_execution")
        .expect("execution");
    assert_eq!(execution.status, ExecutionStatus::Uncertain);
    assert!(execution.lease.is_none());

    let task = fixture
        .store
        .get_goal(&fixture.goal_id)
        .expect("get_goal")
        .expect("task");
    assert_eq!(task.status, GoalStatus::Waiting);

    let results = fixture
        .store
        .list_unread_results(&ScopeRef::global(), 10)
        .expect("list_unread_results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].error_kind, Some(ResultErrorKind::Uncertain));

    let worker = ExecutionWorker::new(
        fixture.store.clone(),
        executor.clone(),
        RecordingPublisher::default(),
        FixedClock(30),
        fixture.host.clone(),
        fixture.session_id.clone(),
        1_000,
    );
    assert_eq!(worker.pump_once().expect("第二次 pump"), None);
    assert_eq!(executor.calls(), 1);
}

#[test]
fn retryable_failure_requeues_with_backoff_and_keeps_task_active() {
    let fixture = failure_fixture("retryable");
    let publisher = RecordingPublisher::default();
    let executor = run_failure(
        &fixture,
        &publisher,
        TurnExecutionError::retryable("Provider 连接超时"),
    );
    assert_eq!(executor.calls(), 1);
    assert_eq!(
        publisher.published(),
        vec!["turn_retry_scheduled".to_owned()]
    );

    let execution = fixture
        .store
        .get_execution(&fixture.execution_id)
        .expect("get_execution")
        .expect("execution");
    assert_eq!(execution.status, ExecutionStatus::Queued);
    assert_eq!(execution.retry_at_ms, Some(1_020));
    assert!(execution.lease.is_none());

    let task = fixture
        .store
        .get_goal(&fixture.goal_id)
        .expect("get_goal")
        .expect("task");
    assert_eq!(task.status, GoalStatus::Active);
    assert!(
        fixture
            .store
            .list_unread_results(&ScopeRef::global(), 10)
            .expect("list_unread_results")
            .is_empty()
    );

    let worker = ExecutionWorker::new(
        fixture.store.clone(),
        executor.clone(),
        RecordingPublisher::default(),
        FixedClock(20),
        fixture.host.clone(),
        fixture.session_id.clone(),
        1_000,
    );
    assert_eq!(worker.pump_once().expect("退避期间 pump"), None);
    assert_eq!(executor.calls(), 1);
}
