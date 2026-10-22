//! 普通用户回合与完成通知隔离测试。

use super::super::completion_delivery::{
    COMPLETION_DISPATCH_BATCH_LIMIT, resolve_completion_targets,
};
use super::delivery_tests::{
    RecordingSteerSink, append_feishu_interaction, notification_of, register_tui_presence,
    register_workspace, seed_completion_request, start_host, unread_results,
};
use axon_domain::{DaemonSessionId, ExecutionId, GoalId, RuntimeHostId};
use axon_session::{ClientDeliveryStore, NotificationDeliveryState, SqliteRuntimeStore};

const EXPIRED_NOW_MS: u64 = 10;
/// 远未来时间戳：用于证明渠道可用，且不越过 SQLite INTEGER 上限。
const FAR_FUTURE_NOW_MS: u64 = i64::MAX as u64;

#[test]
fn user_turn_without_notification_never_acknowledges_results() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("user-turn-session").expect("Session ID");
    let goal_id = GoalId::new("user-turn-task").expect("Goal ID");
    let action = seed_completion_request(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-user-turn",
        "用户回合任务",
    );
    register_tui_presence(&store, &workspace);

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("user-turn-host").expect("Host ID"),
        &steer_sink,
    );
    host.set_feishu_notification_chat_id(Some("oc_delivery_chat"))
        .expect("配置飞书通知目标");
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );
    // 通知已投递给在线 TUI，并等待客户端确认。
    let execution_id = ExecutionId::new("execution-user-turn").expect("Execution ID");
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Dispatched
    );
    assert_eq!(unread_results(&store, &workspace), 1);

    // 普通用户输入的主 turn 与任何完成通知无关：不得确认未读 Result。
    append_feishu_interaction(&store, &workspace, "om_user_turn");
    let tick = host
        .primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    assert!(tick.understood >= 1);
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Dispatched
    );
    assert_eq!(unread_results(&store, &workspace), 1);
}

#[test]
fn unread_completion_expires_without_delivery_and_marks_result_expired() {
    let directory = axon_test_support::test_temp_dir();
    let database_path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(database_path.clone());
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("expiry-session").expect("Session ID");
    let goal_id = GoalId::new("expiry-task").expect("Goal ID");
    let action = seed_completion_request(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-expiry",
        "隔天失效任务",
    );

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("expiry-host").expect("Host ID"),
        &steer_sink,
    );
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );
    let execution_id = ExecutionId::new("execution-expiry").expect("Execution ID");
    let notification = notification_of(&store, &execution_id);

    let connection = rusqlite::Connection::open(database_path).expect("打开 runtime 数据库");
    connection
        .execute(
            "UPDATE jarvis_completion_notifications SET expires_at_ms=?1 WHERE notification_id=?2",
            rusqlite::params![1_i64, notification.notification_id],
        )
        .expect("设置过期时间");

    assert_eq!(
        host.expire_stale_completions(EXPIRED_NOW_MS, COMPLETION_DISPATCH_BATCH_LIMIT)
            .expect("失效扫描"),
        1
    );
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Expired
    );
    assert_eq!(unread_results(&store, &workspace), 0);
    let acknowledged_source: String = connection
        .query_row(
            "SELECT acknowledged_source FROM jarvis_result_reads WHERE result_id=?1",
            [notification.result_ids[0].as_str()],
            |row| row.get(0),
        )
        .expect("读取 Result 过期来源");
    assert_eq!(acknowledged_source, axon_session::RESULT_ACK_SOURCE_EXPIRED);
    assert!(
        host.dispatched_channel_inbox("tui", COMPLETION_DISPATCH_BATCH_LIMIT)
            .expect("读取 TUI 收件箱")
            .is_empty(),
        "过期完成消息不得继续投递"
    );
}

#[test]
fn delivered_notification_expires_only_remaining_channel_deliveries() {
    let directory = axon_test_support::test_temp_dir();
    let database_path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(database_path.clone());
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("partial-expiry-session").expect("Session ID");
    let goal_id = GoalId::new("partial-expiry-task").expect("Goal ID");
    let action = seed_completion_request(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-partial-expiry",
        "部分渠道失效任务",
    );
    append_feishu_interaction(&store, &workspace, "om_partial_expiry");
    register_tui_presence(&store, &workspace);

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("partial-expiry-host").expect("Host ID"),
        &steer_sink,
    );
    host.set_feishu_notification_chat_id(Some("oc_delivery_chat"))
        .expect("配置飞书通知目标");
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("继续运行主 turn");
    let execution_id = ExecutionId::new("execution-partial-expiry").expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    let tui_entry = host
        .dispatched_channel_inbox("tui", COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("读取 TUI 收件箱")
        .into_iter()
        .next()
        .expect("TUI delivery");
    host.acknowledge_channel_delivery(&tui_entry)
        .expect("确认 TUI delivery");
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Delivered
    );

    let connection = rusqlite::Connection::open(database_path).expect("打开 runtime 数据库");
    connection
        .execute(
            "UPDATE jarvis_completion_notifications SET expires_at_ms=?1 WHERE notification_id=?2",
            rusqlite::params![1_i64, notification.notification_id],
        )
        .expect("设置过期时间");
    assert_eq!(
        host.expire_stale_completions(EXPIRED_NOW_MS, COMPLETION_DISPATCH_BATCH_LIMIT)
            .expect("失效扫描"),
        1
    );
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Delivered
    );
    assert_eq!(unread_results(&store, &workspace), 0);
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert!(deliveries.iter().any(|delivery| {
        delivery.channel == "tui" && delivery.state == NotificationDeliveryState::Delivered
    }));
    assert!(deliveries.iter().any(|delivery| {
        delivery.channel == "feishu" && delivery.state == NotificationDeliveryState::Expired
    }));
    assert!(
        host.dispatched_channel_inbox("feishu", COMPLETION_DISPATCH_BATCH_LIMIT)
            .expect("读取 Feishu 收件箱")
            .is_empty()
    );
}

#[test]
fn summarized_completion_with_pending_delivery_is_recovered_by_sweep() {
    let directory = axon_test_support::test_temp_dir();
    let database_path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(database_path.clone());
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("summary-recovery-session").expect("Session ID");
    let goal_id = GoalId::new("summary-recovery-task").expect("Goal ID");
    let action = seed_completion_request(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-summary-recovery",
        "汇总崩溃补偿任务",
    );
    register_tui_presence(&store, &workspace);

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("summary-recovery-host").expect("Host ID"),
        &steer_sink,
    );
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("继续运行主 turn");

    let execution_id = ExecutionId::new("execution-summary-recovery").expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    assert!(!notification.summary.is_empty(), "主 turn 必须回写汇总正文");

    // 模拟「汇总已落库、渠道推进中途崩溃」：把渠道投递退回 pending。
    let connection = rusqlite::Connection::open(database_path).expect("打开 runtime 数据库");
    connection
        .execute(
            "UPDATE jarvis_completion_deliveries SET state='pending' WHERE notification_id=?1",
            rusqlite::params![notification.notification_id],
        )
        .expect("重置渠道投递状态");
    assert!(
        host.dispatched_channel_inbox("tui", COMPLETION_DISPATCH_BATCH_LIMIT)
            .expect("读取 TUI 收件箱")
            .is_empty(),
        "pending 渠道不得出现在收件箱"
    );

    assert_eq!(
        host.dispatch_pending_completions(COMPLETION_DISPATCH_BATCH_LIMIT)
            .expect("投递扫描"),
        1,
        "sweep 必须补偿推进停在 pending 的渠道"
    );
    let inbox = host
        .dispatched_channel_inbox("tui", COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("读取 TUI 收件箱");
    assert_eq!(inbox.len(), 1, "补偿后渠道必须可投递");
    assert_eq!(inbox[0].content, notification.summary);
}

#[test]
fn settled_notification_still_delivers_to_newly_available_channel() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("settled-channel-session").expect("Session ID");
    let goal_id = GoalId::new("settled-channel-task").expect("Goal ID");
    let action = seed_completion_request(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-settled-channel",
        "结算后渠道任务",
    );
    register_tui_presence(&store, &workspace);

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("settled-channel-host").expect("Host ID"),
        &steer_sink,
    );
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("继续运行主 turn");

    let execution_id = ExecutionId::new("execution-settled-channel").expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    let tui_entry = host
        .dispatched_channel_inbox("tui", COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("读取 TUI 收件箱")
        .into_iter()
        .next()
        .expect("TUI delivery");
    host.acknowledge_channel_delivery(&tui_entry)
        .expect("确认 TUI delivery");
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Delivered
    );

    // 结算后飞书才成为可用渠道：必须补登记并在同一轮推进到可投递，不能停在 pending。
    append_feishu_interaction(&store, &workspace, "om_settled_channel");
    host.set_feishu_notification_chat_id(Some("oc_delivery_chat"))
        .expect("配置飞书通知目标");
    let targets = resolve_completion_targets(
        &store,
        &workspace.workspace_id,
        FAR_FUTURE_NOW_MS,
        Some("oc_delivery_chat"),
    )
    .expect("解析投递目标");
    assert!(
        targets.iter().any(|target| target.channel == "feishu"),
        "飞书必须已是可用渠道"
    );

    assert_eq!(
        host.dispatch_pending_completions(COMPLETION_DISPATCH_BATCH_LIMIT)
            .expect("投递扫描"),
        1,
        "结算后补登记的渠道必须被推进"
    );
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert!(deliveries.iter().any(|delivery| {
        delivery.channel == "feishu" && delivery.state == NotificationDeliveryState::Dispatched
    }));
    let feishu_inbox = host
        .dispatched_channel_inbox("feishu", COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("读取 Feishu 收件箱");
    assert_eq!(feishu_inbox.len(), 1, "补登记渠道必须真正可投递");
    assert_eq!(feishu_inbox[0].content, notification.summary);

    // 补登记渠道的确认是幂等的：已结算的通知与 Result 不因第二次确认而改变。
    host.acknowledge_channel_delivery(&feishu_inbox[0])
        .expect("确认 Feishu delivery");
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Delivered
    );
    assert_eq!(unread_results(&store, &workspace), 0);
}
