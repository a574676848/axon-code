//! Daemon 事件订阅的连接恢复策略：握手重试、空闲判定与 durable 进度看门狗。
//!
//! 与 `daemon_event_subscription_client` 的分工：那边只把一条订阅上的消息投影成
//! durable 进度与终态，这边只回答「订阅断了要不要重试、还能等多久才算挂死」。
//! 两侧共用同一组预算常量，前台因此既不会无限重连，也不会被误判为超时。

use std::time::{Duration, Instant};

use axon_daemon::infrastructure::ipc::{
    JsonlIpcClient, JsonlIpcSubscription, SUBSCRIPTION_HEARTBEAT_INTERVAL,
};
use axon_domain::{ConsumerCursor, InteractionStatus};
use axon_protocol::DaemonCommand;

use super::daemon_interaction_submission::{daemon_request, send_command};

pub(super) const INTERACTION_REJECTED_MESSAGE: &str = "Daemon Primary Turn 已拒绝本次 Interaction";
pub(super) const INTERACTION_INTERRUPTED_MESSAGE: &str =
    "Daemon 进程中断，Interaction 已收敛为 interrupted 终态";
pub(super) const SUBSCRIPTION_RETRY_INITIAL_INTERVAL: Duration = Duration::from_millis(250);
pub(super) const SUBSCRIPTION_RETRY_MAX_INTERVAL: Duration = Duration::from_millis(10_000);
pub(super) const SUBSCRIPTION_RETRY_RESYNC_ATTEMPT: u32 = 1;
const SUBSCRIPTION_RETRY_BACKOFF_FACTOR: u32 = 2;
const SUBSCRIPTION_RETRY_MAX_ATTEMPTS: u32 = 11;
/// 订阅连接的读取轮询间隔。
///
/// 它只回答「多久醒来收一次消息」，**不是**失联判定窗口：读超时（静默）不代表订阅失效，
/// 心跳迟到、以及长工具调用期间事件流长时间无话都属正常。判失联的只有真实连接故障。
pub(super) const SUBSCRIPTION_POLL_INTERVAL: Duration = SUBSCRIPTION_HEARTBEAT_INTERVAL;
/// 单次重连尝试允许占用的时间片：握手 + 首次读取。
const SUBSCRIPTION_RECONNECT_ATTEMPT_BUDGET: Duration = Duration::from_secs(5);
/// 订阅在**真实连接故障**下连续重建的容忍窗口。
///
/// 只统计「读错误 / EOF / 连不上」这一类故障，且期间**一次消息都没收到**（连心跳都没有）；
/// 静默不计时，因为健康的长工具调用期间事件流本就可能长时间无话。撤销条件见
/// [`ensure_subscription_recovery`]：拿到任意一条订阅消息（含心跳）即视为订阅健康。
/// 窗口耗尽后必须失败返回，而不是无限重连把前台永久钉在 Working。
const SUBSCRIPTION_RECOVERY_BUDGET: Duration =
    SUBSCRIPTION_RECONNECT_ATTEMPT_BUDGET.saturating_mul(SUBSCRIPTION_RETRY_MAX_ATTEMPTS);

/// 建立订阅；`resume_cursor` 让重建从最后一次确认的 durable 位置继续。
pub(super) fn subscribe_daemon_events(
    endpoint: &str,
    session_id: &str,
    interaction_id: &str,
    resume_cursor: Option<ConsumerCursor>,
) -> Result<JsonlIpcSubscription, String> {
    let request = daemon_request(DaemonCommand::SubscribeEvents {
        session_id: session_id.to_owned(),
        interaction_id: interaction_id.to_owned(),
        resume_cursor,
    });
    JsonlIpcClient::connect(endpoint)
        .and_then(|client| client.subscribe(&request))
        .map_err(|error| format!("连接 Daemon 事件订阅失败：{error}"))
}

/// 只读探测 Interaction 的 durable 状态，用于区分「订阅故障」与「回合已收敛」。
pub(super) fn query_interaction_status(
    endpoint: &str,
    interaction_id: &str,
) -> Result<Option<InteractionStatus>, String> {
    let body = send_command(
        endpoint,
        DaemonCommand::GetInteraction {
            interaction_id: interaction_id.to_owned(),
        },
    )?;
    let Some(status) = body.get("status") else {
        return Ok(None);
    };
    serde_json::from_value(status.clone())
        .map(Some)
        .map_err(|error| format!("解析 Daemon Interaction 状态失败：{error}"))
}

pub(super) fn ensure_subscription_retry(
    attempt: u32,
    subscription_error: &str,
    status: Result<Option<InteractionStatus>, String>,
) -> Result<(), String> {
    match status {
        Ok(None) => Err("Daemon Interaction 不存在，无法恢复事件订阅".to_owned()),
        Ok(Some(InteractionStatus::Rejected)) => Err(INTERACTION_REJECTED_MESSAGE.to_owned()),
        Ok(Some(InteractionStatus::Interrupted)) => Err(INTERACTION_INTERRUPTED_MESSAGE.to_owned()),
        Ok(Some(status)) if is_active_interaction_status(status) => {
            if attempt >= SUBSCRIPTION_RETRY_MAX_ATTEMPTS {
                Err(format!(
                    "Daemon Interaction 仍在处理，但事件订阅连续失败：{subscription_error}"
                ))
            } else {
                Ok(())
            }
        }
        Ok(Some(status)) => Err(format!(
            "Daemon Interaction 状态不支持事件恢复（{status:?}）：{subscription_error}"
        )),
        Err(status_error) => {
            if attempt >= SUBSCRIPTION_RETRY_MAX_ATTEMPTS {
                Err(format!(
                    "无法确认 Daemon Interaction 状态，事件订阅连续失败：{subscription_error}; {status_error}"
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn is_active_interaction_status(status: InteractionStatus) -> bool {
    matches!(
        status,
        InteractionStatus::Received | InteractionStatus::Processing | InteractionStatus::Understood
    )
}

pub(super) fn next_retry_delay(delay: Duration) -> Duration {
    delay
        .checked_mul(SUBSCRIPTION_RETRY_BACKOFF_FACTOR)
        .unwrap_or(SUBSCRIPTION_RETRY_MAX_INTERVAL)
        .min(SUBSCRIPTION_RETRY_MAX_INTERVAL)
}

/// 前台等待是否可以继续：取消与显式截止时间都在这里收口。
pub(super) fn ensure_wait_can_continue(
    deadline: Option<Instant>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if is_cancelled() {
        return Err("前台已取消等待；Daemon 中的 durable Interaction 将继续执行".to_owned());
    }
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(
            "前台等待 Daemon Primary Turn 超时；该 Interaction 仍在 Daemon 中继续执行，\
             可用 `axon debug session` 追读其结果"
                .to_owned(),
        );
    }
    Ok(())
}

/// 订阅必须在此窗口内用**订阅活性**或 **durable 进度**证明自己恢复了。
///
/// 判定分两层：
/// 1. **订阅活性**（`liveness_serial`）：任一条订阅消息都算，含 `subscription_ready`
///    与心跳。心跳在即即视为订阅健康：长工具调用期间回合没有事件、甚至心跳迟到，
///    都不足以把健康回合判死。
/// 2. **durable 进度**（`progress_serial`）：真实 wire 事件或 Interaction 状态推进。
///    只有它能证明回合在往前走；但**只有它**才能撤销看门狗是不成立的（原实现如此，
///    于是「连接通、只是暂时没事件」会被误判）。
///
/// 两个计数任一推进即撤销看门狗；只有「既拿不到任何消息、又没有 durable 进度」
/// 持续满 [`SUBSCRIPTION_RECOVERY_BUDGET`] 才失败返回。
/// 计时从**第一次失去连接**开始而不是从等待开始：健康的长时间静默回合不会因为等待
/// 本身耗时而被误判。
pub(super) fn ensure_subscription_recovery(
    lost_subscription: &mut Option<(u64, u64, Instant)>,
    progress_serial: u64,
    liveness_serial: u64,
) -> Result<(), String> {
    let Some((observed_progress_serial, observed_liveness_serial, lost_at)) = *lost_subscription
    else {
        *lost_subscription = Some((progress_serial, liveness_serial, Instant::now()));
        return Ok(());
    };
    if observed_progress_serial != progress_serial || observed_liveness_serial != liveness_serial {
        *lost_subscription = None;
        return Ok(());
    }
    if lost_at.elapsed() < SUBSCRIPTION_RECOVERY_BUDGET {
        return Ok(());
    }
    Err(format!(
        "Daemon 事件订阅连续 {} 秒未能恢复（既无任何订阅消息也无 durable 进度）；\
         该 Interaction 仍在 Daemon 中，可用 `axon debug session` 追读其结果，\
         或在 TUI 中恢复该活动回合",
        SUBSCRIPTION_RECOVERY_BUDGET.as_secs()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_recovery_budget_bounds_unresponsive_daemon_waits() {
        assert_eq!(SUBSCRIPTION_POLL_INTERVAL, SUBSCRIPTION_HEARTBEAT_INTERVAL);
        assert_eq!(
            SUBSCRIPTION_RECONNECT_ATTEMPT_BUDGET * SUBSCRIPTION_RETRY_MAX_ATTEMPTS,
            SUBSCRIPTION_RECOVERY_BUDGET
        );
        assert_eq!(SUBSCRIPTION_RECOVERY_BUDGET, Duration::from_secs(55));
    }

    #[test]
    fn durable_progress_withdraws_the_recovery_watchdog() {
        let mut lost_subscription = None;

        ensure_subscription_recovery(&mut lost_subscription, 7, 3)
            .expect("首次失去订阅只是开始计时");
        assert!(lost_subscription.is_some());

        ensure_subscription_recovery(&mut lost_subscription, 8, 3)
            .expect("durable 进度必须撤销看门狗");
        assert!(lost_subscription.is_none());
    }

    /// 心跳在即即视为订阅健康：只在活性计数上推进也必须撤销看门狗。
    #[test]
    fn heartbeat_liveness_withdraws_the_recovery_watchdog() {
        let mut lost_subscription = None;

        ensure_subscription_recovery(&mut lost_subscription, 7, 3)
            .expect("首次失去连接只是开始计时");
        assert!(lost_subscription.is_some());

        ensure_subscription_recovery(&mut lost_subscription, 7, 4)
            .expect("收到心跳（订阅活性）必须撤销看门狗");
        assert!(lost_subscription.is_none());
    }

    #[test]
    fn exhausted_recovery_budget_fails_instead_of_reconnecting_forever() {
        let mut lost_subscription =
            Some((3_u64, 5_u64, Instant::now() - SUBSCRIPTION_RECOVERY_BUDGET));

        let error = ensure_subscription_recovery(&mut lost_subscription, 3, 5)
            .expect_err("恢复窗口耗尽必须失败返回");

        assert!(error.contains("未能恢复"), "实际错误：{error}");
    }

    #[test]
    fn active_interaction_allows_subscription_retry() {
        ensure_subscription_retry(1, "连接失败", Ok(Some(InteractionStatus::Processing)))
            .expect("活跃 Interaction 应允许重试");
    }

    #[test]
    fn retry_delay_grows_with_ceiling() {
        assert_eq!(
            next_retry_delay(SUBSCRIPTION_RETRY_INITIAL_INTERVAL),
            Duration::from_millis(500)
        );
        assert_eq!(
            next_retry_delay(SUBSCRIPTION_RETRY_MAX_INTERVAL),
            SUBSCRIPTION_RETRY_MAX_INTERVAL
        );
    }

    #[test]
    fn retry_budget_stays_bounded() {
        assert_eq!(SUBSCRIPTION_RETRY_MAX_ATTEMPTS, 11);
        assert_eq!(
            ensure_subscription_retry(
                SUBSCRIPTION_RETRY_MAX_ATTEMPTS,
                "连接失败",
                Ok(Some(InteractionStatus::Understood)),
            ),
            Err("Daemon Interaction 仍在处理，但事件订阅连续失败：连接失败".to_owned())
        );
    }

    #[test]
    fn retry_budget_stays_near_one_minute() {
        let mut total = Duration::ZERO;
        let mut delay = SUBSCRIPTION_RETRY_INITIAL_INTERVAL;
        for _ in 0..SUBSCRIPTION_RETRY_MAX_ATTEMPTS.saturating_sub(1) {
            total += delay;
            delay = next_retry_delay(delay);
        }
        assert_eq!(total, Duration::from_millis(55_750));
        assert!(total <= Duration::from_secs(60));
    }

    #[test]
    fn missing_or_rejected_interaction_does_not_retry() {
        assert_eq!(
            ensure_subscription_retry(1, "连接失败", Ok(None)),
            Err("Daemon Interaction 不存在，无法恢复事件订阅".to_owned())
        );
        assert_eq!(
            ensure_subscription_retry(1, "连接失败", Ok(Some(InteractionStatus::Rejected))),
            Err(INTERACTION_REJECTED_MESSAGE.to_owned())
        );
    }
}
