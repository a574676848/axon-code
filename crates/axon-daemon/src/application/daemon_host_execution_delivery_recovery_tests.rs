//! 投递的离线与恢复路径：离线补投、显式飞书目标、崩溃后由监督扫描接管并最终确认结果。
use super::super::super::completion_delivery::{
    COMPLETION_DISPATCH_BATCH_LIMIT, completion_notification_id,
};
use super::*;
use axon_domain::{GoalId, RuntimeHostId};
use axon_session::{ClientDeliveryStore, GoalStore, NotificationDeliveryState, SqliteRuntimeStore};

fn new_goal(goal_id: &GoalId, scope: &ScopeRef, title: &str) -> GoalRecord {
    GoalRecord::new(
        goal_id.clone(),
        scope.clone(),
        title,
        GoalPlan {
            objective: format!("验证{title}"),
            steps: vec!["步骤 1".to_owned()],
        },
        vec!["验收标准".to_owned()],
        TEST_NOW_MS,
    )
    .expect("Goal")
}

#[test]
fn declared_feishu_transport_context_delivers_without_global_config() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("feishu-fallback-session").expect("Session ID");
    let goal_id = GoalId::new("feishu-fallback-task").expect("Goal ID");
    let action = seed_completion_request_for_transport(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-feishu-fallback",
        "飞书回落任务",
        private_transport_context(FEISHU_CHANNEL, FEISHU_DESTINATION),
    );

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("feishu-fallback-host").expect("Host ID"),
        &steer_sink,
    );

    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );

    let steered = steer_sink.steered();
    assert_eq!(steered.len(), 1);
    assert_eq!(steered[0].reply_route.channel(), FEISHU_CHANNEL);
    assert_eq!(steered[0].reply_route.destination(), FEISHU_DESTINATION);

    let execution_id = ExecutionId::new("execution-feishu-fallback").expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    assert_eq!(notification.state, NotificationDeliveryState::Dispatched);
    // 飞书由主 turn 汇总回复负责确认，派发本身不得确认 Result。
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].channel, FEISHU_CHANNEL);
    assert_eq!(deliveries[0].destination, FEISHU_DESTINATION);
    assert_eq!(deliveries[0].state, NotificationDeliveryState::Pending);
    assert_eq!(unread_results(&store, &workspace), 1);
}

#[test]
fn crashed_completion_is_recovered_by_supervisor_sweep() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("recovery-session").expect("Session ID");
    let goal_id = GoalId::new("recovery-goal").expect("Goal ID");
    let execution_id = ExecutionId::new("execution-crashed").expect("Execution ID");
    store
        .create_session(&session_id, &workspace.workspace_root, TEST_NOW_MS)
        .expect("创建 Session");
    let goal = new_goal(&goal_id, &scope_of(&workspace), "崩溃补偿 Goal");
    store.create_goal(&goal).expect("保存 Goal");
    store
        .enqueue_execution(
            &ExecutionRecord::new(execution_id.clone(), goal_id.clone(), TEST_NOW_MS)
                .with_session_id(session_id.clone())
                .with_payload_json(
                    serde_json::json!({
                        "transportContext": private_transport_context(
                            TUI_CHANNEL,
                            TUI_DESTINATION,
                        ),
                    })
                    .to_string(),
                ),
        )
        .expect("写入 Execution");
    let claimed = store
        .claim_executions(1, "recovery-owner", TEST_NOW_MS, TEST_LEASE_DURATION_MS)
        .expect("领取 Execution");
    assert_eq!(claimed.len(), 1);
    let lease = claimed[0].lease.clone().expect("Execution lease");
    let result = ResultRecord::success(
        ResultId::new("result-crashed").expect("Result ID"),
        goal_id.clone(),
        execution_id.clone(),
        "崩溃前的执行结论",
        TEST_NOW_MS,
    );
    let mut terminal_execution = claimed[0].clone();
    terminal_execution.status = ExecutionStatus::Completed;
    terminal_execution.updated_at_ms = TEST_NOW_MS;
    let mut updated_goal = goal.clone();
    updated_goal.status = GoalStatus::Completed;
    updated_goal.revision = goal.revision.saturating_add(1);
    updated_goal.updated_at_ms = TEST_NOW_MS;
    store
        .commit_execution(
            &terminal_execution,
            &lease.owner,
            &lease.token,
            &result,
            &updated_goal,
            goal.revision,
            &JarvisEvent {
                event_id: JarvisEventId::new("event-recovery-completed").expect("Event ID"),
                sequence: 0,
                event_type: "execution.completed".to_owned(),
                idempotency_key: "execution.completed:execution-crashed".to_owned(),
                cause_event_id: None,
                payload_json: serde_json::to_string(&result).expect("Result JSON"),
                occurred_at_ms: TEST_NOW_MS,
            },
        )
        .expect("提交完成结论");

    // 模拟 commit 之后、通知落库之前崩溃：结论文档存在，但收件箱为空。
    assert!(
        store
            .completion_notification(&completion_notification_id(&execution_id))
            .expect("读取通知")
            .is_none()
    );

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("recovery-host").expect("Host ID"),
        &steer_sink,
    );
    register_tui_presence(&store, &workspace);

    let recovered = host
        .recover_missing_completions(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("补偿扫描");
    assert_eq!(recovered, 1, "补偿扫描必须重新发现未通知的结论");

    let notification = notification_of(&store, &execution_id);
    assert_eq!(notification.result_ids, vec!["result-crashed".to_owned()]);
    assert_eq!(notification.state, NotificationDeliveryState::Dispatched);
    assert!(notification.content.contains("崩溃补偿 Goal"));
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].channel, TUI_CHANNEL);
    assert_eq!(deliveries[0].destination, TUI_DESTINATION);
    assert_eq!(deliveries[0].state, NotificationDeliveryState::Pending);
    assert_eq!(steer_sink.steered().len(), 1);
    assert_eq!(unread_results(&store, &workspace), 1);
}

#[test]
fn feishu_summary_turn_settles_delivery_and_acknowledges_results() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("feishu-settlement-session").expect("Session ID");
    let goal_id = GoalId::new("feishu-settlement-task").expect("Goal ID");
    let action = seed_completion_request_for_transport(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-feishu-settlement",
        "飞书结算任务",
        private_transport_context(FEISHU_CHANNEL, FEISHU_DESTINATION),
    );

    // 无活跃主 turn：通知先进入持久收件箱并按飞书渠道派发。
    let mut host = DaemonHost::new(
        store.clone(),
        AnswerExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        RuntimeHostId::new("feishu-settlement-host").expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    );
    host.start().expect("启动 Host");
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );

    let execution_id = ExecutionId::new("execution-feishu-settlement").expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    assert_eq!(notification.state, NotificationDeliveryState::Dispatched);
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].channel, FEISHU_CHANNEL);
    assert_eq!(deliveries[0].destination, FEISHU_DESTINATION);
    assert_eq!(deliveries[0].state, NotificationDeliveryState::Pending);
    assert_eq!(unread_results(&store, &workspace), 1);

    // 主 turn 消费该完成通知：汇总回复发布后结算通知并精确确认 Result。
    let tick = host
        .primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    assert!(tick.understood >= 1, "完成通知未被主 turn 消费");
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Dispatched
    );
    assert_eq!(unread_results(&store, &workspace), 1);
    let inbox = host
        .dispatched_channel_inbox(FEISHU_CHANNEL, COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("读取飞书收件箱");
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].content, "主 turn 汇总回复");
    host.acknowledge_channel_delivery(&inbox[0])
        .expect("确认飞书 delivery");
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Delivered
    );
    assert_eq!(unread_results(&store, &workspace), 0);
}
