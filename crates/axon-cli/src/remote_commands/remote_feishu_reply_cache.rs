//! Feishu 引用正文的 worker 私有双限 LRU cache。

use std::collections::{HashMap, VecDeque};

/// 引用正文 cache 最大条目数。
pub(super) const REMOTE_FEISHU_REPLY_CACHE_MAX_ENTRIES: usize = 512;
/// 引用正文 cache 最大 UTF-8 字节数。
pub(super) const REMOTE_FEISHU_REPLY_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheEntry {
    value: Option<String>,
    bytes: usize,
}

/// cache 插入结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReplyCacheInsertReport {
    pub(super) inserted: bool,
    pub(super) evicted_by_entries: usize,
    pub(super) evicted_by_bytes: usize,
}

/// 同时受条目数与总字节数约束的 LRU cache。
#[derive(Debug)]
pub(super) struct RemoteFeishuReplyCache {
    entries: HashMap<String, CacheEntry>,
    lru: VecDeque<String>,
    total_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for RemoteFeishuReplyCache {
    fn default() -> Self {
        Self::new(
            REMOTE_FEISHU_REPLY_CACHE_MAX_ENTRIES,
            REMOTE_FEISHU_REPLY_CACHE_MAX_BYTES,
        )
    }
}

impl RemoteFeishuReplyCache {
    pub(super) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            lru: VecDeque::new(),
            total_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1),
        }
    }

    pub(super) fn get(&mut self, message_id: &str) -> Option<Option<String>> {
        let value = self.entries.get(message_id)?.value.clone();
        self.touch(message_id);
        Some(value)
    }

    pub(super) fn insert(
        &mut self,
        message_id: String,
        value: Option<String>,
    ) -> ReplyCacheInsertReport {
        let bytes = entry_bytes(&message_id, value.as_deref());
        if message_id.is_empty() || bytes > self.max_bytes {
            return ReplyCacheInsertReport::default();
        }
        self.remove(&message_id);
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.entries
            .insert(message_id.clone(), CacheEntry { value, bytes });
        self.lru.push_back(message_id);
        let mut report = ReplyCacheInsertReport {
            inserted: true,
            ..ReplyCacheInsertReport::default()
        };
        while self.entries.len() > self.max_entries {
            self.evict_oldest();
            report.evicted_by_entries += 1;
        }
        while self.total_bytes > self.max_bytes {
            self.evict_oldest();
            report.evicted_by_bytes += 1;
        }
        report
    }

    fn touch(&mut self, message_id: &str) {
        if let Some(index) = self.lru.iter().position(|key| key == message_id) {
            self.lru.remove(index);
        }
        self.lru.push_back(message_id.to_owned());
    }

    fn remove(&mut self, message_id: &str) {
        if let Some(entry) = self.entries.remove(message_id) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        }
        if let Some(index) = self.lru.iter().position(|key| key == message_id) {
            self.lru.remove(index);
        }
    }

    fn evict_oldest(&mut self) {
        let Some(message_id) = self.lru.pop_front() else {
            return;
        };
        if let Some(entry) = self.entries.remove(&message_id) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        }
    }
}

fn entry_bytes(message_id: &str, value: Option<&str>) -> usize {
    message_id.len().saturating_add(value.map_or(0, str::len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keeps_negative_result_and_refreshes_lru_hit() {
        let mut cache = RemoteFeishuReplyCache::new(2, 128);
        cache.insert("a".to_owned(), None);
        cache.insert("b".to_owned(), Some("B".to_owned()));

        assert_eq!(cache.get("a"), Some(None));
        cache.insert("c".to_owned(), Some("C".to_owned()));

        assert_eq!(cache.get("b"), None);
        assert_eq!(cache.get("a"), Some(None));
        assert_eq!(cache.get("c"), Some(Some("C".to_owned())));
    }

    #[test]
    fn cache_enforces_byte_limit_and_skips_oversized_entry() {
        let mut cache = RemoteFeishuReplyCache::new(8, 8);
        cache.insert("a".to_owned(), Some("1234".to_owned()));
        let report = cache.insert("b".to_owned(), Some("5678".to_owned()));

        assert_eq!(report.evicted_by_bytes, 1);
        assert_eq!(cache.get("a"), None);
        assert_eq!(cache.get("b"), Some(Some("5678".to_owned())));

        let oversized = cache.insert("c".to_owned(), Some("x".repeat(9)));
        assert!(!oversized.inserted);
        assert_eq!(cache.get("c"), None);
    }
}
