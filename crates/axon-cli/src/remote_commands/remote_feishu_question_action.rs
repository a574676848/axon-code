//! 飞书卡片问答动作入站分发与结算（UQ-302, UQ-305, UQ-306）。
//!
//! 本模块是飞书渠道问答的闭环入口：
//! - 解析按钮载荷（`answer_question`）并把答案交给统一答案服务做 CAS 结算；
//! - 用真实 [`QuestionMessageSteerer`] 把答案作为一条用户消息提交给 Daemon，
//!   投递失败时答案保持 `delivered = 0`，由对账补投；
//! - 用真实 [`UserQuestionEventSink`] 完成卡片投递与结算回写，回写失败降级为
//!   durable outbox 文本通知。
//!
//! 跨渠道作答与进程重启后的补投由 `remote_feishu_question_card_tick.rs` 收口。

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef, UserAnswerItem, UserQuestionState,
};
use axon_remote_feishu::RemoteFeishuChatType;
use axon_session::{SqliteRuntimeStore, UserQuestionStore, session_lane_id};

use crate::user_question::{
    AnswerError, QuestionMessageSteerer, UserQuestionAnswerService, UserQuestionEventSink,
};

use super::remote_feishu_question_delivery::{
    FeishuQuestionCardWriter, deliver_question_card, settle_question_card,
};
use super::remote_feishu_question_store::{
    FeishuQuestionRoute, lookup_route, read_routes, upsert_route,
};
use super::{RemoteCliContext, now_ms};

/// 飞书渠道标识；与入站 Interaction 的 `source.channel()` 保持同一契约。
pub(crate) const FEISHU_CHANNEL: &str = "feishu";
/// 问答动作的 `axon_action` 取值。
const QUESTION_ANSWER_ACTION: &str = "answer_question";
/// 答案消息注入失败但结算已落定时的回执标记。
const STEER_FAILED_MARKER: &str = "ANSWER_SETTLED_STEER_FAILED";

/// 解析后的飞书问答作答动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeishuQuestionAnswerAction {
    /// 待决问题标识（工具调用 ID）。
    pub(crate) call_id: String,
    /// 问题 ID。
    pub(crate) qid: String,
    /// 选中项标签；跳过为空。
    pub(crate) option: String,
    /// 是否为跳过按钮。
    pub(crate) skip: bool,
}

impl FeishuQuestionAnswerAction {
    /// 投影为统一答案条目。
    pub(crate) fn to_answer(&self) -> UserAnswerItem {
        UserAnswerItem {
            id: self.qid.clone(),
            selected: if self.skip || self.option.is_empty() {
                Vec::new()
            } else {
                vec![self.option.clone()]
            },
            custom: None,
        }
    }
}

/// 解析按钮载荷；非问答动作返回 None。
pub(crate) fn parse_feishu_question_answer(
    payload_json: &str,
) -> Option<FeishuQuestionAnswerAction> {
    let parsed: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    if parsed.get("axon_action")?.as_str()? != QUESTION_ANSWER_ACTION {
        return None;
    }
    let call_id = parsed.get("call_id")?.as_str()?.trim().to_owned();
    let qid = parsed.get("qid")?.as_str()?.trim().to_owned();
    if call_id.is_empty() || qid.is_empty() {
        return None;
    }
    Some(FeishuQuestionAnswerAction {
        call_id,
        qid,
        option: parsed
            .get("opt")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        skip: parsed
            .get("skip")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// 飞书会话受众：群聊可见 Workspace 内容，私聊只允许私密披露。
pub(crate) fn feishu_audience(chat_type: RemoteFeishuChatType) -> AudienceContext {
    match chat_type {
        RemoteFeishuChatType::Group => {
            AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace)
        }
        RemoteFeishuChatType::P2p | RemoteFeishuChatType::Unknown => {
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly)
        }
    }
}

/// 记录 `lane_id -> chat` 绑定，使问答卡片能找回用户所在会话。
///
/// 复合车道 ID 与入站消息完全同源（Workspace + 受众 + channel + destination），因此
/// 网关按消息写入、答案侧按提问记录反查时得到同一把钥匙。
pub(crate) fn record_feishu_question_route(
    store_root: &Path,
    workspace_id: &str,
    chat_id: &str,
    chat_type: RemoteFeishuChatType,
    reply_to_message_id: Option<&str>,
) -> Result<String, String> {
    let lane_id = session_lane_id(
        workspace_id,
        &feishu_audience(chat_type),
        FEISHU_CHANNEL,
        chat_id,
    );
    upsert_route(
        store_root,
        FeishuQuestionRoute {
            lane_id: lane_id.clone(),
            chat_id: chat_id.to_owned(),
            chat_type: chat_type.as_str().to_owned(),
            workspace_id: workspace_id.to_owned(),
            reply_to_message_id: reply_to_message_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            recorded_at_ms: now_ms(),
        },
    )?;
    Ok(lane_id)
}

/// 按会话反查受众类型；未登记时返回 Unknown。
pub(crate) fn resolve_chat_type(store_root: &Path, chat_id: &str) -> RemoteFeishuChatType {
    let routes = read_routes(store_root).unwrap_or_default();
    if let Some(route) = routes.into_iter().find(|route| route.chat_id == chat_id)
        && let Some(chat_type) =
            RemoteFeishuChatType::from_stable_value(Some(route.chat_type.as_str()))
    {
        return chat_type;
    }
    RemoteFeishuChatType::Unknown
}

/// 由提问记录的车道反解受众类型。
///
/// 复合车道 ID 是 (Workspace, 受众, channel, destination) 的哈希，无法直接反演；但候选
/// 受众只有私聊与群聊两种，逐个重算并与提问车道比对即可得到**精确**结论。这比任何
/// 「默认私聊」的猜测都可靠：受众错了，答案就会落到另一条会话车道上。
pub(crate) fn resolve_answer_chat_type(
    context: &RemoteCliContext,
    call_id: &str,
    chat_id: &str,
) -> RemoteFeishuChatType {
    let store =
        SqliteRuntimeStore::from_root(context.axon_root.join(axon_session::AXON_RUNTIME_DATA_DIR));
    let Ok(Some(record)) = store.get_question_by_call_id(call_id) else {
        return resolve_chat_type(&context.root, chat_id);
    };
    let Ok(workspace) = axon_session::WorkspaceResolver.resolve(&context.axon_root, now_ms())
    else {
        return resolve_chat_type(&context.root, chat_id);
    };
    for chat_type in [RemoteFeishuChatType::P2p, RemoteFeishuChatType::Group] {
        let lane_id = session_lane_id(
            &workspace.workspace_id,
            &feishu_audience(chat_type),
            FEISHU_CHANNEL,
            chat_id,
        );
        if lane_id == record.lane_id {
            return chat_type;
        }
    }
    resolve_chat_type(&context.root, chat_id)
}

/// 答案消息的 Daemon IPC 注入器。
///
/// 跨进程无法命中活等待者，因此答案必须以**一条用户消息**的形式回到提问所在复合车道：
/// 提交一条稳定 ID 的 Interaction，Daemon 侧按既有 durable ingress 与 active steer
/// 处理，重复提交由 Interaction ID 去重。
pub(crate) struct FeishuQuestionAnswerSteerer {
    context: RemoteCliContext,
    endpoint: String,
    chat_id: String,
    audience: AudienceContext,
}

impl FeishuQuestionAnswerSteerer {
    /// 构造注入器。
    pub(crate) fn new(
        context: RemoteCliContext,
        endpoint: &str,
        chat_id: &str,
        chat_type: RemoteFeishuChatType,
    ) -> Self {
        Self {
            context,
            endpoint: endpoint.to_owned(),
            chat_id: chat_id.to_owned(),
            audience: feishu_audience(chat_type),
        }
    }
}

impl QuestionMessageSteerer for FeishuQuestionAnswerSteerer {
    fn steer_answer_message(
        &self,
        lane_id: &str,
        _session_id: &str,
        idempotency_key: &str,
        content: &str,
    ) -> Result<(), String> {
        if content.trim().is_empty() {
            return Err("飞书问答答案消息正文为空".to_owned());
        }
        let received_at_ms = now_ms();
        let workspace = crate::run_commands::register_workspace(
            &self.context.axon_root,
            received_at_ms,
            Some(&self.context.config_path),
        )?;
        let expected_lane = session_lane_id(
            &workspace.workspace_id,
            &self.audience,
            FEISHU_CHANNEL,
            &self.chat_id,
        );
        if !lane_id.is_empty() && lane_id != expected_lane {
            // 车道不一致不是拒绝理由：答案必须送达用户提问的那一轮，但必须留下可查告警。
            axon_logger::warning(&format!(
                "Feishu 问答答案车道与提问车道不一致，仍按会话车道投递: expected={expected_lane}, question_lane={lane_id}"
            ));
        }
        let digest = axon_crypto::sha256_hex(idempotency_key);
        let interaction = InteractionRecord::new(
            InteractionId::new(format!("interaction-feishu-answer-{digest}"))
                .map_err(|error| format!("飞书问答答案交互 ID 无效: {error}"))?,
            InteractionSource::new(FEISHU_CHANNEL, format!("question-answer:{idempotency_key}"))
                .map_err(|error| format!("飞书问答答案来源无效: {error}"))?,
            ScopeRef::workspace(&workspace.workspace_id)
                .map_err(|error| format!("飞书问答答案作用域无效: {error}"))?,
            self.audience.clone(),
            ReplyRoute::new(FEISHU_CHANNEL, &self.chat_id)
                .map_err(|error| format!("飞书问答答案回复路由无效: {error}"))?,
            content,
            received_at_ms,
        )
        .map_err(|error| format!("飞书问答答案 Interaction 构造失败: {error}"))?;
        super::remote_feishu_gateway::remote_feishu_gateway_execution::remote_feishu_control::submit_gateway_interaction(
            &self.context,
            &self.endpoint,
            &interaction,
        )
    }
}

/// 卡片事件 sink：把问答状态变化落到真实飞书卡片。
pub(crate) struct FeishuQuestionCardSink {
    axon_root: PathBuf,
    store_root: PathBuf,
    writer: Option<Arc<dyn FeishuQuestionCardWriter>>,
    /// 回答者显示文案；卡片动作入口已知，其他入口从结算渠道推导。
    operator_label: Option<String>,
    /// 已知目标会话；缺失时从路由表按车道反查。
    chat_id: Option<String>,
    /// 已知被点击卡片的消息 ID；缺失时从卡片表反查。
    card_message_id: Option<String>,
}

impl FeishuQuestionCardSink {
    /// 构造 sink。
    pub(crate) fn new(
        context: &RemoteCliContext,
        writer: Option<Arc<dyn FeishuQuestionCardWriter>>,
    ) -> Self {
        Self {
            axon_root: context.axon_root.clone(),
            store_root: context.root.clone(),
            writer,
            operator_label: None,
            chat_id: None,
            card_message_id: None,
        }
    }

    /// 绑定已知的回答者、目标会话与被点击卡片。
    pub(crate) fn with_known_target(
        mut self,
        operator_label: Option<&str>,
        chat_id: &str,
        card_message_id: Option<&str>,
    ) -> Self {
        self.operator_label = operator_label.map(str::to_owned);
        self.chat_id = Some(chat_id.to_owned());
        self.card_message_id = card_message_id.map(str::to_owned);
        self
    }

    fn store(&self) -> SqliteRuntimeStore {
        SqliteRuntimeStore::from_root(self.axon_root.join(axon_session::AXON_RUNTIME_DATA_DIR))
    }
}

impl UserQuestionEventSink for FeishuQuestionCardSink {
    fn emit_question_event(
        &self,
        _session_id: &str,
        _part_id: &str,
        call_id: &str,
        status: &str,
        questions: &[axon_domain::UserQuestionItem],
        answers: &[UserAnswerItem],
        _handoff: Option<&str>,
        _deadline_at_ms: i64,
    ) {
        let Some(writer) = self.writer.as_deref() else {
            if status == "pending" {
                axon_logger::warning(&format!(
                    "Feishu 问答卡片通道不可用，提问未投递: call_id={call_id}"
                ));
            }
            return;
        };
        let Ok(Some(record)) = self.store().get_question_by_call_id(call_id) else {
            axon_logger::warning(&format!(
                "Feishu 问答事件缺少提问记录: call_id={call_id}, status={status}"
            ));
            return;
        };
        match status {
            "pending" => {
                let Some(route) = self.route_for(&record.lane_id) else {
                    // 非飞书来源的提问没有飞书会话绑定，静默跳过是正确的。
                    return;
                };
                if let Err(error) =
                    deliver_question_card(writer, &self.store_root, &record, questions, &route)
                {
                    axon_logger::warning(&format!(
                        "Feishu 问答卡片投递失败，将在网关 tick 重试: call_id={call_id} error={error}"
                    ));
                }
            }
            "settled" => {
                let label = self.operator_label.as_deref().unwrap_or("其他渠道");
                if let Err(error) = settle_question_card(
                    writer,
                    &self.store_root,
                    call_id,
                    &record.question_id,
                    label,
                    answers,
                    self.chat_id.as_deref(),
                    self.card_message_id.as_deref(),
                ) {
                    axon_logger::warning(&format!(
                        "Feishu 问答卡片结算回写失败: call_id={call_id} error={error}"
                    ));
                }
            }
            _ => {}
        }
    }
}

impl FeishuQuestionCardSink {
    /// 按车道反查会话绑定。
    fn route_for(&self, lane_id: &str) -> Option<FeishuQuestionRoute> {
        lookup_route(&self.store_root, lane_id).ok().flatten()
    }
}

/// 结算请求上下文：注入失败时仍需作废卡片的显式入口。
pub(crate) struct FeishuQuestionCardSettlementRequest<'a> {
    /// 飞书运行时根目录。
    pub(crate) store_root: &'a Path,
    /// 卡片写入端口。
    pub(crate) writer: &'a dyn FeishuQuestionCardWriter,
    /// 已知目标会话。
    pub(crate) chat_id: &'a str,
    /// 已知被点击卡片消息 ID。
    pub(crate) card_message_id: Option<&'a str>,
}

/// 问答作答处理核心：可注入 sink 与 steerer，便于对投递语义做真实断言。
pub(crate) fn handle_feishu_question_answer_with_adapters(
    axon_root: &Path,
    payload_json: &str,
    operator_open_id: &str,
    sink: Arc<dyn UserQuestionEventSink>,
    steerer: Arc<dyn QuestionMessageSteerer>,
    settlement: Option<&FeishuQuestionCardSettlementRequest<'_>>,
) -> Option<Result<String, String>> {
    let action = parse_feishu_question_answer(payload_json)?;
    let store = Arc::new(SqliteRuntimeStore::from_root(
        axon_root.join(axon_session::AXON_RUNTIME_DATA_DIR),
    ));
    let service = UserQuestionAnswerService::new(store.clone(), None, sink, steerer);

    match service.submit_answer(
        "",
        &action.call_id,
        vec![action.to_answer()],
        FEISHU_CHANNEL,
    ) {
        Ok(record) => Some(Ok(format!(
            "已由 {operator_open_id} 提交问答结算: call_id={}, state={}",
            action.call_id,
            record.state.as_str()
        ))),
        Err(AnswerError::AlreadySettled(_)) => {
            // UQ-306 多端竞争：他人已作答时飞书侧收到 ALREADY_SETTLED 并可读呈现。
            settle_card_if_present(
                &store,
                &action.call_id,
                operator_open_id,
                &action,
                settlement,
            );
            Some(Ok(format!(
                "提问 {} 已在别处完成结算，本次点击无需重复处理 (ALREADY_SETTLED)",
                action.call_id
            )))
        }
        Err(error) => {
            // 答案注入失败不影响结算事实：CAS 已落定且 delivered 保持 0，由对账补投。
            let settled = question_is_answered(&store, &action.call_id);
            if settled {
                settle_card_if_present(
                    &store,
                    &action.call_id,
                    operator_open_id,
                    &action,
                    settlement,
                );
                axon_logger::warning(&format!(
                    "Feishu 问答答案注入失败，答案已结算等待对账补投: call_id={}, error={error}",
                    action.call_id
                ));
                return Some(Ok(format!(
                    "已由 {operator_open_id} 提交问答结算，答案消息投递失败将由对账补投 ({STEER_FAILED_MARKER}): {error}"
                )));
            }
            Some(Err(format!("飞书问答结算失败: {error}")))
        }
    }
}

fn question_is_answered(store: &SqliteRuntimeStore, call_id: &str) -> bool {
    store
        .get_question_by_call_id(call_id)
        .ok()
        .flatten()
        .is_some_and(|record| record.state == UserQuestionState::Answered)
}

fn settle_card_if_present(
    store: &SqliteRuntimeStore,
    call_id: &str,
    operator_label: &str,
    action: &FeishuQuestionAnswerAction,
    settlement: Option<&FeishuQuestionCardSettlementRequest<'_>>,
) {
    let Some(settlement) = settlement else {
        return;
    };
    let Ok(Some(record)) = store.get_question_by_call_id(call_id) else {
        return;
    };
    let answers = record
        .answer_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Vec<UserAnswerItem>>(raw).ok())
        .unwrap_or_else(|| vec![action.to_answer()]);
    if let Err(error) = settle_question_card(
        settlement.writer,
        settlement.store_root,
        call_id,
        &record.question_id,
        operator_label,
        &answers,
        Some(settlement.chat_id),
        settlement.card_message_id,
    ) {
        axon_logger::warning(&format!(
            "Feishu 问答卡片结算回写失败，结算事实不受影响: call_id={call_id} error={error}"
        ));
    }
}

/// 生产入口：处理飞书卡片问答按钮动作。
///
/// 非问答动作返回 None，调用方按既有卡片动作处理；问答动作返回可读回执，
/// 并在同一调用内完成卡片结算回写。
pub(crate) fn try_handle_feishu_question_action(
    context: &RemoteCliContext,
    endpoint: &str,
    payload_json: &str,
    operator_open_id: &str,
    chat_id: &str,
    card_message_id: Option<&str>,
    workspace_id: Option<&str>,
) -> Option<Result<String, String>> {
    let action = parse_feishu_question_answer(payload_json)?;
    let chat_type = resolve_answer_chat_type(context, &action.call_id, chat_id);
    if chat_type == RemoteFeishuChatType::Unknown {
        axon_logger::warning(&format!(
            "Feishu 问答动作无法还原会话受众，按私聊受众投递答案: chat_id={chat_id}"
        ));
    }
    if let Some(workspace_id) = workspace_id {
        // 卡片动作同样补写路由绑定：进程重启后提问卡片仍要能找回会话。
        if let Err(error) =
            record_feishu_question_route(&context.root, workspace_id, chat_id, chat_type, None)
        {
            axon_logger::warning(&format!("Feishu 问答路由绑定写入失败: {error}"));
        }
    }
    let writer =
        super::remote_feishu_question_delivery::FeishuHttpQuestionCardWriter::connect(context);
    let sink = FeishuQuestionCardSink::new(context, writer.clone()).with_known_target(
        Some(operator_open_id),
        chat_id,
        card_message_id,
    );
    let steerer = FeishuQuestionAnswerSteerer::new(context.clone(), endpoint, chat_id, chat_type);
    let settlement = writer
        .as_ref()
        .map(|writer| FeishuQuestionCardSettlementRequest {
            store_root: &context.root,
            writer: writer.as_ref(),
            chat_id,
            card_message_id,
        });
    handle_feishu_question_answer_with_adapters(
        &context.axon_root,
        payload_json,
        operator_open_id,
        Arc::new(sink),
        Arc::new(steerer),
        settlement.as_ref(),
    )
}

#[cfg(test)]
#[path = "remote_feishu_question_action_tests.rs"]
mod tests;
