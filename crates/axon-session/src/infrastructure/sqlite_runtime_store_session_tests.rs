//! SQLite Daemon Session 与内部 Execution queue 测试。

use super::SqliteRuntimeStore;
use crate::SessionActionRecord;
use crate::ports::{ExecutionStore, GoalStore, ResultStore, RuntimeStore};
use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionRecord, ExecutionStatus, GoalPlan, GoalRecord,
    GoalStatus, ResultErrorKind, RuntimeHostId, ScopeRef, SessionActionId, SessionActionStatus,
    SessionRuntimeStatus,
};

#[test]
fn creating_same_transport_session_returns_original_runtime() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("transport-session-a").expect("session");
    let first = store
        .create_session(&session_id, "first-cwd", 10)
        .expect("首次创建");
    let second = store
        .create_session(&session_id, "second-cwd", 20)
        .expect("幂等创建");
    assert_eq!(first, second);
    assert_eq!(second.cwd, "first-cwd");
}

#[test]
fn waiting_action_is_persistent_but_not_runnable_by_worker() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("session:17").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    let action = SessionActionRecord {
        action_id: SessionActionId::new("session-action-17").expect("action"),
        goal_id: None,
        session_id: session_id.clone(),
        kind: "session_action".to_owned(),
        priority: 100,
        status: SessionActionStatus::Waiting,
        dedupe_key: Some("session-action:17".to_owned()),
        payload_json: "{}".to_owned(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 10).expect("入队");

    assert!(
        store
            .list_runnable_execution_sessions(11, 8)
            .expect("可运行列表")
            .is_empty()
    );
    let pending = store
        .list_pending_actions(&session_id, 8)
        .expect("待处理列表");
    assert_eq!(pending[0].status, SessionActionStatus::Waiting);
    assert_eq!(
        store
            .cancel_action(&action.action_id, &session_id, 12)
            .expect("取消等待 action")
            .status,
        SessionActionStatus::Cancelled
    );
}

#[test]
fn resubmitting_same_action_id_returns_original_record() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("retry-session").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    let action = SessionActionRecord {
        action_id: SessionActionId::new("retry-action").expect("action"),
        goal_id: None,
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 50,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: r#"{"prompt":"第一次"}"#.to_owned(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 10).expect("首次入队");
    let retry = SessionActionRecord {
        payload_json: r#"{"prompt":"重试不应覆盖"}"#.to_owned(),
        ..action.clone()
    };
    let existing = store.enqueue_action(&retry, 20).expect("重试应幂等");
    assert_eq!(existing.context_version, Some(0));
    assert_eq!(existing.payload_json, action.payload_json);
}

#[test]
fn reset_session_cancels_pending_actions_and_rotates_run_identity() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("host-reset").expect("host");
    let host = store.claim_host(&host_id, 1, None, 10, 100).expect("claim");
    let session_id = DaemonSessionId::new("session-reset").expect("session");
    let original = store.create_session(&session_id, ".", 10).expect("session");
    let action = SessionActionRecord {
        action_id: SessionActionId::new("pending-reset-action").expect("action"),
        goal_id: None,
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 50,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: r#"{"prompt":"旧请求"}"#.to_owned(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 11).expect("入队");

    let reset = store
        .reset_session(&session_id, "new-cwd", &host, 20)
        .expect("重置");
    assert_ne!(reset.run_session_id, original.run_session_id);
    assert_eq!(reset.context_version, original.context_version + 1);
    assert_eq!(reset.cwd, "new-cwd");
    assert_eq!(reset.status, SessionRuntimeStatus::Created);
    assert!(
        store
            .list_pending_actions(&session_id, 8)
            .expect("读取队列")
            .is_empty()
    );

    let next_action = SessionActionRecord {
        action_id: SessionActionId::new("next-reset-action").expect("action"),
        payload_json: r#"{"prompt":"新请求"}"#.to_owned(),
        context_version: Some(original.context_version),
        ..action
    };
    let persisted = store
        .enqueue_action(&next_action, 21)
        .expect("新 Action 入队");
    assert_eq!(persisted.context_version, Some(reset.context_version));
}

#[test]
fn stop_session_cancels_pending_actions_without_rotating_run_identity() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("host-stop").expect("host");
    let host = store.claim_host(&host_id, 1, None, 10, 100).expect("claim");
    let session_id = DaemonSessionId::new("session-stop").expect("session");
    let original = store.create_session(&session_id, ".", 10).expect("session");
    let action = SessionActionRecord {
        action_id: SessionActionId::new("pending-stop-action").expect("action"),
        goal_id: None,
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 50,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: r#"{"prompt":"停止前请求"}"#.to_owned(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 11).expect("入队");

    let stopped = store.stop_session(&session_id, &host, 20).expect("停止");
    assert_eq!(stopped.run_session_id, original.run_session_id);
    assert_eq!(stopped.status, SessionRuntimeStatus::Idle);
    assert!(
        store
            .list_pending_actions(&session_id, 8)
            .expect("读取队列")
            .is_empty()
    );
}

#[test]
fn upgrade_handoff_fences_old_generation_without_waiting_for_turn_completion() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let old_id = RuntimeHostId::new("host-old").expect("host");
    let old_host = store.claim_host(&old_id, 1, None, 10, 100).expect("claim");
    let session_id = DaemonSessionId::new("session-upgrade").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    store
        .set_session_status(&session_id, &old_host, SessionRuntimeStatus::Running, 20)
        .expect("bind");
    store
        .request_upgrade(&old_host, "handoff-token", "test", 60, 30)
        .expect("request upgrade");
    store
        .mark_handoff_ready(&old_host, "handoff-token", 31)
        .expect("handoff ready");
    let replacement_id = RuntimeHostId::new("host-new").expect("host");
    let replacement = store
        .claim_upgrade_replacement(&replacement_id, 2, None, "handoff-token", 32, 100)
        .expect("replacement");
    assert!(
        store
            .append_event(&session_id, &old_host, "late_write", "{}", 33)
            .is_err()
    );
    store
        .append_event(&session_id, &replacement, "recovered", "{}", 33)
        .expect("new host write");
}

#[test]
fn failed_action_respects_persistent_retry_at_before_reclaim() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("host-retry-backoff").expect("host");
    let host = store
        .claim_host(&host_id, 1, None, 10, 10_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("session-retry-backoff").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    let action = SessionActionRecord {
        action_id: SessionActionId::new("retry-backoff-action").expect("action"),
        goal_id: None,
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 50,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: r#"{"prompt":"retry"}"#.to_owned(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 20).expect("入队");
    let claimed = store
        .claim_next_action(&session_id, &host, 30, 10_000)
        .expect("claim action")
        .expect("应领取");
    store
        .fail_action(
            &claimed.action_id,
            &session_id,
            &host,
            "Provider 网络请求超时",
            3,
            40,
        )
        .expect("失败结算");
    assert!(
        store
            .claim_next_action(&session_id, &host, 1_039, 10_000)
            .expect("退避期间查询")
            .is_none()
    );
    assert!(
        store
            .claim_next_action(&session_id, &host, 1_040, 10_000)
            .expect("退避到期查询")
            .is_some()
    );
}

#[test]
fn interrupted_running_execution_is_quarantined_as_uncertain_without_reclaim() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("host-interrupted").expect("host"),
            1,
            None,
            10,
            10_000,
        )
        .expect("claim");
    let session_id = DaemonSessionId::new("session-interrupted").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    let mut task = GoalRecord::new(
        axon_domain::GoalId::new("task-interrupted").expect("task"),
        ScopeRef::global(),
        "中断任务",
        GoalPlan {
            objective: "验证中断隔离".to_owned(),
            steps: Vec::new(),
        },
        vec!["产出结论".to_owned()],
        10,
    )
    .expect("task");
    task.transition(GoalStatus::Active, 11).expect("active");
    store.create_goal(&task).expect("save task");
    let mut execution = ExecutionRecord::new(
        ExecutionId::new("execution-interrupted").expect("execution"),
        task.goal_id.clone(),
        20,
    );
    execution.session_id = Some(session_id.clone());
    store.enqueue_execution(&execution).expect("enqueue");
    store
        .claim_executions(1, "worker-interrupted", 30, 1_000)
        .expect("claim");

    assert!(
        store
            .claim_executions(1, "worker-next", 1_100, 1_000)
            .expect("过期 running 不得重领")
            .is_empty()
    );

    let records = store
        .quarantine_interrupted_executions(&host, 1_100, 4)
        .expect("quarantine");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event_type, "turn_uncertain");

    let quarantined = store
        .get_execution(&execution.execution_id)
        .expect("get_execution")
        .expect("execution");
    assert_eq!(quarantined.status, ExecutionStatus::Uncertain);
    assert!(quarantined.lease.is_none());
    assert_eq!(
        store
            .get_goal(&task.goal_id)
            .expect("get_goal")
            .expect("task")
            .status,
        GoalStatus::Waiting
    );
    let results = store
        .list_unread_results(&ScopeRef::global(), 10)
        .expect("list_unread_results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].error_kind, Some(ResultErrorKind::Uncertain));
    assert!(
        store
            .quarantine_interrupted_executions(&host, 1_200, 4)
            .expect("幂等隔离")
            .is_empty()
    );
}

#[test]
fn upgrade_deadline_interrupts_running_execution_as_uncertain_instead_of_requeue() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("host-deadline").expect("host"),
            1,
            None,
            10,
            10_000,
        )
        .expect("claim");
    let session_id = DaemonSessionId::new("session-deadline").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    let mut task = GoalRecord::new(
        axon_domain::GoalId::new("task-deadline").expect("task"),
        ScopeRef::global(),
        "升级中断任务",
        GoalPlan {
            objective: "验证升级 deadline 不盲目重跑".to_owned(),
            steps: Vec::new(),
        },
        Vec::new(),
        11,
    )
    .expect("task");
    task.transition(GoalStatus::Active, 12).expect("active");
    store.create_goal(&task).expect("save task");
    let execution_id = ExecutionId::new("execution-deadline").expect("execution");
    let action = SessionActionRecord {
        action_id: SessionActionId::new(execution_id.as_str()).expect("action"),
        goal_id: Some(axon_domain::GoalId::new("task-deadline").expect("task")),
        session_id: session_id.clone(),
        kind: "user_input".to_owned(),
        priority: 50,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: r#"{"prompt":"升级中断验证"}"#.to_owned(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    };
    store.enqueue_action(&action, 20).expect("入队");
    store
        .claim_next_action(&session_id, &host, 30, 10_000)
        .expect("claim action")
        .expect("应领取");

    let interrupted = store
        .interrupt_host_actions(&host, 31)
        .expect("中断运行中的 Execution");

    assert_eq!(interrupted, 1);
    let settled = store
        .get_execution(&execution_id)
        .expect("get_execution")
        .expect("execution");
    assert_eq!(settled.status, ExecutionStatus::Uncertain);
    assert!(settled.lease.is_none());
    assert!(
        store
            .claim_executions(1, "worker-next", 40, 10_000)
            .expect("中断后的 Execution 不得被重新领取")
            .is_empty()
    );
    let results = store
        .list_unread_results(&ScopeRef::global(), 10)
        .expect("list_unread_results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].error_kind, Some(ResultErrorKind::Uncertain));
}
