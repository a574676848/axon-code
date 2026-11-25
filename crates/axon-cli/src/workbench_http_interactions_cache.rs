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
//!
//! 两个准入规则决定这份缓存对多会话并发是否真的有用：
//!
//! - 每个会话只留最新一页。键里的水位单调前进，旧键不会再被请求到，留着只是占名额。
//! - 运行中的会话不入库。它每落一批事件就换一次键，存进去的条目活不过一次重建，却会把
//!   同根空闲会话的页挤出去；而「切回刚看过的会话」命中的正是那些空闲页。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use super::runtime_store;

/// 同一条数据根最多保留多少个会话的首页。
///
/// 每个会话只留最新一页，因此这个数就是「同时能被缓存覆盖的会话数」。按根分桶而不是
/// 全局共用一份名额：进程里只有 daemon 一个数据根，而测试与 `serve` 各自独立；全局名额
/// 会让并行跑的多个根互相把对方的条目挤掉，缓存因此变得不可预测。
const MAX_SESSIONS_PER_ROOT: usize = 64;
/// 全局条目数上限：防止大量数据根把内存撑大。
const MAX_ENTRIES: usize = 256;
/// 缓存载荷总字节上限；单页载荷正常在几百 KB 量级。
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// 一条缓存键：数据根、会话、事件水位、交互条数、三张投影/派生表的更新时间、
/// 页大小与翻页档。
///
/// 水位与条数只证明「事件写了没有」，而派生事实是在水位提交之后才写的：逐轮答案落在
/// `turn_activity`、累计读数落在 `session_metrics`，没有指标事件的轮次用时兜底读
/// `work_item_history`。把这三样一起放进键，缓存才覆盖得住「事件已经落定、派生事实
/// 随后补齐」的那一段。
///
/// 数据根必须进键：同一进程里 `serve` 与 daemon 可能指向不同运行态目录，测试也是
/// 各自一个临时根；漏掉它会把 A 根的一页发给 B 根。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    root: PathBuf,
    session_id: String,
    event_sequence: u64,
    interaction_count: u64,
    activity_updated_ms: u64,
    metrics_updated_ms: u64,
    work_items_updated_ms: u64,
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
    // 运行中的会话每批事件都会换键，入库条目活不过一次重建，却会挤掉同根空闲会话的页。
    if !freshness.cacheable() {
        return build();
    }
    let key = CacheKey {
        root: axon_root.to_path_buf(),
        session_id: session_id.to_owned(),
        event_sequence: freshness.event_sequence,
        interaction_count: freshness.interaction_count,
        activity_updated_ms: freshness.activity_updated_ms,
        metrics_updated_ms: freshness.metrics_updated_ms,
        work_items_updated_ms: freshness.work_items_updated_ms,
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
/// 四层事实合起来才够：事件水位（有没有新事件）、交互条数（有没有新入站行）、
/// `turn_activity` 与 `session_metrics` 的更新时间（答案与读数补齐了没有），以及
/// `work_item_history` 的更新时间（没有指标事件的老记录靠它给出轮次用时）。
fn freshness(axon_root: &Path, session_id: &str) -> Option<Freshness> {
    use rusqlite::OptionalExtension as _;
    let row = runtime_store(axon_root)
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT s.last_event_sequence,
                            (SELECT COUNT(*) FROM inbox AS i WHERE i.session_id = s.session_id),
                            (SELECT COALESCE(MAX(a.updated_at_ms), 0) FROM turn_activity AS a
                              WHERE a.session_id = s.session_id),
                            COALESCE((SELECT m.updated_at_ms FROM session_metrics AS m
                                       WHERE m.session_id = s.session_id), 0),
                            COALESCE((SELECT MAX(h.updated_at_ms) FROM work_item_history AS h
                                       WHERE h.session_id = s.session_id), 0),
                            s.status
                       FROM sessions AS s WHERE s.session_id = ?1",
                    [session_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, String>(5)?,
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
        activity_updated_ms: u64::try_from(row.2).unwrap_or(0),
        metrics_updated_ms: u64::try_from(row.3).unwrap_or(0),
        work_items_updated_ms: u64::try_from(row.4).unwrap_or(0),
        running: row.5 == "running",
    })
}

/// 一次新鲜度探测的结果。
struct Freshness {
    event_sequence: u64,
    interaction_count: u64,
    activity_updated_ms: u64,
    metrics_updated_ms: u64,
    work_items_updated_ms: u64,
    running: bool,
}

impl Freshness {
    /// 只有空闲会话的页值得入库：运行中的页下一次请求必然换键，缓存它没有收益。
    fn cacheable(&self) -> bool {
        !self.running
    }
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
    // 每个会话只留最新一页：水位单调前进，旧键不会再被请求到，留着只会占名额。
    // 这条同时覆盖「同键重复写入」，不再需要单独的等值去重分支。
    while let Some(position) = cache.entries.iter().position(|(candidate, _, _)| {
        candidate.root == key.root && candidate.session_id == key.session_id
    }) && let Some((_, _, stale)) = cache.entries.remove(position)
    {
        cache.bytes = cache.bytes.saturating_sub(stale);
    }
    cache
        .entries
        .push_front((key.clone(), Arc::new(payload.clone()), size));
    cache.bytes = cache.bytes.saturating_add(size);
    // 同一数据根只留最近的若干会话：从最旧（尾部）开始删本根多出来的条目。
    let mut seen_same_root = 0usize;
    let mut index = cache.entries.len();
    while index > 0 {
        index -= 1;
        if cache.entries[index].0.root != key.root {
            continue;
        }
        seen_same_root += 1;
        if seen_same_root > MAX_SESSIONS_PER_ROOT
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
            activity_updated_ms: 0,
            metrics_updated_ms: 0,
            work_items_updated_ms: 0,
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

    /// 水位与条数都不动、但派生事实变了（工作项用时兜底、投影补齐）：同样必须失效。
    #[test]
    fn derived_facts_invalidate_without_an_event() {
        let stored = key("/root-derived", "session-cache-derived", 7, 3);
        store(&stored, &serde_json::json!({ "value": "old" }));
        let mut activity = stored.clone();
        activity.activity_updated_ms = 42;
        assert!(lookup(&activity).is_none(), "逐轮答案补齐后必须失效");
        let mut metrics = stored.clone();
        metrics.metrics_updated_ms = 43;
        assert!(lookup(&metrics).is_none(), "累计读数写入后必须失效");
        let mut work_items = stored.clone();
        work_items.work_items_updated_ms = 44;
        assert!(
            lookup(&work_items).is_none(),
            "工作项终态写入后必须失效，否则轮次用时会被缓存成「没有」"
        );
        assert!(lookup(&stored).is_some(), "原键仍然命中");
    }

    /// 同一会话换了水位后，旧页必须被顶掉，会话在缓存里只占一格。
    ///
    /// 这一条是多会话并发下的关键：正在跑的会话会把键一路推进，若旧键留着，它就能
    /// 用几十个死条目把同根其它会话的页挤出去。
    #[test]
    fn a_new_watermark_replaces_the_previous_page_of_the_same_session() {
        let first = key("/root-slot", "session-slot", 7, 3);
        store(&first, &serde_json::json!({ "value": "old" }));
        let second = key("/root-slot", "session-slot", 8, 3);
        store(&second, &serde_json::json!({ "value": "new" }));
        assert!(lookup(&first).is_none(), "同一会话的旧页必须被最新页顶掉");
        assert_eq!(
            lookup(&second).expect("最新页必须命中")["value"],
            "new",
            "最新页的内容必须可读回"
        );
        let cache = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let slots = cache
            .entries
            .iter()
            .filter(|(candidate, _, _)| candidate.session_id == "session-slot")
            .count();
        assert_eq!(slots, 1, "同一会话在缓存里只能占一格，实际 {slots}");
    }

    /// 运行中的会话只重建不入库，开关只看会话状态。
    #[test]
    fn a_running_session_is_not_admitted() {
        let running = Freshness {
            event_sequence: 3,
            interaction_count: 2,
            activity_updated_ms: 0,
            metrics_updated_ms: 0,
            work_items_updated_ms: 0,
            running: true,
        };
        assert!(!running.cacheable(), "运行中的会话不入缓存");
        let idle = Freshness {
            running: false,
            ..running
        };
        assert!(idle.cacheable(), "空闲会话必须可入库");
    }

    /// 多会话并发：一个正在跑的会话把水位推进几十次，不得挤掉空闲会话的页。
    ///
    /// 这是「缓存对多会话并发到底有没有用」的核心断言：空闲会话的页一旦被活跃会话的
    /// 死键挤出去，切回看过的会话就要重算，缓存等于没有。
    #[test]
    fn an_active_session_cannot_evict_the_pages_of_idle_ones() {
        for session in ["session-idle-a", "session-idle-b"] {
            store(
                &key("/root-multi", session, 10, 5),
                &serde_json::json!({ "session": session }),
            );
        }
        for sequence in 11..31 {
            store(
                &key("/root-multi", "session-active", sequence, 5),
                &serde_json::json!({ "sequence": sequence }),
            );
        }
        assert!(
            lookup(&key("/root-multi", "session-idle-a", 10, 5)).is_some(),
            "活跃会话推进水位后，空闲会话的页必须仍然命中"
        );
        assert!(
            lookup(&key("/root-multi", "session-idle-b", 10, 5)).is_some(),
            "空闲会话之间不能互相挤掉"
        );
        let cache = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = cache
            .entries
            .iter()
            .filter(|(candidate, _, _)| candidate.root == Path::new("/root-multi"))
            .count();
        assert_eq!(entries, 3, "三个会话各占一格，实际 {entries}");
    }

    #[test]
    fn the_cache_stays_within_its_entry_bound() {
        for index in 0..(MAX_SESSIONS_PER_ROOT + 4) {
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
            .filter(|(candidate, _, _)| candidate.root == Path::new("/root-bound"))
            .count();
        assert!(
            same_root <= MAX_SESSIONS_PER_ROOT,
            "同一数据根的会话数上限必须生效，实际 {same_root}"
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
