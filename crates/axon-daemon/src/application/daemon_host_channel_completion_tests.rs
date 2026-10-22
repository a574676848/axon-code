//! 非通知渠道来源的完成通知投递测试。
//!
//! 产品定义：一个 Execution 完成只通知主 turn 一次，汇总回复投递到当时所有可用渠道。
//! 主 turn 车道取自 Execution 自己的复合车道，与该渠道是否为通知目标无关：
//! HTTP/WS 等渠道没有收件箱消费者，但它们发起的后台任务同样必须产出汇总并投递到
//! TUI presence 与飞书，绝不能停在 `pending` 直到隔天静默失效。

use super::super::completion_delivery::COMPLETION_DISPATCH_BATCH_LIMIT;
use super::delivery_tests::{
    RecordingSteerSink, append_feishu_interaction, notification_of, register_tui_presence,
    register_workspace, seed_completion_request_for_transport, start_host, unread_results,
};
use axon_domain::{DaemonSessionId, ExecutionId, GoalId, RuntimeHostId};
use axon_session::{ClientDeliveryStore, NotificationDeliveryState, SqliteRuntimeStore};

/// 有收件箱消费者的渠道：TUI presence 轮询与飞书持久投递器。
const TUI_CHANNEL: &str = "tui";
const FEISHU_CHANNEL: &str = "feishu";
/// 没有收件箱消费者的外部渠道：可以提交事件，但当前没有出站投递器。
const HTTP_CHANNEL: &str = "http";
const WS_CHANNEL: &str = "ws";

#[test]
fn http_channel_completion_is_delivered_to_available_channels() {
    assert_non_fallback_channel_completion_delivered(
        HTTP_CHANNEL,
        "gateway-http-client",
        "http-lane",
    );
}

#[test]
fn websocket_channel_completion_is_delivered_to_available_channels() {
    assert_non_fallback_channel_completion_delivered(WS_CHANNEL, "gateway-ws-client", "ws-lane");
}

/// 断言非通知渠道发起的后台任务完成通知仍能形成汇总并投递到可用渠道。
fn assert_non_fallback_channel_completion_delivered(channel: &str, destination: &str, tag: &str) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new(format!("{tag}-session")).expect("Session ID");
    let goal_id = GoalId::new(format!("{tag}-task")).expect("Goal ID");
    let execution_id_text = format!("{tag}-execution");
    let action = seed_completion_request_for_transport(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        &execution_id_text,
        "外部渠道发起任务",
        serde_json::json!({
            "channel": channel,
            "destination": destination,
            "audience": {"kind": "private", "disclosure": "private_only"},
        }),
    );
    register_tui_presence(&store, &workspace);
    append_feishu_interaction(&store, &workspace, &format!("om_{tag}"));

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new(format!("{tag}-host")).expect("Host ID"),
        &steer_sink,
    );
    host.set_feishu_notification_chat_id(Some("oc_delivery_chat"))
        .expect("配置飞书通知目标");
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );

    let execution_id = ExecutionId::new(execution_id_text).expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    // 关键回归：来源渠道不在投递白名单时，通知仍必须派发到主 turn，而不是停在 pending。
    assert_eq!(
        notification.state,
        NotificationDeliveryState::Dispatched,
        "{channel} 来源的完成通知必须派发到主 turn"
    );
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert!(
        deliveries
            .iter()
            .all(|delivery| delivery.channel != channel),
        "没有收件箱消费者的渠道不得登记投递目标"
    );
    assert!(
        deliveries
            .iter()
            .any(|delivery| delivery.channel == TUI_CHANNEL),
        "在线 TUI 必须登记投递目标"
    );
    assert!(
        deliveries
            .iter()
            .any(|delivery| delivery.channel == FEISHU_CHANNEL),
        "可用外部渠道必须登记投递目标"
    );
    assert_eq!(unread_results(&store, &workspace), 1);

    // 主 turn 汇总回复产出后，可用渠道必须真正进入可投递状态。
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("继续运行主 turn");
    let notification = notification_of(&store, &execution_id);
    assert!(!notification.summary.is_empty(), "主 turn 必须回写汇总正文");
    let tui_inbox = host
        .dispatched_channel_inbox(TUI_CHANNEL, COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("读取 TUI 收件箱");
    assert_eq!(tui_inbox.len(), 1, "{channel} 来源的汇总必须投递到在线 TUI");
    assert_eq!(tui_inbox[0].content, notification.summary);
    let feishu_inbox = host
        .dispatched_channel_inbox(FEISHU_CHANNEL, COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("读取 Feishu 收件箱");
    assert_eq!(
        feishu_inbox.len(),
        1,
        "{channel} 来源的汇总必须投递到可用外部渠道"
    );
    assert_eq!(feishu_inbox[0].content, notification.summary);

    // 任一渠道 durable 确认后结算 Result；尚未确认的渠道不受影响。
    host.acknowledge_channel_delivery(&feishu_inbox[0])
        .expect("确认 Feishu delivery");
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Delivered
    );
    assert_eq!(unread_results(&store, &workspace), 0);
}

#[test]
fn non_fallback_channel_completion_without_available_channel_stays_pending() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("http-offline-session").expect("Session ID");
    let goal_id = GoalId::new("http-offline-task").expect("Goal ID");
    let execution_id_text = "http-offline-execution";
    let action = seed_completion_request_for_transport(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        execution_id_text,
        "外部渠道离线任务",
        serde_json::json!({
            "channel": HTTP_CHANNEL,
            "destination": "gateway-http-offline-client",
            "audience": {"kind": "private", "disclosure": "private_only"},
        }),
    );

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new("http-offline-host").expect("Host ID"),
        &steer_sink,
    );
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );

    // 没有任何可用渠道时通知保留在持久收件箱等渠道出现，Result 保持未读：
    // 该场景要真正兑现给 HTTP/WS 发起方，还需要该渠道自己的出站投递器。
    let execution_id = ExecutionId::new(execution_id_text).expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    assert_eq!(notification.state, NotificationDeliveryState::Pending);
    assert!(
        store
            .completion_deliveries(&notification.notification_id)
            .expect("读取渠道投递")
            .is_empty(),
        "没有可用渠道时不得登记投递目标"
    );
    assert_eq!(unread_results(&store, &workspace), 1);
}
