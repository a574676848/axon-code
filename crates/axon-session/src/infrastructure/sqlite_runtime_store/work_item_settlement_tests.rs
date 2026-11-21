//! 终态唯一出口的回归测试：终态不变式、口径同源与现场存量收口。

use super::*;
use crate::DaemonHostRecord;
use crate::infrastructure::sqlite_runtime_store_interaction_fixtures::{
    interaction_record, test_session,
};
use crate::ports::{ExecutionStore, GoalStore, InteractionQueryStore, InteractionStore as _};
use axon_domain::{
    ExecutionId, ExecutionRecord, GoalId, GoalPlan, GoalRecord, GoalStatus, RuntimeHostId, ScopeRef,
};
use rusqlite::OptionalExtension;

/// 现场手工修库时被置为 `interrupted` 的交互标识。
const FIELD_INTERACTION_ID: &str = "01M7Z4BFYY8QX8DKF83S6DHBRE";

#[test]
fn uncertain_settlement_writes_terminal_kind_and_archives() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = claim_test_host(&store, "host-uncertain");
    let session_id = DaemonSessionId::new("session-uncertain-settlement").expect("会话");
    store.create_session(&session_id, ".", 10).expect("会话");
    let goal = active_goal("goal-uncertain-settlement");
    store.create_goal(&goal).expect("目标");
    let mut execution = ExecutionRecord::new(
        ExecutionId::new("execution-uncertain-settlement").expect("执行"),
        goal.goal_id.clone(),
        20,
    );
    execution.session_id = Some(session_id.clone());
    store.enqueue_execution(&execution).expect("入队");
    store
        .claim_executions(1, "worker-uncertain", 30, 1_000)
        .expect("领取");
    let execution_id = execution.execution_id.as_str();

    let records = store
        .quarantine_interrupted_executions(&host, 1_100, 4)
        .expect("被动隔离");

    assert_eq!(records.len(), 1, "被动隔离必须产出中断证据");
    assert_eq!(records[0].event_type, "turn_uncertain");
    assert!(
        !work_item_present(&store, execution_id),
        "结论行必须搬出在途表"
    );
    assert_eq!(
        history_column(&store, execution_id, "terminal_kind").as_deref(),
        Some(TERMINAL_INTERRUPTED),
        "历史行必须带上终态类别，否则派生列会退回队列口径"
    );
    assert_eq!(
        history_column(&store, execution_id, "status").as_deref(),
        Some(status_code(ExecutionStatus::Uncertain)),
        "队列口径的结论状态保持不变"
    );
    assert_eq!(
        ledger_event_count(&store, "execution.uncertain"),
        1,
        "既有流水账事件名不得改名"
    );
    assert_eq!(conclusion_violations(&store), 0, "INV-2 必须为零行");
}

#[test]
fn every_conclusive_status_has_terminal_kind() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    for (index, status) in interaction_status::CONCLUSIVE_EXECUTION_STATUSES
        .iter()
        .enumerate()
    {
        let execution = ExecutionRecord::new(
            ExecutionId::new(format!("execution-conclusive-{index}")).expect("执行"),
            GoalId::new(format!("goal-conclusive-{index}")).expect("目标"),
            20,
        );
        store.enqueue_execution(&execution).expect("入队");
        store
            .finish_execution(&execution.execution_id, *status, 30)
            .expect("结算执行");
        let derived = interaction_status::derived_from_conclusive(*status)
            .expect("每个结论状态都必须能派生 Interaction 口径");
        let execution_id = execution.execution_id.as_str();
        assert!(
            !work_item_present(&store, execution_id),
            "{status:?} 结算是终态，行必须搬出在途表"
        );
        assert_eq!(
            history_column(&store, execution_id, "terminal_kind").as_deref(),
            Some(interaction_status::token(derived)),
            "{status:?} 必须与状态同时写 terminal_kind"
        );
        assert_eq!(
            history_column(&store, execution_id, "status").as_deref(),
            Some(status_code(*status))
        );
        assert_eq!(conclusion_violations(&store), 0, "INV-2 必须为零行");
    }
}

#[test]
fn terminal_status_enum_matches_inbox_state_expression() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-state-expression");
    store.create_session(&session, ".", 10).expect("会话");

    // 存量行：只改了 status、没写 terminal_kind。派生列必须自己把结论读出来。
    let mut expected = Vec::new();
    for (index, status) in interaction_status::CONCLUSIVE_EXECUTION_STATUSES
        .iter()
        .enumerate()
    {
        let interaction_id = format!("interaction-expression-{index}");
        store
            .append(&interaction_record(
                &session,
                &interaction_id,
                &format!("message-expression-{index}"),
                20,
            ))
            .expect("入站事实");
        insert_work_item(
            &store,
            &format!("execution-expression-{index}"),
            &interaction_id,
            status_code(*status),
            None,
            Some(9_000),
        );
        expected.push((
            interaction_id,
            interaction_status::derived_from_conclusive(*status).expect("派生状态"),
        ));
    }
    // 对照行：仍在跑的轮次与从未领取的轮次都必须派生为非终态。
    store
        .append(&interaction_record(
            &session,
            "interaction-expression-running",
            "message-expression-running",
            30,
        ))
        .expect("入站事实");
    insert_work_item(
        &store,
        "execution-expression-running",
        "interaction-expression-running",
        "running",
        None,
        Some(9_000),
    );
    store
        .append(&interaction_record(
            &session,
            "interaction-expression-received",
            "message-expression-received",
            40,
        ))
        .expect("入站事实");

    let heads = store
        .interaction_turn_heads(session.as_str(), 10)
        .expect("轮次头");
    for (interaction_id, derived) in &expected {
        let head = heads
            .iter()
            .find(|head| head.interaction_id == *interaction_id)
            .expect("轮次头必须存在");
        assert_eq!(head.status, *derived, "派生列必须与领域枚举同源");
        assert!(
            interaction_status::is_terminal(head.status),
            "结论状态必须被判定为终态"
        );
    }
    for interaction_id in [
        "interaction-expression-running",
        "interaction-expression-received",
    ] {
        let head = heads
            .iter()
            .find(|head| head.interaction_id == interaction_id)
            .expect("轮次头必须存在");
        assert!(
            !interaction_status::is_terminal(head.status),
            "在途轮次不得被判成终态"
        );
    }

    // SQL 侧的词汇必须由枚举生成：结论分支要覆盖每个结论状态，派生目标必须是终态词，
    // 透传的终态词必须都能解码回终态（否则含该轮的整条车道历史读不出来）。
    let expression = super::super::interaction_query_repository::inbox_state_expr();
    for status in interaction_status::CONCLUSIVE_EXECUTION_STATUSES {
        let token = status_code(status);
        assert!(
            expression.contains(&format!("'{token}'")),
            "派生表达式缺结论状态 {token}"
        );
        let derived = interaction_status::derived_from_conclusive(status).expect("派生状态");
        assert!(
            expression.contains(&format!("'{}'", interaction_status::token(derived))),
            "派生表达式缺 {token} 的终态目标"
        );
        assert!(
            interaction_status::is_terminal(derived),
            "派生目标必须是终态"
        );
    }
    for token in [
        interaction_status::token(InteractionStatus::Understood),
        interaction_status::token(InteractionStatus::Rejected),
        interaction_status::token(InteractionStatus::Interrupted),
    ] {
        let parsed = super::super::interaction_repository_row::parse_status(token)
            .expect("终态词必须可解码");
        assert!(
            interaction_status::is_terminal(parsed),
            "透传的终态词 {token} 必须解码为终态"
        );
    }
}

#[test]
fn legacy_uncertain_history_row_still_decodes() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-legacy-uncertain");
    store.create_session(&session, ".", 10).expect("会话");
    store
        .append(&interaction_record(
            &session,
            "interaction-legacy-uncertain",
            "message-legacy-uncertain",
            20,
        ))
        .expect("入站事实");
    // 存量历史行：归档时的 `COALESCE(terminal_kind,status)` 把队列口径的 `uncertain`
    // 直接写进了派生列。认不出这个词，含该轮的整条车道历史都会整体读不出来。
    insert_history_row(
        &store,
        "work-legacy-uncertain",
        "interaction-legacy-uncertain",
        "uncertain",
        "uncertain",
    );

    let interaction_id = InteractionId::new("interaction-legacy-uncertain").expect("交互标识");
    let record = store
        .get_interaction(&interaction_id)
        .expect("存量行必须可解码")
        .expect("轮次必须存在");
    assert_eq!(record.status, InteractionStatus::Interrupted);
    let heads = store
        .interaction_turn_heads(session.as_str(), 10)
        .expect("轮次头");
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].status, InteractionStatus::Interrupted);
}

/// 现场两处手工修库的等价最小行：官方路径重放必须得到同样的终态类别。
///
/// 现场记录（先备份后改）：`work_items`（execution `a3870cba-…`）与 `inbox`
/// （interaction `01M7Z4BFYY8QX8DKF83S6DHBRE`）的 `terminal_kind` 都被手工置为
/// `interrupted`。现场库不可复制，这里用等价最小行重放两条官方收口路径：执行侧走
/// 租约失效隔离，入站侧走 processing 租约失效隔离，断言结果与手工修库一致。
#[test]
fn field_manual_repair_is_reproduced_by_official_settlement() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = claim_test_host(&store, "host-field-replay");
    let session = test_session("session-field-replay");
    store.create_session(&session, ".", 10).expect("会话");

    // 现场一：目标执行行的租约随进程一起消失。
    let goal = active_goal("goal-field-replay");
    store.create_goal(&goal).expect("目标");
    let execution = ExecutionRecord::new(
        ExecutionId::new("a3870cba-0000-4000-8000-000000000001").expect("执行"),
        goal.goal_id.clone(),
        20,
    );
    store.enqueue_execution(&execution).expect("入队");
    store
        .claim_executions(1, "worker-field-replay", 30, 1_000)
        .expect("领取");
    store
        .quarantine_interrupted_executions(&host, 1_100, 4)
        .expect("执行侧收口");

    // 现场二：入站轮次的 processing 租约随进程一起消失。
    store
        .append(&interaction_record(
            &session,
            FIELD_INTERACTION_ID,
            "message-field-replay",
            2_000,
        ))
        .expect("入站事实");
    let field_interaction_id = InteractionId::new(FIELD_INTERACTION_ID).expect("交互标识");
    store
        .claim_processing_by_id(&field_interaction_id, 2_010, 100)
        .expect("领取")
        .expect("租约");
    let records = store
        .quarantine_interrupted_interactions(&host, 2_200, 10)
        .expect("入站侧收口");

    assert_eq!(records.len(), 1);
    assert_eq!(
        history_column(&store, execution.execution_id.as_str(), "terminal_kind").as_deref(),
        Some(TERMINAL_INTERRUPTED),
        "执行侧重放必须与手工修库得到同一终态类别"
    );
    assert_eq!(
        inbox_history_terminal_kind(&store, FIELD_INTERACTION_ID).as_deref(),
        Some(TERMINAL_INTERRUPTED),
        "入站侧重放必须与手工修库得到同一终态类别"
    );
    assert_eq!(ledger_event_count(&store, "execution.uncertain"), 1);
    assert_eq!(ledger_event_count(&store, "interaction.interrupted"), 1);
    let heads = store
        .interaction_turn_heads(session.as_str(), 10)
        .expect("轮次头");
    let field_head = heads
        .iter()
        .find(|head| head.interaction_id == FIELD_INTERACTION_ID)
        .expect("现场轮次必须仍然存在");
    assert_eq!(field_head.status, InteractionStatus::Interrupted);
    assert_eq!(conclusion_violations(&store), 0, "INV-2 必须为零行");
}

fn claim_test_host(store: &SqliteRuntimeStore, host_id: &str) -> DaemonHostRecord {
    store
        .claim_host(
            &RuntimeHostId::new(host_id).expect("Host 标识"),
            1,
            None,
            10,
            1_000_000,
        )
        .expect("Host 租约")
}

fn active_goal(goal_id: &str) -> GoalRecord {
    let mut goal = GoalRecord::new(
        GoalId::new(goal_id).expect("目标标识"),
        ScopeRef::global(),
        "终态出口验证",
        GoalPlan {
            objective: "验证终态唯一出口".to_owned(),
            steps: Vec::new(),
            write_policy: Default::default(),
        },
        Vec::new(),
        10,
    )
    .expect("目标");
    goal.transition(GoalStatus::Active, 11).expect("激活");
    goal
}

fn insert_work_item(
    store: &SqliteRuntimeStore,
    execution_id: &str,
    inbox_id: &str,
    status: &str,
    terminal_kind: Option<&str>,
    lease_expires_at_ms: Option<u64>,
) {
    store
        .connection()
        .expect("控制面连接")
        .execute(
            "INSERT INTO work_items(execution_id,goal_id,session_id,kind,payload_json,attempt,\
             priority,status,terminal_kind,lease_token,lease_expires_at_ms,inbox_id,\
             created_at_ms,updated_at_ms) \
             VALUES(?1,NULL,NULL,'turn','{}',1,0,?2,?3,'lease-field',?4,?5,10,10)",
            params![
                execution_id,
                status,
                terminal_kind,
                lease_expires_at_ms.map(|value| i64::try_from(value).expect("毫秒")),
                inbox_id
            ],
        )
        .expect("插入存量工作项");
}

fn insert_history_row(
    store: &SqliteRuntimeStore,
    work_id: &str,
    inbox_id: &str,
    terminal_kind: &str,
    status: &str,
) {
    store
        .connection()
        .expect("控制面连接")
        .execute(
            "INSERT INTO work_item_history(work_id,goal_id,session_id,kind,payload_json,attempt,\
             priority,inbox_id,terminal_kind,status,created_at_ms,updated_at_ms,finished_at_ms) \
             VALUES(?1,NULL,NULL,'turn','{}',1,0,?2,?3,?4,10,10,20)",
            params![work_id, inbox_id, terminal_kind, status],
        )
        .expect("插入存量历史行");
}

fn work_item_present(store: &SqliteRuntimeStore, execution_id: &str) -> bool {
    read_scalar::<i64>(
        store,
        "SELECT COUNT(*) FROM work_items WHERE execution_id=?1",
        execution_id,
    ) > 0
}

fn history_column(store: &SqliteRuntimeStore, work_id: &str, column: &str) -> Option<String> {
    let sql = format!(
        "SELECT {column} FROM work_item_history WHERE work_id=?1 ORDER BY finished_at_ms DESC LIMIT 1"
    );
    read_optional(store, &sql, work_id)
}

fn inbox_history_terminal_kind(store: &SqliteRuntimeStore, inbox_id: &str) -> Option<String> {
    read_optional(
        store,
        "SELECT terminal_kind FROM work_item_history WHERE inbox_id=?1 ORDER BY finished_at_ms DESC LIMIT 1",
        inbox_id,
    )
}

fn ledger_event_count(store: &SqliteRuntimeStore, event_type: &str) -> i64 {
    read_scalar::<i64>(
        store,
        "SELECT COUNT(*) FROM ledger_events WHERE event_type=?1",
        event_type,
    )
}

/// INV-2 的行数：结论状态却没有 `terminal_kind` 的行必须为零。
fn conclusion_violations(store: &SqliteRuntimeStore) -> i64 {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                &conclusive_work_items_without_terminal_kind_sql(),
                [],
                |row| row.get(0),
            )
        })
        .expect("INV-2 检查")
}

fn read_scalar<T>(store: &SqliteRuntimeStore, sql: &str, key: &str) -> T
where
    T: rusqlite::types::FromSql,
{
    store
        .with_read_connection(|connection| connection.query_row(sql, [key], |row| row.get(0)))
        .expect("只读标量")
}

fn read_optional(store: &SqliteRuntimeStore, sql: &str, key: &str) -> Option<String> {
    store
        .with_read_connection(|connection| {
            connection
                .query_row(sql, [key], |row| row.get::<_, String>(0))
                .optional()
        })
        .expect("只读可空列")
}
