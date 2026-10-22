use crate::{RemoteFeishuGatewaySdkLoopEvent, RemoteFeishuGatewaySdkPayloadMapperRuntime};

#[test]
fn sdk_payload_mapper_maps_card_action_payload_to_runner_event() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
        r#"{
            "event": {
                "token": "token_card_1",
                "context": {"open_chat_id": "oc_card_1"},
                "operator": {"open_id": "ou_operator"},
                "action": {
                    "tag": "button",
                    "value": {"axon_action":"clarify"}
                }
            }
        }"#,
    )
    .expect("map card action");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::CardAction(card_action) => {
            assert!(
                card_action
                    .request_id
                    .starts_with("card-action:token_card_1:")
            );
            assert_eq!(card_action.chat_id, "oc_card_1");
            assert_eq!(card_action.operator_open_id, "ou_operator");
            assert_eq!(card_action.action_tag, "button");
            assert!(
                card_action
                    .action_value_json
                    .contains("\"axon_action\":\"clarify\"")
            );
        }
        RemoteFeishuGatewaySdkLoopEvent::Message(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_defaults_card_action_tag_and_value() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
        r#"{
            "event": {
                "token": "token_card_2",
                "context": {"open_chat_id": "oc_card_2"},
                "operator": {"open_id": "ou_operator"},
                "action": {}
            }
        }"#,
    )
    .expect("map card action defaults");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::CardAction(card_action) => {
            assert!(
                card_action
                    .request_id
                    .starts_with("card-action:token_card_2:")
            );
            assert_eq!(card_action.chat_id, "oc_card_2");
            assert_eq!(card_action.action_tag, "button");
            assert_eq!(card_action.action_value_json, "");
        }
        RemoteFeishuGatewaySdkLoopEvent::Message(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

/// G：去重键必须含操作人。同一张卡片、同一个按钮被两个人点击时，第二次点击不能
/// 被当成重复事件丢弃，否则后来者的确认永远进不了答案入口。
#[test]
fn sdk_payload_mapper_card_action_dedup_id_includes_operator() {
    let payload = |operator: &str| {
        format!(
            r#"{{
                "event": {{
                    "token": "token_card_operator",
                    "context": {{"open_chat_id": "oc_card"}},
                    "operator": {{"open_id": "{operator}"}},
                    "action": {{
                        "tag": "button",
                        "value": {{"axon_action":"answer_question","call_id":"call-1","qid":"q1","opt":"同意"}}
                    }}
                }}
            }}"#
        )
    };

    let alice =
        RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(&payload("ou_alice"))
            .expect("map alice card action");
    let bob =
        RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(&payload("ou_bob"))
            .expect("map bob card action");
    let alice_again =
        RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(&payload("ou_alice"))
            .expect("map duplicate alice card action");

    match (alice, bob, alice_again) {
        (
            RemoteFeishuGatewaySdkLoopEvent::CardAction(alice),
            RemoteFeishuGatewaySdkLoopEvent::CardAction(bob),
            RemoteFeishuGatewaySdkLoopEvent::CardAction(alice_again),
        ) => {
            assert_ne!(
                alice.request_id, bob.request_id,
                "不同操作人必须是两条独立事件"
            );
            assert_eq!(
                alice.request_id, alice_again.request_id,
                "同一操作人的重复回调必须稳定去重"
            );
        }
        other => panic!("unexpected events: {other:?}"),
    }
}

/// 被点击卡片自身的消息 ID 必须随事件带出：结算回写与作废都依赖它。
#[test]
fn sdk_payload_mapper_card_action_carries_clicked_card_message_id() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
        r#"{
            "event": {
                "token": "token_card_message",
                "context": {"open_chat_id": "oc_card", "open_message_id": "om_question_card"},
                "operator": {"open_id": "ou_operator"},
                "action": {"tag": "button", "value": {"axon_action":"answer_question"}}
            }
        }"#,
    )
    .expect("map card action");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::CardAction(card_action) => {
            assert_eq!(
                card_action.message_id.as_deref(),
                Some("om_question_card"),
                "点击的卡片消息 ID 必须可查"
            );
        }
        RemoteFeishuGatewaySdkLoopEvent::Message(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

/// 表单类卡片把字段值放在 `action.form_value`，只读 `action.value` 会得到空载荷。
#[test]
fn sdk_payload_mapper_card_action_reads_form_value_payload() {
    let event = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
        r#"{
            "event": {
                "token": "token_card_form",
                "context": {"open_chat_id": "oc_card"},
                "operator": {"open_id": "ou_operator"},
                "action": {
                    "tag": "button",
                    "form_value": {"axon_action":"answer_question","call_id":"call-form","qid":"q1","opt":"同意"}
                }
            }
        }"#,
    )
    .expect("map form card action");

    match event {
        RemoteFeishuGatewaySdkLoopEvent::CardAction(card_action) => {
            assert!(
                card_action
                    .action_value_json
                    .contains("\"call_id\":\"call-form\""),
                "表单载荷必须可解析: {}",
                card_action.action_value_json
            );
        }
        RemoteFeishuGatewaySdkLoopEvent::Message(other) => {
            panic!("unexpected event: {other:?}")
        }
    }
}

#[test]
fn sdk_payload_mapper_card_action_dedup_id_includes_token_tag_and_value() {
    let first = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
        r#"{
            "event": {
                "token": "token_card_same",
                "context": {"open_chat_id": "oc_card"},
                "operator": {"open_id": "ou_operator"},
                "action": {
                    "tag": "button",
                    "value": {"action":"clarify","label":"补充说明"}
                }
            }
        }"#,
    )
    .expect("map first card action");
    let second = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
        r#"{
            "event": {
                "token": "token_card_same",
                "context": {"open_chat_id": "oc_card"},
                "operator": {"open_id": "ou_operator"},
                "action": {
                    "tag": "button",
                    "value": {"action":"clarify","label":"补充说明"}
                }
            }
        }"#,
    )
    .expect("map duplicate card action");
    let different_value = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
        r#"{
            "event": {
                "token": "token_card_same",
                "context": {"open_chat_id": "oc_card"},
                "operator": {"open_id": "ou_operator"},
                "action": {
                    "tag": "button",
                    "value": {"action":"reset","label":"重置"}
                }
            }
        }"#,
    )
    .expect("map different card action");

    match (first, second, different_value) {
        (
            RemoteFeishuGatewaySdkLoopEvent::CardAction(first),
            RemoteFeishuGatewaySdkLoopEvent::CardAction(second),
            RemoteFeishuGatewaySdkLoopEvent::CardAction(different),
        ) => {
            assert_eq!(first.request_id, second.request_id);
            assert_ne!(first.request_id, different.request_id);
        }
        other => panic!("unexpected events: {other:?}"),
    }
}
