//! 当前会话上下文：通知与界面共用的一份会话归属。
//!
//! 两件事共用这一份事实，因为它们问的是同一个问题「这一刻用户在哪个会话里」：
//!
//! - **通知标题**要带上会话名：只写「本轮已完成」，用户得点进去才知道是哪个会话；
//! - **通知点击**要能回到对应会话：系统通知与窗口之间没有直接的回调通道，
//!   因此通知发出时记下待打开的会话，窗口重新获得焦点即消费它。
//!
//! 上下文由界面在会话切换时写入（桌面端同一时刻只订阅当前会话的事件流，
//! 因此收到轮次终态的一定就是它）。待打开目标带存活时长：过期的目标不再消费，
//! 免得几天之后某次普通的窗口聚焦把人带到一条早已不相关的会话上。

use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 打开会话事件名，与前端 `ipc.ts` 中的常量一致。
pub const LANE_OPEN_EVENT: &str = "axon://open-lane";

/// 待打开目标的最长存活时长。
///
/// 窗口聚焦既可能来自点击通知，也可能来自用户自己切回来；两者无法区分，
/// 因此只在通知刚发出不久时认这一次目标，超时即放弃。
const PENDING_OPEN_TTL_MS: u64 = 120_000;

/// 一次会话上下文快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneContext {
    /// 车道稳定标识。
    pub lane_id: String,
    /// 车道展示标题；界面上的会话名。
    pub title: String,
}

/// 当前时刻（毫秒）。
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn active_slot() -> &'static Mutex<Option<LaneContext>> {
    static ACTIVE: OnceLock<Mutex<Option<LaneContext>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// 待打开目标：会话标识与记下的时刻。
fn pending_slot() -> &'static Mutex<Option<(String, u64)>> {
    static PENDING: OnceLock<Mutex<Option<(String, u64)>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

/// 记下当前会话；空白标识视为「没有会话」并清空。
pub fn set_active_lane(lane_id: &str, title: &str) {
    let lane_id = lane_id.trim();
    let mut slot = active_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if lane_id.is_empty() {
        *slot = None;
        return;
    }
    *slot = Some(LaneContext {
        lane_id: lane_id.to_string(),
        title: title.trim().to_string(),
    });
}

/// 取当前会话。
#[must_use]
pub fn active_lane() -> Option<LaneContext> {
    active_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// 记下「点这条通知应当打开的会话」。
pub fn remember_pending_open(lane_id: &str) {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return;
    }
    let mut slot = pending_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = Some((lane_id.to_string(), now_millis()));
}

/// 取走尚未过期的待打开会话；过期或没有时返回 None。
///
/// 取走即清空：同一次通知只允许把人带过去一次，反复聚焦不该反复跳转。
#[must_use]
pub fn take_pending_open() -> Option<String> {
    let mut slot = pending_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (lane_id, at_ms) = slot.take()?;
    let fresh = now_millis().saturating_sub(at_ms) <= PENDING_OPEN_TTL_MS;
    fresh.then_some(lane_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lane_identity_clears_the_context_instead_of_storing_a_placeholder() {
        set_active_lane("lane-a", "会话甲");
        assert_eq!(
            active_lane().map(|context| context.title),
            Some("会话甲".to_string())
        );
        set_active_lane("   ", "");
        assert!(active_lane().is_none(), "空白标识必须清空上下文");
    }

    #[test]
    fn pending_open_target_is_consumed_once_and_expires() {
        remember_pending_open("");
        assert!(take_pending_open().is_none(), "空白标识不得留下待打开目标");

        remember_pending_open("lane-b");
        assert_eq!(take_pending_open(), Some("lane-b".to_string()));
        assert!(
            take_pending_open().is_none(),
            "同一次通知只允许把人带过去一次"
        );

        // 过期目标不再消费，免得几天后的普通聚焦把人带到无关会话上。
        // 同一份全局状态下按顺序断言，避免多个用例并发改写同一个槽位。
        {
            let mut slot = pending_slot()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *slot = Some((
                "lane-c".to_string(),
                now_millis().saturating_sub(PENDING_OPEN_TTL_MS + 1),
            ));
        }
        assert!(
            take_pending_open().is_none(),
            "超过存活时长的目标必须被丢弃"
        );
    }
}
