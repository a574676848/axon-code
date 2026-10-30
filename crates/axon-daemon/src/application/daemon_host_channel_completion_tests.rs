//! 非通知渠道来源的完成通知：主 turn 仍必须被派发。
//!
//! 关键回归点是「汇总必须产出」。HTTP/WS 这类渠道没有收件箱消费者，但它们是合法的
//! 会话表面：如果因为「该渠道不能投递」而不派发主 turn，汇总正文就永远不会产出，
//! 连带着声明在 `transportContext` 里的目标也收不到任何东西。

use super::super::completion_delivery::COMPLETION_DISPATCH_BATCH_LIMIT;
use super::delivery_tests::{
    RecordingSteerSink, notification_of, register_tui_presence, register_workspace,
    seed_completion_request_for_transport, start_host, unread_results,
};
use axon_domain::{DaemonSessionId, ExecutionId, GoalId, RuntimeHostId};
use axon_session::{ClientDeliveryStore, NotificationDeliveryState, SqliteRuntimeStore};

const HTTP_CHANNEL: &str = "http";
const WS_CHANNEL: &str = "ws";

#[test]
fn http_channel_completion_still_dispatches_primary_turn() {
    assert_declared_channel_completion_dispatched(HTTP_CHANNEL, "gateway-http-client", "http-lane");
}

#[test]
fn websocket_channel_completion_still_dispatches_primary_turn() {
    assert_declared_channel_completion_dispatched(WS_CHANNEL, "gateway-ws-client", "ws-lane");
}

fn assert_declared_channel_completion_dispatched(channel: &str, destination: &str, tag: &str) {
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
    // 在线 TUI 不再影响目标解析：它既不是来源车道，也不该被顺手登记。
    register_tui_presence(&store, &workspace);

    let steer_sink = RecordingSteerSink::default();
    let host = start_host(
        &store,
        RuntimeHostId::new(format!("{tag}-host")).expect("Host ID"),
        &steer_sink,
    );
    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );

    let execution_id = ExecutionId::new(execution_id_text).expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    assert_eq!(
        notification.state,
        NotificationDeliveryState::Dispatched,
        "{channel} 来源的完成通知必须派发到主 turn"
    );
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert_eq!(deliveries.len(), 1, "只登记来源车道这一个目标");
    assert_eq!(deliveries[0].channel, channel);
    assert_eq!(deliveries[0].destination, destination);
    assert_eq!(unread_results(&store, &workspace), 1);

    // 主 turn 汇总回复产出后，来源车道必须真正进入可投递状态。
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("继续运行主 turn");
    let summarized = notification_of(&store, &execution_id);
    assert!(!summarized.summary.is_empty(), "主 turn 必须回写汇总正文");
    let deliveries = store
        .completion_deliveries(&summarized.notification_id)
        .expect("读取渠道投递");
    assert_eq!(deliveries[0].state, NotificationDeliveryState::Dispatched);

    // 该渠道没有收件箱消费者，Result 必须保持未读而不是被静默确认。
    assert_eq!(unread_results(&store, &workspace), 1);
}
