//! Runtime 健康聚合查询测试。

use super::SqliteRuntimeStore;
use crate::ports::{GoalStore, HealthStore, InteractionStore, RuntimeStore};
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, GoalId, GoalPlan, GoalRecord, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};

const NOW_MS: u64 = 10_000;

#[test]
fn health_reports_empty_runtime_without_sessions() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let counters = store.read_runtime_health(NOW_MS).expect("空库健康聚合");
    assert_eq!(counters.interactions.total, 0);
    assert_eq!(counters.goals.total, 0);
    assert_eq!(counters.executions.total, 0);
    assert_eq!(counters.execution_backlog.pending, 0);
    assert_eq!(counters.execution_backlog.oldest_age_ms, None);
}

#[test]
fn health_counts_statuses_and_oldest_execution_age() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let interaction = InteractionRecord::new(
        InteractionId::new("health-interaction").expect("interaction"),
        InteractionSource::new("cli", "health-message").expect("来源"),
        ScopeRef::global(),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("cli", "health-conversation").expect("回复路由"),
        "健康检查",
        1_000,
    )
    .expect("Interaction");
    store.append(&interaction).expect("Interaction 写入");
    let task = GoalRecord::new(
        GoalId::new("health-task").expect("task"),
        ScopeRef::global(),
        "健康检查",
        GoalPlan {
            objective: "验证健康聚合".to_owned(),
            steps: Vec::new(),
        },
        Vec::new(),
        1_000,
    )
    .expect("Task");
    store.create_goal(&task).expect("Task 写入");

    store.initialize().expect("初始化 schema");
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    connection
        .execute(
            "INSERT INTO jarvis_executions(execution_id,goal_id,attempt,priority,status,created_at_ms,updated_at_ms) VALUES('health-execution','health-task',0,0,'queued',2000,2000)",
            [],
        )
        .expect("写入 Execution");
    connection
        .execute(
            "INSERT INTO jarvis_executions(execution_id,goal_id,attempt,priority,status,created_at_ms,updated_at_ms) VALUES('health-completed','health-task',1,0,'completed',3000,3000)",
            [],
        )
        .expect("写入终态 Execution");
    drop(connection);

    let counters = store.read_runtime_health(NOW_MS).expect("健康聚合");

    assert_eq!(counters.interactions.total, 1);
    assert_eq!(counters.interactions.by_status.get("received"), Some(&1));
    assert_eq!(counters.goals.total, 1);
    assert_eq!(counters.goals.by_status.get("proposed"), Some(&1));
    assert_eq!(counters.executions.total, 2);
    assert_eq!(counters.executions.by_status.get("queued"), Some(&1));
    assert_eq!(counters.executions.by_status.get("completed"), Some(&1));
    assert_eq!(counters.execution_backlog.pending, 1);
    assert_eq!(
        counters.execution_backlog.oldest_age_ms,
        Some(NOW_MS - 2_000)
    );
}
