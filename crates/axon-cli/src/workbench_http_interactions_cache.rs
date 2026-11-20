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

/// 同一条数据根最多保留多少页。
///
/// 按根分桶而不是全局共用一份名额：进程里只有 daemon 一个数据根，而测试与 `serve`
/// 各自独立；全局名额会让并行跑的多个根互相把对方的条目挤掉，缓存因此变得不可预测。
const MAX_ENTRIES_PER_ROOT: usize = 8;
/// 全局条目数上限：防止大量数据根把内存撑大。
const MAX_ENTRIES: usize = 256;
/// 缓存载荷总字节上限；单页载荷正常在几百 KB 量级。
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// 一条缓存键：数据根、会话、事件水位、交互条数、标题、两张投影表的更新时间、
/// 页大小与翻页档。
///
/// 水位与条数只证明「事件写了没有」，而派生事实是在水位提交之后才写的：逐轮答案落在
/// `turn_activity`、累计读数落在 `session_metrics`，标题由后台线程单独写。把这三样一起
/// 放进键，缓存才覆盖得住「事件已经落定、投影随后补齐」的那一段。
///
/// 数据根必须进键：同一进程里 `serve` 与 daemon 可能指向不同运行态目录，测试也是
/// 各自一个临时根；漏掉它会把 A 根的一页发给 B 根。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    root: PathBuf,
    session_id: String,
    event_sequence: u64,
    interaction_count: u64,
    title: String,
    activity_updated_ms: u64,
    metrics_updated_ms: u64,
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
/// 新鲜度探测失败只在第一次留痕。
static FRESHNESS_WARNED: std::sync::Once = std::sync::Once::new();

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
    let Some(freshness) = freshness(axon_root, session_id) else {
        return build();
    };
    let key = CacheKey {
        root: axon_root.to_path_buf(),
        session_id: session_id.to_owned(),
        event_sequence: freshness.event_sequence,
        interaction_count: freshness.interaction_count,
        title: freshness.title,
        activity_updated_ms: freshness.activity_updated_ms,
        metrics_updated_ms: freshness.metrics_updated_ms,
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

/// 这条会话当前的新鲜度键；会话行不存在时为 `None`。
///
/// 四层事实合起来才够：事件水位（有没有新事件）、交互条数（有没有新入站行）、标题
/// （后台线程写的重命名）、两张投影表的更新时间（答案与读数补齐了没有）。
fn freshness(axon_root: &Path, session_id: &str) -> Option<Freshness> {
    use rusqlite::OptionalExtension as _;
    let row = runtime_store(axon_root)
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT s.last_event_sequence,
                            (SELECT COUNT(*) FROM inbox AS i WHERE i.session_id = s.session_id),
                            COALESCE(s.title, ''),
                            (SELECT COALESCE(MAX(a.updated_at_ms), 0) FROM turn_activity AS a
                              WHERE a.session_id = s.session_id),
                            COALESCE((SELECT m.updated_at_ms FROM session_metrics AS m
                                       WHERE m.session_id = s.session_id), 0)
                       FROM sessions AS s WHERE s.session_id = ?1",
                    [session_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()
        })
        /*
         * 探测失败就退成「不缓存」：拿不到新鲜度就没有失效依据，宁可每次重算也不发旧页。
         *
         * 但失败只报一次：这是每请求都会跑的路径，真出错（例如 schema 漂移让某一列
         * 消失）必须留痕，否则缓存会静默关闭而没人知道。
         */
        .map_err(|error| {
            FRESHNESS_WARNED.call_once(|| {
                axon_logger::warning(&format!(
                    "索引页缓存新鲜度探测失败，缓存停用直到重启：{error:?}"
                ));
            });
            error
        })
        .ok()
        .flatten()?;
    Some(Freshness {
        event_sequence: u64::try_from(row.0).unwrap_or(0),
        interaction_count: u64::try_from(row.1).unwrap_or(0),
        title: row.2,
        activity_updated_ms: u64::try_from(row.3).unwrap_or(0),
        metrics_updated_ms: u64::try_from(row.4).unwrap_or(0),
    })
}

/// 一次新鲜度探测的结果。
struct Freshness {
    event_sequence: u64,
    interaction_count: u64,
    title: String,
    activity_updated_ms: u64,
    metrics_updated_ms: u64,
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
    // 同一数据根只留最近若干页：从最旧（尾部）开始删本根多出来的条目。
    let mut seen_same_root = 0usize;
    let mut index = cache.entries.len();
    while index > 0 {
        index -= 1;
        if cache.entries[index].0.root != key.root {
            continue;
        }
        seen_same_root += 1;
        if seen_same_root > MAX_ENTRIES_PER_ROOT
            && let Some((_, _, evicted)) = cache.entries.remove(index)
        {
            cache.bytes = cache.bytes.saturating_sub(evicted);
        }
    }
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

    /// 每个用例一个独立数据根：缓存按根分桶，共用根会让并行用例互相挤掉条目。
    fn key(root: &str, session_id: &str, event_sequence: u64, interaction_count: u64) -> CacheKey {
        CacheKey {
            root: PathBuf::from(root),
            session_id: session_id.to_owned(),
            event_sequence,
            interaction_count,
            title: String::new(),
            activity_updated_ms: 0,
            metrics_updated_ms: 0,
            limit: 200,
            brief: false,
        }
    }

    #[test]
    fn lookup_returns_the_stored_payload_for_the_same_key() {
        let key = key("/root-hit", "session-cache-hit", 7, 3);
        store(&key, &serde_json::json!({ "interactions": [1, 2] }));
        let hit = lookup(&key).expect("同键必须命中");
        assert_eq!(hit["interactions"][1], 2);
    }

    #[test]
    fn a_moving_watermark_or_row_count_misses_the_cache() {
        let stored = key("/root-move", "session-cache-invalidate", 7, 3);
        store(&stored, &serde_json::json!({ "value": "old" }));
        assert!(
            lookup(&key("/root-move", "session-cache-invalidate", 8, 3)).is_none(),
            "事件水位前进后必须失效"
        );
        assert!(
            lookup(&key("/root-move", "session-cache-invalidate", 7, 4)).is_none(),
            "交互条数变化后必须失效"
        );
        assert!(lookup(&stored).is_some(), "未变化的那一条必须仍然命中");
    }

    /// 水位与条数都不动、但派生事实变了（换标题、投影补齐）：同样必须失效。
    #[test]
    fn derived_facts_invalidate_without_an_event() {
        let mut stored = key("/root-derived", "session-cache-derived", 7, 3);
        store(&stored, &serde_json::json!({ "value": "old" }));
        let mut renamed = stored.clone();
        renamed.title = "新标题".to_owned();
        assert!(lookup(&renamed).is_none(), "标题变更后必须失效");
        let mut activity = stored.clone();
        activity.activity_updated_ms = 42;
        assert!(lookup(&activity).is_none(), "逐轮答案补齐后必须失效");
        let mut metrics = stored.clone();
        metrics.metrics_updated_ms = 43;
        assert!(lookup(&metrics).is_none(), "累计读数写入后必须失效");
        assert!(lookup(&stored).is_some(), "原键仍然命中");
        stored.title.clear();
    }

    #[test]
    fn the_cache_stays_within_its_entry_bound() {
        for index in 0..(MAX_ENTRIES_PER_ROOT + 4) {
            store(
                &key("/root-bound", &format!("session-bound-{index}"), 1, 1),
                &serde_json::json!({ "index": index }),
            );
        }
        let cache = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let same_root = cache
            .entries
            .iter()
            .filter(|(candidate, _, _)| candidate.root == Path::new("/fixture-root"))
            .count();
        assert!(
            same_root <= MAX_ENTRIES_PER_ROOT,
            "同一数据根的条数上限必须生效，实际 {same_root}"
        );
        assert!(
            cache.entries.len() <= MAX_ENTRIES,
            "全局条数上限必须生效，实际 {}",
            cache.entries.len()
        );
        assert!(
            cache.bytes <= MAX_BYTES,
            "字节上限必须生效，实际 {}",
            cache.bytes
        );
    }
}
