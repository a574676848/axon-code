//! 工人在同一工作区内的任务流与监督扫描：并行车道、重复建任务，以及中断执行的隔离。

use super::*;

#[test]
fn primary_turn_runs_different_workspace_lanes_concurrently() {
    let directory = axon_test_support::test_temp_dir();
    let first_root = directory.path().join("workspace-a");
    let second_root = directory.path().join("workspace-b");
    std::fs::create_dir_all(&first_root).expect("创建首个工作区");
    std::fs::create_dir_all(&second_root).expect("创建第二个工作区");
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let first_workspace = register_workspace(&store, &first_root);
    let second_workspace = register_workspace(&store, &second_root);
    store
        .append(&interaction(
            "concurrent-a",
            "message-a",
            &first_workspace.workspace_id,
            "A",
        ))
        .expect("持久化首个 Interaction");
    store
        .append(&interaction(
            "concurrent-b",
            "message-b",
            &second_workspace.workspace_id,
            "B",
        ))
        .expect("持久化第二个 Interaction");
    let executor = ConcurrentExecutor::default();
    let mut host = test_host(store, executor.clone());
    host.start().expect("启动 Host");
    let tick_host = host.clone();
    let tick = thread::spawn(move || tick_host.primary_turn_tick(10));

    executor.wait_until_started(2);
    executor.release();
    tick.join()
        .expect("Primary Turn 协调线程不得 panic")
        .expect("Primary Turn tick 成功");
}

#[test]
fn supervisor_tick_leaves_primary_turn_to_dedicated_coordinator() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    store
        .append(&interaction(
            "supervisor-boundary",
            "supervisor-boundary-message",
            &workspace.workspace_id,
            "保持 Supervisor 响应",
        ))
        .expect("持久化 Interaction");
    let executor = AnswerExecutor::default();
    let mut host = test_host(store, executor.clone());
    host.start().expect("启动 Host");

    host.supervisor_tick(10).expect("Supervisor tick 成功");

    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn supervisor_tick_quarantines_interrupted_execution_before_reclaim() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("interrupted-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 1)
        .expect("创建 Session");
    let mut task = GoalRecord::new(
        GoalId::new("interrupted-task").expect("Goal ID"),
        ScopeRef::global(),
        "中断任务",
        GoalPlan {
            objective: "验证中断隔离".to_owned(),
            steps: Vec::new(),
        },
        vec!["产出结论".to_owned()],
        1,
    )
    .expect("Goal");
    task.transition(GoalStatus::Active, 2).expect("激活 Goal");
    store.create_goal(&task).expect("保存 Goal");
    let mut execution = ExecutionRecord::new(
        ExecutionId::new("interrupted-execution").expect("Execution ID"),
        task.goal_id.clone(),
        2,
    );
    execution.session_id = Some(session_id.clone());
    store.enqueue_execution(&execution).expect("入队 Execution");
    store
        .claim_executions(1, "dead-worker", 3, 5)
        .expect("模拟中断前的领取");

    let executor = AnswerExecutor::default();
    let mut host = test_host(store.clone(), executor.clone());
    host.start().expect("启动 Host");
    let report = host.supervisor_tick(10).expect("Supervisor tick");

    assert_eq!(report.quarantined, 1);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .get_execution(&execution.execution_id)
            .expect("读取 Execution")
            .expect("Execution")
            .status,
        ExecutionStatus::Uncertain
    );
    assert_eq!(
        store
            .get_goal(&task.goal_id)
            .expect("读取 Goal")
            .expect("Goal")
            .status,
        GoalStatus::Waiting
    );
    let events = store.replay_events(&session_id, 0, 10).expect("重放事件");
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "turn_uncertain")
    );
}
