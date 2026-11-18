//! 会话放行缓存：入站与订阅共用同一份「会话存在 + 归属 + 受众边界」判定。
//!
//! 会话的这三项事实在创建之后不再变化（`kind`、`workspace_id`、`trust_scope` 都只在建行时
//! 写入），因此一次主键查找的结果可以安全复用到下一次入站。缓存定容 + TTL：会话被保留期
//! 清理后最多再放行 TTL 这么长时间，读不到事件自然收场，不会因此泄漏内容。

use std::collections::HashMap;
use std::sync::Mutex;

use axon_session::SessionTrustScope;

/// 放行判定所需的会话事实。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SessionAdmission {
    /// 会话归属的 Workspace；未绑定时为空。
    pub(super) workspace_id: Option<String>,
    /// 受众隔离维度。
    pub(super) trust_scope: SessionTrustScope,
}

/// 一条缓存记录。
#[derive(Clone)]
struct CachedAdmission {
    admission: SessionAdmission,
    recorded_at_ms: u64,
}

/// 定容 + TTL 的会话放行缓存。
pub(super) struct SessionAdmissionCache {
    capacity: usize,
    ttl_ms: u64,
    entries: Mutex<HashMap<String, CachedAdmission>>,
}

impl SessionAdmissionCache {
    /// 建一个定容缓存；容量为 0 时退化成不缓存（每次都回表）。
    pub(super) fn new(capacity: usize, ttl_ms: u64) -> Self {
        Self {
            capacity,
            ttl_ms,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// 取一条未过期的判定；过期条目顺手清掉。
    pub(super) fn get(&self, session_id: &str, now_ms: u64) -> Option<SessionAdmission> {
        if self.capacity == 0 {
            return None;
        }
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.get(session_id)?;
        if now_ms.saturating_sub(entry.recorded_at_ms) >= self.ttl_ms {
            entries.remove(session_id);
            return None;
        }
        Some(entry.admission.clone())
    }

    /// 记下一条判定；满了先清过期条目，仍然满就丢最旧的一半。
    pub(super) fn put(&self, session_id: &str, admission: &SessionAdmission, now_ms: u64) {
        if self.capacity == 0 {
            return;
        }
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if !entries.contains_key(session_id) && entries.len() >= self.capacity {
            let ttl_ms = self.ttl_ms;
            entries.retain(|_, entry| now_ms.saturating_sub(entry.recorded_at_ms) < ttl_ms);
            if entries.len() >= self.capacity {
                evict_oldest_half(&mut entries);
            }
        }
        entries.insert(
            session_id.to_owned(),
            CachedAdmission {
                admission: admission.clone(),
                recorded_at_ms: now_ms,
            },
        );
    }

    /// 丢掉一条判定：订阅关闭时调用，避免为没人再看的会话保留条目。
    pub(super) fn invalidate(&self, session_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(session_id);
        }
    }

    /// 当前条目数；只给用例断言用。
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }
}

/// 丢掉最旧的一半条目：容量满时保住最近用过的那些。
fn evict_oldest_half(entries: &mut HashMap<String, CachedAdmission>) {
    let mut recorded: Vec<(String, u64)> = entries
        .iter()
        .map(|(session_id, entry)| (session_id.clone(), entry.recorded_at_ms))
        .collect();
    recorded.sort_by_key(|(_, recorded_at_ms)| *recorded_at_ms);
    for (session_id, _) in recorded.into_iter().take(entries.len().div_ceil(2)) {
        entries.remove(&session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionAdmission, SessionAdmissionCache};
    use axon_session::SessionTrustScope;

    fn admission(workspace: &str) -> SessionAdmission {
        SessionAdmission {
            workspace_id: Some(workspace.to_owned()),
            trust_scope: SessionTrustScope::Private,
        }
    }

    #[test]
    fn hit_returns_the_recorded_admission_until_the_ttl_expires() {
        let cache = SessionAdmissionCache::new(8, 1_000);
        cache.put("session-a", &admission("workspace-a"), 10);

        assert_eq!(
            cache.get("session-a", 20),
            Some(admission("workspace-a")),
            "TTL 内必须命中"
        );
        assert_eq!(cache.get("session-a", 1_010), None, "到点必须过期");
        assert_eq!(cache.len(), 0, "过期条目顺手清掉");
    }

    #[test]
    fn capacity_eviction_keeps_the_newest_entries() {
        let cache = SessionAdmissionCache::new(2, 10_000);
        cache.put("session-old", &admission("workspace-a"), 10);
        cache.put("session-mid", &admission("workspace-a"), 20);
        cache.put("session-new", &admission("workspace-a"), 30);

        // 满容量时先丢最旧的一半（这里是 1 条），再插入新的：剩下的应是最新的两条。
        assert_eq!(cache.len(), 2, "容量是硬上限");
        assert!(cache.get("session-new", 40).is_some(), "最新的一定保留");
        assert!(cache.get("session-mid", 40).is_some(), "次新的保留");
        assert!(cache.get("session-old", 40).is_none(), "最旧的先丢");
    }

    #[test]
    fn zero_capacity_never_caches() {
        let cache = SessionAdmissionCache::new(0, 10_000);
        cache.put("session-a", &admission("workspace-a"), 10);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get("session-a", 20), None);
    }

    #[test]
    fn invalidate_evicts_only_the_named_session() {
        let cache = SessionAdmissionCache::new(8, 10_000);
        cache.put("session-a", &admission("workspace-a"), 10);
        cache.put("session-b", &admission("workspace-b"), 10);

        cache.invalidate("session-a");

        assert_eq!(cache.get("session-a", 20), None, "被失效的会话必须回表");
        assert!(cache.get("session-b", 20).is_some(), "别的会话不受影响");
    }
}
