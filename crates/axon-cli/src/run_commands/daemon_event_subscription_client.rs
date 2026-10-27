//! Daemon Session Event 订阅状态机。
//!
//! 只负责把一条订阅上的消息投影成 durable 进度与终态；重连/重试/恢复预算等
//! 连接恢复策略在 `daemon_subscription_recovery`。

use std::{thread, time::Instant};

use axon_daemon::infrastructure::ipc::JsonlIpcSubscription;
use axon_domain::{ConsumerCursor, InteractionRecord, InteractionStatus, RuntimeObservationFrame};
use axon_protocol::{DaemonResponse, wire::WireEventEnvelope};
use axon_session::SessionEventRecord;
use serde::Deserialize;

use super::daemon_event_cursor::collect_events;
use super::daemon_interaction_submission::response_body;
use super::daemon_subscription_recovery::{
    INTERACTION_INTERRUPTED_MESSAGE, INTERACTION_REJECTED_MESSAGE, SUBSCRIPTION_POLL_INTERVAL,
    SUBSCRIPTION_RETRY_INITIAL_INTERVAL, SUBSCRIPTION_RETRY_RESYNC_ATTEMPT,
    ensure_subscription_recovery, ensure_subscription_retry, ensure_wait_can_continue,
    next_retry_delay, query_interaction_status, subscribe_daemon_events,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DaemonSubscriptionPhase {
    Connected,
    Reconnecting,
    Resyncing,
}

pub(crate) enum DaemonTurnObservation {
    Wire(WireEventEnvelope),
    SubscriptionPhase(DaemonSubscriptionPhase),
    RuntimeObservation(RuntimeObservationFrame),
}

#[derive(Debug, Default)]
pub(crate) struct DaemonTurnProjection {
    pub(crate) session_id: String,
    pub(crate) response: String,
    /// 终态生命周期事件携带的本轮告警。
    pub(crate) warnings: Vec<String>,
    pub(crate) wire_events: Vec<WireEventEnvelope>,
}

pub(super) struct DaemonTurnState<'a> {
    pub(super) cursor: Option<axon_domain::ConsumerCursor>,
    interaction_id: String,
    phase: Option<DaemonSubscriptionPhase>,
    pub(super) wire_events: Vec<WireEventEnvelope>,
    /// durable 进度单调计数：只有真实的 wire 事件或 Interaction 状态推进会计数，
    /// 心跳与订阅重建都不算：否则挂死的 Daemon 可以用握手把自己伪装成有进度。
    progress_serial: u64,
    /// 订阅活性单调计数：**任意**一条订阅消息（含 `subscription_ready` 与心跳）都推进它。
    ///
    /// 与 `progress_serial` 分工明确：活性只证明「订阅还活着」，证明不了回合在推进；
    /// 但恢复看门狗必须承认它：心跳在即即视为订阅健康，否则健康的长工具调用
    /// （回合长时间没有事件、心跳也可能迟到）会被判成失联。
    liveness_serial: u64,
    on_event: Option<&'a mut dyn FnMut(DaemonTurnObservation)>,
}

impl<'a> DaemonTurnState<'a> {
    pub(super) fn new(
        interaction_id: String,
        on_event: Option<&'a mut dyn FnMut(DaemonTurnObservation)>,
    ) -> Self {
        Self {
            cursor: None,
            interaction_id,
            phase: None,
            wire_events: Vec::new(),
            progress_serial: 0,
            liveness_serial: 0,
            on_event,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SubscriptionControl {
    Continue,
    Reconnect,
    Completed(String),
    Resync,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event")]
pub(super) enum SubscriptionMessage {
    #[serde(rename = "subscription_ready")]
    Ready {
        events: Vec<SessionEventRecord>,
        #[serde(rename = "pageFull")]
        page_full: bool,
        #[serde(rename = "snapshotRequired")]
        snapshot_required: bool,
        #[serde(rename = "replayCursor", default)]
        replay_cursor: Option<ConsumerCursor>,
        #[serde(rename = "interactionStatus")]
        interaction_status: InteractionStatus,
    },
    #[serde(rename = "session_event")]
    SessionEvent { record: SessionEventRecord },
    #[serde(rename = "interaction_status")]
    InteractionStatus { status: InteractionStatus },
    #[serde(rename = "subscription_heartbeat")]
    Heartbeat,
    #[serde(rename = "runtime_observation")]
    RuntimeObservation { frame: RuntimeObservationFrame },
    #[serde(rename = "stream_reset")]
    StreamReset { cursor: Option<ConsumerCursor> },
}

pub(super) fn wait_for_daemon_turn(
    endpoint: &str,
    interaction: &InteractionRecord,
    session_id: &str,
    deadline: Option<Instant>,
    is_cancelled: &dyn Fn() -> bool,
    state: &mut DaemonTurnState<'_>,
) -> Result<String, String> {
    let mut retry_delay = SUBSCRIPTION_RETRY_INITIAL_INTERVAL;
    let mut retry_attempt = 0_u32;
    let mut lost_subscription: Option<(u64, u64, Instant)> = None;
    loop {
        ensure_wait_can_continue(deadline, is_cancelled)?;
        notify_subscription_phase(state, DaemonSubscriptionPhase::Reconnecting);
        match subscribe_daemon_events(
            endpoint,
            session_id,
            interaction.interaction_id.as_str(),
            state.cursor.clone(),
        ) {
            Ok(subscription) => {
                retry_attempt = 0;
                retry_delay = SUBSCRIPTION_RETRY_INITIAL_INTERVAL;
                match receive_daemon_events(subscription, deadline, is_cancelled, state)? {
                    SubscriptionControl::Completed(response) => return Ok(response),
                    SubscriptionControl::Reconnect => continue,
                    SubscriptionControl::Resync => {
                        let error = "Daemon 事件订阅连接中断".to_owned();
                        let status =
                            query_interaction_status(endpoint, interaction.interaction_id.as_str());
                        ensure_subscription_retry(
                            SUBSCRIPTION_RETRY_RESYNC_ATTEMPT,
                            &error,
                            status,
                        )?;
                        ensure_subscription_recovery(
                            &mut lost_subscription,
                            state.progress_serial,
                            state.liveness_serial,
                        )?;
                        notify_subscription_phase(state, DaemonSubscriptionPhase::Resyncing);
                        ensure_wait_can_continue(deadline, is_cancelled)?;
                        continue;
                    }
                    SubscriptionControl::Continue => {
                        return Err("Daemon 事件订阅意外结束".to_owned());
                    }
                }
            }
            Err(error) => {
                retry_attempt = retry_attempt.saturating_add(1);
                notify_subscription_phase(state, DaemonSubscriptionPhase::Reconnecting);
                let status =
                    query_interaction_status(endpoint, interaction.interaction_id.as_str());
                ensure_subscription_retry(retry_attempt, &error, status)?;
                notify_subscription_phase(state, DaemonSubscriptionPhase::Resyncing);
                ensure_wait_can_continue(deadline, is_cancelled)?;
                thread::sleep(retry_delay);
                retry_delay = next_retry_delay(retry_delay);
            }
        }
    }
}

/// 一次订阅读取的结果分类。
///
/// **静默不是失联**：读超时只说明这一拍没有消息：心跳可能迟到，长工具调用期间事件流
/// 也可能长时间无话，但订阅本身仍然健康。只有真实 IO 故障（管道断开、EOF、读错误）
/// 才要求重建订阅并进入恢复预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubscriptionRead {
    /// 收到一条消息（含 `subscription_ready` 与心跳）。
    Message,
    /// 读取超时：这一拍没有消息。
    Silent,
    /// 连接故障：读错误或 EOF。
    Failed,
}

impl SubscriptionRead {
    /// 该读取结果是否意味着「失去订阅」。
    const fn lost_subscription(self) -> bool {
        matches!(self, Self::Failed)
    }
}

const fn classify_subscription_read(
    result: &std::io::Result<Option<DaemonResponse>>,
) -> SubscriptionRead {
    match result {
        Ok(Some(_)) => SubscriptionRead::Message,
        Ok(None) => SubscriptionRead::Silent,
        Err(_) => SubscriptionRead::Failed,
    }
}

fn receive_daemon_events(
    mut subscription: JsonlIpcSubscription,
    deadline: Option<Instant>,
    is_cancelled: &dyn Fn() -> bool,
    state: &mut DaemonTurnState<'_>,
) -> Result<SubscriptionControl, String> {
    loop {
        ensure_wait_can_continue(deadline, is_cancelled)?;
        let read = subscription.recv_timeout(SUBSCRIPTION_POLL_INTERVAL);
        if classify_subscription_read(&read).lost_subscription() {
            notify_subscription_phase(state, DaemonSubscriptionPhase::Reconnecting);
            return Ok(SubscriptionControl::Resync);
        }
        if let Ok(Some(response)) = read {
            // 任意消息都推进订阅活性：心跳在即即视为订阅健康，恢复看门狗据此撤销。
            state.liveness_serial = state.liveness_serial.saturating_add(1);
            let message = decode_subscription_message(response)?;
            match handle_subscription_message(message, state)? {
                SubscriptionControl::Continue => {}
                terminal => return Ok(terminal),
            }
        }
    }
}

fn decode_subscription_message(response: DaemonResponse) -> Result<SubscriptionMessage, String> {
    let body = response_body(response)?;
    serde_json::from_value(body).map_err(|error| format!("解析 Daemon 事件订阅消息失败：{error}"))
}

pub(super) fn handle_subscription_message(
    message: SubscriptionMessage,
    state: &mut DaemonTurnState<'_>,
) -> Result<SubscriptionControl, String> {
    match message {
        SubscriptionMessage::Ready {
            events,
            page_full,
            snapshot_required,
            replay_cursor,
            interaction_status,
        } => {
            notify_subscription_phase(state, DaemonSubscriptionPhase::Connected);
            if snapshot_required {
                state.cursor = None;
                state.wire_events.clear();
            }
            // 顺序契约：必须先把本页事件按真实游标消费完，再采纳服务端 replay 水位。
            //
            // `replayCursor` 是「本页处理完后的服务端高水位」（页内最后一条原始事件），
            // 不是「客户端已消费的事件」；先写它会让 `collect_events` 的去重把页内事件
            // 误判成重复而丢弃：页尾恰好是本轮 `turn_completed` 时，前台等待将永久拿不到
            // 终态（TUI 一直显示「生成回复中」，直到用户按 Esc）。
            let response = collect_subscription_events(events, state)?;
            record_replay_cursor(state, replay_cursor);
            completed_or_status(response, interaction_status, page_full)
        }
        SubscriptionMessage::SessionEvent { record } => {
            let response = collect_subscription_events(vec![record], state)?;
            Ok(response.map_or(
                SubscriptionControl::Continue,
                SubscriptionControl::Completed,
            ))
        }
        SubscriptionMessage::InteractionStatus { status } => {
            state.progress_serial = state.progress_serial.saturating_add(1);
            completed_or_status(None, status, false)
        }
        SubscriptionMessage::Heartbeat => Ok(SubscriptionControl::Continue),
        SubscriptionMessage::RuntimeObservation { frame } => {
            if let Some(observe) = state.on_event.as_deref_mut() {
                observe(DaemonTurnObservation::RuntimeObservation(frame));
            }
            Ok(SubscriptionControl::Continue)
        }
        SubscriptionMessage::StreamReset { cursor } => {
            state.cursor = cursor;
            Ok(SubscriptionControl::Reconnect)
        }
    }
}

fn collect_subscription_events(
    events: Vec<SessionEventRecord>,
    state: &mut DaemonTurnState<'_>,
) -> Result<Option<String>, String> {
    let DaemonTurnState {
        cursor,
        interaction_id,
        wire_events,
        progress_serial,
        on_event,
        ..
    } = state;
    let mut wire_dispatch = on_event.as_deref_mut().map(|observe| {
        move |event: WireEventEnvelope| {
            observe(DaemonTurnObservation::Wire(event));
        }
    });
    let mut on_wire_event: Option<&mut dyn FnMut(WireEventEnvelope)> =
        wire_dispatch.as_mut().map(|observe| observe as _);
    let received_durable_event = !events.is_empty();
    let response = collect_events(
        events,
        interaction_id,
        cursor,
        wire_events,
        &mut on_wire_event,
    );
    if received_durable_event {
        *progress_serial = progress_serial.saturating_add(1);
    }
    response
}

pub(super) fn completed_or_status(
    response: Option<String>,
    status: InteractionStatus,
    page_full: bool,
) -> Result<SubscriptionControl, String> {
    if let Some(response) = response {
        return Ok(SubscriptionControl::Completed(response));
    }
    if page_full {
        return Ok(SubscriptionControl::Reconnect);
    }
    match status {
        InteractionStatus::Rejected => Err(INTERACTION_REJECTED_MESSAGE.to_owned()),
        InteractionStatus::Interrupted => Err(INTERACTION_INTERRUPTED_MESSAGE.to_owned()),
        InteractionStatus::Received
        | InteractionStatus::Processing
        | InteractionStatus::Understood => Ok(SubscriptionControl::Continue),
    }
}

/// 采纳服务端 replay 水位：只推进、不回退。
///
/// 必须在 `collect_subscription_events` 之后调用：它代表这一页的服务端高水位，
/// 而不是客户端已经消费过的事件；提前写入会把页内事件误判成重复。
fn record_replay_cursor(state: &mut DaemonTurnState<'_>, cursor: Option<ConsumerCursor>) {
    let Some(cursor) = cursor else {
        return;
    };
    let advances = state
        .cursor
        .as_ref()
        .is_none_or(|previous| cursor.advances_durable_from(previous));
    if advances {
        state.cursor = Some(cursor);
    }
}

fn notify_subscription_phase(state: &mut DaemonTurnState<'_>, phase: DaemonSubscriptionPhase) {
    if state.phase == Some(phase) {
        return;
    }
    state.phase = Some(phase);
    if let Some(observe) = state.on_event.as_deref_mut() {
        observe(DaemonTurnObservation::SubscriptionPhase(phase));
    }
}

#[cfg(test)]
#[path = "daemon_event_subscription_client_tests.rs"]
mod tests;
