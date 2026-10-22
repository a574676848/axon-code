//! 飞书问答动作分发的行为测试：载荷解析、跨操作人身份、投递语义与结算回写降级。

use std::sync::{Arc, Mutex};

use axon_domain::{UserAnswerItem, UserQuestionRecord, UserQuestionState};
use axon_remote_feishu::RemoteFeishuChatType;
use axon_session::{SqliteRuntimeStore, UserQuestionStore};

use crate::remote_commands::remote_feishu_question_delivery::FeishuQuestionCardTarget;
use crate::user_question::{
    NoopUserQuestionEventSink, QuestionMessageSteerer, UserQuestionEventSink,
};

use super::*;

/// 记录注入内容的测试 steerer；`fail` 为真时模拟 Daemon 不可达。
#[derive(Default)]
struct TestSteerer {
    steered: Mutex<Vec<String>>,
    fail: bool,
}

impl QuestionMessageSteerer for TestSteerer {
    fn steer_answer_message(
        &self,
        _lane_id: &str,
        _session_id: &str,
        _idempotency_key: &str,
        content: &str,
    ) -> Result<(), String> {
        if self.fail {
            return Err("Daemon IPC 不可达".to_owned());
        }
        self.steered.lock().unwrap().push(content.to_owned());
        Ok(())
    }
}

/// 记录事件的测试 sink。
#[derive(Default)]
struct TestSink {
    statuses: Mutex<Vec<String>>,
}

impl UserQuestionEventSink for TestSink {
    fn emit_question_event(
        &self,
        _session_id: &str,
        _part_id: &str,
        _call_id: &str,
        status: &str,
        _questions: &[axon_domain::UserQuestionItem],
        _answers: &[UserAnswerItem],
        _handoff: Option<&str>,
        _deadline_at_ms: i64,
    ) {
        self.statuses.lock().unwrap().push(status.to_owned());
    }
}

fn seed_question(root: &std::path::Path, call_id: &str, lane_id: &str) -> SqliteRuntimeStore {
    let store = SqliteRuntimeStore::from_root(root.join(axon_session::AXON_RUNTIME_DATA_DIR));
    let _ = axon_session::RuntimeStore::initialize(&store);
    let record = UserQuestionRecord {
        question_id: format!("uq-{call_id}"),
        idempotency_key: format!("idem-{call_id}"),
        lane_id: lane_id.to_owned(),
        session_id: "1".to_owned(),
        interaction_id: "1".to_owned(),
        turn_id: "turn-1".to_owned(),
        tool_call_id: call_id.to_owned(),
        questions_json:
            r#"[{"id":"q1","question":"确认？","options":[{"label":"同意"}],"multi_select":false}]"#
                .to_owned(),
        handoff_text: None,
        state: UserQuestionState::Pending,
        deadline_at_ms: 10_000,
        answer_json: None,
        answered_at_ms: None,
        answered_channel: None,
        delivered: false,
        delivery_mode: None,
        created_at_ms: 1_000,
        expires_at_ms: 86_400_000,
    };
    let _ = store.insert_question(&record);
    store
}

fn answer_payload(call_id: &str, qid: &str, option: &str) -> String {
    serde_json::json!({
        "axon_action": "answer_question",
        "call_id": call_id,
        "qid": qid,
        "opt": option,
        "skip": option.is_empty(),
    })
    .to_string()
}

#[test]
fn parses_answer_payload_and_rejects_other_actions() {
    let parsed = parse_feishu_question_answer(&answer_payload("call-1", "q1", "同意"))
        .expect("问答动作应被解析");
    assert_eq!(parsed.call_id, "call-1");
    assert_eq!(parsed.qid, "q1");
    assert_eq!(parsed.to_answer().selected, vec!["同意".to_owned()]);

    let skip = parse_feishu_question_answer(&answer_payload("call-1", "q1", ""))
        .expect("跳过按钮也是问答动作");
    assert!(skip.skip || skip.to_answer().selected.is_empty());

    assert!(parse_feishu_question_answer(r#"{"axon_action":"other"}"#).is_none());
    assert!(parse_feishu_question_answer(r#"{"action":"clarify"}"#).is_none());
    assert!(parse_feishu_question_answer("not-json").is_none());
}

/// 载荷必须来自 `action_value_json`：网关把按钮载荷写进事件源的该字段，
/// 卡片动作的 `text` 恒为空，任何依赖 `text` 的实现都拿不到答案。
#[test]
fn action_value_payload_reaches_the_answer_branch() {
    let temp = axon_test_support::test_temp_root();
    seed_question(&temp, "call-fs-1", "session-lane-1");
    let steerer = Arc::new(TestSteerer::default());

    let outcome = handle_feishu_question_answer_with_adapters(
        &temp,
        &answer_payload("call-fs-1", "q1", "同意"),
        "ou_operator",
        Arc::new(TestSink::default()),
        steerer.clone(),
        None,
    )
    .expect("问答动作必须进入答案分支")
    .expect("结算应成功");

    assert!(outcome.contains("已由 ou_operator 提交问答结算"));
    let steered = steerer.steered.lock().unwrap().clone();
    assert_eq!(steered.len(), 1, "答案必须成为一条用户消息");
    assert!(steered[0].contains("[问答回执]"));
    assert!(steered[0].contains("回答待决问题：call-fs-1"));
}

/// G：去重键必须含操作人身份，因此两个人点同一按钮是两条独立有效事件。
#[test]
fn different_operators_produce_two_valid_events() {
    let temp = axon_test_support::test_temp_root();
    seed_question(&temp, "call-fs-2", "session-lane-1");
    let steerer = Arc::new(TestSteerer::default());
    let sink = Arc::new(TestSink::default());
    let payload = answer_payload("call-fs-2", "q1", "同意");

    let first = handle_feishu_question_answer_with_adapters(
        &temp,
        &payload,
        "ou_alice",
        sink.clone(),
        steerer.clone(),
        None,
    )
    .expect("问答动作")
    .expect("第一次结算");
    let second = handle_feishu_question_answer_with_adapters(
        &temp,
        &payload,
        "ou_bob",
        sink,
        steerer.clone(),
        None,
    )
    .expect("问答动作")
    .expect("第二次点击必须被处理而不是当成重复事件丢弃");

    assert!(first.contains("ou_alice"));
    assert!(
        second.contains("ALREADY_SETTLED"),
        "第二个操作人得到可读回执"
    );
    assert_eq!(steerer.steered.lock().unwrap().len(), 1, "答案只注入一次");
}

/// B：答案消息未真正投递时 `delivered` 不得置真，保证对账会补投。
#[test]
fn steer_failure_keeps_answer_undelivered_for_reconciliation() {
    let temp = axon_test_support::test_temp_root();
    seed_question(&temp, "call-fs-3", "session-lane-1");
    let sink = Arc::new(TestSink::default());
    let steerer = Arc::new(TestSteerer {
        fail: true,
        ..TestSteerer::default()
    });

    let outcome = handle_feishu_question_answer_with_adapters(
        &temp,
        &answer_payload("call-fs-3", "q1", "同意"),
        "ou_operator",
        sink.clone(),
        steerer,
        None,
    )
    .expect("问答动作")
    .expect("结算已落定，回执必须可读");

    assert!(outcome.contains(STEER_FAILED_MARKER));
    let store = SqliteRuntimeStore::from_root(temp.join(axon_session::AXON_RUNTIME_DATA_DIR));
    let record = store
        .get_question_by_call_id("call-fs-3")
        .expect("查询")
        .expect("存在");
    assert_eq!(record.state, UserQuestionState::Answered);
    assert!(!record.delivered, "未投递绝不能标记为已投递");
    assert!(
        sink.statuses.lock().unwrap().is_empty(),
        "投递失败时不得发出 settled 事件谎报完成"
    );
    assert_eq!(
        store
            .list_unreconciled_answered(10)
            .expect("对账扫描")
            .len(),
        1
    );
}

/// D：结算回写失败不影响答案结算成功，且卡片收口走降级路径。
#[test]
fn card_write_back_failure_does_not_block_settlement() {
    struct FailingWriter;

    impl FeishuQuestionCardWriter for FailingWriter {
        fn create_card(
            &self,
            _target: &FeishuQuestionCardTarget,
            _card: &serde_json::Value,
            _dispatch_id: &str,
        ) -> Result<String, String> {
            Err("Feishu 问答建卡失败".to_owned())
        }

        fn edit_card(
            &self,
            _message_id: &str,
            _card: &serde_json::Value,
            _dispatch_id: &str,
        ) -> Result<(), String> {
            Err("Feishu 问答卡片回写失败".to_owned())
        }

        fn notify_text(
            &self,
            _target: &FeishuQuestionCardTarget,
            _body: &str,
            _dispatch_id: &str,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    let temp = axon_test_support::test_temp_root();
    seed_question(&temp, "call-fs-4", "session-lane-1");
    let writer = FailingWriter;
    let settlement = FeishuQuestionCardSettlementRequest {
        store_root: &temp,
        writer: &writer,
        chat_id: "oc_1",
        card_message_id: Some("om_clicked"),
    };
    let steerer = Arc::new(TestSteerer::default());

    let outcome = handle_feishu_question_answer_with_adapters(
        &temp,
        &answer_payload("call-fs-4", "q1", "同意"),
        "ou_operator",
        Arc::new(NoopUserQuestionEventSink),
        steerer,
        Some(&settlement),
    )
    .expect("问答动作")
    .expect("回写失败不得让结算失败");

    assert!(outcome.contains("已由 ou_operator 提交问答结算"));
    let store = SqliteRuntimeStore::from_root(temp.join(axon_session::AXON_RUNTIME_DATA_DIR));
    let record = store
        .get_question_by_call_id("call-fs-4")
        .expect("查询")
        .expect("存在");
    assert_eq!(record.state, UserQuestionState::Answered);
    assert!(record.delivered);
}

#[test]
fn audience_matches_chat_type_for_lane_binding() {
    assert_eq!(
        feishu_audience(RemoteFeishuChatType::P2p).kind(),
        axon_domain::AudienceKind::Private
    );
    assert_eq!(
        feishu_audience(RemoteFeishuChatType::Group).kind(),
        axon_domain::AudienceKind::Group
    );
}

#[test]
fn route_binding_round_trips_and_resolves_chat_type() {
    let temp = axon_test_support::test_temp_root();

    let lane_id = record_feishu_question_route(
        &temp,
        "workspace-1",
        "oc_1",
        RemoteFeishuChatType::P2p,
        Some("om_trigger"),
    )
    .expect("写入路由绑定");

    assert!(lane_id.starts_with("session-lane-"));
    let route = lookup_route(&temp, &lane_id).expect("查询").expect("存在");
    assert_eq!(route.chat_id, "oc_1");
    assert_eq!(route.reply_to_message_id.as_deref(), Some("om_trigger"));
    assert_eq!(
        resolve_chat_type(&temp, "oc_1"),
        RemoteFeishuChatType::P2p,
        "受众必须按绑定还原，不能默认成群聊"
    );
    assert_eq!(
        resolve_chat_type(&temp, "oc_missing"),
        RemoteFeishuChatType::Unknown
    );
}
