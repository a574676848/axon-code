use super::*;
use axon_domain::DaemonSessionId;

fn event(payload: Value) -> SessionEventRecord {
    SessionEventRecord {
        session_id: DaemonSessionId::new("feishu-session-42").expect("Session ID 有效"),
        sequence: 7,
        emitted_at_ms: 8,
        generation: "generation-a".to_owned(),
        event_type: TURN_COMPLETED_EVENT.to_owned(),
        payload_json: payload.to_string(),
    }
}

#[test]
fn persists_artifact_and_indexed_outbox_for_feishu_turn_completion() {
    let root = axon_test_support::test_temp_dir();
    let publisher =
        FeishuDaemonEventPublisher::new(root.path().to_path_buf(), root.path().join("config.toml"));
    publisher
        .publish(
            &event(serde_json::json!({
                "kind": "user_input",
                "response": "已完成",
                "transportContext": {
                    "kind": "feishu",
                    "chatId": "oc_chat",
                    "threadId": "omt_thread",
                    "replyToMessageId": "om_message"
                }
            })),
            &axon_domain::ConsumerCursor::new("generation-a", 7),
        )
        .expect("Feishu egress 应持久化");

    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root.path());
    let outbox = RemoteNotificationOutboxRuntime::pending_for_transport(
        RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root),
        SessionId(42),
        RemoteTransportKind::Feishu,
        u64::MAX,
    )
    .expect("outbox 应可按 session 索引读取");
    assert_eq!(outbox.records.len(), 1);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::latest_for_dispatch(
        RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&runtime_root),
        &outbox.records[0].dispatch_id,
    )
    .expect("artifact 查询应成功")
    .expect("artifact 应存在");
    assert_eq!(artifact.body, "已完成");
    assert_eq!(artifact.reply_to_message_id.as_deref(), Some("om_message"));
}

#[test]
fn persists_outbox_for_target_state_interaction_completion() {
    let root = axon_test_support::test_temp_dir();
    let publisher =
        FeishuDaemonEventPublisher::new(root.path().to_path_buf(), root.path().join("config.toml"));
    let interaction_event = SessionEventRecord {
        session_id: DaemonSessionId::new("interaction-7a8b9c").expect("Session ID 有效"),
        sequence: 1,
        emitted_at_ms: 100,
        generation: "generation-a".to_owned(),
        event_type: TURN_COMPLETED_EVENT.to_owned(),
        payload_json: serde_json::json!({
            "kind": "user_input",
            "response": "这是目标态回答",
            "transportContext": {
                "channel": "feishu",
                "destination": "oc_chat_target",
                "sourceMessageId": "oc_chat_target:om_msg_target"
            }
        })
        .to_string(),
    };
    publisher
        .publish(
            &interaction_event,
            &axon_domain::ConsumerCursor::new("generation-a", 1),
        )
        .expect("目标态 Feishu egress 应持久化");

    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root.path());
    let expected_session_id = numeric_session_id("interaction-7a8b9c");
    let outbox = RemoteNotificationOutboxRuntime::pending_for_transport(
        RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root),
        expected_session_id,
        RemoteTransportKind::Feishu,
        u64::MAX,
    )
    .expect("outbox 应可读取");
    assert_eq!(outbox.records.len(), 1);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::latest_for_dispatch(
        RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&runtime_root),
        &outbox.records[0].dispatch_id,
    )
    .expect("artifact 查询应成功")
    .expect("artifact 应存在");
    assert_eq!(artifact.body, "这是目标态回答");
    assert_eq!(
        artifact.reply_to_message_id.as_deref(),
        Some("om_msg_target")
    );
}

#[test]
fn ignores_non_feishu_and_non_completion_events() {
    let root = axon_test_support::test_temp_dir();
    let publisher =
        FeishuDaemonEventPublisher::new(root.path().to_path_buf(), root.path().join("config.toml"));
    let mut non_completion = event(serde_json::json!({"response":"忽略"}));
    non_completion.event_type = "turn_started".to_owned();
    publisher
        .publish(
            &non_completion,
            &axon_domain::ConsumerCursor::new("generation-a", 7),
        )
        .expect("非完成事件不应失败");
    publisher
        .publish(
            &event(serde_json::json!({
                "kind": "user_input",
                "response": "忽略",
                "transportContext": {"kind": "cli", "chatId": "oc_chat"}
            })),
            &axon_domain::ConsumerCursor::new("generation-a", 7),
        )
        .expect("非 Feishu 事件不应失败");
    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root.path());
    assert!(!RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root).exists());
}

#[test]
fn background_execution_reply_never_reaches_feishu_outbox() {
    let root = axon_test_support::test_temp_dir();
    let publisher =
        FeishuDaemonEventPublisher::new(root.path().to_path_buf(), root.path().join("config.toml"));
    // 后台 Execution 的原始回复必须被拦截：飞书完成消息统一由主 turn 汇总回复负责。
    publisher
        .publish(
            &event(serde_json::json!({
                "actionId": "action-background",
                "kind": "turn",
                "response": "后台原始回复不得直发飞书",
                "transportContext": {
                    "kind": "feishu",
                    "chatId": "oc_chat"
                }
            })),
            &axon_domain::ConsumerCursor::new("generation-a", 7),
        )
        .expect("后台 Execution 终态不应失败");
    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root.path());
    assert!(!RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root).exists());
}

#[test]
fn fully_projected_completion_with_empty_response_does_not_create_duplicate_outbox() {
    let root = axon_test_support::test_temp_dir();
    let publisher =
        FeishuDaemonEventPublisher::new(root.path().to_path_buf(), root.path().join("config.toml"));
    publisher
        .publish(
            &event(serde_json::json!({
                "actionId": "action-1",
                "kind": "user_input",
                "response": "",
                "transportContext": {
                    "kind": "feishu",
                    "chatId": "oc_chat"
                }
            })),
            &axon_domain::ConsumerCursor::new("generation-a", 7),
        )
        .expect("已完整投影的终态不应失败");
    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root.path());
    assert!(!RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root).exists());
}

#[test]
fn persists_feishu_failure_reply_for_turn_failed_event() {
    let root = axon_test_support::test_temp_dir();
    let publisher =
        FeishuDaemonEventPublisher::new(root.path().to_path_buf(), root.path().join("config.toml"));
    let failed_event = SessionEventRecord {
        session_id: DaemonSessionId::new("feishu-session-43").expect("Session ID 有效"),
        sequence: 9,
        emitted_at_ms: 10,
        generation: "generation-a".to_owned(),
        event_type: "turn_failed".to_owned(),
        payload_json: serde_json::json!({
            "actionId": "action-failed",
            "kind": axon_daemon::PRIMARY_TURN_ACTION_KIND,
            "response": "本次处理失败：供应商请求不可用",
            "transportContext": {
                "kind": "feishu",
                "chatId": "oc_chat",
                "replyToMessageId": "om_failed"
            }
        })
        .to_string(),
    };

    publisher
        .publish(
            &failed_event,
            &axon_domain::ConsumerCursor::new("generation-a", 9),
        )
        .expect("Feishu 失败回执应持久化");

    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root.path());
    let outbox = RemoteNotificationOutboxRuntime::pending_for_transport(
        RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root),
        SessionId(43),
        RemoteTransportKind::Feishu,
        u64::MAX,
    )
    .expect("失败回执 outbox 应可读取");
    assert_eq!(outbox.records.len(), 1);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::latest_for_dispatch(
        RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&runtime_root),
        &outbox.records[0].dispatch_id,
    )
    .expect("失败回执 artifact 查询应成功")
    .expect("失败回执 artifact 应存在");
    assert_eq!(artifact.body, "本次处理失败：供应商请求不可用");
    assert_eq!(artifact.reply_to_message_id.as_deref(), Some("om_failed"));
}
