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
    fn evicted_entries_fall_back_to_the_store() {
        let cache = SessionAdmissionCache::new(2, 10_000);
        cache.put("session-a", &admission("workspace-a"), 10);
        cache.put("session-b", &admission("workspace-b"), 20);
        // 容量触顶后淘汰最旧的一半：被淘汰的会话必须重新回表。
        cache.put("session-c", &admission("workspace-c"), 30);

        assert_eq!(cache.get("session-a", 40), None, "被淘汰的会话必须回表");
        assert!(cache.get("session-b", 40).is_some(), "较新的条目保留");
        assert!(cache.get("session-c", 40).is_some());
    }

    /// 订阅放行：命中缓存时帧路上必须零查库。
    ///
    /// 存储架构 §10.1 的红线是「订阅放行帧路零查库，命中进程内缓存」。放行发生在每一帧上，
    /// 因此这里按微秒计量并把上限钉在 50µs：超过它说明缓存退化成回表或锁竞争。
    #[test]
    #[ignore = "性能验收：订阅放行的缓存命中必须零查库，p95 上限 50µs"]
    fn cached_subscription_admission_stays_within_the_frame_budget() {
        const SESSION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        const SAMPLES: usize = 100_000;

        let cache = SessionAdmissionCache::new(1_024, 30_000);
        cache.put(SESSION_ID, &admission("workspace-a"), 1_000);
        for _ in 0..1_000 {
            assert!(cache.get(SESSION_ID, 1_001).is_some(), "预热必须命中");
        }

        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = std::time::Instant::now();
            let hit = cache.get(SESSION_ID, 1_001);
            let elapsed_us = started.elapsed().as_secs_f64() * 1_000_000.0;
            assert!(hit.is_some(), "缓存必须命中，命中时才谈得上零查库");
            samples.push(elapsed_us);
        }
        samples.sort_by(|left, right| left.partial_cmp(right).expect("微秒读数可比"));
        let p95_us = samples[samples.len() * 95 / 100];
        println!("subscription_admission_cached_us={p95_us:.3} samples={SAMPLES} 门槛=50us");
        assert!(
            p95_us <= 50.0,
            "订阅放行缓存命中超出 50µs：p95={p95_us:.3}µs"
        );
    }
}
