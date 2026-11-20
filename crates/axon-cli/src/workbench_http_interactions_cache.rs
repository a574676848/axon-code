//! 索引页载荷缓存：同一会话在同一数据水位上的同一页只组装一次。
//!
//! 为什么需要它：切回一个刚看过的会话时，索引页要读一页 Interaction、再用一次事件尾扫
//! 补逐轮读数与最终正文；数据没变，重复付出的是几十毫秒与若干次读盘。缓存键因此必须
//! 包含「数据有没有变」的证据：**事件水位 + 交互条数**。两者任一前进，键就不同，旧条目
//! 自然失效，不需要额外的失效通知，也不会把旧页发给客户端。
//!
//! 条数也进键，是因为新消息先落 `inbox` 再产生事件：只认水位会让「刚发出、还没开跑」
//! 的那一轮在若干毫秒里从索引页消失。
//!
//! 只缓存首页（无翻页游标）：翻页是「更早的一页」，与「切回看过的会话」不是同一种访问，
//! 缓存它只会挤掉真正复用的条目。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use super::runtime_store;

/// 缓存条目数上限。
const MAX_ENTRIES: usize = 32;
/// 缓存载荷总字节上限；单页载荷正常在几百 KB 量级。
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// 一条缓存键：数据根、会话、事件水位、交互条数、页大小与翻页档。
///
/// 数据根必须进键：同一进程里 `serve` 与 daemon 可能指向不同运行态目录，测试也是
/// 各自一个临时根；漏掉它会把 A 根的一页发给 B 根。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    root: PathBuf,
    session_id: String,
    event_sequence: u64,
    interaction_count: u64,
    limit: u32,
    brief: bool,
}

/// 进程内索引页缓存。
#[derive(Default)]
struct Cache {
    /// 最近的条目在前；命中时移到最前，淘汰从尾部开始。
    entries: VecDeque<(CacheKey, Arc<serde_json::Value>, usize)>,
    bytes: usize,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

/// 命中就整页返回，未命中则调用 `build` 组装并入库。
///
/// 会话行不存在时一是取不到水位、二是载荷本身仍可读（能力块为 null），此时直接组装、
/// 不入缓存：没有水位就没有失效依据，宁可每次重算也不发旧页。
pub(super) fn cached_page<F>(
    axon_root: &Path,
    session_id: &str,
    limit: u32,
    brief: bool,
    build: F,
) -> Result<serde_json::Value, String>
where
    F: FnOnce() -> Result<serde_json::Value, String>,
{
    let Some((event_sequence, interaction_count)) = freshness(axon_root, session_id) else {
        return build();
    };
    let key = CacheKey {
        root: axon_root.to_path_buf(),
        session_id: session_id.to_owned(),
        event_sequence,
        interaction_count,
        limit,
        brief,
    };
    if let Some(payload) = lookup(&key) {
        return Ok(payload);
    }
    let payload = build()?;
    store(&key, &payload);
    Ok(payload)
}

/// 这条会话当前的新鲜度键：事件水位与交互条数；会话行不存在时为 `None`。
fn freshness(axon_root: &Path, session_id: &str) -> Option<(u64, u64)> {
    use rusqlite::OptionalExtension as _;
    let row = runtime_store(axon_root)
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT s.last_event_sequence,
                            (SELECT COUNT(*) FROM inbox AS i WHERE i.session_id = s.session_id)
                       FROM sessions AS s WHERE s.session_id = ?1",
                    [session_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
        })
        .ok()
        .flatten()?;
    Some((
        u64::try_from(row.0).unwrap_or(0),
        u64::try_from(row.1).unwrap_or(0),
    ))
}

/// 命中则返回整页并把条目提到最前。
fn lookup(key: &CacheKey) -> Option<serde_json::Value> {
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let position = cache
        .entries
        .iter()
        .position(|(candidate, _, _)| candidate == key)?;
    let entry = cache.entries.remove(position)?;
    let payload = Arc::clone(&entry.1);
    cache.entries.push_front(entry);
    Some((*payload).clone())
}

/// 写入一条载荷；超出条数或字节上限时从最旧开始淘汰。
fn store(key: &CacheKey, payload: &serde_json::Value) {
    let size = serde_json::to_vec(payload).map_or(0, |bytes| bytes.len());
    if size == 0 || size > MAX_BYTES {
        return;
    }
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(position) = cache
        .entries
        .iter()
        .position(|(candidate, _, _)| candidate == key)
        && let Some((_, _, previous)) = cache.entries.remove(position)
    {
        cache.bytes = cache.bytes.saturating_sub(previous);
    }
    cache
        .entries
        .push_front((key.clone(), Arc::new(payload.clone()), size));
    cache.bytes = cache.bytes.saturating_add(size);
    while cache.entries.len() > MAX_ENTRIES || cache.bytes > MAX_BYTES {
        let Some((_, _, evicted)) = cache.entries.pop_back() else {
            break;
        };
        cache.bytes = cache.bytes.saturating_sub(evicted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(session_id: &str, event_sequence: u64, interaction_count: u64) -> CacheKey {
        CacheKey {
            root: PathBuf::from("/fixture-root"),
            session_id: session_id.to_owned(),
            event_sequence,
            interaction_count,
            limit: 200,
            brief: false,
        }
    }

    #[test]
    fn lookup_returns_the_stored_payload_for_the_same_key() {
        let key = key("session-cache-hit", 7, 3);
        store(&key, &serde_json::json!({ "interactions": [1, 2] }));
        let hit = lookup(&key).expect("同键必须命中");
        assert_eq!(hit["interactions"][1], 2);
    }

    #[test]
    fn a_moving_watermark_or_row_count_misses_the_cache() {
        let stored = key("session-cache-invalidate", 7, 3);
        store(&stored, &serde_json::json!({ "value": "old" }));
        assert!(
            lookup(&key("session-cache-invalidate", 8, 3)).is_none(),
            "事件水位前进后必须失效"
        );
        assert!(
            lookup(&key("session-cache-invalidate", 7, 4)).is_none(),
            "交互条数变化后必须失效"
        );
        assert!(lookup(&stored).is_some(), "未变化的那一条必须仍然命中");
    }

    #[test]
    fn the_cache_stays_within_its_entry_bound() {
        for index in 0..(MAX_ENTRIES + 4) {
            store(
                &key(&format!("session-bound-{index}"), 1, 1),
                &serde_json::json!({ "index": index }),
            );
        }
        let cache = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            cache.entries.len() <= MAX_ENTRIES,
            "条数上限必须生效，实际 {}",
            cache.entries.len()
        );
        assert!(
            cache.bytes <= MAX_BYTES,
            "字节上限必须生效，实际 {}",
            cache.bytes
        );
    }
}
