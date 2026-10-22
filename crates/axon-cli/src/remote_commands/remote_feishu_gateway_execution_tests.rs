use axon_remote_feishu::RemoteFeishuGovernanceDecisionStatus;
use serde_json::json;

use super::remote_feishu_gateway_event_execution::{
    is_card_action_kind, question_answer_payload, record_gateway_chat_route,
};
use super::*;

fn message_input(message_kind: Option<&str>, text: &str) -> FeishuGatewayInputEvent {
    serde_json::from_value(json!({
        "requestId": "req",
        "chatId": "oc_chat",
        "text": text,
        "messageKind": message_kind,
    }))
    .expect("测试消息应有效")
}

fn ingress_message(
    request_id: &str,
    source_created_at_ms: u64,
    text: &str,
) -> FeishuGatewayInputEvent {
    serde_json::from_value(json!({
        "kind": "message",
        "requestId": request_id,
        "chatId": "oc_chat",
        "chatType": "p2p",
        "messageId": format!("om_{request_id}"),
        "sourceCreatedAtMs": source_created_at_ms,
        "senderOpenId": "ou_sender",
        "text": text,
    }))
    .expect("入站测试消息应有效")
}

#[test]
fn reply_enrichment_only_accepts_plain_text_messages() {
    assert!(should_enrich_reply_context(&message_input(
        Some("text"),
        "继续处理",
    )));
    assert!(!should_enrich_reply_context(&message_input(
        Some("command"),
        "/stop",
    )));
    assert!(!should_enrich_reply_context(&message_input(None, "/stop")));
}

#[test]
fn message_with_valid_source_timestamp_passes_the_ingress_frontier() {
    // reaction 事件补齐 action_time 后走的正是这条判定：有时间戳即可进入治理。
    let root = axon_test_support::test_temp_dir();
    let context = test_context(root.path());
    let input = ingress_message("req-with-timestamp", 100, "带时间戳");

    assert!(
        !should_skip_gateway_input_event(&context, &input).expect("应可判断"),
        "有有效 sourceCreatedAtMs 的消息不得被水位丢弃"
    );
}

#[test]
fn message_without_source_timestamp_is_confirmed_and_dropped() {
    // 缺口时间戳即被丢弃是既有水位策略；reaction 曾因 mapper 不填该字段而整体失效，
    // 因此这里把判定钉住：只要 mapper 不再丢字段，上面的用例就会失败。
    let root = axon_test_support::test_temp_dir();
    let context = test_context(root.path());
    let mut input = ingress_message("req-without-timestamp", 100, "缺时间戳");
    input.source_created_at_ms = None;

    assert!(
        should_skip_gateway_input_event(&context, &input).expect("应可判断"),
        "缺少 sourceCreatedAtMs 的消息会被确认并丢弃"
    );
}

fn test_context(root: &std::path::Path) -> RemoteCliContext {
    RemoteCliContext::new(
        root.join("config.toml"),
        root.to_path_buf(),
        axon_config::RemoteFeishuWebSocketConfig {
            app_id: "cli_test_app".to_owned(),
            ..axon_config::RemoteFeishuWebSocketConfig::default()
        },
    )
}

fn test_gateway_command() -> RemoteGatewayCommand {
    RemoteGatewayCommand {
        session_id: 7,
        event_json: None,
        event_file: None,
        daemon: true,
        interval_ms: None,
        max_ticks: Some(1),
        cursor_file: None,
        daemon_endpoint: "test-endpoint".to_owned(),
    }
}

#[test]
fn reaction_materialization_fills_chat_id_chat_type_and_action_time() {
    // reaction 事件的完整补齐链：消息归属查 chat_id、会话信息查 chat_type、mapper 取
    // action_time。三者任一缺失都会让该事件在入站主链上被丢弃或让 Interaction 构造失败，
    // 因此这是一个端到端回归用例。
    let root = axon_test_support::test_temp_dir();
    let context = test_context(root.path());
    let command = test_gateway_command();
    let governance =
        RemoteFeishuGovernanceRuntime::new(axon_domain::RemoteCapabilityPolicy::default_sidecar());
    let store_path = root.path().join("remote_feishu_governance.sqlite3");
    let runtime = FeishuGatewayExecutionRuntime {
        context: &context,
        command: &command,
        tenant_access_token: "tenant-token",
        api_base_url: Some("https://open.feishu.cn"),
        governance: &governance,
        store_path: &store_path,
    };
    let raw_payload = serde_json::json!({
        "header": {"event_type": "im.message.reaction.created_v1"},
        "event": {
            "event_id": "evt_reaction",
            "message_id": "om_reacted",
            "action_time": "1681370240000",
            "operator": {"open_id": "ou_user"},
            "reaction_type": {"emoji_type": "keyboard"}
        }
    })
    .to_string();
    let input: FeishuGatewayInputEvent = serde_json::from_value(serde_json::json!({
        "kind": "reactionPending",
        "requestId": "evt_reaction",
        "chatId": "",
        "messageId": "om_reacted",
        "rawPayloadJson": raw_payload,
    }))
    .expect("reactionPending 事件应有效");

    let materialized = materialize_pending_reaction_with_http(&runtime, &input, |request| {
        let body = if request.url.contains("/chats/") {
            r#"{"code":0,"data":{"chat_mode":"group"}}"#
        } else {
            r#"{"code":0,"data":{"items":[{"sender":{"id":"cli_test_app"},"chat_id":"oc_reaction"}]}}"#
        };
        Ok(RemoteFeishuHttpResponse {
            status: 200,
            body: body.to_owned(),
        })
    })
    .expect("reaction 补齐应成功")
    .expect("补齐后的 reaction 应可处理");

    assert_eq!(materialized.kind, "message");
    assert_eq!(materialized.chat_id, "oc_reaction");
    assert_eq!(materialized.chat_type.as_deref(), Some("group"));
    assert_eq!(materialized.source_created_at_ms, Some(1_681_370_240_000));
}

#[test]
fn reaction_materialization_skips_event_when_chat_type_is_unknown() {
    // 会话类型查不到时必须跳过，而不是把群聊默认记成私聊或落进治理链。
    let root = axon_test_support::test_temp_dir();
    let context = test_context(root.path());
    let command = test_gateway_command();
    let governance =
        RemoteFeishuGovernanceRuntime::new(axon_domain::RemoteCapabilityPolicy::default_sidecar());
    let store_path = root.path().join("remote_feishu_governance.sqlite3");
    let runtime = FeishuGatewayExecutionRuntime {
        context: &context,
        command: &command,
        tenant_access_token: "tenant-token",
        api_base_url: Some("https://open.feishu.cn"),
        governance: &governance,
        store_path: &store_path,
    };
    let input: FeishuGatewayInputEvent = serde_json::from_value(serde_json::json!({
        "kind": "reactionPending",
        "requestId": "evt_reaction_unknown",
        "chatId": "",
        "messageId": "om_reacted",
        "rawPayloadJson": serde_json::json!({
            "event": {
                "event_id": "evt_reaction_unknown",
                "message_id": "om_reacted",
                "action_time": "1681370240000",
                "operator": {"open_id": "ou_user"},
                "reaction_type": {"emoji_type": "keyboard"}
            }
        })
        .to_string(),
    }))
    .expect("reactionPending 事件应有效");

    let materialized = materialize_pending_reaction_with_http(&runtime, &input, |request| {
        let body = if request.url.contains("/chats/") {
            r#"{"code":0,"data":{}}"#
        } else {
            r#"{"code":0,"data":{"items":[{"sender":{"id":"cli_test_app"},"chat_id":"oc_reaction"}]}}"#
        };
        Ok(RemoteFeishuHttpResponse {
            status: 200,
            body: body.to_owned(),
        })
    })
    .expect("补齐流程本身不应失败");

    assert!(materialized.is_none(), "会话类型未知时必须跳过该事件");
}

#[test]
fn card_action_kind_accepts_both_event_source_spellings() {
    // 事件源写 cardAction，外部 adapter 可能写 card-action；只认其一会让整条问答
    // 入站分发不可达（这正是历史缺陷：分支写成 card_action，永远不命中）。
    assert!(is_card_action_kind("cardAction"));
    assert!(is_card_action_kind("card-action"));
    assert!(!is_card_action_kind("message"));
    assert!(!is_card_action_kind("reactionPending"));
}

#[test]
fn card_action_question_payload_is_read_from_action_value_json() {
    let mut input: FeishuGatewayInputEvent = serde_json::from_value(json!({
        "kind": "cardAction",
        "requestId": "card-req-1",
        "chatId": "oc_chat",
        "messageId": "om_question_card",
        "operatorOpenId": "ou_operator",
        "actionTag": "button",
        "actionValueJson": r#"{"axon_action":"answer_question","call_id":"call-1","qid":"q1","opt":"同意"}"#,
    }))
    .expect("卡片动作事件应有效");

    assert!(input.text.is_none(), "卡片动作的 text 恒为空");
    let payload = question_answer_payload(&input).expect("载荷必须取自 actionValueJson");
    let parsed =
        crate::remote_commands::remote_feishu_question_action::parse_feishu_question_answer(
            payload,
        )
        .expect("问答动作应可解析");
    assert_eq!(parsed.call_id, "call-1");
    assert_eq!(parsed.option, "同意");
    assert_eq!(input.message_id.as_deref(), Some("om_question_card"));

    // 非卡片动作事件不产生问答载荷。
    input.kind = "message".to_owned();
    assert!(question_answer_payload(&input).is_none());
}

#[test]
fn card_action_without_payload_does_not_enter_the_answer_branch() {
    let input: FeishuGatewayInputEvent = serde_json::from_value(json!({
        "kind": "cardAction",
        "requestId": "card-req-2",
        "chatId": "oc_chat",
        "operatorOpenId": "ou_operator",
        "actionTag": "button",
    }))
    .expect("卡片动作事件应有效");

    assert!(
        question_answer_payload(&input).is_none(),
        "无载荷的卡片动作必须回落到既有卡片动作链路"
    );
}

#[test]
fn message_ingress_records_feishu_chat_route() {
    let root = axon_test_support::test_temp_dir();
    let context = test_context(root.path());
    let command = test_gateway_command();
    let governance =
        RemoteFeishuGovernanceRuntime::new(axon_domain::RemoteCapabilityPolicy::default_sidecar());
    let store_path = root.path().join("remote_feishu_governance.sqlite3");
    let runtime = FeishuGatewayExecutionRuntime {
        context: &context,
        command: &command,
        tenant_access_token: "tenant-token",
        api_base_url: Some("https://open.feishu.cn"),
        governance: &governance,
        store_path: &store_path,
    };
    let input = ingress_message("req-route", 200, "请开始");

    record_gateway_chat_route(&runtime, &input, "workspace-1");

    let lane_id = axon_session::session_lane_id(
        "workspace-1",
        &axon_domain::AudienceContext::new(
            axon_domain::AudienceKind::Private,
            axon_domain::DisclosurePolicy::PrivateOnly,
        ),
        "feishu",
        "oc_chat",
    );
    let route = crate::remote_commands::remote_feishu_question_action::resolve_chat_type(
        &context.root,
        "oc_chat",
    );
    assert_eq!(
        route,
        axon_remote_feishu::RemoteFeishuChatType::P2p,
        "入站消息的车道绑定必须落盘，问答卡片据此找回会话"
    );
    assert!(
        crate::remote_commands::remote_feishu_question_store::lookup_route(
            &context.root,
            &lane_id,
        )
        .expect("查询路由")
        .is_some()
    );
}

#[test]
fn accepted_newer_activity_filters_older_delivery() {
    let root = axon_test_support::test_temp_dir();
    let newer = ingress_message("req-newer", 200, "最新输入");

    advance_accepted_gateway_input(
        root.path(),
        &newer,
        RemoteFeishuGovernanceDecisionStatus::Accepted,
        300,
    )
    .expect("Accepted 消息应推进水位");

    assert!(
        remote_feishu_ingress_watermark::should_reject(
            root.path(),
            "oc_chat",
            "req-older",
            Some(100),
        )
        .expect("旧投递应可判断")
    );
}

#[test]
fn non_accepted_message_does_not_advance_frontier() {
    let root = axon_test_support::test_temp_dir();
    let rejected = ingress_message("req-rejected", 200, "未通过治理");

    advance_accepted_gateway_input(
        root.path(),
        &rejected,
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist,
        300,
    )
    .expect("非 Accepted 消息应被忽略");

    assert!(
        remote_feishu_ingress_watermark::current(root.path(), "oc_chat")
            .expect("水位应可读取")
            .is_none()
    );
}
