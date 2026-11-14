//! 反思候选池的本机 JSONL 持久化。
//!
//! 池文件不是事实源：它只记录「哪些卡片还没被批处理」。显著度无法从
//! `rollout_turns` 复原，所以不做自动重建；读取侧对坏行只跳过、不中断。
//! 幂等键是 `turn_id:trace_id`，重复入池与补跑重放都按它去重。

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use axon_domain::{AxonError, AxonResult};

use crate::{ReflectionPool, ReflectionPoolEntry, runtime_store_path::runtime_store_file_path};

/// 反思池文件名。
pub const REFLECTION_POOL_STORE_FILE: &str = "reflection_pool.jsonl";

/// 池文件互斥锁文件名。
const REFLECTION_POOL_LOCK_FILE: &str = "reflection_pool.lock";
/// 取锁重试次数与间隔：够短，不拖慢单轮；够长，能容忍另一次写盘的毫秒级占用。
const POOL_LOCK_RETRY: u32 = 10;
const POOL_LOCK_RETRY_INTERVAL_MS: u64 = 20;
/// 锁文件超过这个年龄视为崩溃残留，允许抢占。
const POOL_LOCK_STALE_MS: u64 = 60_000;

/// 池文件的跨进程互斥。
///
/// 每轮治理都会派生一个独立 worker，共用同一个池文件；`load → append → rewrite`
/// 无锁会让两个 worker 互相吞卡。用独占创建 lock 文件做互斥：拿不到就短暂重试，
/// 仍失败则放弃本轮：宁可晚一轮，也不要并发写坏池。
pub struct ReflectionPoolLock {
    path: PathBuf,
}

impl ReflectionPoolLock {
    /// 尝试获取池文件锁；获取失败返回 `None`。
    #[must_use]
    pub fn acquire(pool_path: impl AsRef<Path>) -> Option<Self> {
        let lock_path = pool_path.as_ref().with_file_name(REFLECTION_POOL_LOCK_FILE);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        for _ in 0..POOL_LOCK_RETRY {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Some(Self { path: lock_path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Self::is_stale(&lock_path) {
                        let _ = std::fs::remove_file(&lock_path);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(
                        POOL_LOCK_RETRY_INTERVAL_MS,
                    ));
                }
                Err(_) => return None,
            }
        }
        None
    }

    /// 锁文件是否已成残骸：写入方崩溃时文件会留下，靠年龄兜底而不是靠进程存活。
    fn is_stale(lock_path: &Path) -> bool {
        let Ok(modified) = std::fs::metadata(lock_path).and_then(|meta| meta.modified()) else {
            return false;
        };
        modified
            .elapsed()
            .map(|age| age.as_millis() > u128::from(POOL_LOCK_STALE_MS))
            .unwrap_or(false)
    }
}

impl Drop for ReflectionPoolLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 本次判定是否属于进程启动后的首次扫描。
///
/// 进程内只有第一次调用返回 true：冷启动补漏靠它区分「刚起来要兜旧池」与
/// 「常规轮次按 30 分钟池龄走」。
#[must_use]
pub fn take_cold_start() -> bool {
    static COLD_START: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    COLD_START.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// 反思池持久化 runtime。
///
/// 写入是「先查重、再追加」：查重走全量读，因为池的规模以十计，
/// 与反思批次同量级，不值得为它建索引。
#[derive(Debug, Default, Clone, Copy)]
pub struct ReflectionPoolStoreRuntime;

impl ReflectionPoolStoreRuntime {
    /// 解析默认池文件路径：`<axon-root>/data/runtime/reflection_pool.jsonl`。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        runtime_store_file_path(root, REFLECTION_POOL_STORE_FILE)
    }

    /// 追加一条池条目；同一幂等键已存在时返回 `false` 且不落盘。
    pub fn append_entry(path: impl AsRef<Path>, entry: &ReflectionPoolEntry) -> AxonResult<bool> {
        let path = path.as_ref();
        if Self::load_entries(path)
            .iter()
            .any(|existing| existing.idempotency_key() == entry.idempotency_key())
        {
            return Ok(false);
        }
        let line = serde_json::to_string(entry)
            .map_err(|_| AxonError::runtime_failure("反思池条目序列化失败"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| AxonError::runtime_failure("反思池目录创建失败"))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|_| AxonError::runtime_failure("反思池文件打开失败"))?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|_| AxonError::runtime_failure("反思池条目落盘失败"))?;
        Ok(true)
    }

    /// 读取池条目；坏行跳过，重复幂等键只保留最早一条，顺序按入池顺序。
    #[must_use]
    pub fn load_entries(path: impl AsRef<Path>) -> Vec<ReflectionPoolEntry> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let mut pool = ReflectionPool::default();
        for line in text.lines() {
            let Ok(entry) = serde_json::from_str::<ReflectionPoolEntry>(line) else {
                continue;
            };
            pool.enqueue(entry);
        }
        pool.entries().to_vec()
    }

    /// 重建内存池，供启动补跑与批处理推进使用。
    #[must_use]
    pub fn load_pool(path: impl AsRef<Path>) -> ReflectionPool {
        let mut pool = ReflectionPool::default();
        for entry in Self::load_entries(path) {
            pool.enqueue(entry);
        }
        pool
    }

    /// 用给定条目重写池文件；批处理取走一批后回写剩余队列。
    ///
    /// 覆盖写是「至少一次」语义：崩在取走与回写之间会重放同一批，
    /// 代价是重复反思，收益是不丢卡片。
    pub fn rewrite_entries(
        path: impl AsRef<Path>,
        entries: &[ReflectionPoolEntry],
    ) -> AxonResult<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| AxonError::runtime_failure("反思池目录创建失败"))?;
        }
        let mut body = String::new();
        for entry in entries {
            let line = serde_json::to_string(entry)
                .map_err(|_| AxonError::runtime_failure("反思池条目序列化失败"))?;
            body.push_str(&line);
            body.push('\n');
        }
        std::fs::write(path, body).map_err(|_| AxonError::runtime_failure("反思池回写失败"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BatchBlock, EvidenceRange, HourBucket, ReflectionCard, SalienceVector, WhatBlock,
        WhenBlock, WhereBlock,
    };

    fn card(trace_id: &str) -> ReflectionCard {
        ReflectionCard {
            card_id: format!("card:{trace_id}:0"),
            trace_id: trace_id.to_owned(),
            segment_index: 0,
            segment_count: 1,
            when: WhenBlock {
                started_at_ms: 1_000,
                duration_ms: 0,
                hour_bucket: HourBucket::Afternoon,
            },
            location: WhereBlock {
                cwd_ref: "E:/zbg/axon-code".to_owned(),
                workspace_kind: "project".to_owned(),
            },
            what: WhatBlock {
                intent_ref: "message:user:1".to_owned(),
                intent_chars: 8,
                intent_digest: "digest-intent".to_owned(),
                outcome_ref: "turn:turn-1:assistant".to_owned(),
                outcome_chars: 12,
                outcome_digest: "digest-outcome".to_owned(),
                final_status: "completed".to_owned(),
            },
            evidence_range: EvidenceRange {
                first_ref: "receipt:1".to_owned(),
                last_ref: "receipt:1".to_owned(),
                count: 1,
                verified_count: 1,
                rejected_count: 0,
            },
            top_evidence: Vec::new(),
            signals: SalienceVector::default(),
            batch: BatchBlock {
                window_started_at_ms: 0,
                window_ended_at_ms: 0,
                card_count: 1,
                accumulated_salience: 0,
            },
            hints: Vec::new(),
        }
    }

    fn entry(turn_id: &str, trace_id: &str, enqueued_at_ms: u64) -> ReflectionPoolEntry {
        ReflectionPoolEntry {
            turn_id: turn_id.to_owned(),
            trace_id: trace_id.to_owned(),
            enqueued_at_ms,
            card: card(trace_id),
            bucket: String::new(),
            correction_bp: 0,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "axon-reflection-pool-{}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn appended_entries_survive_a_reload_in_order() {
        let path = scratch("reload");
        let _ = std::fs::remove_file(&path);

        assert!(
            ReflectionPoolStoreRuntime::append_entry(&path, &entry("turn-1", "trace:a", 1))
                .expect("首条")
        );
        assert!(
            ReflectionPoolStoreRuntime::append_entry(&path, &entry("turn-2", "trace:b", 2))
                .expect("次条")
        );

        let loaded = ReflectionPoolStoreRuntime::load_entries(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded[0].idempotency_key(),
            format!("turn-1:trace:a:{}", loaded[0].card.segment_index)
        );
        assert_eq!(loaded[1].enqueued_at_ms, 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn duplicate_idempotency_key_is_not_appended_again() {
        let path = scratch("dedupe");
        let _ = std::fs::remove_file(&path);
        let first = entry("turn-1", "trace:a", 1);

        assert!(ReflectionPoolStoreRuntime::append_entry(&path, &first).expect("首条"));
        assert!(
            !ReflectionPoolStoreRuntime::append_entry(&path, &first).expect("重复条"),
            "重复入池必须被幂等键挡下"
        );
        assert_eq!(ReflectionPoolStoreRuntime::load_entries(&path).len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_lines_are_skipped_without_losing_healthy_ones() {
        let path = scratch("corrupt");
        let _ = std::fs::remove_file(&path);
        assert!(
            ReflectionPoolStoreRuntime::append_entry(&path, &entry("turn-1", "trace:a", 1))
                .expect("首条")
        );
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("落盘文件");
            file.write_all(b"{not-json}\n").expect("写入坏行");
        }
        assert!(
            ReflectionPoolStoreRuntime::append_entry(&path, &entry("turn-2", "trace:b", 2))
                .expect("次条")
        );

        let loaded = ReflectionPoolStoreRuntime::load_entries(&path);
        assert_eq!(loaded.len(), 2, "坏行只跳过，不连坐");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rebuild_pool_projects_trigger_input() {
        let path = scratch("rebuild");
        let _ = std::fs::remove_file(&path);
        assert!(
            ReflectionPoolStoreRuntime::append_entry(&path, &entry("turn-1", "trace:a", 1_000))
                .expect("首条")
        );

        let pool = ReflectionPoolStoreRuntime::load_pool(&path);
        let trigger = pool.trigger_input(1_000, false, false);

        assert_eq!(trigger.candidate_count, 1);
        assert_eq!(trigger.oldest_age_ms, 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rewrite_drops_consumed_entries() {
        let path = scratch("rewrite");
        let _ = std::fs::remove_file(&path);
        assert!(
            ReflectionPoolStoreRuntime::append_entry(&path, &entry("turn-1", "trace:a", 1))
                .expect("首条")
        );
        assert!(
            ReflectionPoolStoreRuntime::append_entry(&path, &entry("turn-2", "trace:b", 2))
                .expect("次条")
        );
        let mut pool = ReflectionPoolStoreRuntime::load_pool(&path);
        let batch = pool.drain_batch(3);

        ReflectionPoolStoreRuntime::rewrite_entries(&path, pool.entries()).expect("回写");

        assert_eq!(batch.len(), 2, "两张小卡都在批内字节预算内");
        assert!(
            ReflectionPoolStoreRuntime::load_entries(&path).is_empty(),
            "已消费条目必须从池文件消失，否则下次触发会重放同一批"
        );

        let _ = std::fs::remove_file(&path);
    }
}
