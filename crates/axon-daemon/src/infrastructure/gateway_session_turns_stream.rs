//! 会话轮次头订阅：只推「新增一轮」与「这一轮收口了」，不推正文。
//!
//! 独立会话（目标执行、子代理）不在侧栏清单里，前端没有别的渠道发现「又多了一轮」。
//! 这一路订阅因此只做差异比较：按固定节拍把会话最近的轮次清单读一遍，比上次多出来的
//! 报 `interaction.created`，状态从在途变成终态的报 `interaction.settled`。
//!
//! 清单直接走 `interaction_turn_heads` 端口：终态判定与索引页同源（同一个状态表达式），
//! 这里不再自己拼 SQL，否则两处口径迟早漂移。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axon_domain::DaemonSessionId;

use super::gateway_kernel::{GatewayEvent, GatewayEventStream, GatewayKernelError};

use axon_session::InteractionTurnHead;

/// 活跃节拍：新轮与收口要尽快被看见。
const TURNS_POLL_MIN: Duration = Duration::from_millis(700);
/// 空闲节拍上限：连续多次没有差异后逐步退到这里，久坐的会话不再按活跃频率查库。
const TURNS_POLL_MAX: Duration = Duration::from_secs(5);
/// 连续多少次无差异后开始退避。
const TURNS_IDLE_POLLS_BEFORE_BACKOFF: u32 = 4;
/// 每次比较读取的轮次上限：只关心头部有没有出现新轮、有没有收口。
const TURNS_WINDOW: u32 = 50;
/// 已知轮次集合的容量上限：超出后按窗口修剪，避免长会话无界增长。
const TURNS_KNOWN_LIMIT: usize = 256;

/// 轮次头事件类型。
pub(super) const INTERACTION_CREATED_EVENT: &str = "interaction.created";
/// 轮次收口事件类型。
pub(super) const INTERACTION_SETTLED_EVENT: &str = "interaction.settled";

/// 只看轮次头的会话订阅。
pub(super) struct SessionTurnsStream<S> {
    store: S,
    session_id: DaemonSessionId,
    /// 已经播报过的轮次标识（含基线里已有的）。
    known: HashSet<String>,
    /// 已经播报过终态的轮次标识。
    settled: HashSet<String>,
    /// 基线锚点：订阅建立那一刻最新的一轮；随 subscribed 回执交给客户端对齐。
    baseline: Option<InteractionTurnHead>,
    last_poll: Option<Instant>,
    /// 当前节拍：活跃时是下限，连续无差异后退避到上限。
    interval: Duration,
    /// 连续无差异的轮询次数；出现差异立即归零。
    idle_polls: u32,
    pending: Vec<GatewayEvent>,
}

impl<S> SessionTurnsStream<S>
where
    S: axon_session::InteractionQueryStore + Clone,
{
    /// 建流时立刻读一次基线：subscribed 回执要带着它一起发出去。
    ///
    /// 基线只表示「这条流已经知道这些轮次」，不补发任何历史帧；订阅之后的第一次
    /// 查询起就按差异播报，因此订阅与读索引之间出现的新轮不会被吞掉。
    pub(super) fn new(store: S, session_id: DaemonSessionId) -> Result<Self, GatewayKernelError> {
        let mut stream = Self {
            store,
            session_id,
            known: HashSet::new(),
            settled: HashSet::new(),
            baseline: None,
            last_poll: None,
            interval: TURNS_POLL_MIN,
            idle_polls: 0,
            pending: Vec::new(),
        };
        stream.prime()?;
        Ok(stream)
    }

    /// 读一次基线：已知集合与锚点在同一份快照上确定，两者不会各说各话。
    fn prime(&mut self) -> Result<(), GatewayKernelError> {
        let rows = self
            .store
            .interaction_turn_heads(self.session_id.as_str(), TURNS_WINDOW)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        self.baseline = rows.first().cloned();
        for head in &rows {
            let interaction_id = head.interaction_id.clone();
            let terminal = is_terminal(status_name(head.status));
            self.known.insert(interaction_id.clone());
            if terminal {
                self.settled.insert(interaction_id);
            }
        }
        Ok(())
    }

    fn due(&mut self) -> bool {
        let now = Instant::now();
        let due = self
            .last_poll
            .is_none_or(|last| now.duration_since(last) >= self.interval);
        if due {
            self.last_poll = Some(now);
        }
        due
    }

    /// 按本次是否有差异调整节拍：有差异回到活跃频率，连续无差异逐级退避。
    fn tune_pace(&mut self, changed: bool) {
        if changed {
            self.idle_polls = 0;
            self.interval = TURNS_POLL_MIN;
            return;
        }
        self.idle_polls = self.idle_polls.saturating_add(1);
        if self.idle_polls < TURNS_IDLE_POLLS_BEFORE_BACKOFF {
            return;
        }
        self.interval = (self.interval * 2).min(TURNS_POLL_MAX);
    }

    /// 读一次轮次清单并产出与上次的差异。
    fn poll(&mut self) -> Result<(), GatewayKernelError> {
        // 只读轮次头三列：整条记录带正文与附件，按节拍解码它们是纯浪费。
        let rows = self
            .store
            .interaction_turn_heads(self.session_id.as_str(), TURNS_WINDOW)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let current: HashSet<&str> = rows
            .iter()
            .map(|head| head.interaction_id.as_str())
            .collect();
        for head in &rows {
            let interaction_id = head.interaction_id.clone();
            let status = status_name(head.status);
            let first_seen = self.known.insert(interaction_id.clone());
            // 先报「多了一轮」再报「这一轮收口了」：轮询间隔内跑完的一轮也必须让订阅者
            // 先看到它的存在，否则界面会凭空收到一条没有行号的收口帧。
            if first_seen {
                self.pending.push(self.event(
                    INTERACTION_CREATED_EVENT,
                    serde_json::json!({
                        "interactionId": interaction_id,
                        "receivedAtMs": head.received_at_ms,
                        "status": status,
                    }),
                ));
            }
            if is_terminal(status) && self.settled.insert(interaction_id.clone()) {
                self.pending.push(self.event(
                    INTERACTION_SETTLED_EVENT,
                    serde_json::json!({
                        "interactionId": interaction_id,
                        "status": status,
                    }),
                ));
            }
        }
        // 修剪：只保留仍在这个窗口里的标识，旧标识离窗后不会再出现在头部比较里。
        if self.known.len() > TURNS_KNOWN_LIMIT {
            self.known.retain(|id| current.contains(id.as_str()));
        }
        if self.settled.len() > TURNS_KNOWN_LIMIT {
            self.settled.retain(|id| current.contains(id.as_str()));
        }
        let changed = !self.pending.is_empty();
        // 在途轮次保持活跃节拍：收口那一刻的帧不该等退避结束才被看见。
        let busy = rows
            .iter()
            .any(|head| !is_terminal(status_name(head.status)));
        self.tune_pace(changed || busy);
        Ok(())
    }

    fn event(&self, event_type: &str, payload: serde_json::Value) -> GatewayEvent {
        GatewayEvent {
            event_type: event_type.to_owned(),
            payload_json: payload.to_string(),
            cursor: None,
            replay: false,
        }
    }
}

impl<S> GatewayEventStream for SessionTurnsStream<S>
where
    S: axon_session::InteractionQueryStore + Clone + Send,
{
    fn subscription_baseline(&self) -> Option<serde_json::Value> {
        self.baseline.as_ref().map(baseline_payload)
    }

    fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<GatewayEvent>, GatewayKernelError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = self.pending.first().cloned() {
                self.pending.remove(0);
                return Ok(Some(event));
            }
            if self.due() {
                self.poll()?;
                continue;
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            // 直接睡到下一次该查库的时刻：空闲订阅每秒醒 20 次没有意义，
            // 睡到节拍点后唤醒次数与查库次数同阶。
            let next_poll = self
                .last_poll
                .map(|last| last + self.interval)
                .unwrap_or(now);
            let wake_at = next_poll.min(deadline);
            let nap = wake_at.saturating_duration_since(now);
            if !nap.is_zero() {
                std::thread::sleep(nap);
            }
        }
    }
}

/// 订阅建立时的对齐锚点：最新一轮的标识、状态与接收时间。
///
/// 客户端拿它和自己读到的索引比对：索引落后就再读一次，interaction.created 帧
/// 从此只负责「订阅之后」的新轮，不再承担补齐历史的职责。
fn baseline_payload(head: &InteractionTurnHead) -> serde_json::Value {
    serde_json::json!({
        "latestInteractionId": head.interaction_id,
        "latestInteractionStatus": status_name(head.status),
        "latestInteractionReceivedAtMs": head.received_at_ms,
    })
}

/// 终态判定：与索引页的状态表达式同一套取值。
fn is_terminal(status: &str) -> bool {
    matches!(status, "understood" | "rejected" | "interrupted")
}

/// 状态取值与库里的派生列同名：协议帧直接用这一套词。
fn status_name(status: axon_domain::InteractionStatus) -> &'static str {
    match status {
        axon_domain::InteractionStatus::Received => "received",
        axon_domain::InteractionStatus::Processing => "processing",
        axon_domain::InteractionStatus::Understood => "understood",
        axon_domain::InteractionStatus::Rejected => "rejected",
        axon_domain::InteractionStatus::Interrupted => "interrupted",
    }
}

/// 订阅目标解析：`session-turns:` 前缀只认会话标识。
pub(super) fn session_turns_target(subscribe_id: &str) -> Option<&str> {
    let session_id = subscribe_id.strip_prefix("session-turns:")?.trim();
    if session_id.is_empty() {
        None
    } else {
        Some(session_id)
    }
}

#[cfg(test)]
#[path = "gateway_session_turns_stream_tests.rs"]
mod tests;
