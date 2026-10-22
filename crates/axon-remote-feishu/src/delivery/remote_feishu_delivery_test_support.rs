//! Feishu delivery 测试共享 fixture。

use crate::{
    RemoteFeishuWebSocketDispatchPlan, RemoteNotificationAdapterKind,
    RemoteNotificationDispatchStatus, RemoteNotificationOutboxRecord,
    RemoteNotificationRenderArtifact, RemoteTransportKind,
};

pub(crate) fn accepted_record() -> RemoteNotificationOutboxRecord {
    RemoteNotificationOutboxRecord {
        schema_version: axon_agent_runtime::REMOTE_NOTIFICATION_OUTBOX_SCHEMA_VERSION,
        dispatch_id: "dispatch-1".to_owned(),
        session_id: 11,
        subscriber_id: "subscriber".to_owned(),
        connection_id: "oc_chat_1".to_owned(),
        pairing_id: "pairing-1".to_owned(),
        transport: RemoteTransportKind::Feishu,
        status: RemoteNotificationDispatchStatus::Attempting,
        delivery_count: 1,
        dropped_count: 0,
        delivery_kinds: vec![],
        recorded_at_ms: 100,
        updated_at_ms: 200,
        attempt_count: 0,
        next_retry_after_ms: None,
        last_error: None,
        failure: None,
        adapter_message_id: None,
        attempt_started_at_ms: Some(200),
        attempt_owner: Some("pid:test:start:1".to_owned()),
        recovery_marker: None,
        source_trace: "remote_notification_outbox:test".to_owned(),
        reason: "remote_notification_adapter_dispatch:accepted".to_owned(),
        sequence: None,
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        raw_body_persisted: false,
        raw_payload_persisted: false,
    }
}

pub(crate) fn dispatch_plan() -> RemoteFeishuWebSocketDispatchPlan {
    RemoteFeishuWebSocketDispatchPlan {
        adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
        app_id: "cli_xxx".to_owned(),
        app_secret_env: "FEISHU_APP_SECRET".to_owned(),
        websocket_url: "wss://open.feishu.cn/open-apis/ws/v2".to_owned(),
        artifact: RemoteNotificationRenderArtifact {
            adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
            transport: RemoteTransportKind::Feishu,
            content_type: "application/json".to_owned(),
            payload: "{\"msg\":\"card\"}".to_owned(),
            network_send_allowed: false,
        },
        notification_sending_enabled: true,
        reasons: vec![],
    }
}
