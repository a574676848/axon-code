//! 后台 Execution 完成通知的投递路由与持久收件箱派发。
//!
//! 产品定义：一个 Execution 完成时只通知主 turn 一次，主 turn 的汇总回复必须
//! 投递到当时所有可用渠道（在线客户端 presence + 已配置的外部通知目标）。因此不再存在
//! 「TUI 在线就只投 TUI」的选路：所有可用渠道都收到同一份主 turn 汇总回复。
//!
//! 任何渠道都不允许静默确认：没有可用渠道时通知保留在持久收件箱，等待下一次
//! 投递机会；Result 只在至少一个渠道确认送达后才允许确认。隔天仍未读的通知
//! 整体失效并静默停止投递。

use super::{DaemonHost, DaemonHostError};
use crate::application::interaction_ingress::PrimaryTurnIngress;
use crate::ports::ProcessIdentity;
use axon_domain::{
    ExecutionId, GoalRecord, InteractionId, InteractionRecord, InteractionSource, ReplyRoute,
    ResultId, ScopeRef,
};
use axon_session::{CompletionNotificationRecord, NotificationDeliveryState, RuntimeStore};
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

use std::sync::Arc;

#[path = "completion_delivery_content.rs"]
mod completion_delivery_content;
mod completion_delivery_route;
use completion_delivery_content::completion_notification;
pub(super) use completion_delivery_content::{
    completion_expiry_at_ms, settle_notification_results,
};

pub(super) use completion_delivery_route::resolve_completion_targets;
use completion_delivery_route::{
    execution_completion_route, resolve_completion_targets_for_execution,
};

/// 单轮待派发通知的扫描上限：保证 sweep 的写锁持有时间与内存占用有界。
pub(super) const COMPLETION_DISPATCH_BATCH_LIMIT: u32 = 32;
/// 客户端 presence 的存活窗口。
///
/// 客户端必须在该窗口内持续 poll 才能维持在线；超时即视为离线，完成通知不再
/// 为它登记新的投递目标。
pub(super) const CLIENT_PRESENCE_TTL_MS: u64 = 30_000;
/// 完成通知的摘要上限：steer 必须保持可读，完整结论仍以 Ledger Result 为唯一事实源。
use axon_domain::GOAL_RESULT_SUMMARY_MAX_CHARS as COMPLETION_SUMMARY_MAX_CHARS;
/// 完成通知 steer Interaction 的稳定 ID 前缀。
const STEER_RESULT_ID_PREFIX: &str = "steer-result-";
/// 无法换算本地日界时的失效兜底窗口：等价于「一个自然日」。
const COMPLETION_EXPIRY_FALLBACK_MS: u64 = 24 * 60 * 60 * 1_000;
/// 完成通知的稳定 ID：同一 Execution 只产生一条通知。
pub(super) fn completion_notification_id(execution_id: &ExecutionId) -> String {
    format!("completion-notification-{}", execution_id.as_str())
}

/// 完成通知 steer Interaction 的稳定 ID。
pub(super) fn completion_steer_interaction_id(execution_id: &ExecutionId) -> String {
    format!("{STEER_RESULT_ID_PREFIX}{}", execution_id.as_str())
}

/// 完成通知 steer Interaction 的稳定来源 ID。
pub(super) fn completion_steer_source_id(execution_id: &ExecutionId) -> String {
    format!(
        "{}{}",
        axon_domain::COMPLETION_STEER_SOURCE_PREFIX,
        execution_id.as_str()
    )
}

/// 从 Cron 派发 Interaction 的来源 ID 解析投递项身份。
///
/// 只有 Cron 派发的 Interaction 才返回 `Some`：这类主 turn 的汇总回复与 Goal 完成
/// 消息共用同一套持久收件箱，渠道 egress 不再直接出站。
pub(super) fn cron_delivery_id(source_message_id: &str) -> Option<&str> {
    source_message_id
        .strip_prefix(axon_domain::CRON_DELIVERY_SOURCE_PREFIX)
        .filter(|value| !value.is_empty())
}

/// 正文已就绪投递项的通知 ID：由投递身份稳定派生。
///
/// 与 Cron 派发主 turn 的通知 ID 共用同一前缀命名空间，便于运维按前缀检索同一类
/// 系统投递；重复登记同一投递身份命中唯一约束，不会产生第二条投递项。
pub(super) fn ready_delivery_notification_id(delivery_id: &str) -> String {
    format!(
        "{}{}",
        axon_domain::CRON_DELIVERY_NOTIFICATION_PREFIX,
        delivery_id
    )
}

/// 从完成通知 steer Interaction 的来源 ID 解析 Execution ID。
///
/// 只有由完成通知派生的 Interaction 才返回 `Some`，普通主 turn 的汇总结算
/// 因此不会误确认其他 Result。
pub(super) fn completion_steer_execution_id(source_message_id: &str) -> Option<ExecutionId> {
    let raw = source_message_id.strip_prefix(axon_domain::COMPLETION_STEER_SOURCE_PREFIX)?;
    ExecutionId::new(raw.to_owned()).ok()
}

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 记录并派发一个已形成结论的 Execution 的完成通知。
    ///
    /// 该入口同时服务于实时结算与崩溃补偿扫描：只要 Result 已提交，通知就一定
    /// 会进入持久收件箱，不会因为进程中断而丢失。
    pub(super) fn record_execution_completion(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<(), DaemonHostError> {
        self.record_execution_completion_inbox(execution_id)?;
        self.dispatch_pending_completions(COMPLETION_DISPATCH_BATCH_LIMIT)?;
        Ok(())
    }

    fn record_execution_completion_inbox(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<(), DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let Some(execution) = self
            .store
            .get_execution(execution_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?
        else {
            return Ok(());
        };
        if !execution.status.is_conclusive() {
            return Ok(());
        }
        let Some(goal) = self
            .store
            .get_goal(&execution.goal_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?
        else {
            return Ok(());
        };
        let result = self
            .store
            .result_for_execution(execution_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let result_ids = result
            .as_ref()
            .map(|item| vec![item.result_id.clone()])
            .unwrap_or_default();
        let content = completion_notification(&goal, &execution, result.as_ref());
        self.record_completion_notification(&goal, execution_id, &result_ids, &content, now_ms)?;
        Ok(())
    }

    /// 补偿 commit 成功但通知尚未落库的 Execution。
    ///
    /// 崩溃窗口内的 Result 会在这里被重新发现，保证「禁止静默 ack」不会因为
    /// 进程重启而退化成「永不投递」。
    pub(super) fn recover_missing_completions(&self, limit: u32) -> Result<u32, DaemonHostError> {
        let execution_ids = self
            .store
            .unnotified_conclusive_executions(limit)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let mut recovered = 0_u32;
        for execution_id in execution_ids {
            let Ok(execution_id) = ExecutionId::new(execution_id) else {
                continue;
            };
            if self
                .record_execution_completion_inbox(&execution_id)
                .is_ok()
            {
                recovered = recovered.saturating_add(1);
            }
        }
        if recovered > 0 {
            self.dispatch_pending_completions(COMPLETION_DISPATCH_BATCH_LIMIT)?;
        }
        Ok(recovered)
    }
    /// 幂等记录后台 Execution 的完成通知。
    ///
    /// 通知先进入持久收件箱，并按当前可用渠道登记投递目标；只有主 turn 汇总回复
    /// 产出并进入渠道后，才允许确认对应 Result。
    pub(super) fn record_completion_notification(
        &self,
        goal: &GoalRecord,
        execution_id: &ExecutionId,
        result_ids: &[ResultId],
        content: &str,
        now_ms: u64,
    ) -> Result<bool, DaemonHostError> {
        let workspace_id = match goal.scope.workspace_id() {
            Some(id) => id.as_str().to_owned(),
            None => self
                .store
                .get_execution(execution_id)
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?
                .and_then(|execution| {
                    serde_json::from_str::<serde_json::Value>(&execution.payload_json).ok()
                })
                .and_then(|payload| {
                    payload
                        .get("transportContext")?
                        .get("workspaceId")?
                        .as_str()
                        .map(str::to_owned)
                })
                .ok_or_else(|| {
                    DaemonHostError::Worker("完成通知缺少创建时的 Workspace 路由".to_owned())
                })?,
        };
        let notification_id = completion_notification_id(execution_id);
        let notification = CompletionNotificationRecord {
            notification_id: notification_id.clone(),
            workspace_id: workspace_id.clone(),
            goal_id: goal.goal_id.as_str().to_owned(),
            execution_id: execution_id.as_str().to_owned(),
            content: content.to_owned(),
            summary: String::new(),
            result_ids: result_ids
                .iter()
                .map(|result_id| result_id.as_str().to_owned())
                .collect(),
            state: NotificationDeliveryState::Pending,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            expires_at_ms: completion_expiry_at_ms(now_ms),
        };
        let inserted = self
            .store
            .upsert_completion_notification(&notification)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        self.refresh_completion_deliveries(
            &notification_id,
            &workspace_id,
            Some(execution_id.as_str()),
            now_ms,
        )?;
        Ok(inserted)
    }

    /// 按当前可用渠道刷新通知的投递目标。
    ///
    /// 幂等：已登记目标保留自身状态，新出现的渠道被追加。没有可用渠道时不写任何
    /// 目标，通知保持 `pending`，绝不允许因为「暂时没人可投」而提前确认 Result。
    fn refresh_completion_deliveries(
        &self,
        notification_id: &str,
        workspace_id: &str,
        execution_id: Option<&str>,
        now_ms: u64,
    ) -> Result<u32, DaemonHostError> {
        let targets = execution_id
            .map_or_else(
                || {
                    resolve_completion_targets(
                        &self.store,
                        workspace_id,
                        now_ms,
                        self.feishu_notification_target()?.as_deref(),
                    )
                },
                |execution_id| {
                    resolve_completion_targets_for_execution(
                        &self.store,
                        workspace_id,
                        execution_id,
                        now_ms,
                        self.feishu_notification_target()?.as_deref(),
                    )
                },
            )
            .map_err(DaemonHostError::Worker)?;
        if targets.is_empty() {
            return Ok(0);
        }
        self.store
            .upsert_completion_deliveries(notification_id, &targets, now_ms)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))
    }

    /// 派发一批待投递的完成通知，返回本轮成功派发数量。
    ///
    /// sweep 由客户端上线、RegisterClient 与维护 tick 触发；没有可用渠道的通知
    /// 保持 pending，等待下一次机会。
    pub(super) fn dispatch_pending_completions(&self, limit: u32) -> Result<u32, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        // 先让隔天未读的通知失效，再派发：否则过期的完成消息会在客户端上线
        // 的瞬间被补发，违背「隔天未读自动失效，静默不再通知」。
        self.expire_stale_completions(now_ms, limit)?;
        // 每轮先刷新所有仍可投递通知的目标，覆盖新上线的 TUI 或未来新增渠道。
        self.refresh_deliverable_completion_targets(now_ms, limit)?;
        let mut dispatched = self.dispatch_ready_completions(limit)?;
        dispatched = dispatched.saturating_add(self.dispatch_summarized_completions(limit)?);
        let pending = self
            .store
            .pending_completion_notifications(limit)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        for notification in pending {
            match self.dispatch_completion_notification(&notification, now_ms) {
                Ok(true) => dispatched = dispatched.saturating_add(1),
                Ok(false) => {}
                Err(error) => {
                    eprintln!(
                        "完成通知派发失败，保留持久收件箱等待重试: notification={} error={error}",
                        notification.notification_id
                    );
                }
            }
        }
        Ok(dispatched)
    }

    /// 为「正文已就绪但还没有渠道目标」的投递项补登记渠道并推进投递。
    fn dispatch_ready_completions(&self, limit: u32) -> Result<u32, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let awaiting = self
            .store
            .notifications_awaiting_channel_delivery(now_ms, limit)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let mut dispatched = 0_u32;
        for notification in awaiting {
            let targets = if notification.result_ids.is_empty() {
                resolve_completion_targets(
                    &self.store,
                    &notification.workspace_id,
                    now_ms,
                    self.feishu_notification_target()
                        .map_err(DaemonHostError::Worker)?
                        .as_deref(),
                )
            } else {
                resolve_completion_targets_for_execution(
                    &self.store,
                    &notification.workspace_id,
                    &notification.execution_id,
                    now_ms,
                    self.feishu_notification_target()
                        .map_err(DaemonHostError::Worker)?
                        .as_deref(),
                )
            }
            .map_err(DaemonHostError::Worker)?;
            if targets.is_empty() {
                continue;
            }
            self.store
                .upsert_completion_deliveries(&notification.notification_id, &targets, now_ms)
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            let advanced = self
                .store
                .dispatch_completion_deliveries(&notification.notification_id, now_ms)
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            dispatched = dispatched.saturating_add(advanced);
        }
        Ok(dispatched)
    }

    /// 补偿「汇总已落库但渠道未推进」的窗口。
    ///
    /// 覆盖两类情况：汇总回写与渠道推进之间崩溃，以及通知结算后才补登记的渠道。
    /// 两者都会让渠道停在 `pending`；这里按有界批次重新推进，保证完成消息不会丢失。
    fn dispatch_summarized_completions(&self, limit: u32) -> Result<u32, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let notification_ids = self
            .store
            .notification_ids_with_pending_deliveries(now_ms, limit)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let mut dispatched = 0_u32;
        for notification_id in notification_ids {
            let advanced = self
                .store
                .dispatch_completion_deliveries(&notification_id, now_ms)
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            dispatched = dispatched.saturating_add(advanced);
        }
        Ok(dispatched)
    }

    /// 为有效通知登记当前全部可用渠道；已存在渠道状态保持不变。
    fn refresh_deliverable_completion_targets(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<(), DaemonHostError> {
        let notifications = self
            .store
            .deliverable_completion_notifications(now_ms, limit)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        for notification in notifications {
            self.refresh_completion_deliveries(
                &notification.notification_id,
                &notification.workspace_id,
                Some(&notification.execution_id),
                now_ms,
            )?;
        }
        Ok(())
    }

    /// 登记一条正文已就绪的投递项，并推进到当时所有可用渠道。
    ///
    /// 用于定时任务等系统发起的投递：正文本身就是最终交付内容，不需要主 turn
    /// 汇总。没有可用渠道时投递项保留在收件箱（`dispatched` 且无渠道目标），
    /// 等渠道出现后由 sweep 补投，绝不静默丢弃。
    pub(super) fn enqueue_ready_delivery(
        &self,
        workspace_id: &str,
        goal_id: &str,
        delivery_id: &str,
        content: &str,
    ) -> Result<(String, u32), DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let notification_id = ready_delivery_notification_id(delivery_id);
        let notification = CompletionNotificationRecord {
            notification_id: notification_id.clone(),
            workspace_id: workspace_id.to_owned(),
            goal_id: goal_id.to_owned(),
            execution_id: delivery_id.to_owned(),
            content: content.to_owned(),
            summary: content.to_owned(),
            result_ids: Vec::new(),
            state: NotificationDeliveryState::Dispatched,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            expires_at_ms: completion_expiry_at_ms(now_ms),
        };
        self.store
            .upsert_completion_notification(&notification)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let targets = resolve_completion_targets(
            &self.store,
            workspace_id,
            now_ms,
            self.feishu_notification_target()
                .map_err(DaemonHostError::Worker)?
                .as_deref(),
        )
        .map_err(DaemonHostError::Worker)?;
        if targets.is_empty() {
            return Ok((notification_id, 0));
        }
        let registered = self
            .store
            .upsert_completion_deliveries(&notification_id, &targets, now_ms)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        self.store
            .dispatch_completion_deliveries(&notification_id, now_ms)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        Ok((notification_id, registered))
    }

    /// 读取某渠道上全部目标等待确认送达的收件箱条目。
    ///
    /// 供进程内渠道投递器使用：外部推送渠道没有客户端 presence，只能按渠道整批
    /// 取出后逐个写入自己的 durable 出站队列，再逐条确认送达。
    pub fn dispatched_channel_inbox(
        &self,
        channel: &str,
        limit: u32,
    ) -> Result<Vec<axon_session::ClientInboxEntry>, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        self.store
            .dispatched_channel_inbox(channel, now_ms, limit)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))
    }

    /// 按明确目标读取渠道收件箱，避免无关目标占满批次。
    pub fn dispatched_channel_inbox_for_destination(
        &self,
        channel: &str,
        destination: &str,
        limit: u32,
    ) -> Result<Vec<axon_session::ClientInboxEntry>, DaemonHostError> {
        self.store
            .dispatched_channel_inbox_for_destination(
                channel,
                destination,
                self.clock.now_ms(),
                limit,
            )
            .map_err(|error| DaemonHostError::Worker(error.to_string()))
    }

    /// 渠道投递器把正文写入自己的 durable 出站队列后确认送达。
    ///
    /// 这是外部渠道唯一的确认点：不允许在正文尚未 durable 落盘时提前确认，也不允许
    /// 反过来让未确认的条目从收件箱消失。任一渠道确认送达即确认对应 Result。
    pub fn acknowledge_channel_delivery(
        &self,
        entry: &axon_session::ClientInboxEntry,
    ) -> Result<(), DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let changed = self
            .store
            .settle_completion_delivery(
                &entry.notification_id,
                &entry.channel,
                &entry.destination,
                NotificationDeliveryState::Delivered,
                now_ms,
            )
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        if !changed {
            return Ok(());
        }
        let notification = self
            .store
            .completion_notification(&entry.notification_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        if let Some(notification) = notification {
            self.store
                .settle_completion_notification(
                    &notification.notification_id,
                    NotificationDeliveryState::Delivered,
                    now_ms,
                )
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            settle_notification_results(&self.store, &notification, now_ms, false)?;
        }
        Ok(())
    }

    /// 让隔天未读的完成通知自动失效。
    ///
    /// 失效是终态：通知不再投递，对应 Result 以「过期作废」来源落账。先作废
    /// Result 再结算通知，中途崩溃时下一轮仍能读到同一条通知并幂等重试；反过来
    /// 则会让 Result 永久滞留在「未读」，连带阻塞 Goal 的保留清理。
    pub(super) fn expire_stale_completions(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<u32, DaemonHostError> {
        let stale = self
            .store
            .stale_completion_notifications(now_ms, limit)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let mut expired = 0_u32;
        for notification in stale {
            if notification.state != NotificationDeliveryState::Delivered {
                if let Err(error) =
                    settle_notification_results(&self.store, &notification, now_ms, true)
                {
                    eprintln!(
                        "完成通知失效时作废 Result 失败，保留待下一轮重试: notification={} error={error}",
                        notification.notification_id
                    );
                    continue;
                }
                self.store
                    .settle_completion_notification(
                        &notification.notification_id,
                        NotificationDeliveryState::Expired,
                        now_ms,
                    )
                    .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            }
            // 渠道投递同步失效：否则已就绪的条目会在客户端轮询时继续展示。
            let expired_deliveries = self
                .store
                .expire_completion_deliveries(&notification.notification_id, now_ms)
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            if notification.state != NotificationDeliveryState::Delivered || expired_deliveries > 0
            {
                expired = expired.saturating_add(1);
            }
        }
        Ok(expired)
    }

    /// 把一条通知交给主 turn；返回是否完成派发。
    ///
    /// 同一 Execution 只通知主 turn 一次：steer Interaction 使用稳定 ID，重复调用
    /// 会被 ingress 幂等去重。主 turn 车道取自 Execution 自己的复合车道，全部可用
    /// 渠道在这里登记为投递目标，但投递内容必须等主 turn 汇总回复产出后才允许进入渠道。
    fn dispatch_completion_notification(
        &self,
        notification: &CompletionNotificationRecord,
        now_ms: u64,
    ) -> Result<bool, String> {
        let completion_route = execution_completion_route(
            &self.store,
            &notification.workspace_id,
            &notification.execution_id,
            now_ms,
            self.feishu_notification_target()?.as_deref(),
        )?;
        let Some(primary) = completion_route.primary else {
            return Ok(false);
        };
        self.store
            .upsert_completion_deliveries(
                &notification.notification_id,
                &completion_route.targets,
                now_ms,
            )
            .map_err(|error| error.to_string())?;
        let execution_id = ExecutionId::new(notification.execution_id.clone())
            .map_err(|error| error.to_string())?;
        let interaction_id = InteractionId::new(completion_steer_interaction_id(&execution_id))
            .map_err(|error| error.to_string())?;
        let source = InteractionSource::new(
            primary.channel.clone(),
            completion_steer_source_id(&execution_id),
        )
        .map_err(|error| error.to_string())?;
        let route = ReplyRoute::new(primary.channel.clone(), primary.destination.clone())
            .map_err(|error| error.to_string())?;
        let scope = ScopeRef::workspace(notification.workspace_id.clone())
            .map_err(|error| error.to_string())?;
        let interaction = InteractionRecord::new(
            interaction_id,
            source,
            scope,
            completion_route.audience,
            route,
            notification.content.clone(),
            now_ms,
        )
        .map_err(|error| error.to_string())?;
        let ingress = PrimaryTurnIngress::new(
            self.store.clone(),
            Arc::clone(&self.active_turn_steer_sink),
            now_ms,
            self.action_lease_duration_ms,
        );
        ingress
            .submit(&interaction)
            .map_err(|error| error.to_string())?;
        self.store
            .settle_completion_notification(
                &notification.notification_id,
                NotificationDeliveryState::Dispatched,
                now_ms,
            )
            .map_err(|error| error.to_string())?;
        Ok(true)
    }
}
#[cfg(test)]
#[path = "completion_notice_chain_tests.rs"]
mod completion_notice_chain_tests;
