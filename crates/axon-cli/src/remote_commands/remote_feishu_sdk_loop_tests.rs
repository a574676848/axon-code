use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_domain::AxonError;
use axon_jsonl::ShardedJsonlStore;
use axon_remote_feishu::RemoteFeishuOapiWsIncomingPayload;

use super::{
    RealSdkPayloadAppendInput, append_real_sdk_payload, append_real_sdk_payload_with_http,
};

const TEST_APP_ID: &str = "cli_bot_app";

fn temp_root(name: &str) -> PathBuf {
    let mut root = axon_test_support::test_temp_root();
    root.push(format!(
        "axon-feishu-sdk-loop-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("测试目录应可创建");
    root
}

fn incoming_payload(value: &serde_json::Value) -> RemoteFeishuOapiWsIncomingPayload {
    RemoteFeishuOapiWsIncomingPayload {
        kind: "event".to_owned(),
        message_id: String::new(),
        trace_id: String::new(),
        payload: serde_json::to_vec(value).expect("payload 应可序列化"),
    }
}

fn read_event_source(path: &Path) -> String {
    let lines = ShardedJsonlStore::new()
        .read_lines(path)
        .expect("事件源应可读取");
    lines
        .into_iter()
        .map(|line| String::from_utf8(line).expect("事件源 UTF-8"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn event_source_exists(path: &Path) -> bool {
    !ShardedJsonlStore::new()
        .shard_paths(path)
        .expect("检查事件源 shard")
        .is_empty()
}

fn append_input(
    session_id: u64,
    event_source_file: PathBuf,
    _root: &Path,
) -> RealSdkPayloadAppendInput {
    RealSdkPayloadAppendInput {
        app_id: TEST_APP_ID.to_owned(),
        session_id,
        event_source_file,
        text_context: None,
        tenant_access_token: "tenant-token".to_owned(),
        api_base_url: Some("http://127.0.0.1".to_owned()),
    }
}

#[test]
fn append_real_sdk_payload_fast_acks_message_without_network_side_effects() {
    let root = temp_root("message-fast-ack");
    let event_source = root.join("events.jsonl");
    let payload = incoming_payload(&serde_json::json!({
        "header": {"event_type": "im.message.receive_v1"},
        "event": {
            "event_id": "evt_message_ack",
            "sender": {"sender_id": {"open_id": "ou_sender"}},
            "message": {
                "message_id": "om_received_ack",
                "chat_id": "oc_received_ack",
                "message_type": "text",
                "content": "{\"text\":\"你好\"}"
            }
        }
    }));

    let report = append_real_sdk_payload_with_http(
        append_input(18, event_source.clone(), &root),
        &payload,
        |_| panic!("消息 WS ack 前不应调用 Feishu OpenAPI"),
    )
    .expect("真实消息应快速写入事件源");

    assert_eq!(report.input_payloads, 1);
    assert_eq!(report.mapped_events, 1);
    assert_eq!(report.appended_events, 1);
    assert_eq!(report.received_acked_events, 1);
    assert_eq!(report.received_ack_failed_events, 0);
    let content = read_event_source(&event_source);
    assert!(content.contains("om_received_ack"));
    assert!(content.contains("ou_sender"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn append_real_sdk_payload_acks_card_action_callback() {
    let root = temp_root("card-action-ack");
    let event_source = root.join("events.jsonl");
    let payload = incoming_payload(&serde_json::json!({
        "header": {"event_type": "card.action.trigger"},
        "event": {
            "token": "token_card_ack",
            "context": {"open_chat_id": "oc_card_ack"},
            "operator": {"open_id": "ou_operator"},
            "action": {
                "tag": "button",
                "value": {"action":"clarify","label":"补充说明"}
            }
        }
    }));

    let report = append_real_sdk_payload_with_http(
        append_input(18, event_source.clone(), &root),
        &payload,
        |_| panic!("卡片动作 ack 前不应调用 Feishu OpenAPI"),
    )
    .expect("卡片动作应快速写入事件源");

    assert_eq!(report.input_payloads, 1);
    assert_eq!(report.mapped_events, 1);
    assert_eq!(report.appended_events, 1);
    assert_eq!(report.received_acked_events, 1);
    assert_eq!(report.received_ack_failed_events, 0);
    let content = read_event_source(&event_source);
    assert!(content.contains("cardAction"));
    assert!(content.contains("token_card_ack"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn append_real_sdk_payload_routes_reaction_on_bot_owned_message() {
    let root = temp_root("reaction-bot-owned");
    let event_source = root.join("events.jsonl");
    let payload = incoming_payload(&serde_json::json!({
        "header": {"event_type": "im.message.reaction.created_v1"},
        "event": {
            "event_id": "evt_reaction_bot",
            "message_id": "om_reacted",
            "operator": {"open_id": "ou_user"},
            "reaction_type": {"emoji_type": "keyboard"}
        }
    }));

    let report = append_real_sdk_payload_with_http(
        append_input(18, event_source.clone(), &root),
        &payload,
        |_| panic!("reaction append 不应在 SDK callback 阶段查询消息"),
    )
    .expect("本 bot 消息上的 reaction 应进入事件源");

    assert_eq!(report.input_payloads, 1);
    assert_eq!(report.mapped_events, 1);
    assert_eq!(report.appended_events, 1);
    let content = read_event_source(&event_source);
    assert!(content.contains("reactionPending"));
    assert!(content.contains("rawPayloadJson"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn append_real_sdk_payload_routes_reaction_with_user_id_sender() {
    let root = temp_root("reaction-user-id");
    let event_source = root.join("events.jsonl");
    let payload = incoming_payload(&serde_json::json!({
        "header": {"event_type": "im.message.reaction.created_v1"},
        "event": {
            "event_id": "evt_reaction_user_id",
            "message_id": "om_reacted",
            "user_id": {"open_id": "ou_reactor"},
            "reaction_type": {"emoji_type": "keyboard"}
        }
    }));

    let report = append_real_sdk_payload_with_http(
        append_input(18, event_source.clone(), &root),
        &payload,
        |_| panic!("reaction append 不应在 SDK callback 阶段查询消息"),
    )
    .expect("user_id 形式的 reaction 应进入事件源");

    assert_eq!(report.input_payloads, 1);
    assert_eq!(report.mapped_events, 1);
    assert_eq!(report.appended_events, 1);
    let content = read_event_source(&event_source);
    assert!(content.contains("ou_reactor"));
    assert!(content.contains("reactionPending"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn append_real_sdk_payload_ignores_reaction_on_user_message() {
    let root = temp_root("reaction-user-owned");
    let event_source = root.join("events.jsonl");
    let payload = incoming_payload(&serde_json::json!({
        "header": {"event_type": "im.message.reaction.created_v1"},
        "event": {
            "event_id": "evt_reaction_user",
            "message_id": "om_user_message",
            "chat_id": "oc_reaction",
            "operator": {"open_id": "ou_user"},
            "reaction_type": {"emoji_type": "keyboard"}
        }
    }));

    let report = append_real_sdk_payload_with_http(
        append_input(18, event_source.clone(), &root),
        &payload,
        |_| panic!("reaction append 不应在 SDK callback 阶段查询消息"),
    )
    .expect("非本 bot 消息上的 reaction 应被忽略");

    assert_eq!(report.input_payloads, 1);
    assert_eq!(report.mapped_events, 1);
    assert_eq!(report.appended_events, 1);
    assert!(event_source_exists(&event_source));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn append_real_sdk_payload_reaction_persists_before_http_failure() {
    let root = temp_root("reaction-http-failure");
    let event_source = root.join("events.jsonl");
    let payload = incoming_payload(&serde_json::json!({
        "header": {"event_type": "im.message.reaction.created_v1"},
        "event": {
            "event_id": "evt_reaction_http_failure",
            "message_id": "om_reacted",
            "operator": {"open_id": "ou_user"},
            "reaction_type": {"emoji_type": "keyboard"}
        }
    }));

    let report = append_real_sdk_payload_with_http(
        append_input(18, event_source.clone(), &root),
        &payload,
        |_| Err(AxonError::runtime_failure("模拟网络故障")),
    )
    .expect("SDK callback 不应等待 HTTP");

    assert_eq!(report.appended_events, 1);
    assert!(read_event_source(&event_source).contains("rawPayloadJson"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn append_real_sdk_payload_ignores_app_origin_reaction_without_lookup() {
    let root = temp_root("reaction-app-origin");
    let event_source = root.join("events.jsonl");
    let payload = incoming_payload(&serde_json::json!({
        "header": {"event_type": "im.message.reaction.created_v1"},
        "event": {
            "event_id": "evt_reaction_app",
            "message_id": "om_reacted",
            "chat_id": "oc_reaction",
            "operator_type": "app",
            "operator": {"open_id": "ou_bot"},
            "reaction_type": {"emoji_type": "keyboard"}
        }
    }));

    let report = append_real_sdk_payload_with_http(
        append_input(18, event_source.clone(), &root),
        &payload,
        |_| panic!("app/bot 来源 reaction 不应查询目标消息"),
    )
    .expect("app 来源 reaction 应被忽略");

    assert_eq!(report.input_payloads, 1);
    assert_eq!(report.mapped_events, 0);
    assert_eq!(report.appended_events, 0);
    assert!(!event_source_exists(&event_source));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn append_real_sdk_payload_acks_known_ignored_events_without_appending() {
    for (index, event_type) in [
        "im.message.message_read_v1",
        "im.chat.member.bot.added_v1",
        "im.chat.member.bot.deleted_v1",
        "im.chat.access_event.bot_p2p_chat_entered_v1",
        "im.message.recalled_v1",
        "drive.notice.comment_add_v1",
        "vc.bot.meeting_invited_v1",
    ]
    .iter()
    .enumerate()
    {
        let root = temp_root(&format!("ignored-{index}"));
        let event_source = root.join("events.jsonl");
        let payload = incoming_payload(&serde_json::json!({
            "header": {"event_type": event_type},
            "event": {"event_id": format!("evt_ignored_{index}")}
        }));

        let report =
            append_real_sdk_payload(append_input(18, event_source.clone(), &root), &payload)
                .expect("已知忽略事件应成功 ack");

        assert_eq!(report.input_payloads, 1);
        assert_eq!(report.mapped_events, 0);
        assert_eq!(report.appended_events, 0);
        assert_eq!(report.received_acked_events, 0);
        assert_eq!(report.received_ack_failed_events, 0);
        assert!(!event_source_exists(&event_source));
        let _ = fs::remove_dir_all(root);
    }
}
