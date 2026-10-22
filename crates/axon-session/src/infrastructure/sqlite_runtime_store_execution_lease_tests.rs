//! Runtime SQLite Store 的执行与主机租约行为测试。
//!
//! 与 `sqlite_runtime_store_tests.rs` 同源，按关注点拆开：这里只覆盖执行记录的领取与结算、
//! 业务键去重、失败退避，以及 Daemon 主机租约的续期与回收。

use super::SqliteRuntimeStore;
use crate::{ExecutionStore, GoalStore, ports::RuntimeStore};
use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionRecord, ExecutionStatus, GoalId, GoalPlan, GoalRecord,
    RuntimeHostId, ScopeRef, SessionRuntimeStatus,
};
#[test]
fn execution_store_claims_with_lease_and_requires_matching_owner_to_settle() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let task = GoalRecord::new(
        GoalId::new("task-store-parent").expect("Goal ID"),
        ScopeRef::global(),
        "父 Task",
        GoalPlan {
            objective: "执行".to_owned(),
            steps: Vec::new(),
        },
        vec!["完成".to_owned()],
        1,
    )
    .expect("Task");
    store.create_goal(&task).expect("创建父 Task");
    let execution = ExecutionRecord::new(
        ExecutionId::new("execution-store-1").expect("Execution ID"),
        GoalId::new("task-store-parent").expect("Goal ID"),
        1,
    );
    assert!(store.enqueue_execution(&execution).expect("入队"));
    let claimed = store.claim_executions(1, "worker-a", 2, 100).expect("领取");
    assert_eq!(claimed.len(), 1);
    let lease = claimed[0].lease.as_ref().expect("lease");
    assert!(
        store
            .settle_execution(
                &execution.execution_id,
                "worker-b",
                &lease.token,
                ExecutionStatus::Completed,
                3,
            )
            .is_err()
    );
    store
        .renew_execution(&execution.execution_id, "worker-a", &lease.token, 3, 100)
        .expect("续租");
    assert!(
        store
            .renew_execution(&execution.execution_id, "worker-a", "wrong-token", 4, 100)
            .is_err()
    );
    store
        .cancel_execution(&execution.execution_id, "worker-a", &lease.token, 5)
        .expect("取消 Execution");
}

#[test]
fn execution_store_rejects_duplicate_active_business_key_and_releases_it_after_terminal_settle() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let task = GoalRecord::new(
        GoalId::new("task-dedupe").expect("Goal ID"),
        ScopeRef::global(),
        "幂等 Task",
        GoalPlan {
            objective: "验证幂等".to_owned(),
            steps: Vec::new(),
        },
        vec!["完成".to_owned()],
        1,
    )
    .expect("Task");
    store.create_goal(&task).expect("创建 Task");

    let mut first = ExecutionRecord::new(
        ExecutionId::new("execution-dedupe-1").expect("Execution ID"),
        task.goal_id.clone(),
        1,
    );
    first.dedupe_key = Some("external:charge:42".to_owned());
    assert!(store.enqueue_execution(&first).expect("首次入队"));

    let mut duplicate = ExecutionRecord::new(
        ExecutionId::new("execution-dedupe-2").expect("Execution ID"),
        task.goal_id.clone(),
        2,
    );
    duplicate.attempt = 2;
    duplicate.dedupe_key = first.dedupe_key.clone();
    assert!(store.enqueue_execution(&duplicate).is_err());

    let claimed = store.claim_executions(1, "worker", 2, 100).expect("领取");
    let lease = claimed[0].lease.as_ref().expect("lease");
    store
        .settle_execution(
            &first.execution_id,
            "worker",
            &lease.token,
            ExecutionStatus::Completed,
            3,
        )
        .expect("完成");
    assert!(
        store
            .enqueue_execution(&duplicate)
            .expect("终态后允许重用幂等键")
    );
}

#[test]
fn execution_store_claims_in_priority_then_creation_order() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let task = GoalRecord::new(
        GoalId::new("task-priority").expect("Goal ID"),
        ScopeRef::global(),
        "优先级 Task",
        GoalPlan {
            objective: "验证排序".to_owned(),
            steps: Vec::new(),
        },
        vec!["完成".to_owned()],
        1,
    )
    .expect("Task");
    store.create_goal(&task).expect("创建 Task");

    for (id, attempt, priority, created_at_ms) in [
        ("execution-priority-low", 1, 10, 1),
        ("execution-priority-high-new", 2, 100, 3),
        ("execution-priority-high-old", 3, 100, 2),
    ] {
        let mut execution = ExecutionRecord::new(
            ExecutionId::new(id).expect("Execution ID"),
            task.goal_id.clone(),
            created_at_ms,
        );
        execution.attempt = attempt;
        execution.priority = priority;
        assert!(store.enqueue_execution(&execution).expect("入队"));
    }

    let claimed = store.claim_executions(3, "worker", 10, 100).expect("领取");
    assert_eq!(
        claimed
            .iter()
            .map(|execution| execution.execution_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "execution-priority-high-old",
            "execution-priority-high-new",
            "execution-priority-low"
        ]
    );
}

#[test]
fn execution_store_applies_failure_backoff_and_excludes_uncertain_from_claims() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let task = GoalRecord::new(
        GoalId::new("task-failure-policy").expect("Goal ID"),
        ScopeRef::global(),
        "失败策略 Task",
        GoalPlan {
            objective: "验证失败退避".to_owned(),
            steps: Vec::new(),
        },
        vec!["完成".to_owned()],
        1,
    )
    .expect("Task");
    store.create_goal(&task).expect("创建 Task");
    let execution = ExecutionRecord::new(
        ExecutionId::new("execution-failure-policy").expect("Execution ID"),
        task.goal_id.clone(),
        1,
    );
    store.enqueue_execution(&execution).expect("入队");

    let claimed = store.claim_executions(1, "worker", 2, 100).expect("领取");
    let lease = claimed[0].lease.as_ref().expect("lease").clone();
    store
        .fail_execution(&execution.execution_id, "worker", &lease.token, 3, 3)
        .expect("失败退避");
    let queued = store
        .get_execution(&execution.execution_id)
        .expect("读取")
        .expect("Execution");
    assert_eq!(queued.attempt, 2);
    assert_eq!(queued.retry_at_ms, Some(1_003));
    assert!(
        store
            .claim_executions(1, "worker", 1_002, 100)
            .expect("退避期间")
            .is_empty()
    );

    let claimed = store
        .claim_executions(1, "worker", 1_003, 100)
        .expect("退避到期领取");
    let lease = claimed[0].lease.as_ref().expect("lease").clone();
    store
        .fail_execution(&execution.execution_id, "worker", &lease.token, 2, 1_004)
        .expect("达到上限后失败");
    assert_eq!(
        store
            .get_execution(&execution.execution_id)
            .expect("读取")
            .expect("Execution")
            .status,
        ExecutionStatus::Failed
    );

    let uncertain = ExecutionRecord::new(
        ExecutionId::new("execution-uncertain-policy").expect("Execution ID"),
        task.goal_id,
        5,
    );
    store.enqueue_execution(&uncertain).expect("uncertain 入队");
    let claimed = store.claim_executions(1, "worker", 6, 100).expect("领取");
    let lease = claimed[0].lease.as_ref().expect("lease").clone();
    store
        .settle_execution(
            &uncertain.execution_id,
            "worker",
            &lease.token,
            ExecutionStatus::Uncertain,
            7,
        )
        .expect("标记 uncertain");
    assert!(
        store
            .claim_executions(1, "worker", 10_000, 100)
            .expect("uncertain 不领取")
            .is_empty()
    );
}

#[test]
fn renewed_host_remains_writable_after_initial_lease_expiry() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("host-renewed").expect("host");
    let host = store.claim_host(&host_id, 1, None, 10, 100).expect("claim");
    let session_id = DaemonSessionId::new("session-renewed").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");

    store.renew_host(&host, 100, 100).expect("续租");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 150)
        .expect("初始租约过期后仍应以数据库续租记录通过 fencing");
}

#[test]
fn dead_host_lease_is_reaped_before_ttl_expires() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let stale_id = RuntimeHostId::new("host-dead").expect("host");
    store
        .claim_host(&stale_id, 999_999, None, 10, 100_000)
        .expect("创建 stale host");

    assert_eq!(store.reap_dead_hosts(20).expect("回收 stale host"), 1);

    let replacement_id = RuntimeHostId::new("host-replacement").expect("host");
    store
        .claim_host(&replacement_id, 2, None, 20, 100)
        .expect("stale lease 不应阻塞新 Host");
}

#[test]
fn stale_host_cannot_append_event() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("host-a").expect("host");
    let host = store.claim_host(&host_id, 1, None, 10, 100).expect("claim");
    let session_id = DaemonSessionId::new("session-a").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 20)
        .expect("bind");
    let stale = crate::DaemonHostRecord {
        generation: "old".to_owned(),
        ..host
    };
    assert!(
        store
            .append_event(&session_id, &stale, "x", "{}", 20)
            .is_err()
    );
}

#[test]
fn distinct_instances_share_process_level_schema_initialization_cache() {
    let directory = axon_test_support::test_temp_dir();
    let db_path = directory.path().join("runtime.sqlite3");
    let store1 = SqliteRuntimeStore::new(&db_path);
    store1.initialize().expect("初次初始化成功");

    // 第二个全新实例无需再次执行完整 DDL
    let store2 = SqliteRuntimeStore::new(&db_path);
    assert!(store2.initialize().is_ok());
}
