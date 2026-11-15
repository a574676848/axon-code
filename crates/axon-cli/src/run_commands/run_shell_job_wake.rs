//! 临时后台 Shell Job 跨 Turn 唤醒回注桥。
//!
//! 后台 Shell 作业在原回合结束前完成时，通过原回合的 `RunSteerInbox` 实时注回；
//! 若原回合已结束（`Inactive`），则把非父回合取消导致的 Shell 作业终态包装为
//! 独立的 `shell-job-wake:` Interaction 投递回原会话车道，并唤醒主 turn 领取执行。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axon_agent_runtime::{
    AgentRuntimeNoteKind, CompletionNotice, CompletionNoticeBatch, CompletionNoticeSubscription,
    JOB_REASON_CANCELLED, JobProducerKind, JobRuntime, JobTerminalKind, RunCancelToken,
    subscribe_completion_notices,
};
use axon_crypto::sha256_hex_prefix;
use axon_daemon::{InteractionDelivery, PrimaryTurnIngress};
use axon_domain::{
    AudienceContext, InteractionId, InteractionRecord, InteractionSource, ReplyRoute,
    SHELL_JOB_WAKE_SOURCE_PREFIX, SYSTEM_SOURCE_CHANNEL, ScopeRef,
};
use axon_session::{LaneQueryStore, LaneSummaryRow, SqliteRuntimeStore};

use super::run_execution::now_ms;
use super::run_steer::{
    ActivePrimaryTurnRegistry, RunSteerSendError, try_send_deferred_runtime_note,
};
use crate::user_question::lane_steerer::{ActiveLaneTurnSteerSink, PrimaryTurnWakeHook};

/// 跨 Turn 唤醒 Interaction ID 的前缀。
const SHELL_JOB_WAKE_INTERACTION_PREFIX: &str = "shell-job-wake-";
/// 唤醒标识哈希前缀长度（96 位十六进制前缀，确保不超过 Interaction ID 长度上限）。
const SHELL_JOB_WAKE_HASH_CHARS: usize = 24;
/// 唤醒路由在最后一次活跃之后保留的时长。
///
/// 后台 Shell 作业最长 30 分钟无输出即被判 `Unresponsive` 收口，路由必须活得比作业
/// 更久，跨 Turn 终态才送得回去；超过这个窗口的 owner 既不会再有作业、也不会再有
/// 通知，继续留在表里只会让路由表与通知订阅无界增长。
const SHELL_JOB_WAKE_ROUTE_TTL_MS: u64 = 45 * 60 * 1_000;
/// 同时保留的唤醒路由上限，超出后按最近活跃时间淘汰最旧者。
const SHELL_JOB_WAKE_ROUTE_CAPACITY: usize = 256;

/// 单个 `run_session_id` 在原回合结束后的跨 Turn 唤醒路由上下文。
#[derive(Clone)]
pub(crate) struct WakeRouteInfo {
    pub(crate) lane_id: String,
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) turn_cancel_token: RunCancelToken,
}

struct WakeRouteEntry {
    route: WakeRouteInfo,
    /// 最后一次登记或命中通知的墙钟毫秒，用于回收与容量淘汰。
    last_seen_ms: u64,
    _subscription: CompletionNoticeSubscription,
}

/// 进程内临时后台 Shell Job 的跨 Turn 唤醒桥。
///
/// 由 Daemon 组合根持有并在整个进程生命周期内复用：每个活跃的 `run_session_id`
/// 只建立唯一一份持久订阅，不随单次 Provider turn 结束而退订，因此原回合完成后
/// 才收口的后台 Shell 作业依然能唤醒同一车道上的下一轮响应。
#[derive(Clone)]
pub(crate) struct ShellJobWakeBridge {
    store: SqliteRuntimeStore,
    active_primary_turns: ActivePrimaryTurnRegistry,
    lease_duration_ms: u64,
    wake: PrimaryTurnWakeHook,
    routes: Arc<Mutex<HashMap<String, WakeRouteEntry>>>,
}

impl ShellJobWakeBridge {
    /// 创建跨 Turn 唤醒桥。
    #[must_use]
    pub(crate) fn new(
        store: SqliteRuntimeStore,
        active_primary_turns: ActivePrimaryTurnRegistry,
        lease_duration_ms: u64,
        wake: PrimaryTurnWakeHook,
    ) -> Self {
        Self {
            store,
            active_primary_turns,
            lease_duration_ms,
            wake,
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 为指定 `owner_id`（即 `run_session_id`）登记或刷新跨 Turn 唤醒路由，并确保只持有一份订阅。
    ///
    /// 每次登记都会顺带回收过期与超额路由：路由持有通知订阅，而订阅句柄只有在路由被
    /// 移除时才会随 `Drop` 退订，不设上界就等于让通知总线上的订阅随运行时长单调增长。
    pub(crate) fn register_turn(&self, owner_id: &str, route: WakeRouteInfo) {
        let trimmed_owner = owner_id.trim();
        if trimmed_owner.is_empty() || route.lane_id.trim().is_empty() {
            return;
        }
        let now = now_ms();
        let mut pending_route = Some(route);
        {
            let mut routes = self
                .routes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = routes.get_mut(trimmed_owner) {
                entry.route = pending_route.take().expect("刚校验过待安装路由");
                entry.last_seen_ms = now;
            }
        }
        let Some(route) = pending_route else {
            self.retain_live_routes(now);
            return;
        };
        let bridge = self.clone();
        let owner_key = trimmed_owner.to_owned();
        let subscription = subscribe_completion_notices(
            trimmed_owner,
            Arc::new(move |batch| bridge.handle_batch(&owner_key, batch)),
        );
        let mut routes = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::trim_routes(&mut routes, now);
        if let Some(entry) = routes.get_mut(trimmed_owner) {
            // 并发注册同一 owner 时保留先建的订阅，多出的新订阅在锁外释放。
            entry.route = route;
            entry.last_seen_ms = now;
            drop(routes);
            drop(subscription);
            return;
        }
        routes.insert(
            trimmed_owner.to_owned(),
            WakeRouteEntry {
                route,
                last_seen_ms: now,
                _subscription: subscription,
            },
        );
    }

    /// 原回合收口后按需注销：本 owner 名下已无未终态作业时摘除路由与订阅。
    ///
    /// 作业只在所属回合内提交，回合结束且作业全部收口后，这个 `run_session_id` 不会再
    /// 产生完成通知，订阅留着纯占内存。仍在后台跑的作业会保留路由，跨 Turn 终态照常唤醒。
    pub(crate) fn release_idle_turn(&self, owner_id: &str) {
        let trimmed_owner = owner_id.trim();
        if trimmed_owner.is_empty()
            || !JobRuntime::global()
                .pending_job_ids(trimmed_owner)
                .is_empty()
        {
            return;
        }
        let removed = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(trimmed_owner);
        // 锁外释放：`Drop` 会回到通知总线上做退订。
        drop(removed);
    }

    /// 按 TTL 回收过期路由。
    fn retain_live_routes(&self, now: u64) {
        let mut routes = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::trim_routes(&mut routes, now);
    }

    /// 在已持锁的路由表上做 TTL 回收与容量淘汰。
    ///
    /// 被移除的条目由调用方在锁外释放，随 `Drop` 退订，因此这里不能持有通知总线的锁。
    fn trim_routes(routes: &mut HashMap<String, WakeRouteEntry>, now: u64) {
        routes.retain(|_, entry| {
            now.saturating_sub(entry.last_seen_ms) < SHELL_JOB_WAKE_ROUTE_TTL_MS
        });
        if routes.len() < SHELL_JOB_WAKE_ROUTE_CAPACITY {
            return;
        }
        let mut oldest: Vec<(String, u64)> = routes
            .iter()
            .map(|(owner, entry)| (owner.clone(), entry.last_seen_ms))
            .collect();
        oldest.sort_by_key(|(_, last_seen_ms)| *last_seen_ms);
        let overflow = routes.len() + 1 - SHELL_JOB_WAKE_ROUTE_CAPACITY;
        for (owner, _) in oldest.into_iter().take(overflow) {
            routes.remove(&owner);
        }
    }

    /// 处理一次完成通知批次：活跃回合内实时注回，原回合已结束则跨 Turn 唤醒原车道。
    fn handle_batch(&self, owner_id: &str, batch: CompletionNoticeBatch) {
        let rendered = batch.render();
        match try_send_deferred_runtime_note(
            owner_id,
            AgentRuntimeNoteKind::CompletionNotice,
            rendered,
        ) {
            Ok(()) => return,
            Err(RunSteerSendError::CapacityExceeded) => {
                // 队列满不能只留一行日志：通知已经从总线取走，直接返回等于丢件。
                // 改走跨 Turn 唤醒，由 durable Interaction 兜底送达。
                axon_logger::warning(&format!(
                    "完成通知实时注入队列已满，改走跨 Turn 唤醒: session={owner_id}"
                ));
            }
            Err(RunSteerSendError::Inactive) => {}
        }

        let route_info = {
            let mut routes = self
                .routes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            routes.get_mut(owner_id).map(|entry| {
                // 通知到达即视为活跃，避免正在等唤醒的 owner 被 TTL 当冷路由回收。
                entry.last_seen_ms = now_ms();
                entry.route.clone()
            })
        };
        let Some(route_info) = route_info else {
            return;
        };

        let turn_cancelled = route_info.turn_cancel_token.is_cancelled();
        // 后台 Shell 与后端子代理都要跨回合唤醒：两者都在回合结束前提交、回合结束后
        // 才收口。只唤醒 Shell 时，子代理终态会一直躺在 durable continuation 里，直到
        // 用户下次输入才被装配，而提示词又宣称「跑完会自动通知你」。
        let wake_notices = batch
            .notices
            .into_iter()
            .filter(|notice| {
                matches!(
                    notice.producer,
                    JobProducerKind::Shell | JobProducerKind::Subagent
                ) && !(notice.terminal == JobTerminalKind::Killed
                    && (turn_cancelled || notice.exit_reason == JOB_REASON_CANCELLED))
            })
            .collect::<Vec<_>>();
        if wake_notices.is_empty() {
            return;
        }

        let wake_batch = CompletionNoticeBatch {
            notices: wake_notices,
            merged_count: batch.merged_count,
        };
        if let Err(error) = self.submit_wake_interaction(&route_info, &wake_batch) {
            axon_logger::warning(&format!(
                "后台 Shell 作业跨 Turn 唤醒提交失败: lane={}, owner={owner_id}, error={error}",
                route_info.lane_id
            ));
        }
    }

    fn submit_wake_interaction(
        &self,
        route_info: &WakeRouteInfo,
        batch: &CompletionNoticeBatch,
    ) -> Result<(), String> {
        let Some((scope, audience, reply_route)) = self.lane_route(&route_info.lane_id)? else {
            return Ok(());
        };
        if reply_route.channel() == SYSTEM_SOURCE_CHANNEL {
            return Ok(());
        }
        let digest = wake_identity(&batch.notices);
        let interaction_id =
            InteractionId::new(format!("{SHELL_JOB_WAKE_INTERACTION_PREFIX}{digest}"))
                .map_err(|error| format!("构造唤醒 Interaction ID 失败: {error}"))?;
        let source = InteractionSource::new(
            reply_route.channel(),
            format!("{SHELL_JOB_WAKE_SOURCE_PREFIX}{digest}"),
        )
        .map_err(|error| format!("构造唤醒 Interaction 来源失败: {error}"))?;
        let timestamp = now_ms();
        let mut interaction = InteractionRecord::new(
            interaction_id,
            source,
            scope,
            audience,
            reply_route,
            format_wake_content(&batch.render()),
            timestamp,
        )
        .map_err(|error| format!("构造唤醒 Interaction 失败: {error}"))?;
        interaction.model = route_info.model.clone();
        interaction.reasoning_effort = route_info.reasoning_effort.clone();

        let ingress = PrimaryTurnIngress::new(
            self.store.clone(),
            ActiveLaneTurnSteerSink::new(self.active_primary_turns.clone()),
            timestamp,
            self.lease_duration_ms,
        );
        let receipt = ingress
            .submit_new_interaction(&interaction)
            .map_err(|error| format!("提交唤醒 Interaction 失败: {error:?}"))?;
        if receipt.delivery == InteractionDelivery::Queued
            && let Err(error) = (self.wake)()
        {
            axon_logger::warning(&format!(
                "后台 Shell 作业唤醒信号发送失败，等待下一轮领取: lane={}, error={error}",
                route_info.lane_id
            ));
        }
        Ok(())
    }

    fn lane_route(
        &self,
        lane_id: &str,
    ) -> Result<Option<(ScopeRef, AudienceContext, ReplyRoute)>, String> {
        let rows = LaneQueryStore::list_lane_summaries(&self.store)
            .map_err(|error| format!("读取车道路由失败: {error:?}"))?;
        let Some(row): Option<&LaneSummaryRow> = rows.iter().find(|row| row.lane_id == lane_id)
        else {
            axon_logger::warning(&format!(
                "后台 Shell 作业唤醒跳过未登记的车道: lane={lane_id}"
            ));
            return Ok(None);
        };
        let scope: ScopeRef = serde_json::from_str(&row.scope_json)
            .map_err(|error| format!("车道作用域解析失败: {error}"))?;
        let audience: AudienceContext = serde_json::from_str(&row.audience_json)
            .map_err(|error| format!("车道受众解析失败: {error}"))?;
        let route: ReplyRoute = serde_json::from_str(&row.reply_route_json)
            .map_err(|error| format!("车道回复路由解析失败: {error}"))?;
        Ok(Some((scope, audience, route)))
    }
}

fn wake_identity(notices: &[CompletionNotice]) -> String {
    let joined = notices
        .iter()
        .map(|notice| notice.notice_key.as_str())
        .collect::<Vec<_>>()
        .join("|");
    sha256_hex_prefix(joined.as_bytes(), SHELL_JOB_WAKE_HASH_CHARS)
}

fn format_wake_content(rendered_notices: &str) -> String {
    format!(
        "后台任务已结束，跑完后结果已作为一条新消息送到这里。\n\
         请结合下方终态与「完整结果」路径继续处理并向用户汇报。完整输出直接读下方路径；\
         作业终态只在短窗口内可经 builtin.job_output 读取，过期即回收。\n\n\
         {rendered_notices}"
    )
}

#[cfg(test)]
#[path = "run_shell_job_wake_tests.rs"]
mod tests;
