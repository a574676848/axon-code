//! 用户答案的生产消息注入：把答案正文投进目标会话车道（UQ-006）。
//!
//! 沿用既有 [`PrimaryTurnIngress`] 的 `route_input` 语义：车道上有活跃主 turn 时把
//! 答案作为一条用户输入并入该回合（steer），否则落成一条新的 queued Interaction。
//! 两种路径共用同一个稳定幂等键，重复投递被去重。
//!
//! 落成 queued Interaction 的那条路径必须显式唤醒一次主 turn 领取：迟到作答的续跑
//! 不能取决于维护定时器的轮询周期，否则唤醒会退化成「等下一次兜底」。

use std::sync::Arc;

use axon_daemon::{ActiveTurnSteerSink, InteractionDelivery, PrimaryTurnIngress};
use axon_domain::{
    AudienceContext, InteractionId, InteractionRecord, InteractionSource, ReplyRoute,
    SYSTEM_SOURCE_CHANNEL, ScopeRef,
};
use axon_session::{LaneQueryStore, LaneSummaryRow, SqliteRuntimeStore};

use super::answer_service::QuestionMessageSteerer;
use crate::run_commands::run_steer::ActivePrimaryTurnRegistry;

/// 答案 Interaction 的稳定来源 ID 前缀。
const ANSWER_SOURCE_PREFIX: &str = "question-answer-source:";

/// 答案落成 durable Interaction 后的主 turn 唤醒端口。
///
/// 返回 `Err` 只用于留痕：唤醒是提示而不是事实源，durable Interaction 仍在仓库里，
/// 下一次领取照样会消费它，答案不会因为唤醒失败而丢。
pub(crate) type PrimaryTurnWakeHook = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// 主 turn 的活跃回合注入端口：按会话车道亲和键把输入并入正在运行的那一轮。
#[derive(Clone)]
pub(crate) struct ActiveLaneTurnSteerSink {
    active_primary_turns: ActivePrimaryTurnRegistry,
}

impl ActiveLaneTurnSteerSink {
    /// 以活跃主 turn 注册表构造。
    pub(crate) fn new(active_primary_turns: ActivePrimaryTurnRegistry) -> Self {
        Self {
            active_primary_turns,
        }
    }
}

impl ActiveTurnSteerSink for ActiveLaneTurnSteerSink {
    fn try_steer(&self, interaction: &InteractionRecord) -> Result<bool, String> {
        let lane_id = interaction_lane_id(interaction)?;
        self.active_primary_turns
            .try_send(
                &ActivePrimaryTurnRegistry::affinity_key(&lane_id),
                interaction.content.clone(),
            )
            .map(|result| result.is_some())
    }

    fn append_turn_input(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<Option<InteractionId>, String> {
        if interaction.source.channel() == SYSTEM_SOURCE_CHANNEL {
            return Ok(None);
        }
        let lane_id = interaction_lane_id(interaction)?;
        self.active_primary_turns.try_send(
            &ActivePrimaryTurnRegistry::affinity_key(&lane_id),
            interaction.content.clone(),
        )
    }
}

fn interaction_lane_id(interaction: &InteractionRecord) -> Result<String, String> {
    axon_session::interaction_session_lane_id(interaction)
        .ok_or_else(|| "问答答案注入必须绑定 Workspace".to_owned())
}

/// 生产答案消息注入器。
///
/// 答案记录只带车道身份，因此这里按车道行回读它自己的作用域与回复路由：
/// 重建出来的 Interaction 必然解析回同一条车道，注入不会落到别的会话表面上。
pub(crate) struct LaneQuestionMessageSteerer {
    store: SqliteRuntimeStore,
    steer_sink: ActiveLaneTurnSteerSink,
    lease_duration_ms: u64,
    /// 迟到作答落库后的主 turn 唤醒端口。
    wake: PrimaryTurnWakeHook,
}

impl LaneQuestionMessageSteerer {
    /// 以运行态存储、活跃主 turn 注册表、动作租约时长与唤醒端口构造。
    pub(crate) fn new(
        store: SqliteRuntimeStore,
        active_primary_turns: ActivePrimaryTurnRegistry,
        lease_duration_ms: u64,
        wake: PrimaryTurnWakeHook,
    ) -> Self {
        Self {
            store,
            steer_sink: ActiveLaneTurnSteerSink::new(active_primary_turns),
            lease_duration_ms,
            wake,
        }
    }

    /// 唤醒一次主 turn 领取；失败只留痕，不影响答案已落库这一事实。
    fn wake_claim(&self, lane_id: &str) {
        match (self.wake)() {
            Ok(()) => axon_logger::debug(&format!(
                "用户答案已落库，已唤醒主 turn 领取: lane={lane_id}"
            )),
            Err(error) => axon_logger::warning(&format!(
                "用户答案已落库但唤醒主 turn 失败，等待下一次领取: lane={lane_id} error={error}"
            )),
        }
    }

    /// 按车道身份回读该车道的作用域、受众与回复路由。
    fn lane_route(&self, lane_id: &str) -> Result<(ScopeRef, AudienceContext, ReplyRoute), String> {
        let rows = LaneQueryStore::list_lane_summaries(&self.store)
            .map_err(|error| format!("读取车道路由失败: {error:?}"))?;
        let row: &LaneSummaryRow = rows
            .iter()
            .find(|row| row.lane_id == lane_id)
            .ok_or_else(|| format!("车道 {lane_id} 尚未登记任何 Interaction，无法投递答案"))?;
        let scope: ScopeRef = serde_json::from_str(&row.scope_json)
            .map_err(|error| format!("车道作用域解析失败: {error}"))?;
        let audience: AudienceContext = serde_json::from_str(&row.audience_json)
            .map_err(|error| format!("车道受众解析失败: {error}"))?;
        let route: ReplyRoute = serde_json::from_str(&row.reply_route_json)
            .map_err(|error| format!("车道回复路由解析失败: {error}"))?;
        Ok((scope, audience, route))
    }
}

impl QuestionMessageSteerer for LaneQuestionMessageSteerer {
    fn steer_answer_message(
        &self,
        lane_id: &str,
        session_id: &str,
        idempotency_key: &str,
        content: &str,
    ) -> Result<(), String> {
        let (scope, audience, route) = self.lane_route(lane_id)?;
        let interaction_id = InteractionId::new(idempotency_key.to_string())
            .map_err(|error| format!("答案 Interaction ID 非法: {error}"))?;
        let source = InteractionSource::new(
            route.channel(),
            format!("{ANSWER_SOURCE_PREFIX}{idempotency_key}"),
        )
        .map_err(|error| format!("答案 Interaction 来源非法: {error}"))?;
        let now_ms = crate::tool_orchestrator::now_ms();
        let interaction = InteractionRecord::new(
            interaction_id,
            source,
            scope,
            audience,
            route,
            content,
            now_ms,
        )
        .map_err(|error| format!("答案 Interaction 构造失败: {error}"))?;
        let ingress = PrimaryTurnIngress::new(
            self.store.clone(),
            self.steer_sink.clone(),
            now_ms,
            self.lease_duration_ms,
        );
        let receipt = ingress
            .route_input(&interaction)
            .map_err(|error| format!("答案消息入站失败: {error:?}"))?;
        if let Some(error) = receipt.steer_error {
            return Err(format!("答案消息立即注入失败: {error}"));
        }
        // 车道上有活跃主 turn 时答案已并入该回合（Steered），不得再开新轮；没有活跃
        // 主 turn 时答案落成 durable queued Interaction，必须立即唤醒一次领取。重复
        // 投递由答案 Interaction 的稳定幂等键去重：重复投递只命中同一条记录，不会被
        // 领取成第二轮。
        if receipt.delivery == InteractionDelivery::Queued {
            self.wake_claim(lane_id);
        }
        // 运行会话身份只用于留痕：注入路由完全由车道决定。
        axon_logger::debug(&format!(
            "用户答案已投递: lane={lane_id} session={session_id} delivery={:?}",
            receipt.delivery
        ));
        Ok(())
    }
}
