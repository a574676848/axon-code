use axon_domain::{RemoteCapabilityPolicy, SessionId};

use super::{
    REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED, REMOTE_FEISHU_GOVERNANCE_REASON_BOT_NOT_MENTIONED,
    RemoteFeishuCardActionEvent, RemoteFeishuChatGovernancePolicy, RemoteFeishuChatType,
    RemoteFeishuGovernanceConfig, RemoteFeishuGovernanceDecisionStatus,
    RemoteFeishuGovernanceRuntime, RemoteFeishuGovernanceStoreRuntime, RemoteFeishuIncomingMessage,
    RemoteFeishuIncomingMessageKind, RemoteFeishuThreadState,
};

fn message(
    request_id: &str,
    sender_open_id: &str,
    session_busy: bool,
) -> RemoteFeishuIncomingMessage {
    RemoteFeishuIncomingMessage {
        session_id: SessionId(7),
        client_id: "feishu-client".to_owned(),
        request_id: request_id.to_owned(),
        message_id: format!("om_{request_id}"),
        chat_id: "oc_7".to_owned(),
        chat_type: RemoteFeishuChatType::Group,
        sender_open_id: sender_open_id.to_owned(),
        mentions: vec!["ou_bot".to_owned()],
        text: "继续推进飞书远控".to_owned(),
        media_refs: Vec::new(),
        message_kind: RemoteFeishuIncomingMessageKind::Text,
        reply_context: None,
        session_busy,
    }
}

fn card_action(request_id: &str, operator_open_id: &str) -> RemoteFeishuCardActionEvent {
    RemoteFeishuCardActionEvent {
        session_id: SessionId(7),
        client_id: "feishu-client".to_owned(),
        request_id: request_id.to_owned(),
        message_id: Some("om_governance_card".to_owned()),
        chat_id: "oc_7".to_owned(),
        operator_open_id: operator_open_id.to_owned(),
        action_tag: "button".to_owned(),
        action_value_json: r#"{"axon_action":"clarify"}"#.to_owned(),
        reply_context: None,
        session_busy: true,
    }
}

fn config() -> RemoteFeishuGovernanceConfig {
    RemoteFeishuGovernanceConfig {
        allowed_sender_open_ids: vec![],
        blocked_sender_open_ids: Vec::new(),
        require_mention: false,
        bot_open_id: Some("ou_bot".to_owned()),
        chat_policies: Vec::new(),
    }
}

#[test]
fn governance_rejects_sender_outside_allowlist() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec!["ou_allowed".to_owned()],
                blocked_sender_open_ids: Vec::new(),
                require_mention: false,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: Vec::new(),
            },
            &message("req-1", "ou_blocked", false),
            "pairing-feishu",
            1_000,
            &[],
        )
        .expect("governance decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist
    );
    assert!(decision.dispatch_report.is_none());
}

#[test]
fn governance_rejects_sender_inside_blacklist_before_allowlist() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec!["ou_blocked".to_owned()],
                blocked_sender_open_ids: vec!["ou_blocked".to_owned()],
                require_mention: false,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: Vec::new(),
            },
            &message("req-blacklist", "ou_blocked", false),
            "pairing-feishu",
            1_500,
            &[],
        )
        .expect("governance decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByBlacklist
    );
    assert!(decision.dispatch_report.is_none());
}

#[test]
fn governance_drops_duplicate_request_id_from_persisted_records() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let accepted = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec![],
                blocked_sender_open_ids: Vec::new(),
                require_mention: true,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: Vec::new(),
            },
            &message("req-1", "ou_allowed", false),
            "pairing-feishu",
            100,
            &[],
        )
        .expect("accepted");
    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec![],
                blocked_sender_open_ids: Vec::new(),
                require_mention: true,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: Vec::new(),
            },
            &message("req-1", "ou_allowed", false),
            "pairing-feishu",
            2_000,
            &[accepted.record],
        )
        .expect("governance decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::DuplicateDropped
    );
    assert!(decision.dispatch_report.is_none());
}

#[test]
fn governance_accepts_message_and_marks_processing_or_queue() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let accepted = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec![],
                blocked_sender_open_ids: Vec::new(),
                require_mention: true,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: Vec::new(),
            },
            &message("req-2", "ou_allowed", false),
            "pairing-feishu",
            2_000,
            &[],
        )
        .expect("accepted");
    let queued = runtime
        .handle_message(
            &config(),
            &message("req-3", "ou_allowed", true),
            "pairing-feishu",
            3_000,
            &[],
        )
        .expect("queued");

    assert_eq!(accepted.thread_state, RemoteFeishuThreadState::Processing);
    assert!(accepted.dispatch_report.is_some());
    assert_eq!(accepted.record.message_kind.as_deref(), Some("text"));
    assert_eq!(queued.thread_state, RemoteFeishuThreadState::Queued);
}

#[test]
fn governance_latest_threads_fold_by_chat() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let first = runtime
        .handle_message(
            &config(),
            &message("req-1", "ou_a", false),
            "pairing-feishu",
            100,
            &[],
        )
        .expect("first")
        .record;
    let second = runtime
        .handle_message(
            &config(),
            &message("req-2", "ou_a", true),
            "pairing-feishu",
            200,
            &[],
        )
        .expect("second")
        .record;
    let latest = RemoteFeishuGovernanceStoreRuntime::latest_threads(&[first, second]);

    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].thread_state, RemoteFeishuThreadState::Queued);
}

#[test]
fn governance_rejects_message_when_bot_not_mentioned() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let mut event = message("req-4", "ou_allowed", false);
    event.mentions.clear();
    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec![],
                blocked_sender_open_ids: Vec::new(),
                require_mention: true,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: Vec::new(),
            },
            &event,
            "pairing-feishu",
            4_000,
            &[],
        )
        .expect("decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByMentionGate
    );
    assert_eq!(
        decision.reason,
        REMOTE_FEISHU_GOVERNANCE_REASON_BOT_NOT_MENTIONED
    );
}

#[test]
fn governance_accepts_p2p_message_without_bot_mention() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let mut event = message("req-p2p", "ou_allowed", false);
    event.chat_type = RemoteFeishuChatType::P2p;
    event.mentions.clear();

    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                require_mention: true,
                ..config()
            },
            &event,
            "pairing-feishu",
            4_100,
            &[],
        )
        .expect("p2p decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::Accepted
    );
    assert!(decision.dispatch_report.is_some());
}

#[test]
fn governance_rejects_unknown_chat_without_bot_mention() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let mut event = message("req-unknown", "ou_allowed", false);
    event.chat_type = RemoteFeishuChatType::Unknown;
    event.mentions.clear();

    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                require_mention: true,
                ..config()
            },
            &event,
            "pairing-feishu",
            4_200,
            &[],
        )
        .expect("unknown chat decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByMentionGate
    );
}

#[test]
fn governance_applies_per_chat_allowlist_override() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let mut event = message("req-chat-policy", "ou_chat_allowed", false);
    event.chat_id = "oc_special".to_owned();
    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec!["ou_global_only".to_owned()],
                blocked_sender_open_ids: Vec::new(),
                require_mention: false,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: vec![RemoteFeishuChatGovernancePolicy {
                    chat_id: "oc_special".to_owned(),
                    allowed_sender_open_ids: vec!["ou_chat_allowed".to_owned()],
                    blocked_sender_open_ids: Vec::new(),
                    require_mention: None,
                }],
            },
            &event,
            "pairing-feishu",
            4_500,
            &[],
        )
        .expect("decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::Accepted
    );
    assert!(decision.dispatch_report.is_some());
}

#[test]
fn governance_applies_per_chat_mention_gate_override() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let mut event = message("req-chat-mention", "ou_allowed", false);
    event.chat_id = "oc_require_mention".to_owned();
    event.mentions.clear();
    let decision = runtime
        .handle_message(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: Vec::new(),
                blocked_sender_open_ids: Vec::new(),
                require_mention: false,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: vec![RemoteFeishuChatGovernancePolicy {
                    chat_id: "oc_require_mention".to_owned(),
                    allowed_sender_open_ids: Vec::new(),
                    blocked_sender_open_ids: Vec::new(),
                    require_mention: Some(true),
                }],
            },
            &event,
            "pairing-feishu",
            4_600,
            &[],
        )
        .expect("decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByMentionGate
    );
}

#[test]
fn governance_ignores_self_message_event() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let decision = runtime
        .handle_message(
            &config(),
            &message("req-5", "ou_bot", false),
            "pairing-feishu",
            5_000,
            &[],
        )
        .expect("decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::IgnoredSelfEvent
    );
    assert!(decision.dispatch_report.is_none());
}

#[test]
fn governance_card_action_respects_operator_allowlist() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let decision = runtime
        .handle_card_action(
            &RemoteFeishuGovernanceConfig {
                allowed_sender_open_ids: vec!["ou_allowed".to_owned()],
                blocked_sender_open_ids: Vec::new(),
                require_mention: false,
                bot_open_id: Some("ou_bot".to_owned()),
                chat_policies: Vec::new(),
            },
            &card_action("card-1", "ou_blocked"),
            "pairing-feishu",
            6_000,
            &[],
        )
        .expect("decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist
    );
    assert!(decision.dispatch_report.is_none());
}

#[test]
fn governance_card_action_records_operator_and_chat_identity() {
    let runtime = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let decision = runtime
        .handle_card_action(
            &config(),
            &card_action("card-2", "ou_operator"),
            "pairing-feishu",
            7_000,
            &[],
        )
        .expect("decision");

    assert_eq!(
        decision.status,
        RemoteFeishuGovernanceDecisionStatus::Accepted
    );
    assert_eq!(decision.reason, REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED);
    assert!(decision.dispatch_report.is_some());
    assert_eq!(decision.thread_state, RemoteFeishuThreadState::Queued);
    assert!(decision.record.queued);
    assert_eq!(decision.record.message_kind, None);
}
