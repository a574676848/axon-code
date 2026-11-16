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
            "INSERT INTO work_items(execution_id,goal_id,attempt,priority,status,created_at_ms,updated_at_ms) VALUES('health-execution','health-task',0,0,'queued',2000,2000)",
            [],
        )
        .expect("写入 Execution");
    connection
        .execute(
            "INSERT INTO work_items(execution_id,goal_id,attempt,priority,status,created_at_ms,updated_at_ms) VALUES('health-completed','health-task',1,0,'completed',3000,3000)",
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

/// 旧的单条 JOIN + GROUP BY 口径，只用于给性能验收提供基线对照。
const LEGACY_INTERACTION_STATUS_SQL: &str = "SELECT CASE WHEN w.status='running' THEN 'processing' \
     WHEN h.terminal_kind IS NOT NULL THEN h.terminal_kind \
     WHEN w.terminal_kind IS NOT NULL THEN w.terminal_kind ELSE 'received' END AS state,COUNT(*) \
     FROM inbox AS i LEFT JOIN work_items AS w ON w.inbox_id=i.interaction_id \
     LEFT JOIN work_item_history AS h ON h.inbox_id=i.interaction_id GROUP BY state";

#[test]
#[ignore = "性能验收：2 万入站事实 + 2 万终态历史下健康聚合门槛 30ms"]
fn runtime_health_aggregation_is_index_bounded() {
    const ROWS: i64 = 20_000;
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    store.initialize().expect("初始化");
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .expect("开启批量写入");
    {
        let mut inbox = connection
            .prepare(
                "INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,\
                 audience_json,reply_route_json,content,received_at_ms,updated_at_ms,\
                 attachments_json,session_id) \
                 VALUES(?1,'tui',?1,'{}','{}','{}','hi',?2,?2,'[]','session-health')",
            )
            .expect("准备入站写入");
        let mut history = connection
            .prepare(
                "INSERT INTO work_item_history(work_id,kind,attempt,priority,status,terminal_kind,\
                 inbox_id,created_at_ms,updated_at_ms,finished_at_ms) \
                 VALUES(?1,'turn',1,0,'completed','understood',?2,?3,?3,?3)",
            )
            .expect("准备历史写入");
        for index in 0..ROWS {
            inbox
                .execute(rusqlite::params![format!("interaction-{index:06}"), index])
                .expect("写入入站事实");
            history
                .execute(rusqlite::params![
                    format!("work-{index:06}"),
                    format!("interaction-{index:06}"),
                    index
                ])
                .expect("写入终态历史");
        }
    }
    connection.execute_batch("COMMIT").expect("提交批量写入");

    let legacy_started = std::time::Instant::now();
    {
        let mut statement = connection
            .prepare(LEGACY_INTERACTION_STATUS_SQL)
            .expect("准备旧口径");
        let rows = statement
            .query_map([], |row| row.get::<_, i64>(1))
            .expect("执行旧口径")
            .collect::<Result<Vec<_>, _>>()
            .expect("读取旧口径");
        assert_eq!(rows.iter().sum::<i64>(), ROWS);
    }
    let legacy_ms = legacy_started.elapsed().as_millis();

    let started = std::time::Instant::now();
    let counters = store.read_runtime_health(NOW_MS).expect("健康聚合");
    let elapsed_ms = started.elapsed().as_millis();
    assert_eq!(
        counters.interactions.total,
        u64::try_from(ROWS).expect("行数")
    );
    assert_eq!(
        counters.interactions.by_status.get("understood"),
        Some(&u64::try_from(ROWS).expect("行数"))
    );
    println!(
        "runtime_health inbox_rows={ROWS} history_rows={ROWS} new={elapsed_ms}ms legacy={legacy_ms}ms 门槛=30ms"
    );
    assert!(elapsed_ms < 30, "健康聚合超出 30ms 门槛：{elapsed_ms}ms");
    assert!(
        elapsed_ms.saturating_mul(2) < legacy_ms.max(1),
        "新口径必须明显快于逐行 JOIN：new={elapsed_ms}ms legacy={legacy_ms}ms"
    );
}
