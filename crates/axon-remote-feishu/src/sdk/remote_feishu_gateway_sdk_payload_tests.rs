use crate::{
    RemoteFeishuChatType, RemoteFeishuGatewaySdkLoopEvent,
    RemoteFeishuGatewaySdkPayloadMapperRuntime, RemoteFeishuGatewaySdkPayloadTextContext,
    RemoteFeishuIncomingMessageKind, RemoteFeishuMediaKind,
};

#[test]
fn sdk_payload_mapper_maps_message_payload_to_runner_event() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(
        r#"{
            "event": {
                "event_id": "evt_msg_1",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_msg_1",
                    "create_time": "1720000000123",
                    "chat_id": "oc_chat_1",
                    "chat_type": "p2p",
                    "message_type": "text",
                    "content": "{\"text\":\"继续推进\"}",
                    "mentions": [{"open_id":"ou_bot"}],
                    "root_id": "root_1",
                    "parent_id": "parent_1"
                }
            }
        }"#,
    )
    .expect("map message");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.request_id, "om_msg_1");
            assert_eq!(message.message_id, "om_msg_1");
            assert_eq!(message.source_created_at_ms, Some(1_720_000_000_123));
            assert_eq!(message.chat_id, "oc_chat_1");
            assert_eq!(message.chat_type, RemoteFeishuChatType::P2p);
            assert_eq!(message.sender_open_id, "ou_sender");
            assert_eq!(message.mentions, vec!["ou_bot"]);
            assert_eq!(message.message_kind, RemoteFeishuIncomingMessageKind::Text);
            assert_eq!(message.text, "继续推进");
            assert!(message.media_refs.is_empty());
            assert_eq!(
                message
                    .reply_context
                    .as_ref()
                    .and_then(|ctx| ctx.reply_to_message_id.as_deref()),
                Some("parent_1")
            );
            assert_eq!(
                message
                    .reply_context
                    .as_ref()
                    .and_then(|ctx| ctx.thread_id.as_deref()),
                Some("root_1")
            );
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_accepts_official_sender_id_shape() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(
        r#"{
            "event": {
                "sender": {"sender_id": {"open_id": "ou_official_sender"}},
                "message": {
                    "message_id": "om_official_sender",
                    "chat_id": "oc_official_sender",
                    "message_type": "text",
                    "content": "{\"text\":\"你好啊\"}"
                }
            }
        }"#,
    )
    .expect("map official sender_id shape");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.sender_open_id, "ou_official_sender");
            assert_eq!(message.chat_type, RemoteFeishuChatType::Unknown);
            assert_eq!(message.text, "你好啊");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_uses_message_id_as_message_dedup_request_id() {
    let first = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(
        r#"{
            "event": {
                "event_id": "evt_delivery_1",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_same_message",
                    "chat_id": "oc_chat_1",
                    "message_type": "text",
                    "content": "{\"text\":\"重复投递\"}"
                }
            }
        }"#,
    )
    .expect("map first delivery");
    let second = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(
        r#"{
            "event": {
                "event_id": "evt_delivery_2",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_same_message",
                    "chat_id": "oc_chat_1",
                    "message_type": "text",
                    "content": "{\"text\":\"重复投递\"}"
                }
            }
        }"#,
    )
    .expect("map second delivery");

    match (first, second) {
        (
            RemoteFeishuGatewaySdkLoopEvent::Message(first),
            RemoteFeishuGatewaySdkLoopEvent::Message(second),
        ) => {
            assert_eq!(first.request_id, "om_same_message");
            assert_eq!(second.request_id, first.request_id);
        }
        other => panic!("unexpected events: {other:?}"),
    }
}

#[test]
fn sdk_payload_mapper_strips_edge_self_mentions_before_command_detection() {
    let context = RemoteFeishuGatewaySdkPayloadTextContext::new(
        Some("ou_bot".to_owned()),
        Some("Axon".to_owned()),
    );
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload_with_text_context(
        r#"{
            "event": {
                "event_id": "evt_self_mention_1",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_self_mention_1",
                    "chat_id": "oc_chat_1",
                    "message_type": "text",
                    "content": "{\"text\":\" @Axon /status @Axon!\"}",
                    "mentions": [{"open_id":"ou_bot","name":"Axon"}]
                }
            }
        }"#,
        Some(&context),
    )
    .expect("map self mention command");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.mentions, vec!["ou_bot"]);
            assert_eq!(
                message.message_kind,
                RemoteFeishuIncomingMessageKind::Command
            );
            assert_eq!(message.text, "/status!");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_rejects_empty_text_after_self_mention_strip() {
    let context = RemoteFeishuGatewaySdkPayloadTextContext::new(
        Some("ou_bot".to_owned()),
        Some("Axon".to_owned()),
    );
    let result = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload_with_text_context(
        r#"{
            "event": {
                "event_id": "evt_self_mention_empty",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_self_mention_empty",
                    "chat_id": "oc_chat_1",
                    "message_type": "text",
                    "content": "{\"text\":\" @Axon \"}",
                    "mentions": [{"open_id":"ou_bot","name":"Axon"}]
                }
            }
        }"#,
        Some(&context),
    );

    assert!(result.is_err());
}

#[test]
fn sdk_payload_mapper_keeps_mid_sentence_self_mentions() {
    let context = RemoteFeishuGatewaySdkPayloadTextContext::new(
        Some("ou_bot".to_owned()),
        Some("Axon".to_owned()),
    );
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload_with_text_context(
        r#"{
            "event": {
                "event_id": "evt_self_mention_2",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_self_mention_2",
                    "chat_id": "oc_chat_1",
                    "message_type": "text",
                    "content": "{\"text\":\"不要 @Axon 再重复提醒\"}",
                    "mentions": [{"open_id":"ou_bot","name":"Axon"}]
                }
            }
        }"#,
        Some(&context),
    )
    .expect("map mid sentence mention");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.text, "不要 @Axon 再重复提醒");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_injects_non_self_mention_hint_for_text() {
    let context = RemoteFeishuGatewaySdkPayloadTextContext::new(
        Some("ou_bot".to_owned()),
        Some("Axon".to_owned()),
    );
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload_with_text_context(
        r#"{
            "event": {
                "event_id": "evt_mention_hint_1",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_mention_hint_1",
                    "chat_id": "oc_chat_1",
                    "message_type": "text",
                    "content": "{\"text\":\"请结合上下文处理\"}",
                    "mentions": [
                        {"open_id":"ou_bot","name":"Axon"},
                        {"open_id":"ou_user","name":"张三"},
                        {"open_id":"ou_user","name":"张三"}
                    ]
                }
            }
        }"#,
        Some(&context),
    )
    .expect("map mention hint");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(
                message.text,
                "[Mentioned: 张三 (open_id=ou_user)]\n\n请结合上下文处理"
            );
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_does_not_inject_mention_hint_for_commands() {
    let context = RemoteFeishuGatewaySdkPayloadTextContext::new(
        Some("ou_bot".to_owned()),
        Some("Axon".to_owned()),
    );
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload_with_text_context(
        r#"{
            "event": {
                "event_id": "evt_mention_hint_2",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_mention_hint_2",
                    "chat_id": "oc_chat_1",
                    "message_type": "text",
                    "content": "{\"text\":\"@Axon /status\"}",
                    "mentions": [
                        {"open_id":"ou_bot","name":"Axon"},
                        {"open_id":"ou_user","name":"张三"}
                    ]
                }
            }
        }"#,
        Some(&context),
    )
    .expect("map command mention hint");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.text, "/status");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_uses_upper_message_and_root_reply_fallbacks() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(
        r#"{
            "event": {
                "event_id": "evt_msg_reply",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_msg_reply",
                    "chat_id": "oc_chat_reply",
                    "message_type": "text",
                    "content": "{\"text\":\"继续处理话题回复\"}",
                    "root_id": "root_reply",
                    "upper_message_id": "upper_reply"
                }
            }
        }"#,
    )
    .expect("map reply message");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            let reply_context = message.reply_context.expect("reply context");
            assert_eq!(
                reply_context.reply_to_message_id.as_deref(),
                Some("upper_reply")
            );
            assert_eq!(reply_context.thread_id.as_deref(), Some("root_reply"));
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_maps_reaction_payload_to_synthetic_message() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_reaction_payload(
        r#"{
            "header": {"event_type": "im.message.reaction.created_v1"},
            "event": {
                "event_id": "evt_reaction_1",
                "message_id": "om_reacted_1",
                "chat_id": "oc_reaction_1",
                "operator": {"open_id": "ou_operator"},
                "reaction_type": {"emoji_type": "thumbsup"}
            }
        }"#,
    )
    .expect("map reaction");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.request_id, "evt_reaction_1");
            assert_eq!(message.message_id, "om_reacted_1");
            assert_eq!(message.chat_id, "oc_reaction_1");
            assert_eq!(message.sender_open_id, "ou_operator");
            assert_eq!(message.text, "reaction:added:thumbsup");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_maps_reaction_action_time_to_source_timestamp() {
    // reaction 必须带自有事件时间：ingress 水位按 sourceCreatedAtMs 判定新旧，
    // 缺失即被「确认并丢弃」，该字段丢失会让 reaction 路径整体失效。
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_reaction_payload(
        r#"{
            "header": {"event_type": "im.message.reaction.created_v1"},
            "event": {
                "event_id": "evt_reaction_time",
                "message_id": "om_reacted_time",
                "chat_id": "oc_reaction_time",
                "action_time": "1681370240000",
                "operator": {"open_id": "ou_operator"},
                "reaction_type": {"emoji_type": "thumbsup"}
            }
        }"#,
    )
    .expect("map reaction with action_time");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.source_created_at_ms, Some(1_681_370_240_000));
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_maps_reaction_user_id_to_sender() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_reaction_payload(
        r#"{
            "header": {"event_type": "im.message.reaction.created_v1"},
            "event": {
                "event_id": "evt_reaction_user_id",
                "message_id": "om_reacted_user_id",
                "chat_id": "oc_reaction_user_id",
                "user_id": {"open_id": "ou_reactor"},
                "reaction_type": {"emoji_type": "keyboard"}
            }
        }"#,
    )
    .expect("map reaction user_id");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.sender_open_id, "ou_reactor");
            assert_eq!(message.text, "reaction:added:keyboard");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_maps_image_message_to_text_and_media_ref() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(
        r#"{
            "event": {
                "event_id": "evt_msg_2",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_msg_2",
                    "chat_id": "oc_chat_2",
                    "message_type": "image",
                    "content": "{\"image_key\":\"img\"}"
                }
            }
        }"#,
    )
    .expect("map image");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.text, "[Feishu image: image]");
            assert_eq!(message.media_refs.len(), 1);
            assert_eq!(message.media_refs[0].kind, RemoteFeishuMediaKind::Image);
            assert_eq!(message.media_refs[0].key, "img");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_maps_file_audio_and_media_messages() {
    for (message_type, expected_kind) in [
        ("file", RemoteFeishuMediaKind::File),
        ("audio", RemoteFeishuMediaKind::Audio),
        ("media", RemoteFeishuMediaKind::Media),
    ] {
        let raw = format!(
            r#"{{
                "event": {{
                    "sender": {{"user_id": {{"open_id": "ou_sender"}}}},
                    "message": {{
                        "message_id": "om_{message_type}",
                        "chat_id": "oc_chat_media",
                        "message_type": "{message_type}",
                        "content": "{{\"file_key\":\"file_key_{message_type}\",\"file_name\":\"附件.bin\"}}"
                    }}
                }}
            }}"#
        );

        let event =
            RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(&raw).expect("map");

        match event {
            RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
                assert_eq!(message.media_refs.len(), 1);
                assert_eq!(message.media_refs[0].kind, expected_kind);
                assert_eq!(message.media_refs[0].file_name.as_deref(), Some("附件.bin"));
            }
            RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
                panic!("unexpected event: {other:?}")
            }
        }
    }
}

#[test]
fn sdk_payload_mapper_maps_sticker_message_to_media_ref() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(
        r#"{
            "event": {
                "event_id": "evt_sticker_1",
                "sender": {"user_id": {"open_id": "ou_sender"}},
                "message": {
                    "message_id": "om_sticker_1",
                    "chat_id": "oc_chat_sticker",
                    "message_type": "sticker",
                    "content": "{\"file_key\":\"sticker_key_1\"}"
                }
            }
        }"#,
    )
    .expect("map sticker");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
            assert_eq!(message.text, "[Feishu sticker: sticker]");
            assert_eq!(message.media_refs.len(), 1);
            assert_eq!(message.media_refs[0].kind, RemoteFeishuMediaKind::Sticker);
            assert_eq!(message.media_refs[0].key, "sticker_key_1");
        }
        RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_maps_lightweight_structured_messages_to_visible_text() {
    for (message_type, content, expected) in [
        (
            "folder",
            r#"{"file_key":"folder_key_1","file_name":"研发资料"}"#,
            "[Feishu folder: file_key=folder_key_1, file_name=研发资料]",
        ),
        (
            "location",
            r#"{"name":"上海办公室","latitude":"31.2304","longitude":"121.4737"}"#,
            "[Feishu location: name=上海办公室, latitude=31.2304, longitude=121.4737]",
        ),
        (
            "share_user",
            r#"{"user_id":"ou_contact_1"}"#,
            "[Feishu contact: user_id=ou_contact_1]",
        ),
        (
            "system",
            r#"{"template":"Hello {name}, welcome to {place}","name":"Alice","place":"Axon"}"#,
            "Hello Alice, welcome to Axon",
        ),
        (
            "vote",
            r#"{"topic":"午餐选项","options":["米饭","面"]}"#,
            "[Feishu vote]\n午餐选项\n米饭\n面",
        ),
        (
            concat!("to", "do"),
            r#"{"summary":{"title":"处理飞书闭环"}}"#,
            "[Feishu task]\n处理飞书闭环",
        ),
    ] {
        let raw = format!(
            r#"{{
                "event": {{
                    "sender": {{"user_id": {{"open_id": "ou_sender"}}}},
                    "message": {{
                        "message_id": "om_{message_type}",
                        "chat_id": "oc_chat_structured",
                        "message_type": "{message_type}",
                        "content": {}
                    }}
                }}
            }}"#,
            serde_json::to_string(content).expect("content json")
        );

        let event =
            RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(&raw).expect("map");

        match event {
            RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
                assert_eq!(message.text, expected);
                assert!(message.media_refs.is_empty());
            }
            RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
                panic!("unexpected event: {other:?}")
            }
        }
    }
}

#[test]
fn sdk_payload_mapper_extracts_rich_message_visible_text() {
    for (message_type, content, expected) in [
        (
            "merge_forward",
            r#"{"title":"转发摘要","messages":[{"sender_name":"李四","text":"第一条"}]}"#,
            "转发摘要\n- 李四: 第一条",
        ),
        (
            "share_chat",
            r#"{"chat_name":"研发群","chat_id":"oc_share"}"#,
            "Shared chat: 研发群\nChat ID: oc_share",
        ),
        (
            "interactive",
            r#"{"card":{"header":{"title":{"content":"审批卡片"}},"elements":[{"tag":"div","text":{"content":"请确认"}},{"tag":"button","text":{"content":"通过"}}]}}"#,
            "审批卡片\n请确认\n通过",
        ),
    ] {
        let raw = format!(
            r#"{{
                "event": {{
                    "sender": {{"user_id": {{"open_id": "ou_sender"}}}},
                    "message": {{
                        "message_id": "om_{message_type}",
                        "chat_id": "oc_chat_rich",
                        "message_type": "{message_type}",
                        "content": {}
                    }}
                }}
            }}"#,
            serde_json::to_string(content).expect("content json")
        );

        let event =
            RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload(&raw).expect("map");

        match event {
            RemoteFeishuGatewaySdkLoopEvent::Message(message) => {
                assert_eq!(message.text, expected);
                assert!(message.media_refs.is_empty());
            }
            RemoteFeishuGatewaySdkLoopEvent::CardAction(other) => {
                panic!("unexpected event: {other:?}")
            }
        }
    }
}
