//! 飞书问答卡片的后台补投与结算收口 tick。
//!
//! 覆盖两类网关进程无法就地感知的问答事实：
//! - 提问在 Daemon 侧的 Agent 回合里产生（网关只持有共享 SQLite 视图）；
//! - 提问在别的渠道（桌面端、TUI）被作答。
//!
//! 两者都必须落到同一张飞书卡片上：前者补建卡片，后者补写结算态。tick 只在网关
//! 后台 worker 内运行，网络写入有硬间隔，绝不阻塞入站主链。

use std::sync::Arc;

use axon_domain::{UserAnswerItem, UserQuestionState};
use axon_session::{SqliteRuntimeStore, UserQuestionStore};

use super::remote_feishu_question_delivery::{
    FeishuHttpQuestionCardWriter, FeishuQuestionCardWriter, deliver_question_card,
    settle_question_card,
};
use super::remote_feishu_question_store::{is_settled, lookup_card, read_cards, read_routes};
use super::{RemoteCliContext, now_ms};

/// 单次 tick 处理的会话绑定上限。
const QUESTION_TICK_LANE_LIMIT: usize = 64;
/// 单次 tick 收口的卡片记录上限。
const QUESTION_TICK_CARD_LIMIT: usize = 64;
/// 单条车道单次扫描的待决提问上限。
const QUESTION_TICK_LANE_QUESTION_LIMIT: usize = 4;

/// 网关 tick 报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FeishuQuestionCardTickReport {
    /// 扫描的会话绑定数。
    pub(crate) inspected_lanes: usize,
    /// 本轮投递的卡片数。
    pub(crate) delivered_cards: usize,
    /// 本轮收口的卡片数。
    pub(crate) settled_cards: usize,
    /// 因提问已结算或卡片已收口而跳过的数量。
    pub(crate) skipped: usize,
}

/// 网关后台问答卡片 tick。
pub(crate) struct FeishuQuestionCardTickRuntime {
    context: RemoteCliContext,
    writer: Option<Arc<dyn FeishuQuestionCardWriter>>,
    interval_ms: u64,
    last_tick_at_ms: u64,
}

impl FeishuQuestionCardTickRuntime {
    /// 构造 tick 运行态；`interval_ms` 是两次真实网络扫描的最小间隔。
    pub(crate) fn new(context: RemoteCliContext, interval_ms: u64) -> Self {
        Self {
            context,
            writer: None,
            interval_ms: interval_ms.max(1),
            last_tick_at_ms: 0,
        }
    }

    /// 按节奏执行一轮 tick；未到间隔时返回 None（调用方无需区分）。
    pub(crate) fn tick(&mut self) -> Option<Result<FeishuQuestionCardTickReport, String>> {
        let now = now_ms();
        if self.last_tick_at_ms != 0 && now.saturating_sub(self.last_tick_at_ms) < self.interval_ms
        {
            return None;
        }
        self.last_tick_at_ms = now;
        Some(self.run_tick())
    }

    fn run_tick(&mut self) -> Result<FeishuQuestionCardTickReport, String> {
        let config = &self.context.feishu_websocket;
        if !config.enabled || !config.enable_notification_sending {
            return Ok(FeishuQuestionCardTickReport::default());
        }
        if self.writer.is_none() {
            // 客户端只在首个真实 tick 建立一次：tenant access token 不能每轮重取。
            self.writer = FeishuHttpQuestionCardWriter::connect(&self.context);
        }
        let Some(writer) = self.writer.clone() else {
            return Err("Feishu 问答卡片通道不可用".to_owned());
        };
        let store = SqliteRuntimeStore::from_root(
            self.context
                .axon_root
                .join(axon_session::AXON_RUNTIME_DATA_DIR),
        );
        let mut report = FeishuQuestionCardTickReport::default();
        self.deliver_pending_cards(&store, writer.as_ref(), &mut report)?;
        self.settle_finished_cards(&store, writer.as_ref(), &mut report)?;
        Ok(report)
    }

    /// 为已登记车道上尚无卡片的待决提问补建卡片。
    fn deliver_pending_cards(
        &self,
        store: &SqliteRuntimeStore,
        writer: &dyn FeishuQuestionCardWriter,
        report: &mut FeishuQuestionCardTickReport,
    ) -> Result<(), String> {
        let store_root = self.context.root.as_path();
        for route in read_routes(store_root)?
            .into_iter()
            .take(QUESTION_TICK_LANE_LIMIT)
        {
            report.inspected_lanes += 1;
            for state in [UserQuestionState::Pending, UserQuestionState::Suspended] {
                let questions = store
                    .list_questions_by_lane(
                        &route.lane_id,
                        Some(state),
                        QUESTION_TICK_LANE_QUESTION_LIMIT,
                    )
                    .map_err(|error| format!("读取飞书车道待决提问失败: {error}"))?;
                for record in questions {
                    if lookup_card(store_root, &record.tool_call_id)?.is_some() {
                        report.skipped += 1;
                        continue;
                    }
                    let parsed: Vec<axon_domain::UserQuestionItem> =
                        serde_json::from_str(&record.questions_json).unwrap_or_default();
                    if parsed.is_empty() {
                        report.skipped += 1;
                        continue;
                    }
                    match deliver_question_card(writer, store_root, &record, &parsed, &route) {
                        Ok(_) => report.delivered_cards += 1,
                        Err(error) => axon_logger::warning(&format!(
                            "Feishu 问答卡片后台投递失败，下轮重试: call_id={} error={error}",
                            record.tool_call_id
                        )),
                    }
                }
            }
        }
        Ok(())
    }

    /// 收口已在任何渠道结算的提问卡片。
    fn settle_finished_cards(
        &self,
        store: &SqliteRuntimeStore,
        writer: &dyn FeishuQuestionCardWriter,
        report: &mut FeishuQuestionCardTickReport,
    ) -> Result<(), String> {
        let store_root = self.context.root.as_path();
        for card in read_cards(store_root)?
            .into_iter()
            .take(QUESTION_TICK_CARD_LIMIT)
        {
            if is_settled(&card) {
                continue;
            }
            let Some(record) = store
                .get_question_by_call_id(&card.call_id)
                .map_err(|error| format!("读取飞书问答记录失败: {error}"))?
            else {
                continue;
            };
            let Some((label, answers)) = settlement_payload(&record) else {
                report.skipped += 1;
                continue;
            };
            match settle_question_card(
                writer,
                store_root,
                &card.call_id,
                &card.question_id,
                &label,
                &answers,
                Some(card.chat_id.as_str()),
                None,
            ) {
                Ok(_) => report.settled_cards += 1,
                Err(error) => axon_logger::warning(&format!(
                    "Feishu 问答卡片后台结算回写失败，下轮重试: call_id={} error={error}",
                    card.call_id
                )),
            }
        }
        Ok(())
    }
}

/// 终态提问的结算文案与答案；非终态返回 None。
fn settlement_payload(
    record: &axon_domain::UserQuestionRecord,
) -> Option<(String, Vec<UserAnswerItem>)> {
    match record.state {
        UserQuestionState::Answered => Some((
            record
                .answered_channel
                .clone()
                .unwrap_or_else(|| "其他渠道".to_owned()),
            record
                .answer_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Vec<UserAnswerItem>>(raw).ok())
                .unwrap_or_default(),
        )),
        // 取消/过期同样要让卡片失效：留着可点击的按钮会让用户以为还能作答。
        UserQuestionState::Cancelled | UserQuestionState::Expired => {
            Some(("系统".to_owned(), Vec::new()))
        }
        _ => None,
    }
}
