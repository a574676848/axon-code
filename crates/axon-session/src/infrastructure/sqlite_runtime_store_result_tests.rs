use super::SqliteRuntimeStore;
use crate::{GoalStore, ResultStore};
use axon_domain::{GoalId, GoalPlan, GoalRecord, ResultId, ScopeRef};

#[test]
fn result_store_reads_only_unacknowledged_results_in_scope() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let scope = ScopeRef::workspace("result-workspace").expect("作用域");
    let task = GoalRecord::new(
        GoalId::new("task-result-read").expect("Goal ID"),
        scope.clone(),
        "读取结果",
        GoalPlan {
            objective: "验证 unread 投影".to_owned(),
            steps: Vec::new(),
        },
        Vec::new(),
        1,
    )
    .expect("Task");
    store.create_goal(&task).expect("创建 Task");
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    for (result_id, created_at) in [("result-read", 2_i64), ("result-acked", 3_i64)] {
        connection
            .execute(
                "INSERT INTO jarvis_executions(execution_id,goal_id,attempt,priority,status,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,'completed',?5,?5)",
                rusqlite::params![format!("execution-{result_id}"), task.goal_id.as_str(), if result_id == "result-read" { 0_i64 } else { 1_i64 }, 0_i64, created_at],
            )
            .expect("写入 Execution");
        connection
            .execute(
                "INSERT INTO jarvis_results(result_id,goal_id,execution_id,succeeded,summary,receipt_refs_json,artifact_refs_json,error_kind,created_at_ms) VALUES(?1,?2,?3,1,?4,'[]','[]',NULL,?5)",
                rusqlite::params![result_id, task.goal_id.as_str(), format!("execution-{result_id}"), result_id, created_at],
            )
            .expect("写入 Result");
    }
    store
        .acknowledge_results(
            &scope,
            &[
                ResultId::new("result-acked").expect("Result ID"),
                ResultId::new("result-acked").expect("Result ID"),
            ],
            4,
        )
        .expect("确认 Result");
    assert_eq!(
        store
            .acknowledge_results(
                &scope,
                &[ResultId::new("result-acked").expect("Result ID")],
                5,
            )
            .expect("重复确认 Result"),
        0
    );
    let (read_count, acknowledged_at): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),MIN(acknowledged_at_ms) FROM jarvis_result_reads WHERE result_id=?1",
            rusqlite::params!["result-acked"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("读取确认记录");
    assert_eq!((read_count, acknowledged_at), (1, 4));
    assert_eq!(
        store
            .acknowledge_results(
                &scope,
                &[ResultId::new("missing-result").expect("Result ID")],
                6,
            )
            .expect("确认未知 Result"),
        0
    );
    assert_eq!(
        store
            .acknowledge_results(
                &ScopeRef::global(),
                &[ResultId::new("result-read").expect("Result ID")],
                6,
            )
            .expect("错误作用域确认"),
        0
    );
    let unread = store
        .list_unread_results(&scope, 10)
        .expect("读取 unread Result");
    assert_eq!(unread.len(), 1);
    assert_eq!(unread[0].result_id.as_str(), "result-read");
    assert!(
        store
            .list_unread_results(&ScopeRef::global(), 10)
            .expect("隔离读取")
            .is_empty()
    );
    assert_eq!(
        store
            .acknowledge_results(
                &scope,
                &[ResultId::new("result-read").expect("Result ID")],
                7,
            )
            .expect("确认剩余 Result"),
        1
    );
    assert!(
        store
            .list_unread_results(&scope, 10)
            .expect("读取已清空的 unread Result")
            .is_empty()
    );
}
