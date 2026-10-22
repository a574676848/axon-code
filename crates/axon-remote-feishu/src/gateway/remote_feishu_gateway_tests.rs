use axon_domain::{RemoteCapabilityPolicy, SessionId};

use crate::{
    RemoteFeishuCardActionEvent, RemoteFeishuGatewayInboundEvent, RemoteFeishuGatewayRuntime,
    RemoteFeishuGatewayTickInput, RemoteFeishuGovernanceConfig,
    RemoteFeishuGovernanceDecisionStatus, RemoteFeishuGovernanceRuntime,
    RemoteFeishuGovernanceStoreRuntime, RemoteFeishuIncomingMessage,
    RemoteFeishuIncomingMessageKind, RemoteFeishuMediaKind, RemoteFeishuMediaRef,
    RemoteFeishuReplyContext,
};

const SINGLE_EVENT_QUEUE_DEPTH: usize = 1;

#[test]
fn gateway_defers_second_event_in_same_chat_until_turn_completes() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::CardAction(card_action("req-1", "oc_1")),
            1,
        )
        .expect("enqueue first");
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::CardAction(card_action("req-2", "oc_1")),
            2,
        )
        .expect("enqueue second");

    let first = gateway.drain_ready();

    assert_eq!(first.ready_events.len(), 1);
    assert_eq!(first.pending_events, 1);
    assert_eq!(first.active_chat_turns, 1);
    assert_eq!(first.deferred_by_chat, 1);

    let release = gateway.complete_chat_turn("oc_1").expect("release turn");
    assert!(release.released);
    let second = gateway.drain_ready();

    assert_eq!(second.ready_events.len(), 1);
    assert_eq!(second.ready_events[0].meta.request_id, "req-2");
    assert_eq!(gateway.pending_len(), 0);
}

#[test]
fn gateway_merges_compatible_text_bursts_before_drain() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let mut second = message("req-2", "oc_1");
    second.text = "继续补充".to_owned();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-1", "oc_1")),
            1,
        )
        .expect("enqueue first");
    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::Message(second), 2)
        .expect("enqueue second");

    let report = gateway.drain_ready();

    assert_eq!(report.ready_events.len(), 1);
    assert_eq!(report.ready_events[0].meta.request_id, "req-2");
    assert_eq!(report.ready_events[0].merged_message_count, 2);
    match &report.ready_events[0].event {
        RemoteFeishuGatewayInboundEvent::Message(message) => {
            assert_eq!(message.text, "继续推进\n继续补充");
        }
        RemoteFeishuGatewayInboundEvent::CardAction(other) => {
            panic!("unexpected event: {other:?}");
        }
    }
}

#[test]
fn gateway_merges_compatible_text_when_pending_queue_is_full() {
    let mut gateway = RemoteFeishuGatewayRuntime::new(SINGLE_EVENT_QUEUE_DEPTH);
    gateway.connect();
    let mut second = message("req-2", "oc_1");
    second.text = "继续补充".to_owned();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-1", "oc_1")),
            1,
        )
        .expect("首条消息应可入队");
    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::Message(second), 2)
        .expect("兼容文本应在队满时合并");

    let report = gateway.drain_ready();

    assert_eq!(report.ready_events.len(), SINGLE_EVENT_QUEUE_DEPTH);
    assert_eq!(report.ready_events[0].merged_message_count, 2);
    assert_eq!(report.ready_events[0].meta.request_id, "req-2");
}

#[test]
fn gateway_carries_reply_thread_context_in_inbound_meta() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let mut event = message("req-reply", "oc_2");
    event.reply_context = Some(RemoteFeishuReplyContext {
        reply_to_message_id: Some("om_parent".to_owned()),
        reply_to_text: Some("上一条消息".to_owned()),
        thread_id: Some("thread_1".to_owned()),
    });

    let meta = gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::Message(event), 1)
        .expect("enqueue reply");

    let reply_context = meta.reply_context.expect("reply context");
    assert_eq!(
        reply_context.reply_to_message_id.as_deref(),
        Some("om_parent")
    );
    assert_eq!(reply_context.thread_id.as_deref(), Some("thread_1"));
}

#[test]
fn gateway_report_keeps_quoted_sidecar_and_text_attachments_for_agent() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-gateway-agent-files-{}",
        unique_suffix()
    ));
    std::fs::create_dir_all(&root).expect("测试目录应可创建");
    let text_file = root.join("report.txt");
    let binary_file = root.join("image.bin");
    std::fs::write(&text_file, "附件正文").expect("文本附件应可写入");
    std::fs::write(&binary_file, [0xff, 0xfe]).expect("二进制附件应可写入");
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let mut event = message("req-attachment", "oc_attachment");
    "feishu-sdk".clone_into(&mut event.client_id);
    "om_attachment".clone_into(&mut event.message_id);
    "ou_sender".clone_into(&mut event.sender_open_id);
    event.mentions.clear();
    event.text = format!(
        "请处理附件\n\n[Attachment: report.txt]\n{}\n[Attachment: image.png]\n{}",
        text_file.display(),
        binary_file.display()
    );
    event.reply_context = Some(RemoteFeishuReplyContext {
        reply_to_message_id: Some("om_parent".to_owned()),
        reply_to_text: Some("引用原文".to_owned()),
        thread_id: Some("root_1".to_owned()),
    });
    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::Message(event), 200)
        .expect("入站应可入队");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let report = gateway
        .drain_into_governance(
            &governance,
            &crate::RemoteFeishuGatewayGovernanceInput {
                governance_config: RemoteFeishuGovernanceConfig {
                    allowed_sender_open_ids: Vec::new(),
                    blocked_sender_open_ids: Vec::new(),
                    require_mention: false,
                    bot_open_id: None,
                    chat_policies: Vec::new(),
                },
                pairing_id: "pairing-feishu".to_owned(),
                existing_records: Vec::new(),
            },
        )
        .expect("gateway 应生成治理报告");

    let event = &report.event_reports[0];
    let prompt = event.agent_prompt.as_ref().expect("prompt 应存在");
    assert!(!prompt.contains("引用原文"));
    assert!(prompt.contains("[Attachment: image.png]"));
    assert!(prompt.contains("[Content of report.txt]:\n附件正文"));
    assert!(!prompt.contains("[Content of image.bin]"));
    let quoted = event.quoted_message.as_ref().expect("引用 sidecar 应存在");
    assert_eq!(quoted.message_id, "om_parent");
    assert_eq!(quoted.visible_text, "引用原文");
    assert_eq!(event.agent_files, vec![text_file, binary_file]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn gateway_text_merge_preserves_media_refs() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let mut first = message("req-media-1", "oc_attachment");
    first.media_refs = vec![RemoteFeishuMediaRef {
        kind: RemoteFeishuMediaKind::Image,
        key: "img_1".to_owned(),
        file_name: Some("a.png".to_owned()),
    }];
    let mut second = message("req-media-2", "oc_attachment");
    second.text = "第二段".to_owned();
    second.media_refs = vec![RemoteFeishuMediaRef {
        kind: RemoteFeishuMediaKind::Image,
        key: "img_2".to_owned(),
        file_name: Some("b.png".to_owned()),
    }];

    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::Message(first), 100)
        .expect("第一条消息应可入队");
    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::Message(second), 101)
        .expect("第二条消息应可合并");

    let queued = gateway.drain_ready().ready_events.remove(0);
    let RemoteFeishuGatewayInboundEvent::Message(message) = queued.event else {
        panic!("应为消息事件");
    };
    assert_eq!(message.media_refs.len(), 2);
    assert!(message.text.contains("第二段"));
}

#[test]
fn gateway_shutdown_can_keep_pending_for_replay() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-1", "oc_1")),
            1,
        )
        .expect("enqueue");

    gateway.shutdown(false);

    assert_eq!(gateway.pending_len(), 1);
}

#[test]
fn gateway_drain_enters_governance_session_and_remote_transport() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-1", "oc_1")),
            1,
        )
        .expect("enqueue first");
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-2", "oc_1")),
            2,
        )
        .expect("enqueue second");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());

    let report = gateway
        .drain_into_governance(&governance, &governance_input())
        .expect("drain governance");

    assert_eq!(report.drain.ready_events.len(), 1);
    assert_eq!(report.drain.pending_events, 0);
    assert_eq!(report.event_reports.len(), 1);
    assert_eq!(report.event_reports[0].meta.request_id, "req-2");
    assert!(report.event_reports[0].decision.dispatch_report.is_some());
}

#[test]
fn gateway_drain_keeps_rejected_governance_decision_without_dispatch() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-blocked", "oc_1")),
            1,
        )
        .expect("enqueue");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let input = crate::RemoteFeishuGatewayGovernanceInput {
        governance_config: RemoteFeishuGovernanceConfig {
            allowed_sender_open_ids: vec!["ou_allowed".to_owned()],
            blocked_sender_open_ids: Vec::new(),
            require_mention: false,
            bot_open_id: Some("ou_bot".to_owned()),
            chat_policies: Vec::new(),
        },
        pairing_id: "pairing-feishu".to_owned(),
        existing_records: Vec::new(),
    };

    let report = gateway
        .drain_into_governance(&governance, &input)
        .expect("drain governance");

    assert_eq!(report.event_reports.len(), 1);
    assert!(report.event_reports[0].decision.dispatch_report.is_none());
}

#[test]
fn gateway_drain_drops_duplicate_request_id_within_same_tick() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::CardAction(card_action("card-dup", "oc_1")),
            1,
        )
        .expect("enqueue first card action");
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::CardAction(card_action("card-dup", "oc_2")),
            2,
        )
        .expect("enqueue duplicate card action");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());

    let report = gateway
        .drain_into_governance(&governance, &governance_input())
        .expect("drain governance");

    assert_eq!(report.event_reports.len(), 2);
    assert_eq!(
        report.event_reports[0].decision.status,
        crate::RemoteFeishuGovernanceDecisionStatus::Accepted
    );
    assert_eq!(
        report.event_reports[1].decision.status,
        crate::RemoteFeishuGovernanceDecisionStatus::DuplicateDropped
    );
    assert!(report.event_reports[1].decision.dispatch_report.is_none());
}

#[test]
fn gateway_ordinary_card_action_generates_agent_visible_summary() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let mut event = card_action("card-ordinary", "oc_card");
    event.action_tag = "select_option".to_owned();
    event.action_value_json = r#"{"label":"选择 A","value":"A"}"#.to_owned();
    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::CardAction(event), 1)
        .expect("enqueue ordinary card action");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());

    let report = gateway
        .drain_into_governance(&governance, &governance_input())
        .expect("drain governance");

    let prompt = report.event_reports[0]
        .agent_prompt
        .as_ref()
        .expect("普通卡片动作应进入 Agent prompt");
    assert!(prompt.contains("用户点击了飞书卡片动作"));
    assert!(prompt.contains("动作标签：选择 A"));
    assert!(prompt.contains("\"value\":\"A\""));
}

#[test]
fn gateway_question_answer_card_action_never_dispatches_a_turn() {
    // 问答作答由飞书问答结算入口就地处理；若这里仍生成摘要并派发，同一次点击会
    // 重复触发一轮 agent turn，用户会看到同一条回答被处理两次。
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let mut event = card_action("card-answer", "oc_card");
    event.action_tag = "button".to_owned();
    event.action_value_json =
        r#"{"axon_action":"answer_question","call_id":"call-1","qid":"q1","opt":"同意"}"#
            .to_owned();
    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::CardAction(event), 1)
        .expect("enqueue question answer");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());

    let report = gateway
        .drain_into_governance(&governance, &governance_input())
        .expect("drain governance");

    let event_report = &report.event_reports[0];
    assert_eq!(
        event_report.decision.status,
        crate::RemoteFeishuGovernanceDecisionStatus::Accepted
    );
    assert!(
        event_report.agent_prompt.is_none(),
        "问答作答不得生成 Agent 摘要派发"
    );
    assert!(
        crate::interaction_from_gateway_report(event_report, "workspace-1", 42)
            .expect("投影判定")
            .is_none(),
        "问答作答不得进入 durable Interaction 派发链"
    );
}

#[test]
fn gateway_card_action_meta_carries_operator_and_clicked_message() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let event = card_action("card-meta", "oc_card");
    gateway
        .enqueue(RemoteFeishuGatewayInboundEvent::CardAction(event), 1)
        .expect("enqueue card action");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());

    let report = gateway
        .drain_into_governance(&governance, &governance_input())
        .expect("drain governance");

    let meta = &report.event_reports[0].meta;
    assert_eq!(meta.operator_open_id.as_deref(), Some("ou_user"));
    assert_eq!(meta.message_id.as_deref(), Some("om_gateway_card"));
}

#[test]
fn gateway_tick_defers_accepted_governance_record_until_dispatch() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-1", "oc_1")),
            1,
        )
        .expect("enqueue first");
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-2", "oc_1")),
            2,
        )
        .expect("enqueue second");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let store_path = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-gateway-governance-{}.jsonl",
        unique_suffix()
    ));

    let report = gateway
        .tick_and_persist_governance(
            &governance,
            &RemoteFeishuGatewayTickInput {
                governance: governance_input(),
                governance_store_path: store_path.clone(),
            },
        )
        .expect("tick");

    // Accepted 记录语义是「已交付」：tick 不得抢先落盘，否则 dispatch 失败后
    // 的重投会被自己的记录判成重复而被静默丢弃。
    assert_eq!(report.persisted_records, 0);
    assert_eq!(report.active_chat_turns, 1);
    assert_eq!(report.governance.drain.pending_events, 0);
    assert!(
        RemoteFeishuGovernanceStoreRuntime::read_records(&store_path)
            .expect("records")
            .is_empty()
    );

    // 调用方在 dispatch 提交成功后才落盘；落盘之后重投才具备幂等短路依据。
    let event_report = &report.governance.event_reports[0];
    assert_eq!(
        event_report.decision.status,
        RemoteFeishuGovernanceDecisionStatus::Accepted
    );
    RemoteFeishuGovernanceStoreRuntime::append_record(&store_path, &event_report.decision.record)
        .expect("提交成功后落盘");
    assert_eq!(
        RemoteFeishuGovernanceStoreRuntime::read_records(&store_path)
            .expect("records")
            .len(),
        1
    );
    let _ = std::fs::remove_file(store_path);
}

#[test]
fn gateway_tick_persists_rejected_governance_record_immediately() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-rejected", "oc_1")),
            1,
        )
        .expect("enqueue");
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let store_path = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-gateway-governance-rejected-{}.jsonl",
        unique_suffix()
    ));
    let mut input = governance_input();
    input.governance_config.allowed_sender_open_ids = vec!["ou_other".to_owned()];

    let report = gateway
        .tick_and_persist_governance(
            &governance,
            &RemoteFeishuGatewayTickInput {
                governance: input,
                governance_store_path: store_path.clone(),
            },
        )
        .expect("tick");
    let records = RemoteFeishuGovernanceStoreRuntime::read_records(&store_path).expect("records");

    // 终结性拒绝无需等待 dispatch，落盘即生效。
    assert_eq!(report.persisted_records, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].decision_status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist
    );
    let _ = std::fs::remove_file(store_path);
}

#[test]
fn gateway_reports_noop_turn_release_for_unknown_chat() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    gateway
        .enqueue(
            RemoteFeishuGatewayInboundEvent::Message(message("req-1", "oc_1")),
            1,
        )
        .expect("enqueue");
    let first = gateway.drain_ready();

    let release = gateway
        .complete_chat_turn("oc_unknown")
        .expect("release unknown turn");

    assert_eq!(first.active_chat_turns, 1);
    assert!(!release.released);
    assert_eq!(release.active_chat_turns, 1);
    assert_eq!(gateway.active_chat_turn_count(), 1);
}

#[test]
fn gateway_rate_limit_blocks_chat_after_budget() {
    let mut gateway = RemoteFeishuGatewayRuntime::default();
    gateway.connect();
    let mut limiter = crate::RemoteFeishuRateLimiter::new(1_000, 1, 16).expect("limiter");

    gateway
        .enqueue_with_rate_limit(
            RemoteFeishuGatewayInboundEvent::Message(message("req-1", "oc_1")),
            100,
            &mut limiter,
        )
        .expect("first event");
    let blocked = gateway.enqueue_with_rate_limit(
        RemoteFeishuGatewayInboundEvent::Message(message("req-2", "oc_1")),
        200,
        &mut limiter,
    );

    assert!(blocked.is_err());
    assert_eq!(gateway.pending_len(), 1);
}

fn message(request_id: &str, chat_id: &str) -> RemoteFeishuIncomingMessage {
    RemoteFeishuIncomingMessage {
        session_id: SessionId(1),
        client_id: "feishu-client".to_owned(),
        request_id: request_id.to_owned(),
        message_id: format!("om_{request_id}"),
        chat_id: chat_id.to_owned(),
        chat_type: crate::RemoteFeishuChatType::Group,
        sender_open_id: "ou_user".to_owned(),
        mentions: vec!["ou_bot".to_owned()],
        text: "继续推进".to_owned(),
        media_refs: Vec::new(),
        message_kind: RemoteFeishuIncomingMessageKind::Text,
        reply_context: None,
        session_busy: false,
    }
}

fn card_action(request_id: &str, chat_id: &str) -> RemoteFeishuCardActionEvent {
    RemoteFeishuCardActionEvent {
        session_id: SessionId(1),
        client_id: "feishu-client".to_owned(),
        request_id: request_id.to_owned(),
        message_id: Some("om_gateway_card".to_owned()),
        chat_id: chat_id.to_owned(),
        operator_open_id: "ou_user".to_owned(),
        action_tag: "button".to_owned(),
        action_value_json: "{}".to_owned(),
        reply_context: None,
        session_busy: false,
    }
}

fn governance_input() -> crate::RemoteFeishuGatewayGovernanceInput {
    crate::RemoteFeishuGatewayGovernanceInput {
        governance_config: RemoteFeishuGovernanceConfig {
            allowed_sender_open_ids: Vec::new(),
            blocked_sender_open_ids: Vec::new(),
            require_mention: false,
            bot_open_id: Some("ou_bot".to_owned()),
            chat_policies: Vec::new(),
        },
        pairing_id: "pairing-feishu".to_owned(),
        existing_records: Vec::new(),
    }
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos()
}
