//! 没有所属目标的执行行（`work_items.goal_id` 为空）的读写契约。
//!
//! 入站领取的轮次与会话动作都不归属任何目标，事实源里就是 NULL。曾经按非空字符串
//! 读这一列，后果是 supervisor 每次隔离扫描都整体失败、待处理列表无法读出，而失败
//! 原因只是「这条事实本来就没有目标」。

use super::SqliteRuntimeStore;
use crate::ports::{ExecutionStore, RuntimeStore};
use axon_domain::{DaemonSessionId, ExecutionId, ExecutionStatus, RuntimeHostId};

/// 一条没有所属目标的执行行，租约按需给定。
fn insert_goal_less_work_item(
    store: &SqliteRuntimeStore,
    execution_id: &str,
    session_id: &DaemonSessionId,
    status: &str,
    lease_expires_at_ms: Option<i64>,
) {
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    connection
        .execute(
            "INSERT INTO work_items(execution_id,goal_id,session_id,kind,priority,status,payload_json,attempt,lease_expires_at_ms,created_at_ms,updated_at_ms) VALUES(?1,NULL,?2,'turn',0,?3,'{}',1,?4,10,10)",
            rusqlite::params![
                execution_id,
                session_id.as_str(),
                status,
                lease_expires_at_ms
            ],
        )
        .expect("写入无目标执行行");
}

#[test]
fn goal_less_pending_action_reads_back_without_goal() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("session-goal-less-queue").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    insert_goal_less_work_item(&store, "turn-goal-less", &session_id, "queued", None);

    let pending = store
        .list_pending_actions(&session_id, 8)
        .expect("无目标的待处理动作必须可读");
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0].goal_id.is_none(),
        "空 goal_id 必须原样读成 None，不能编造出一个目标"
    );
}

#[test]
fn expired_goal_less_execution_quarantines_without_touching_goal_ledgers() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("host-goal-less").expect("host"),
            1,
            None,
            10,
            10_000,
        )
        .expect("claim");
    let session_id = DaemonSessionId::new("session-goal-less-run").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    insert_goal_less_work_item(
        &store,
        "turn-goal-less-run",
        &session_id,
        "running",
        Some(20),
    );

    let records = store
        .quarantine_interrupted_executions(&host, 100, 4)
        .expect("无目标的过期执行必须照样被隔离");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event_type, "turn_uncertain");
    assert!(
        records[0].payload_json.contains(r#""goalId":null"#),
        "无目标事件的 goalId 必须是 null，不能伪造目标标识：{}",
        records[0].payload_json
    );

    let execution = store
        .get_execution(&ExecutionId::new("turn-goal-less-run").expect("execution"))
        .expect("读取执行")
        .expect("执行存在");
    assert_eq!(execution.status, ExecutionStatus::Uncertain);

    // 结果账本按 goal 归属：没有目标就没有可归属的结果行，伪造一条会撞外键约束。
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    let results: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM goal_results WHERE execution_id=?1",
            rusqlite::params!["turn-goal-less-run"],
            |row| row.get(0),
        )
        .expect("统计结果行");
    assert_eq!(results, 0, "无目标执行不得写入目标结果账本");

    assert!(
        store
            .quarantine_interrupted_executions(&host, 200, 4)
            .expect("重复隔离应幂等")
            .is_empty()
    );
}

/// 暂停与取消都要真的让在途轮次停下来：标记落库，运行侧按会话查得到。
#[test]
fn execution_stop_marks_the_running_work_item_for_its_session() {
    use crate::ports::GoalStore as _;
    use axon_domain::GoalId;

    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("session-stop").expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    let other = DaemonSessionId::new("session-other").expect("session");
    store.create_session(&other, ".", 10).expect("session");

    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    connection
        .execute(
            "INSERT INTO work_items(execution_id,goal_id,session_id,kind,payload_json,attempt,priority,status,created_at_ms,updated_at_ms) \
             VALUES('exec-stop','goal-stop','session-stop','turn','{}',0,0,'running',10,10)",
            [],
        )
        .expect("插入执行行");
    drop(connection);

    assert!(
        !store
            .session_cancellation_requested(&session_id)
            .expect("查询")
    );
    store
        .request_execution_stop(&GoalId::new("goal-stop").expect("goal"), 20)
        .expect("写终止标记");
    assert!(
        store
            .session_cancellation_requested(&session_id)
            .expect("查询"),
        "暂停或取消之后，运行侧必须能查到终止标记"
    );
    assert!(
        !store.session_cancellation_requested(&other).expect("查询"),
        "标记只作用于该目标自己的会话"
    );
}
