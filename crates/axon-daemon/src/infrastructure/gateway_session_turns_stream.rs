//! 会话轮次头订阅：只推「新增一轮」与「这一轮收口了」，不推正文。
//!
//! 独立会话（目标执行、子代理）不在侧栏清单里，前端没有别的渠道发现「又多了一轮」。
//! 这一路订阅因此只做差异比较：按固定节拍把会话最近的轮次清单读一遍，比上次多出来的
//! 报 `interaction.created`，状态从在途变成终态的报 `interaction.settled`。
//!
//! 清单直接走 `interactions_for_session` 端口：终态判定与索引页同源（同一个状态表达式），
//! 这里不再自己拼 SQL，否则两处口径迟早漂移。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axon_domain::DaemonSessionId;

use super::gateway_kernel::{GatewayEvent, GatewayEventStream, GatewayKernelError};

/// 节拍：轻帧允许一个轮询间隔的延迟，换掉的是不必给每条会话常驻一条事件流。
const TURNS_POLL_INTERVAL: Duration = Duration::from_millis(700);
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
    /// 第一次读取只建立基线，不补发历史：历史由索引页负责。
    primed: bool,
    last_poll: Option<Instant>,
    pending: Vec<GatewayEvent>,
}

impl<S> SessionTurnsStream<S>
where
    S: axon_session::InteractionQueryStore + Clone,
{
    pub(super) fn new(store: S, session_id: DaemonSessionId) -> Self {
        Self {
            store,
            session_id,
            known: HashSet::new(),
            settled: HashSet::new(),
            primed: false,
            last_poll: None,
            pending: Vec::new(),
        }
    }

    fn due(&mut self) -> bool {
        let now = Instant::now();
        let due = self
            .last_poll
            .is_none_or(|last| now.duration_since(last) >= TURNS_POLL_INTERVAL);
        if due {
            self.last_poll = Some(now);
        }
        due
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
            if !self.primed {
                // 基线：已有的轮次都算「已经知道」，已收口的直接记为已播报。
                if is_terminal(status) {
                    self.settled.insert(interaction_id);
                }
                continue;
            }
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
        self.primed = true;
        // 修剪：只保留仍在这个窗口里的标识，旧标识离窗后不会再出现在头部比较里。
        if self.known.len() > TURNS_KNOWN_LIMIT {
            self.known.retain(|id| current.contains(id.as_str()));
        }
        if self.settled.len() > TURNS_KNOWN_LIMIT {
            self.settled.retain(|id| current.contains(id.as_str()));
        }
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
                .map(|last| last + TURNS_POLL_INTERVAL)
                .unwrap_or(now);
            let wake_at = next_poll.min(deadline);
            let nap = wake_at.saturating_duration_since(now);
            if !nap.is_zero() {
                std::thread::sleep(nap);
            }
        }
    }
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
