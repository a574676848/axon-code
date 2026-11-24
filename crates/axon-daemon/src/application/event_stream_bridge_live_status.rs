//! 订阅建立之前发出的瞬态读数：留最后一帧，供新订阅者补看。
//!
//! provider 状态这类读数只走实时通道、没有 durable 副本，订阅建立前发出的那些
//! 本来会永久丢失：界面只能挂一句中性占位，直到下一次真实 provider 请求。
//! 与 `event_stream_bridge` 的分发逻辑分开成文件，是为了让「留什么、留多久」
//! 独立于「投给谁」演进。

use super::event_stream_bridge_notifications::LiveStreamEvent;
use axon_domain::{DaemonSessionId, InteractionId};
use std::collections::HashMap;

/// 可补发瞬态读数的保鲜期。
///
/// provider 状态只在「此刻确实在请求模型」时有意义：十分钟前的 `generating`
/// 补出去就是把过期事实当成现状，比空白更误导。补发前一律按这条时效裁剪。
pub(super) const REMEMBERED_LIVE_TTL_MS: u64 = 60_000;

/// 一帧可补发的瞬态读数。
#[derive(Debug, Clone)]
struct RememberedLive {
    event: LiveStreamEvent,
    captured_at_ms: u64,
}

/// `(Session, Interaction)` → 最后一帧可补发的瞬态读数。
///
/// 只装低频读数（provider 状态每轮几十帧）。正文增量与用量每 token 一帧，
/// 一律不进这张表，因此它的写入频率与锁竞争都可以忽略。
#[derive(Debug, Default)]
pub(super) struct RememberedLiveTable {
    by_target: HashMap<(DaemonSessionId, InteractionId), RememberedLive>,
}

impl RememberedLiveTable {
    /// 记下最后一帧；同一轮次后到的覆盖先到的。
    ///
    /// 顺手清掉过期条目：表的体量等于「保鲜期内发生过 provider 请求的轮次数」，
    /// 不清也不会暴涨，但能让补发判断只留一道。
    pub(super) fn remember(&mut self, event: &LiveStreamEvent, captured_at_ms: u64) {
        self.by_target
            .retain(|_, remembered| is_fresh(remembered.captured_at_ms, captured_at_ms));
        self.by_target.insert(
            (event.session_id.clone(), event.interaction_id.clone()),
            RememberedLive {
                event: event.clone(),
                captured_at_ms,
            },
        );
    }

    /// 轮次收口：丢掉该轮的读数。
    ///
    /// 收口之后它不再代表任何正在进行的事，留着只会在下一次订阅时被补发出去，
    /// 把上一轮的阶段当成新一轮的现状。
    pub(super) fn forget(&mut self, session_id: &DaemonSessionId, interaction_id: &InteractionId) {
        self.by_target
            .remove(&(session_id.clone(), interaction_id.clone()));
    }

    /// 取一帧可补发的读数：只给还在保鲜期内的。
    ///
    /// 保鲜按「此刻」判断而不是记账时刻：写入与订阅之间可能隔着一次调度，
    /// 也可能隔着整段空闲。
    pub(super) fn replay_for(
        &self,
        session_id: &DaemonSessionId,
        interaction_id: &InteractionId,
        now_ms: u64,
    ) -> Option<LiveStreamEvent> {
        let remembered = self
            .by_target
            .get(&(session_id.clone(), interaction_id.clone()))?;
        is_fresh(remembered.captured_at_ms, now_ms).then(|| remembered.event.clone())
    }
}

/// 读数是否还在保鲜期内。
fn is_fresh(captured_at_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(captured_at_ms) <= REMEMBERED_LIVE_TTL_MS
}

/// 当前 Unix 毫秒；与 `RuntimeClock::now_ms` 同一基准。
pub(super) fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}
