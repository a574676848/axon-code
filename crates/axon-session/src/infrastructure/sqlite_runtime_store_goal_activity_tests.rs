use super::SqliteRuntimeStore;
use crate::{GoalMutationStore, GoalStore, LedgerEventStore};
use axon_domain::{GoalId, GoalPlan, GoalRecord, GoalStatus, JarvisEvent, JarvisEventId, ScopeRef};

fn task(id: &str) -> GoalRecord {
    GoalRecord::new(
        GoalId::new(id).expect("Goal ID"),
        ScopeRef::global(),
        "测试任务",
        GoalPlan {
            objective: "验证 Goal 账本一致性".to_owned(),
            steps: vec!["读取事件".to_owned()],
        },
        vec!["事件可查询".to_owned()],
        1,
    )
    .expect("Task")
}

fn event(id: &str, key: &str, sequence: u64, payload_json: String) -> JarvisEvent {
    JarvisEvent {
        event_id: JarvisEventId::new(id).expect("Event ID"),
        sequence,
        event_type: "goal.updated".to_owned(),
        idempotency_key: key.to_owned(),
        cause_event_id: None,
        payload_json,
        occurred_at_ms: sequence.max(1),
    }
}

#[test]
fn task_event_lookup_is_bounded_indexed_and_supports_legacy_payload_key() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let target = task("task-event-target");
    store.create_goal(&target).expect("创建 Task");

    for sequence in 1..=300 {
        let unrelated = event(
            &format!("event-unrelated-{sequence}"),
            &format!("event-unrelated-key-{sequence}"),
            sequence,
            format!(r#"{{"goal_id":"other-{sequence}"}}"#),
        );
        store.append_ledger_event(&unrelated).expect("追加无关事件");
    }
    let target_event = event(
        "event-target",
        "event-target-key",
        301,
        format!(r#"{{"goalId":"{}"}}"#, target.goal_id),
    );
    store
        .append_ledger_event(&target_event)
        .expect("追加目标事件");
    let latest_target_event = event(
        "event-target-latest",
        "event-target-latest-key",
        302,
        format!(r#"{{"goal_id":"{}"}}"#, target.goal_id),
    );
    store
        .append_ledger_event(&latest_target_event)
        .expect("追加最新目标事件");

    let events = store
        .list_events_for_goal(&target.goal_id, 1)
        .expect("按 Goal 查询事件");
    assert_eq!(events, vec![latest_target_event.clone()]);
    let all_events = store
        .list_events_for_goal(&target.goal_id, 2)
        .expect("读取目标事件窗口");
    assert_eq!(
        all_events,
        vec![latest_target_event.clone(), target_event.clone()]
    );

    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    let index_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='jarvis_events_goal_lookup')",
            [],
            |row| row.get(0),
        )
        .expect("检查 Task 事件索引");
    assert!(index_exists);
    let plan = connection
        .prepare("EXPLAIN QUERY PLAN SELECT event_id FROM jarvis_events WHERE COALESCE(json_extract(payload_json,'$.goal_id'),json_extract(payload_json,'$.goalId'))=?1 ORDER BY sequence ASC LIMIT ?2")
        .expect("准备查询计划")
        .query_map([target.goal_id.as_str(), "1"], |row| row.get::<_, String>(3))
        .expect("读取查询计划")
        .collect::<Result<Vec<_>, _>>()
        .expect("收集查询计划");
    assert!(
        plan.iter()
            .any(|detail| detail.contains("jarvis_events_goal_lookup")),
        "查询计划未命中 Task 事件索引：{plan:?}"
    );
}

#[test]
fn goal_update_and_audit_event_commit_atomically_with_cas() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let task = task("task-update-audit");
    store.create_goal(&task).expect("创建 Task");

    let mut updated = task.clone();
    updated
        .transition(GoalStatus::Active, 2)
        .expect("激活 Task");
    let update_event = event(
        "event-update-audit",
        "goal.updated:task-update-audit:1",
        0,
        format!(r#"{{"goal_id":"{}","revision":1}}"#, task.goal_id),
    );
    store
        .update_goal_with_event(&updated, 0, &update_event)
        .expect("原子更新");
    assert_eq!(
        store
            .get_goal(&task.goal_id)
            .expect("读取 Task")
            .expect("Task")
            .revision,
        1
    );
    assert_eq!(
        store
            .list_events_for_goal(&task.goal_id, 10)
            .expect("读取审计")
            .len(),
        1
    );

    assert!(
        store
            .update_goal_with_event(&updated, 0, &update_event)
            .is_err()
    );
    assert_eq!(
        store
            .list_events_for_goal(&task.goal_id, 10)
            .expect("读取审计")
            .len(),
        1
    );

    let mut retry = updated.clone();
    retry.transition(GoalStatus::Waiting, 3).expect("等待 Task");
    let conflicting_event = event(
        "event-update-conflict",
        "goal.updated:task-update-audit:1",
        0,
        format!(r#"{{"goal_id":"{}","revision":2}}"#, task.goal_id),
    );
    assert!(
        store
            .update_goal_with_event(&retry, 1, &conflicting_event)
            .is_err()
    );
    let persisted = store
        .get_goal(&task.goal_id)
        .expect("读取回滚 Task")
        .expect("Task");
    assert_eq!(persisted.revision, 1, "Event 冲突时 Goal 更新必须回滚");
    assert_eq!(
        store
            .list_events_for_goal(&task.goal_id, 10)
            .expect("读取回滚审计")
            .len(),
        1
    );
}
