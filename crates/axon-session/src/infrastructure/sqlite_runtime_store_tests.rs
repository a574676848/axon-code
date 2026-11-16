//! Runtime SQLite Store 行为测试。

use super::SqliteRuntimeStore;
use crate::{
    DecisionStore, ExecutionStore, GoalCommitStore, GoalStore, LedgerCommitStore, LedgerEventStore,
    ports::RuntimeStore,
};
use axon_domain::{
    DecisionEffect, DecisionId, DecisionRecord, ExecutionId, ExecutionRecord, ExecutionStatus,
    GoalId, GoalListQuery, GoalPlan, GoalQueryScope, GoalRecord, GoalStatus, JarvisEvent,
    JarvisEventId, ResultId, ResultRecord, ScopeRef,
};

#[test]
fn runtime_schema_contains_work_ledger_tables_and_hot_query_indexes() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(&path);
    store.initialize().expect("初始化目标 schema");
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    for table in [
        "workspaces",
        "inbox",
        "goals",
        "work_items",
        "goal_results",
        "decisions",
        "ledger_events",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .expect("查询表");
        assert!(exists, "缺少目标表：{table}");
    }
    let indexed: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='inbox_source')",
            [],
            |row| row.get(0),
        )
        .expect("查询索引");
    assert!(indexed);
    let active_indexed: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='inbox_scope_recent')",
            [],
            |row| row.get(0),
        )
        .expect("查询活动 Interaction 索引");
    assert!(active_indexed);
    let session_recent_indexed: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='inbox_session_recent')",
            [],
            |row| row.get(0),
        )
        .expect("查询车道列表索引");
    assert!(session_recent_indexed);
    // 车道列表按 session 分组、组内按接收时间倒序取首行：索引列序必须与之吻合，
    // 否则窗口函数会退化成全表扫描加临时排序。
    let session_recent_columns = connection
        .prepare("PRAGMA index_info(inbox_session_recent)")
        .expect("读取车道列表索引")
        .query_map([], |row| row.get::<_, String>(2))
        .expect("枚举车道列表索引列")
        .collect::<Result<Vec<_>, _>>()
        .expect("收集车道列表索引列");
    assert_eq!(
        session_recent_columns,
        vec![
            "session_id".to_owned(),
            "received_at_ms".to_owned(),
            "interaction_id".to_owned()
        ]
    );
    let claim_index_columns = connection
        .prepare("PRAGMA index_info(work_items_inbox)")
        .expect("读取 claim 索引")
        .query_map([], |row| row.get::<_, String>(2))
        .expect("枚举 claim 索引列")
        .collect::<Result<Vec<_>, _>>()
        .expect("收集 claim 索引列");
    assert_eq!(
        claim_index_columns,
        vec!["inbox_id".to_owned()],
        "一条入站事实最多对应一个工作项"
    );
    let execution_dedupe_indexed: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='work_items_active_dedupe')",
            [],
            |row| row.get(0),
        )
        .expect("查询 Execution 幂等索引");
    assert!(execution_dedupe_indexed);
    let execution_columns = connection
        .prepare("PRAGMA table_info(work_items)")
        .expect("读取 Execution 列")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("枚举 Execution 列")
        .collect::<Result<Vec<_>, _>>()
        .expect("收集 Execution 列");
    assert!(execution_columns.iter().any(|column| column == "priority"));
    assert!(
        execution_columns
            .iter()
            .any(|column| column == "dedupe_key")
    );
}

#[test]
fn task_commit_atomically_creates_task_first_execution_and_event_idempotently() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let goal = GoalRecord::new(
        GoalId::new("goal-start").expect("Goal ID"),
        ScopeRef::workspace("workspace-start").expect("作用域"),
        "首次任务",
        GoalPlan {
            objective: "验证原子创建".to_owned(),
            steps: vec!["创建首个执行".to_owned()],
        },
        vec!["事件可重放".to_owned()],
        10,
    )
    .expect("goal");
    let execution = ExecutionRecord::new(
        ExecutionId::new("execution-start").expect("Execution ID"),
        goal.goal_id.clone(),
        10,
    );
    let event = JarvisEvent {
        event_id: JarvisEventId::new("event-start").expect("Event ID"),
        sequence: 0,
        event_type: "goal.created".to_owned(),
        idempotency_key: "goal-start:created".to_owned(),
        cause_event_id: None,
        payload_json: "{}".to_owned(),
        occurred_at_ms: 10,
    };

    store
        .commit_goal_start(&goal, &execution, &event)
        .expect("首次 goal 提交");
    store
        .commit_goal_start(&goal, &execution, &event)
        .expect("幂等 goal 提交");
    let mut conflicting_event = event.clone();
    conflicting_event.payload_json = "{\"changed\":true}".to_owned();
    assert!(
        store
            .commit_goal_start(&goal, &execution, &conflicting_event)
            .is_err()
    );
    assert_eq!(
        store.get_goal(&goal.goal_id).expect("读取 goal"),
        Some(goal)
    );
    assert_eq!(
        store
            .get_execution(&execution.execution_id)
            .expect("读取 Execution")
            .map(|record| record.status),
        Some(ExecutionStatus::Queued)
    );
    assert_eq!(
        store.replay_ledger_events(0, 10).expect("读取 Event").len(),
        1
    );
}

#[test]
fn ledger_commit_rolls_back_all_records_on_stale_task_revision() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut goal = GoalRecord::new(
        GoalId::new("goal-atomic-commit").expect("Goal ID"),
        ScopeRef::global(),
        "原子提交 goal",
        GoalPlan {
            objective: "验证事务回滚".to_owned(),
            steps: Vec::new(),
        },
        vec!["结果写入".to_owned()],
        1,
    )
    .expect("goal");
    store.create_goal(&goal).expect("创建 goal");
    goal.transition(GoalStatus::Active, 2).expect("激活");
    store.update_goal(&goal, 0).expect("更新 goal");

    let execution = ExecutionRecord::new(
        ExecutionId::new("execution-atomic-commit").expect("Execution ID"),
        goal.goal_id.clone(),
        3,
    );
    store.enqueue_execution(&execution).expect("入队");
    let claimed = store.claim_executions(1, "worker", 4, 100).expect("领取");
    let lease = claimed[0].lease.as_ref().expect("lease").clone();
    let mut completed_execution = claimed[0].clone();
    completed_execution.status = ExecutionStatus::Completed;
    completed_execution.lease = None;
    completed_execution.updated_at_ms = 5;
    let mut completed_task = goal.clone();
    completed_task
        .transition(GoalStatus::Completed, 5)
        .expect("完成 goal");
    let result = ResultRecord::success(
        ResultId::new("result-atomic-commit").expect("Result ID"),
        goal.goal_id.clone(),
        execution.execution_id.clone(),
        "已完成",
        5,
    );
    let event = JarvisEvent {
        event_id: JarvisEventId::new("event-atomic-commit").expect("Event ID"),
        sequence: 1,
        event_type: "execution.completed".to_owned(),
        idempotency_key: "execution-atomic-commit:completed".to_owned(),
        cause_event_id: None,
        payload_json: "{}".to_owned(),
        occurred_at_ms: 5,
    };

    assert!(
        store
            .commit_execution(
                &completed_execution,
                "worker",
                &lease.token,
                &result,
                &completed_task,
                99,
                &event,
            )
            .is_err()
    );
    assert_eq!(
        store
            .get_execution(&execution.execution_id)
            .expect("读取 Execution")
            .expect("Execution")
            .status,
        ExecutionStatus::Running
    );
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    for table in ["goal_results", "ledger_events"] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("查询事务结果");
        assert_eq!(count, 0, "回滚后不应存在 {table}");
    }

    store
        .commit_execution(
            &completed_execution,
            "worker",
            &lease.token,
            &result,
            &completed_task,
            1,
            &event,
        )
        .expect("原子提交");
    assert_eq!(
        store
            .get_execution(&execution.execution_id)
            .expect("读取 Execution")
            .expect("Execution")
            .status,
        ExecutionStatus::Completed
    );
    for table in ["goal_results", "ledger_events"] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("查询提交结果");
        assert_eq!(count, 1, "提交后应存在一条 {table}");
    }
}

#[test]
fn decision_store_enforces_scope_revision_and_latest_revoke_fails_closed() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let scope = ScopeRef::workspace("decision-workspace").expect("作用域");
    let allow = DecisionRecord {
        decision_id: DecisionId::new("decision-allow").expect("Decision ID"),
        scope: scope.clone(),
        actor: "operator".to_owned(),
        effect: DecisionEffect::Allow,
        revision: 1,
        expires_at_ms: Some(100),
        created_at_ms: 1,
    };
    store.append_decision(&allow, 0).expect("追加 allow");
    assert!(
        store
            .get_latest_decision(&scope, 10)
            .expect("读取")
            .expect("Decision")
            .is_effective_at(10)
    );
    assert!(
        store.append_decision(&allow, 0).is_err(),
        "旧 revision 必须拒绝"
    );

    let revoke = DecisionRecord {
        decision_id: DecisionId::new("decision-revoke").expect("Decision ID"),
        scope: scope.clone(),
        actor: "operator".to_owned(),
        effect: DecisionEffect::Revoke,
        revision: 2,
        expires_at_ms: None,
        created_at_ms: 2,
    };
    store.append_decision(&revoke, 1).expect("追加 revoke");
    let latest = store
        .get_latest_decision(&scope, 10)
        .expect("读取最新 Decision")
        .expect("Decision");
    assert_eq!(latest.effect, DecisionEffect::Revoke);
    assert!(!latest.is_effective_at(10));
}

#[test]
fn ledger_event_store_is_idempotent_and_replay_does_not_mutate_business_state() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let first = JarvisEvent {
        event_id: JarvisEventId::new("event-replay-1").expect("Event ID"),
        sequence: 1,
        event_type: "goal.created".to_owned(),
        idempotency_key: "goal-replay:created".to_owned(),
        cause_event_id: None,
        payload_json: "{}".to_owned(),
        occurred_at_ms: 1,
    };
    let second = JarvisEvent {
        event_id: JarvisEventId::new("event-replay-2").expect("Event ID"),
        sequence: 2,
        event_type: "goal.completed".to_owned(),
        idempotency_key: "goal-replay:completed".to_owned(),
        cause_event_id: Some(first.event_id.clone()),
        payload_json: "{}".to_owned(),
        occurred_at_ms: 2,
    };
    assert!(store.append_ledger_event(&first).expect("首次追加"));
    assert!(!store.append_ledger_event(&first).expect("重复追加幂等"));
    assert!(store.append_ledger_event(&second).expect("追加第二事件"));
    let replayed = store.replay_ledger_events(0, 10).expect("重放");
    assert_eq!(replayed, vec![first.clone(), second]);
    assert!(
        store
            .replay_ledger_events(2, 10)
            .expect("空重放")
            .is_empty()
    );
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM ledger_events", [], |row| row.get(0))
        .expect("查询事件数");
    assert_eq!(count, 2);
}

#[test]
fn goal_store_uses_idempotency_and_revision_compare_and_swap() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut goal = GoalRecord::new(
        GoalId::new("goal-store-1").expect("Goal ID"),
        ScopeRef::global(),
        "编译项目",
        GoalPlan {
            objective: "完成编译和测试".to_owned(),
            steps: vec!["cargo check".to_owned()],
        },
        vec!["检查通过".to_owned()],
        1,
    )
    .expect("goal");
    assert!(store.create_goal(&goal).expect("创建 goal"));
    assert!(!store.create_goal(&goal).expect("幂等创建"));
    goal.transition(GoalStatus::Active, 2).expect("激活");
    store.update_goal(&goal, 0).expect("CAS 更新");
    assert!(store.update_goal(&goal, 0).is_err(), "旧 revision 必须拒绝");
    assert_eq!(store.list_active_goals(10).expect("读取 active").len(), 1);
}

#[test]
fn goal_list_cursor_is_bound_to_scope_and_status_filter() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let goal = GoalRecord::new(
        GoalId::new("goal-cursor").expect("Goal ID"),
        ScopeRef::global(),
        "分页",
        GoalPlan {
            objective: "分页".to_owned(),
            steps: vec!["一步".to_owned()],
        },
        vec!["完成".to_owned()],
        1,
    )
    .expect("goal");
    store.create_goal(&goal).expect("创建 goal");
    let second = GoalRecord::new(
        GoalId::new("goal-cursor-2").expect("Goal ID"),
        ScopeRef::global(),
        "分页二",
        GoalPlan {
            objective: "分页二".to_owned(),
            steps: vec!["一步".to_owned()],
        },
        vec!["完成".to_owned()],
        2,
    )
    .expect("goal");
    store.create_goal(&second).expect("创建第二个 goal");
    let first = store
        .list_goals(
            &ScopeRef::global(),
            &GoalListQuery {
                scope: GoalQueryScope::Global,
                statuses: Vec::new(),
                limit: 1,
                cursor: None,
            },
        )
        .expect("第一页");
    if let Some(cursor) = first.next_cursor {
        let error = store
            .list_goals(
                &ScopeRef::global(),
                &GoalListQuery {
                    scope: GoalQueryScope::Global,
                    statuses: vec![GoalStatus::Active],
                    limit: 1,
                    cursor: Some(cursor),
                },
            )
            .expect_err("过滤器变化必须拒绝游标");
        assert!(error.to_string().contains("游标"));
    }
}

#[test]
fn cancelling_task_persists_request_and_event_and_blocks_claim() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let goal = GoalRecord::new(
        GoalId::new("goal-cancel").expect("Goal ID"),
        ScopeRef::global(),
        "取消",
        GoalPlan {
            objective: "取消".to_owned(),
            steps: vec!["一步".to_owned()],
        },
        vec!["完成".to_owned()],
        1,
    )
    .expect("goal");
    store.create_goal(&goal).expect("创建 goal");
    let execution = ExecutionRecord::new(
        ExecutionId::new("execution-cancel").expect("Execution ID"),
        goal.goal_id.clone(),
        1,
    );
    store.enqueue_execution(&execution).expect("入队");
    let mut cancelled = goal.clone();
    cancelled
        .transition(GoalStatus::Cancelled, 2)
        .expect("取消");
    cancelled.cancellation_reason = Some("用户取消".to_owned());
    cancelled.cancellation_requested_at_ms = Some(2);
    store
        .cancel_goal(&cancelled, 0, "用户取消", 2)
        .expect("原子取消");
    assert!(
        store
            .claim_executions(1, "worker", 3, 100)
            .expect("领取")
            .is_empty()
    );
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    let event_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM ledger_events WHERE event_type='goal.cancelled'",
            [],
            |row| row.get(0),
        )
        .expect("查询取消事件");
    assert_eq!(event_count, 1);
}
