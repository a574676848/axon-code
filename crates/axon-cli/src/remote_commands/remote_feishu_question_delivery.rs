//! 飞书问答卡片的真实投递与结算回写（UQ-303, UQ-305, UQ-306）。
//!
//! 投递链路完全复用既有出站机制：建卡走进度卡片客户端的 interactive 分支（幂等 uuid
//! 加指数退避重试），结算回写走同一条 PATCH 通道。回写失败**不改变结算事实**：结算
//! 已在问答记录里 CAS 落定，这里只把结论降级为 durable outbox 文本通知。

use std::{path::PathBuf, sync::Arc, time::Duration};

use axon_domain::{UserAnswerItem, UserQuestionItem, UserQuestionRecord};
use axon_remote_feishu::{
    RemoteFeishuMessageTarget, RemoteFeishuProgressClient, build_settled_feishu_question_card,
    plan_feishu_question_cards,
};

use super::remote_feishu_daemon_egress::{FeishuEgressInput, persist_egress};
use super::remote_feishu_question_store::{
    FeishuQuestionCardRecord, FeishuQuestionRoute, lookup_card, upsert_card,
};

/// 建卡与回写的单次受限网络超时。
const QUESTION_CARD_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 卡片建卡 dispatch 前缀。
const QUESTION_CARD_DISPATCH_PREFIX: &str = "feishu-question";
/// 降级文本通知的 dispatch 前缀。
const QUESTION_NOTICE_DISPATCH_PREFIX: &str = "feishu-question-notice";
/// 结算回写路径：PATCH 成功。
const SETTLEMENT_VIA_CARD: &str = "card";
/// 结算回写路径：降级为 outbox 文本通知。
const SETTLEMENT_VIA_FALLBACK: &str = "fallback";

/// 一张问答卡片的目标会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeishuQuestionCardTarget {
    /// 目标会话或用户 ID。
    pub(crate) chat_id: String,
    /// 回复目标；优先挂在触发提问的那条消息线程里。
    pub(crate) reply_to_message_id: Option<String>,
}

impl FeishuQuestionCardTarget {
    fn message_target(&self) -> RemoteFeishuMessageTarget {
        RemoteFeishuMessageTarget {
            receive_id: self.chat_id.clone(),
            receive_id_type: if self.chat_id.starts_with("ou_") {
                "open_id".to_owned()
            } else {
                "chat_id".to_owned()
            },
            reply_to_message_id: self.reply_to_message_id.clone(),
        }
    }
}

/// 卡片写入端口。
///
/// 抽象出端口是为了让「回写失败仍完成结算」这条降级路径可被真实测试覆盖：生产实现走
/// 飞书 OpenAPI，测试实现注入失败。
pub(crate) trait FeishuQuestionCardWriter: Send + Sync {
    /// 创建一张问答卡片并返回消息 ID。
    fn create_card(
        &self,
        target: &FeishuQuestionCardTarget,
        card: &serde_json::Value,
        dispatch_id: &str,
    ) -> Result<String, String>;

    /// 用结算卡片整体替换既有卡片。
    fn edit_card(
        &self,
        message_id: &str,
        card: &serde_json::Value,
        dispatch_id: &str,
    ) -> Result<(), String>;

    /// 落一条 durable outbox 文本通知（PATCH 失败时的降级路径）。
    fn notify_text(
        &self,
        target: &FeishuQuestionCardTarget,
        body: &str,
        dispatch_id: &str,
    ) -> Result<(), String>;
}

/// 生产实现：飞书 OpenAPI 卡片客户端加 durable outbox 降级。
pub(crate) struct FeishuHttpQuestionCardWriter {
    axon_root: PathBuf,
    client: RemoteFeishuProgressClient,
}

impl FeishuHttpQuestionCardWriter {
    /// 从远端上下文建立卡片客户端；未启用发送或缺少凭据时返回 None。
    pub(crate) fn connect(
        context: &super::RemoteCliContext,
    ) -> Option<Arc<dyn FeishuQuestionCardWriter>> {
        let config = &context.feishu_websocket;
        if !config.enabled || !config.enable_notification_sending {
            return None;
        }
        let app_secret = context.feishu_app_secret()?;
        let api_base_url =
            (!config.api_base_url.trim().is_empty()).then_some(config.api_base_url.as_str());
        match RemoteFeishuProgressClient::connect(
            &config.app_id,
            &app_secret,
            api_base_url,
            QUESTION_CARD_REQUEST_TIMEOUT,
        ) {
            Ok(client) => Some(Arc::new(Self {
                axon_root: context.axon_root.clone(),
                client,
            })),
            Err(error) => {
                axon_logger::warning(&format!(
                    "Feishu 问答卡片客户端建连失败，问答卡片降级: {error:?}"
                ));
                None
            }
        }
    }
}

impl FeishuQuestionCardWriter for FeishuHttpQuestionCardWriter {
    fn create_card(
        &self,
        target: &FeishuQuestionCardTarget,
        card: &serde_json::Value,
        dispatch_id: &str,
    ) -> Result<String, String> {
        self.client
            .create_interactive_card(&target.message_target(), card, dispatch_id)
            .map_err(|error| format!("Feishu 问答建卡失败: {error:?}"))
    }

    fn edit_card(
        &self,
        message_id: &str,
        card: &serde_json::Value,
        dispatch_id: &str,
    ) -> Result<(), String> {
        self.client
            .edit_interactive_card(message_id, card, dispatch_id)
            .map_err(|error| format!("Feishu 问答卡片回写失败: {error:?}"))
    }

    fn notify_text(
        &self,
        target: &FeishuQuestionCardTarget,
        body: &str,
        dispatch_id: &str,
    ) -> Result<(), String> {
        persist_egress(
            &self.axon_root,
            FeishuEgressInput::from_question_notice(
                &target.chat_id,
                body,
                dispatch_id,
                super::now_ms(),
            ),
        )
    }
}

/// 建卡结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CardDeliveryOutcome {
    /// 全部分片都已创建。
    Delivered,
    /// 已存在卡片记录，本次无需重复投递。
    AlreadyDelivered,
    /// 提问超出飞书卡片硬限，已降级为文本通知。
    DegradedToNotice,
}

/// 结算回写结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CardSettlementOutcome {
    /// PATCH 成功，卡片进入已结算态。
    Patched,
    /// PATCH 失败，结算结论已降级为 durable outbox 文本通知。
    FallbackNotified,
    /// 该提问没有对应卡片（从未投递过），无需回写。
    NoCard,
    /// 该卡片已结算过，本次为重复结算。
    AlreadySettled,
}

/// 投递一张待决问答卡片。
///
/// 分片按问题粒度生成，每片使用稳定 dispatch（同一分片重复创建命中同一 uuid），
/// 因此重试与重启都不会产生第二张卡片。
pub(crate) fn deliver_question_card(
    writer: &dyn FeishuQuestionCardWriter,
    store_root: &std::path::Path,
    record: &UserQuestionRecord,
    questions: &[UserQuestionItem],
    route: &FeishuQuestionRoute,
) -> Result<CardDeliveryOutcome, String> {
    let target = FeishuQuestionCardTarget {
        chat_id: route.chat_id.clone(),
        reply_to_message_id: route.reply_to_message_id.clone(),
    };
    if let Some(existing) = lookup_card(store_root, &record.tool_call_id)?
        && (existing.failure.is_some() || !existing.message_ids.is_empty())
    {
        return Ok(CardDeliveryOutcome::AlreadyDelivered);
    }

    let cards = match plan_feishu_question_cards(
        &record.tool_call_id,
        questions,
        record.handoff_text.as_deref(),
    ) {
        Ok(cards) => cards,
        Err(reason) => {
            // 硬限无法满足时绝不静默丢卡：把可读原因作为文本通知落 durable outbox。
            let body = format!("模型提问无法在飞书卡片中呈现：{reason}");
            writer.notify_text(&target, &body, &notice_dispatch_id(&record.tool_call_id))?;
            upsert_card(
                store_root,
                FeishuQuestionCardRecord {
                    call_id: record.tool_call_id.clone(),
                    question_id: record.question_id.clone(),
                    lane_id: record.lane_id.clone(),
                    chat_id: route.chat_id.clone(),
                    message_ids: Vec::new(),
                    settled: false,
                    settlement: None,
                    failure: Some(reason),
                    updated_at_ms: super::now_ms(),
                },
            )?;
            return Ok(CardDeliveryOutcome::DegradedToNotice);
        }
    };

    let mut message_ids = Vec::with_capacity(cards.len());
    for (index, card) in cards.iter().enumerate() {
        let dispatch_id = card_dispatch_id(&record.tool_call_id, index);
        let message_id = writer.create_card(&target, card, &dispatch_id)?;
        message_ids.push(message_id);
    }
    upsert_card(
        store_root,
        FeishuQuestionCardRecord {
            call_id: record.tool_call_id.clone(),
            question_id: record.question_id.clone(),
            lane_id: record.lane_id.clone(),
            chat_id: route.chat_id.clone(),
            message_ids,
            settled: false,
            settlement: None,
            failure: None,
            updated_at_ms: super::now_ms(),
        },
    )?;
    Ok(CardDeliveryOutcome::Delivered)
}

/// 结算回写：把卡片置为「已由 X 提交确认」并移除按钮。
///
/// 返回 `Ok` 只表示**回写路径**已尽力完成（PATCH 成功或已降级为文本通知），
/// 与答案是否结算无关；结算事实由问答记录 CAS 独立保证。
pub(crate) fn settle_question_card(
    writer: &dyn FeishuQuestionCardWriter,
    store_root: &std::path::Path,
    call_id: &str,
    question_id: &str,
    operator_label: &str,
    answers: &[UserAnswerItem],
    fallback_chat_id: Option<&str>,
    fallback_card_message_id: Option<&str>,
) -> Result<CardSettlementOutcome, String> {
    let Some(mut record) = lookup_card(store_root, call_id)? else {
        // 卡片可能由点击事件直接带上消息 ID（映射表尚未落盘）；此时仍要作废该卡片。
        if let (Some(chat_id), Some(message_id)) = (fallback_chat_id, fallback_card_message_id) {
            return settle_card_messages(
                writer,
                store_root,
                FeishuQuestionCardRecord {
                    call_id: call_id.to_owned(),
                    question_id: question_id.to_owned(),
                    lane_id: String::new(),
                    chat_id: chat_id.to_owned(),
                    message_ids: vec![message_id.to_owned()],
                    settled: false,
                    settlement: None,
                    failure: None,
                    updated_at_ms: super::now_ms(),
                },
                operator_label,
                answers,
            );
        }
        return Ok(CardSettlementOutcome::NoCard);
    };
    if record.settled {
        return Ok(CardSettlementOutcome::AlreadySettled);
    }
    if record.message_ids.is_empty()
        && let Some(message_id) = fallback_card_message_id
        && !message_id.trim().is_empty()
    {
        record.message_ids.push(message_id.to_owned());
    }
    settle_card_messages(writer, store_root, record, operator_label, answers)
}

fn settle_card_messages(
    writer: &dyn FeishuQuestionCardWriter,
    store_root: &std::path::Path,
    mut record: FeishuQuestionCardRecord,
    operator_label: &str,
    answers: &[UserAnswerItem],
) -> Result<CardSettlementOutcome, String> {
    let target = FeishuQuestionCardTarget {
        chat_id: record.chat_id.clone(),
        reply_to_message_id: None,
    };
    let settled_card = build_settled_feishu_question_card(&record.call_id, operator_label, answers);
    let dispatch_id = settlement_dispatch_id(&record.call_id);
    let mut first_error = None;
    for message_id in &record.message_ids {
        if let Err(error) = writer.edit_card(message_id, &settled_card, &dispatch_id)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    let outcome = match (record.message_ids.is_empty(), first_error) {
        (true, _) => {
            // 没有卡片消息可回写：仍要留下可读结论，避免结算事实只存在于数据库。
            writer.notify_text(
                &target,
                &settlement_notice(operator_label, answers),
                &notice_dispatch_id(&record.call_id),
            )?;
            record.settlement = Some(SETTLEMENT_VIA_FALLBACK.to_owned());
            CardSettlementOutcome::FallbackNotified
        }
        (false, None) => {
            record.settlement = Some(SETTLEMENT_VIA_CARD.to_owned());
            CardSettlementOutcome::Patched
        }
        (false, Some(error)) => {
            axon_logger::warning(&format!(
                "Feishu 问答卡片回写失败，已降级为 outbox 文本通知: call_id={} error={error}",
                record.call_id
            ));
            writer.notify_text(
                &target,
                &settlement_notice(operator_label, answers),
                &notice_dispatch_id(&record.call_id),
            )?;
            record.settlement = Some(SETTLEMENT_VIA_FALLBACK.to_owned());
            CardSettlementOutcome::FallbackNotified
        }
    };
    record.settled = true;
    record.updated_at_ms = super::now_ms();
    upsert_card(store_root, record)?;
    Ok(outcome)
}

/// 结算降级通知正文：与卡片文案保持同一事实，不额外发明结论。
fn settlement_notice(operator_label: &str, answers: &[UserAnswerItem]) -> String {
    let mut lines = vec![format!("问答已结算：已由 {operator_label} 提交确认。")];
    for answer in answers {
        let text = answer.custom.clone().unwrap_or_else(|| {
            if answer.selected.is_empty() {
                "（已跳过）".to_owned()
            } else {
                answer.selected.join("、")
            }
        });
        lines.push(format!("{}：{}", answer.id, text));
    }
    lines.join("\n")
}

/// 分片建卡 dispatch：稳定、可重现，飞书按 uuid 幂等去重。
fn card_dispatch_id(call_id: &str, index: usize) -> String {
    if index == 0 {
        format!("{QUESTION_CARD_DISPATCH_PREFIX}-{call_id}")
    } else {
        format!("{QUESTION_CARD_DISPATCH_PREFIX}-{call_id}-{index}")
    }
}

/// 结算回写 dispatch：同一结算重复回写命中同一 uuid。
fn settlement_dispatch_id(call_id: &str) -> String {
    format!("{QUESTION_CARD_DISPATCH_PREFIX}-settled-{call_id}")
}

/// 降级文本通知 dispatch：稳定，重复兜底只落一条 outbox 记录。
pub(crate) fn notice_dispatch_id(call_id: &str) -> String {
    format!("{QUESTION_NOTICE_DISPATCH_PREFIX}-{call_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingWriter {
        created: Mutex<Vec<String>>,
        edited: Mutex<Vec<String>>,
        notified: Mutex<Vec<String>>,
        fail_edit: bool,
    }

    impl FeishuQuestionCardWriter for RecordingWriter {
        fn create_card(
            &self,
            _target: &FeishuQuestionCardTarget,
            _card: &serde_json::Value,
            dispatch_id: &str,
        ) -> Result<String, String> {
            self.created.lock().unwrap().push(dispatch_id.to_owned());
            Ok(format!("om_{}", self.created.lock().unwrap().len()))
        }

        fn edit_card(
            &self,
            message_id: &str,
            _card: &serde_json::Value,
            _dispatch_id: &str,
        ) -> Result<(), String> {
            if self.fail_edit {
                return Err("Feishu 问答卡片回写失败: 网络不可达".to_owned());
            }
            self.edited.lock().unwrap().push(message_id.to_owned());
            Ok(())
        }

        fn notify_text(
            &self,
            _target: &FeishuQuestionCardTarget,
            body: &str,
            _dispatch_id: &str,
        ) -> Result<(), String> {
            self.notified.lock().unwrap().push(body.to_owned());
            Ok(())
        }
    }

    fn record(call_id: &str) -> UserQuestionRecord {
        UserQuestionRecord {
            question_id: format!("uq-{call_id}"),
            idempotency_key: format!("idem-{call_id}"),
            lane_id: "session-lane-1".to_owned(),
            session_id: "1".to_owned(),
            interaction_id: "1".to_owned(),
            turn_id: "turn-1".to_owned(),
            tool_call_id: call_id.to_owned(),
            questions_json: r#"[{"id":"q1","question":"确认?","options":[{"label":"是"}]}]"#
                .to_owned(),
            handoff_text: None,
            state: axon_domain::UserQuestionState::Pending,
            deadline_at_ms: 10_000,
            answer_json: None,
            answered_at_ms: None,
            answered_channel: None,
            delivered: false,
            delivery_mode: None,
            created_at_ms: 1_000,
            expires_at_ms: 86_400_000,
        }
    }

    fn questions() -> Vec<UserQuestionItem> {
        serde_json::from_str(r#"[{"id":"q1","question":"确认?","options":[{"label":"是"}]}]"#)
            .expect("问题解析")
    }

    fn route() -> FeishuQuestionRoute {
        FeishuQuestionRoute {
            lane_id: "session-lane-1".to_owned(),
            chat_id: "oc_1".to_owned(),
            chat_type: "p2p".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            reply_to_message_id: Some("om_trigger".to_owned()),
            recorded_at_ms: 1,
        }
    }

    #[test]
    fn delivery_records_message_ids_and_is_idempotent() {
        let root = axon_test_support::test_temp_root();
        let writer = RecordingWriter::default();

        let first =
            deliver_question_card(&writer, &root, &record("call-1"), &questions(), &route())
                .expect("首次投递");
        let second =
            deliver_question_card(&writer, &root, &record("call-1"), &questions(), &route())
                .expect("重复投递");

        assert_eq!(first, CardDeliveryOutcome::Delivered);
        assert_eq!(second, CardDeliveryOutcome::AlreadyDelivered);
        assert_eq!(writer.created.lock().unwrap().len(), 1, "不得重复建卡");
        let stored = lookup_card(&root, "call-1").expect("查询").expect("存在");
        assert_eq!(stored.message_ids, vec!["om_1".to_owned()]);
        assert_eq!(stored.question_id, "uq-call-1");
    }

    #[test]
    fn oversized_question_degrades_to_durable_notice() {
        let root = axon_test_support::test_temp_root();
        let writer = RecordingWriter::default();
        let huge: Vec<UserQuestionItem> = serde_json::from_str(&format!(
            r#"[{{"id":"q1","question":"{}","options":[{{"label":"是"}}]}}]"#,
            "超".repeat(9_000)
        ))
        .expect("问题解析");

        let outcome = deliver_question_card(&writer, &root, &record("call-huge"), &huge, &route())
            .expect("降级投递");

        assert_eq!(outcome, CardDeliveryOutcome::DegradedToNotice);
        assert_eq!(writer.created.lock().unwrap().len(), 0);
        assert_eq!(writer.notified.lock().unwrap().len(), 1);
        let stored = lookup_card(&root, "call-huge")
            .expect("查询")
            .expect("存在");
        assert!(stored.failure.is_some(), "失败原因必须可查");
    }

    #[test]
    fn settlement_patch_marks_card_settled() {
        let root = axon_test_support::test_temp_root();
        let writer = RecordingWriter::default();
        deliver_question_card(&writer, &root, &record("call-1"), &questions(), &route())
            .expect("投递");

        let outcome = settle_question_card(
            &writer,
            &root,
            "call-1",
            "uq-call-1",
            "ou_operator",
            &[UserAnswerItem {
                id: "q1".to_owned(),
                selected: vec!["是".to_owned()],
                custom: None,
            }],
            Some("oc_1"),
            None,
        )
        .expect("结算回写");

        assert_eq!(outcome, CardSettlementOutcome::Patched);
        assert_eq!(writer.edited.lock().unwrap().len(), 1);
        let stored = lookup_card(&root, "call-1").expect("查询").expect("存在");
        assert!(stored.settled);
        assert_eq!(stored.settlement.as_deref(), Some("card"));
    }

    #[test]
    fn settlement_patch_failure_falls_back_without_losing_the_fact() {
        let root = axon_test_support::test_temp_root();
        let writer = RecordingWriter {
            fail_edit: true,
            ..RecordingWriter::default()
        };
        deliver_question_card(&writer, &root, &record("call-1"), &questions(), &route())
            .expect("投递");

        let outcome = settle_question_card(
            &writer,
            &root,
            "call-1",
            "uq-call-1",
            "ou_operator",
            &[UserAnswerItem {
                id: "q1".to_owned(),
                selected: vec!["是".to_owned()],
                custom: None,
            }],
            Some("oc_1"),
            None,
        )
        .expect("回写失败必须降级而不是报错");

        assert_eq!(outcome, CardSettlementOutcome::FallbackNotified);
        assert_eq!(writer.notified.lock().unwrap().len(), 1, "必须落文本兜底");
        let stored = lookup_card(&root, "call-1").expect("查询").expect("存在");
        assert!(stored.settled, "结算事实不得依赖回写成功");
        assert_eq!(stored.settlement.as_deref(), Some("fallback"));
    }

    #[test]
    fn settlement_uses_clicked_card_message_when_mapping_is_missing() {
        let root = axon_test_support::test_temp_root();
        let writer = RecordingWriter::default();

        let outcome = settle_question_card(
            &writer,
            &root,
            "call-unknown",
            "uq-call-unknown",
            "ou_operator",
            &[],
            Some("oc_1"),
            Some("om_clicked"),
        )
        .expect("作废被点击的卡片");

        assert_eq!(outcome, CardSettlementOutcome::Patched);
        assert_eq!(
            writer.edited.lock().unwrap().as_slice(),
            &["om_clicked".to_owned()]
        );
    }

    #[test]
    fn settlement_without_card_reports_no_card() {
        let root = axon_test_support::test_temp_root();
        let writer = RecordingWriter::default();

        let outcome =
            settle_question_card(&writer, &root, "call-none", "uq", "ou", &[], None, None)
                .expect("无卡片");

        assert_eq!(outcome, CardSettlementOutcome::NoCard);
        assert!(writer.notified.lock().unwrap().is_empty());
    }
}
