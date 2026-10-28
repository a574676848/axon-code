//! `execution_for_session` 端口测试：按运行时会话取最近一条 Execution。

use super::SqliteRuntimeStore;
use crate::ports::{ExecutionStore, GoalStore, RuntimeStore};
use axon_domain::{DaemonSessionId, ExecutionId, ExecutionRecord, GoalPlan, GoalRecord, ScopeRef};

/// 建一条可落库的全局目标。
fn goal(goal_id: &str, now_ms: u64) -> GoalRecord {
    let mut record = GoalRecord::new(
        axon_domain::GoalId::new(goal_id).expect("goal id"),
        ScopeRef::global(),
        "按会话取执行",
        GoalPlan {
            objective: "验证按会话取最近执行".to_owned(),
            steps: vec!["写入三条执行".to_owned()],
        },
        vec!["取到最近一条".to_owned()],
        now_ms,
    )
    .expect("goal");
    record
        .transition(axon_domain::GoalStatus::Active, now_ms + 1)
        .expect("active");
    record
}

/// 目标执行会话必须先登记到 `session_runtimes`：执行表对会话有外键。
fn session(store: &SqliteRuntimeStore, session_id: &str) -> DaemonSessionId {
    let id = DaemonSessionId::new(session_id).expect("session");
    store.create_session(&id, "E:/repo", 10).expect("session");
    id
}

#[test]
fn latest_execution_wins_regardless_of_insert_order() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let target = goal("goal-session-lookup", 10);
    store.create_goal(&target).expect("goal");
    let session_id = session(&store, "goal-session-goal-session-lookup");
    // 写入顺序刻意打乱：判定依据是 created_at_ms，不是插入顺序。
    for (index, created_at_ms) in [10_u64, 30, 20].into_iter().enumerate() {
        let mut execution = ExecutionRecord::new(
            ExecutionId::new(format!("execution-{index}")).expect("execution id"),
            target.goal_id.clone(),
            created_at_ms,
        );
        execution.session_id = Some(session_id.clone());
        store.enqueue_execution(&execution).expect("execution");
    }
    let latest = store
        .execution_for_session(&session_id)
        .expect("查询成功")
        .expect("应有记录");
    assert_eq!(latest.execution_id.as_str(), "execution-1");
    assert_eq!(latest.created_at_ms, 30);
}

#[test]
fn session_without_execution_returns_none() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let target = goal("goal-other-session", 10);
    store.create_goal(&target).expect("goal");
    let bound = session(&store, "goal-session-a");
    let mut execution = ExecutionRecord::new(
        ExecutionId::new("execution-bound-elsewhere").expect("execution id"),
        target.goal_id.clone(),
        10,
    );
    execution.session_id = Some(bound);
    store.enqueue_execution(&execution).expect("execution");

    // 该会话根本没有执行记录：必须返回空，而不是借用同一目标的其他会话。
    let unbound = session(&store, "goal-session-b");
    assert!(
        store
            .execution_for_session(&unbound)
            .expect("查询成功")
            .is_none()
    );
}

#[test]
fn unknown_session_returns_none() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    assert!(
        store
            .execution_for_session(&DaemonSessionId::new("goal-session-missing").expect("session"))
            .expect("查询成功")
            .is_none()
    );
}
