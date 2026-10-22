use super::daemon_host_runner::{DaemonEventPublisher, DaemonMaintenanceSignal};
use axon_domain::{ConsumerCursor, DaemonSessionId};
use axon_session::SessionEventRecord;
use axon_session_runtime::EventPublisher;
use std::sync::Arc;

fn publisher(root: &std::path::Path, signal: Arc<DaemonMaintenanceSignal>) -> DaemonEventPublisher {
    DaemonEventPublisher::new(root.to_path_buf(), root.join("config.toml"), signal)
}

fn session_event(event_type: &str, payload_json: serde_json::Value) -> SessionEventRecord {
    SessionEventRecord {
        session_id: DaemonSessionId::new("feishu-session-42").expect("Session ID 有效"),
        sequence: 1,
        emitted_at_ms: 2,
        generation: "generation-a".to_owned(),
        event_type: event_type.to_owned(),
        payload_json: payload_json.to_string(),
    }
}

#[test]
fn event_without_feishu_work_does_not_notify_delivery_worker() {
    let root = axon_test_support::test_temp_dir();
    let signal = Arc::new(DaemonMaintenanceSignal::default());
    let publisher = publisher(root.path(), Arc::clone(&signal));
    let event = session_event(
        "wire_event",
        serde_json::json!({"interactionId": "interaction-tui"}),
    );

    publisher
        .publish(&event, &ConsumerCursor::new("generation-a", 1))
        .expect("普通事件发布应成功");
    assert!(!*signal.notified.lock().expect("delivery signal 可读取"));
}

#[test]
fn feishu_outbox_persistence_notifies_delivery_worker() {
    let root = axon_test_support::test_temp_dir();
    let signal = Arc::new(DaemonMaintenanceSignal::default());
    let publisher = publisher(root.path(), Arc::clone(&signal));
    let event = session_event(
        "turn_completed",
        serde_json::json!({
            "kind": "user_input",
            "response": "事件唤醒验证",
            "transportContext": {
                "kind": "feishu",
                "chatId": "oc_chat",
                "replyToMessageId": null,
                "threadId": null
            }
        }),
    );

    publisher
        .publish(&event, &ConsumerCursor::new("generation-a", 1))
        .expect("飞书事件应持久化");
    assert!(*signal.notified.lock().expect("delivery signal 可读取"));
}
