use super::SqliteRuntimeStore;
use crate::{ActivityQuery, ActivityService, GoalStore, LedgerEventStore, RuntimeStore};
use axon_domain::{GoalId, GoalPlan, GoalRecord, GoalStatus, JarvisEvent, JarvisEventId, ScopeRef};

fn active_task(id: &str, scope: ScopeRef, now_ms: u64) -> GoalRecord {
    let mut task = GoalRecord::new(
        GoalId::new(id).expect("Goal ID"),
        scope,
        id,
        GoalPlan {
            objective: format!("{id} objective"),
            steps: Vec::new(),
        },
        Vec::new(),
        now_ms,
    )
    .expect("Task");
    task.transition(GoalStatus::Active, now_ms + 1)
        .expect("激活 Task");
    task
}

fn insert_result(
    connection: &rusqlite::Connection,
    task: &GoalRecord,
    result_id: &str,
    created_at_ms: u64,
) {
    let execution_id = format!("execution-{result_id}");
    connection
        .execute(
            "INSERT INTO jarvis_executions(execution_id,goal_id,attempt,priority,status,created_at_ms,updated_at_ms) VALUES(?1,?2,0,0,'completed',?3,?3)",
            rusqlite::params![execution_id, task.goal_id.as_str(), created_at_ms as i64],
        )
        .expect("写入 Execution");
    connection
        .execute(
            "INSERT INTO jarvis_results(result_id,goal_id,execution_id,succeeded,summary,receipt_refs_json,artifact_refs_json,error_kind,created_at_ms) VALUES(?1,?2,?3,1,?4,'[]','[]',NULL,?5)",
            rusqlite::params![
                result_id,
                task.goal_id.as_str(),
                format!("execution-{result_id}"),
                result_id,
                created_at_ms as i64
            ],
        )
        .expect("写入 Result");
}

#[test]
fn activity_reads_scoped_facts_with_task_result_and_event_bounds() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let scope_a = ScopeRef::workspace("activity-a").expect("作用域");
    let scope_b = ScopeRef::workspace("activity-b").expect("作用域");
    let task_a_old = active_task("activity-task-a-old", scope_a.clone(), 1);
    let task_a_new = active_task("activity-task-a-new", scope_a.clone(), 2);
    let task_b = active_task("activity-task-b", scope_b, 3);

    for task in [&task_a_old, &task_a_new, &task_b] {
        assert!(store.create_goal(task).expect("创建 Task"));
    }

    store.initialize().expect("初始化 schema");
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    insert_result(&connection, &task_a_old, "activity-result-a-old", 10);
    insert_result(&connection, &task_a_new, "activity-result-a-new", 11);
    insert_result(&connection, &task_b, "activity-result-b", 12);
    drop(connection);

    for sequence in 1..=3 {
        let event = JarvisEvent {
            event_id: JarvisEventId::new(format!("activity-event-{sequence}")).expect("Event ID"),
            sequence,
            event_type: "goal.updated".to_owned(),
            idempotency_key: format!("activity-event-key-{sequence}"),
            cause_event_id: None,
            payload_json: format!("{{\"sequence\":{sequence}}}"),
            occurred_at_ms: sequence,
        };
        assert!(store.append_ledger_event(&event).expect("追加 Event"));
    }

    let snapshot = ActivityService::new(store)
        .read(&ActivityQuery {
            scope: scope_a,
            goal_limit: 1,
            result_limit: 1,
            event_after_sequence: 1,
            event_limit: 1,
        })
        .expect("读取 Activity");

    assert_eq!(snapshot.active_goals.len(), 1);
    assert_eq!(
        snapshot.active_goals[0].goal_id.as_str(),
        "activity-task-a-old"
    );
    assert_eq!(snapshot.unread_results.len(), 1);
    assert_eq!(
        snapshot.unread_results[0].result_id.as_str(),
        "activity-result-a-old"
    );
    assert_eq!(snapshot.events.len(), 1);
    assert_eq!(snapshot.events[0].sequence, 2);
}
