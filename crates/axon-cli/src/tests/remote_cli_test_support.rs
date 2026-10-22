use std::path::{Path, PathBuf};

use axon_remote_runtime::{
    remote_notification_wire_lifecycle_event,
    RemoteNotificationDelivery, RemoteNotificationDispatchStatus, RemoteNotificationFanoutPlan,
    RemoteNotificationKind,
    RemoteNotificationOutboxInput, RemoteNotificationOutboxRecord, RemoteNotificationOutboxRuntime,
    RemoteNotificationSeverity, WireLifecycleStatus, RemoteNotificationSubscription,
    RemoteSubscriptionStoreInput, RemoteSubscriptionStoreRecord, RemoteSubscriptionStoreRuntime,
    RemoteTransportKind,
};
use axon_domain::SessionId;

pub(crate) fn seed_subscription(root: &Path, subscriber_id: &str, revoked: bool) {
    let path = RemoteSubscriptionStoreRuntime::resolve_store_path(remote_runtime_root(root));
    let record = RemoteSubscriptionStoreRecord::from_input(RemoteSubscriptionStoreInput {
        subscription: RemoteNotificationSubscription {
            session_id: SessionId(7),
            subscriber_id: subscriber_id.to_owned(),
            kinds: vec![RemoteNotificationKind::TaskStarted],
            max_batch: 4,
        },
        connection_id: "secret-connection".to_owned(),
        pairing_id: "secret-pairing".to_owned(),
        transport: RemoteTransportKind::WebSocket,
        recorded_at_ms: 100,
        updated_at_ms: if revoked { 300 } else { 200 },
        expires_at_ms: Some(9_999_999_999_999),
        revoked,
        source_trace: "remote_subscription:secret-source".to_owned(),
        reason: "remote_subscription:persisted".to_owned(),
    })
    .expect("远控订阅记录应可构造");
    RemoteSubscriptionStoreRuntime::append_record(path, &record).expect("远控订阅应可写入");
}

pub(crate) fn seed_outbox(
    root: &Path,
    dispatch_id: &str,
    status: RemoteNotificationDispatchStatus,
) {
    let path = RemoteNotificationOutboxRuntime::resolve_store_path(remote_runtime_root(root));
    let record = RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
        dispatch_id: dispatch_id.to_owned(),
        plan: RemoteNotificationFanoutPlan {
            session_id: SessionId(7),
            subscriber_id: "secret-client".to_owned(),
            connection_id: "secret-connection".to_owned(),
            pairing_id: "secret-pairing".to_owned(),
            transport: RemoteTransportKind::Mobile,
            subscribed_kinds: vec![RemoteNotificationKind::InputRequired],
            delivered: vec![RemoteNotificationDelivery {
                kind: RemoteNotificationKind::TaskStarted,
                wire_event: remote_notification_wire_lifecycle_event(
                    SessionId(7),
                    dispatch_id,
                    WireLifecycleStatus::Started,
                    "远控任务开始",
                    None,
                    100,
                ),
                bounded_body: "通知正文 secret raw payload".to_owned(),
                reference_ids: vec!["thread:7".to_owned()],
                local_action_hint: "本机打开线程".to_owned(),
                severity: RemoteNotificationSeverity::Info,
            }],
            dropped_count: 0,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
        },
        status,
        recorded_at_ms: 100,
        updated_at_ms: 200,
        attempt_count: u32::from(status == RemoteNotificationDispatchStatus::Failed),
        next_retry_after_ms: None,
        last_error: None,
        adapter_message_id: None,
        source_trace: "remote_notification_outbox:secret-source".to_owned(),
        reason: "remote_notification_outbox:planned".to_owned(),
    })
    .expect("远控 outbox 记录应可构造");
    RemoteNotificationOutboxRuntime::append_record(path, &record).expect("远控 outbox 应可写入");
}

pub(crate) fn seed_feishu_outbox(root: &Path, dispatch_id: &str) {
    let path = RemoteNotificationOutboxRuntime::resolve_store_path(feishu_runtime_root(root));
    let record = RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
        dispatch_id: dispatch_id.to_owned(),
        plan: RemoteNotificationFanoutPlan {
            session_id: SessionId(9),
            subscriber_id: "secret-client".to_owned(),
            connection_id: "secret-connection".to_owned(),
            pairing_id: "secret-pairing".to_owned(),
            transport: RemoteTransportKind::Feishu,
            subscribed_kinds: vec![RemoteNotificationKind::InputRequired],
            delivered: vec![RemoteNotificationDelivery {
                kind: RemoteNotificationKind::InputRequired,
                wire_event: remote_notification_wire_lifecycle_event(
                    SessionId(9),
                    dispatch_id,
                    WireLifecycleStatus::Waiting,
                    "远控需要输入",
                    None,
                    100,
                ),
                bounded_body: "通知正文 secret raw payload".to_owned(),
                reference_ids: vec!["thread:9".to_owned()],
                local_action_hint: "本机打开线程".to_owned(),
                severity: RemoteNotificationSeverity::ActionRequired,
            }],
            dropped_count: 0,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
        },
        status: RemoteNotificationDispatchStatus::Pending,
        recorded_at_ms: 100,
        updated_at_ms: 200,
        attempt_count: 0,
        next_retry_after_ms: None,
        last_error: None,
        adapter_message_id: None,
        source_trace: "remote_notification_outbox:secret-source".to_owned(),
        reason: "remote_notification_outbox:planned".to_owned(),
    })
    .expect("Feishu 远控 outbox 记录应可构造");
    RemoteNotificationOutboxRuntime::append_record(path, &record)
        .expect("Feishu 远控 outbox 应可写入");
}


pub(crate) fn seed_feishu_websocket_config(path: &Path) {
    let mut config = axon_config::AxonConfig::default();
    config.remote.feishu_websocket.enabled = true;
    "cli_xxx".clone_into(&mut config.remote.feishu_websocket.app_id);
    "FEISHU_APP_SECRET".clone_into(&mut config.remote.feishu_websocket.app_secret_env);
    "https://open.feishu.cn".clone_into(&mut config.remote.feishu_websocket.api_base_url);
    config.remote.feishu_websocket.enable_notification_sending = false;
    config.remote.feishu_websocket.allowed_sender_open_ids = vec!["ou_simulated".to_owned()];
    config.remote.feishu_websocket.require_mention = true;
    axon_config::ConfigLoader::save_to_path(path, &config).expect("Feishu WebSocket 配置应可写入");
}

pub(crate) fn test_root(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-remote-cli-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

fn remote_runtime_root(root: &Path) -> PathBuf {
    root.join("data").join("remote")
}

fn feishu_runtime_root(root: &Path) -> PathBuf {
    remote_runtime_root(root).join("feishu")
}
