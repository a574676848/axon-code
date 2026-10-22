//! Gateway 外部事件防抖：按业务关联键做滑动窗口合并。

use axon_domain::InteractionId;
use axon_protocol::SubmitInteractionReceipt;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// 防抖窗口默认时长。
pub const GATEWAY_DEBOUNCE_DEFAULT_WINDOW: Duration = Duration::from_secs(3);
/// 防抖表最多跟踪的关联键数量；超出时先清理已过期条目。
const GATEWAY_DEBOUNCE_MAX_ENTRIES: usize = 1_024;

/// 关联键滑动窗口防抖器。
///
/// 窗口内的重复事件不再进入 durable ingress：它们合并到窗口代表事件上，
/// 回执复用代表事件的 Interaction ID 并标记 `deduplicated`，因此既不会
/// 重复唤醒主 turn，也不会让调用方拿到不存在的 Interaction。
#[derive(Debug)]
pub struct GatewayDebouncer {
    window_ms: u64,
    entries: Mutex<HashMap<String, DebounceEntry>>,
}

#[derive(Debug)]
struct DebounceEntry {
    deadline_ms: u64,
    interaction_id: InteractionId,
}

impl GatewayDebouncer {
    /// 创建指定窗口长度的防抖器。
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self {
            window_ms: u64::try_from(window.as_millis()).unwrap_or(u64::MAX).max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// 判断事件是否被窗口吸收；命中时刷新滑动截止时间并返回窗口代表事件回执。
    pub fn absorb(&self, correlation_key: &str, now_ms: u64) -> Option<SubmitInteractionReceipt> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(correlation_key)
            && entry.deadline_ms > now_ms
        {
            entry.deadline_ms = now_ms.saturating_add(self.window_ms);
            return Some(SubmitInteractionReceipt {
                interaction_id: entry.interaction_id.clone(),
                accepted: true,
                deduplicated: true,
            });
        }
        entries.remove(correlation_key);
        None
    }

    /// 登记窗口代表事件；随后同键事件在窗口内合并到该事件。
    pub fn register(&self, correlation_key: &str, now_ms: u64, interaction_id: InteractionId) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        prune(&mut entries, now_ms);
        entries.insert(
            correlation_key.to_owned(),
            DebounceEntry {
                deadline_ms: now_ms.saturating_add(self.window_ms),
                interaction_id,
            },
        );
    }
}

fn prune(entries: &mut HashMap<String, DebounceEntry>, now_ms: u64) {
    if entries.len() < GATEWAY_DEBOUNCE_MAX_ENTRIES {
        return;
    }
    entries.retain(|_, entry| entry.deadline_ms > now_ms);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_millis(3_000);

    fn interaction_id(value: &str) -> InteractionId {
        InteractionId::new(value.to_owned()).expect("合法 Interaction ID")
    }

    #[test]
    fn window_absorbs_repeats_and_slides_the_deadline() {
        let debouncer = GatewayDebouncer::new(WINDOW);
        assert!(debouncer.absorb("jira:AXON-1:assigned", 1_000).is_none());
        debouncer.register(
            "jira:AXON-1:assigned",
            1_000,
            interaction_id("interaction-1"),
        );
        let receipt = debouncer
            .absorb("jira:AXON-1:assigned", 2_000)
            .expect("窗口内重复事件必须被吸收");
        assert_eq!(receipt.interaction_id.as_str(), "interaction-1");
        assert!(receipt.accepted);
        assert!(receipt.deduplicated);
        // 滑动窗口：2_000 的重复事件把截止时间推到 5_000，4_999 仍在窗口内。
        assert!(debouncer.absorb("jira:AXON-1:assigned", 4_999).is_some());
        assert!(debouncer.absorb("other:key", 4_999).is_none());
    }

    #[test]
    fn expired_window_stops_absorbing() {
        let debouncer = GatewayDebouncer::new(WINDOW);
        debouncer.register(
            "jira:AXON-1:assigned",
            1_000,
            interaction_id("interaction-1"),
        );
        assert!(debouncer.absorb("jira:AXON-1:assigned", 4_001).is_none());
        assert!(debouncer.absorb("jira:AXON-1:assigned", 4_002).is_none());
    }

    #[test]
    fn table_prunes_expired_entries_at_capacity() {
        let debouncer = GatewayDebouncer::new(WINDOW);
        for index in 0..GATEWAY_DEBOUNCE_MAX_ENTRIES {
            debouncer.register(
                &format!("key-{index}"),
                index as u64 + 1,
                interaction_id(&format!("interaction-{index}")),
            );
        }
        debouncer.register(
            "key-fresh",
            GATEWAY_DEBOUNCE_MAX_ENTRIES as u64 + WINDOW.as_millis() as u64 + 1,
            interaction_id("interaction-fresh"),
        );
        let entries = debouncer.entries.lock().expect("防抖表可加锁");
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("key-fresh"));
    }
}
