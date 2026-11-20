//! 长会话治理：thread 窗口重写、事件日志正文分级与 WAL 截断。
//!
//! 三者解决的是同一类问题：活着的长会话不会被保留期回收，却一直在给打开、翻页、
//! 备份与恢复付代价：
//!
//! - **thread 窗口重写**：把已被压缩快照覆盖的旧轮次从 JSONL 里真正去掉；
//! - **事件日志正文分级**：把旧交互的大载荷正文搬进归档，日志里只留标记；
//! - **WAL 截断**：`PASSIVE` 回写只搬页不缩文件，维护窗口用 `TRUNCATE` 把 WAL 压回 0。
//!
//! 两条硬约束：
//!
//! - **时间预算**：每个阶段都在预算内推进，超时即结束本轮，剩余会话由下一轮续跑；
//! - **体量预筛**：先用文件大小挡掉小会话，只有真正常到需要治理的对象才付出投影代价。

use std::path::Path;
use std::time::{Duration, Instant};

use axon_domain::DaemonSessionId;
use axon_session::{
    AXON_RUNTIME_DATA_DIR, JsonlThreadEventStore, SqliteRuntimeStore, ThreadIndexSnapshot,
    ThreadService,
};

/// 单轮治理阶段每个阶段的墙上时钟预算（执行态）。
///
/// 两个阶段各拿一份：共用一份时，第一个阶段的候选会把时间吃光，第二个阶段永远拿不到
/// 时间片（实测长会话里 thread 阶段很容易跑满）。
const GOVERNANCE_TIME_BUDGET: Duration = Duration::from_secs(15);
/// 单轮治理阶段每个阶段的墙上时钟预算（dry-run）：只算账也必须保持交互可接受。
const GOVERNANCE_DRY_RUN_TIME_BUDGET: Duration = Duration::from_secs(5);
/// thread 窗口保留的轮数。
///
/// 取 20：界面一屏放不下更多，而模型可见历史由压缩快照承担，不依赖这段窗口。
const THREAD_WINDOW_KEEP_TURNS: usize = 20;
/// 值得投影整条 thread 的体量下限。
///
/// 单轮 JSON 记录的体量差异极大（实测几十 KB 到数 MB），体量小的 thread 即使超出
/// 窗口也不值得付一次全量投影；4 MiB 是「重写收益明显大于本次读成本」的经验分界。
const THREAD_WINDOW_MIN_BYTES: u64 = 4 * 1024 * 1024;
/// 事件日志正文保留的最近交互数。
const EVENT_BODY_KEEP_INTERACTIONS: usize = 40;
/// 值得整体重写的事件日志体量下限。
const EVENT_BODY_MIN_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// 静默期：最近这段时间内被写过的对象一律跳过。
///
/// 重写要在写锁内读全量再整体替换，与正在跑的一轮撞上时，写入方会在写锁超时后失败。
/// 只治理「已静默半小时」的会话，把冲突概率压到可忽略；活着的长会话下一轮再治理。
const GOVERNANCE_MIN_IDLE_MS: u64 = 30 * 60 * 1000;

/// 一个治理阶段的剩余时间。
pub(super) struct GovernanceBudget {
    deadline: Instant,
}

impl GovernanceBudget {
    /// 按当前是否 dry-run 取预算。
    pub(super) fn new(dry_run: bool) -> Self {
        let budget = if dry_run {
            GOVERNANCE_DRY_RUN_TIME_BUDGET
        } else {
            GOVERNANCE_TIME_BUDGET
        };
        Self {
            deadline: Instant::now() + budget,
        }
    }

    /// 预算是否已耗尽：耗尽即结束本阶段，剩余对象留给下一轮。
    pub(super) fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

/// 重写超窗 thread；返回（重写条数，预计/实际移出的轮次数）。
pub(super) fn govern_thread_windows(
    root: &Path,
    dry_run: bool,
    budget: &GovernanceBudget,
) -> Result<(usize, usize), String> {
    let Some(snapshot) = thread_index_or_rebuild(root)? else {
        // 重建后仍然没有索引：没有任何可治理对象。
        return Ok((0, 0));
    };
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut rewritten = 0usize;
    let mut dropped = 0usize;
    let idle_before = axon_time::unix_now_ms().saturating_sub(GOVERNANCE_MIN_IDLE_MS);
    for entry in &snapshot.entries {
        if budget.expired() {
            break;
        }
        let (bytes, newest_ms) = thread_shard_stats(root, &entry.thread_id)?;
        if bytes < THREAD_WINDOW_MIN_BYTES || newest_ms >= idle_before {
            continue;
        }
        if dry_run {
            if let Some(turns) =
                service.plan_thread_window(root, &entry.thread_id, THREAD_WINDOW_KEEP_TURNS)?
            {
                rewritten = rewritten.saturating_add(1);
                dropped = dropped.saturating_add(turns);
            }
            continue;
        }
        match service.rewrite_to_window(root, &entry.thread_id, THREAD_WINDOW_KEEP_TURNS) {
            Ok(true) => rewritten = rewritten.saturating_add(1),
            Ok(false) => {}
            // 单个 thread 重写失败不阻断整轮：事件面仍完整，下一轮再试。
            Err(error) => axon_logger::warning(&format!(
                "thread 窗口重写失败，保留原文件: thread={} error={error}",
                entry.thread_id
            )),
        }
    }
    Ok((rewritten, dropped))
}

/// 分级旧交互的事件正文；返回（换成标记的帧数，回收的日志字节数）。
pub(super) fn tier_event_bodies(
    root: &Path,
    dry_run: bool,
    budget: &GovernanceBudget,
) -> Result<(usize, u64), String> {
    let store = SqliteRuntimeStore::from_root(root.join(AXON_RUNTIME_DATA_DIR));
    let events_root = store.events_root();
    if !events_root.is_dir() {
        return Ok((0, 0));
    }
    let mut frames = 0usize;
    let mut reclaimed = 0u64;
    let idle_before = axon_time::unix_now_ms().saturating_sub(GOVERNANCE_MIN_IDLE_MS);
    for log in session_log_files(events_root)? {
        if budget.expired() {
            break;
        }
        let Ok(metadata) = std::fs::metadata(&log) else {
            continue;
        };
        if metadata.len() < EVENT_BODY_MIN_LOG_BYTES || modified_ms(&metadata) >= idle_before {
            continue;
        }
        // 目录名是转义片段、不可逆；会话标识以日志 header 为准。
        let Ok(Some(header)) = axon_eventlog::read_header(&log) else {
            continue;
        };
        let Ok(session_id) = DaemonSessionId::new(header.session_id) else {
            continue;
        };
        if dry_run {
            if let Some(plan) = store
                .plan_event_body_tiering(&session_id, EVENT_BODY_KEEP_INTERACTIONS)
                .map_err(|error| format!("事件日志正文分级算账失败：{error}"))?
            {
                frames = frames.saturating_add(plan.tiered_frames);
                reclaimed = reclaimed.saturating_add(plan.tiered_bytes);
            }
            continue;
        }
        match store.tier_event_body(&session_id, EVENT_BODY_KEEP_INTERACTIONS) {
            Ok(Some(report)) => {
                frames = frames.saturating_add(report.tiered_frames);
                // 净回收：日志减少量扣掉归档新增量。只报日志侧会把「日志变小、磁盘变大」
                // 记成回收。
                reclaimed = reclaimed.saturating_add(report.net_reclaimed_bytes());
                if report.skipped_interactions > 0 {
                    axon_logger::warning(&format!(
                        "事件正文归档写入失败，正文保持内联: session={} interactions={}",
                        session_id.as_str(),
                        report.skipped_interactions
                    ));
                }
            }
            Ok(None) => {}
            // 单条会话失败（例如日志被并发写者占住）不阻断整轮。
            Err(error) => axon_logger::warning(&format!(
                "事件日志正文分级失败，原文保持内联: log={} error={error}",
                log.display()
            )),
        }
    }
    Ok((frames, reclaimed))
}

/// 把 WAL 回写主库并截断 WAL 文件本身。
///
/// 只在执行态调用：dry-run 不产生任何写与截断。控制面库还不存在时什么都不做，避免维护
/// 任务抢先建出一个没有 schema 的空库。
pub(super) fn truncate_wal(root: &Path) -> Result<(), String> {
    let data_root = root.join(AXON_RUNTIME_DATA_DIR);
    if !data_root
        .join(axon_session::CONTROL_DATABASE_FILE_NAME)
        .is_file()
    {
        return Ok(());
    }
    let store = SqliteRuntimeStore::from_root(data_root);
    let checkpoint = store
        .checkpoint_wal_truncate()
        .map_err(|error| format!("WAL 截断失败：{error}"))?;
    if checkpoint.busy {
        axon_logger::warning(&format!(
            "WAL 截断被并发读者挡住，本轮保留 {} 帧待下次回写",
            checkpoint.log_frames
        ));
    }
    Ok(())
}

/// 取 thread 索引；缺失就先重建一次再读。
///
/// 索引是派生缓存，单条补丁写失败只丢缓存；`session_governance` 若因此静默跳过，那些
/// 被漏写的 thread 永远轮不到治理，且没有任何自愈路径。索引缺失本身极少发生，重建代价
/// 只在这一次付。
fn thread_index_or_rebuild(root: &Path) -> Result<Option<ThreadIndexSnapshot>, String> {
    let store = JsonlThreadEventStore;
    let snapshot = store
        .read_thread_index(root)
        .map_err(|error| format!("读取 thread 索引失败：{error}"))?;
    if snapshot.is_some() {
        return Ok(snapshot);
    }
    axon_logger::warning("thread 索引缺失，先重建再治理会话");
    store
        .rebuild_thread_index(root)
        .map_err(|error| format!("重建 thread 索引失败：{error}"))?;
    store
        .read_thread_index(root)
        .map_err(|error| format!("读取 thread 索引失败：{error}"))
}

/// 一条 thread 的全部分片体量与最近写入时刻；没有分片时按 0 计。
fn thread_shard_stats(root: &Path, thread_id: &str) -> Result<(u64, u64), String> {
    let mut total = 0u64;
    let mut newest = 0u64;
    for path in JsonlThreadEventStore::thread_shard_paths(root, thread_id)? {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        total = total.saturating_add(metadata.len());
        newest = newest.max(modified_ms(&metadata));
    }
    Ok((total, newest))
}

/// 文件最后修改时刻（Unix 毫秒）；读不到时按 0 计（当作很久没动）。
fn modified_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 会话事件目录下的日志文件清单。
fn session_log_files(events_root: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let entries =
        std::fs::read_dir(events_root).map_err(|error| format!("读取会话事件目录失败：{error}"))?;
    let mut logs = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("读取会话事件目录条目失败：{error}"))?
            .path();
        if !path.is_dir() {
            continue;
        }
        // 代数由 axon-eventlog 决定，这里按「同一个目录里最像日志的文件」取，避免
        // 维护任务把代数写死成常量、升级后静默跳过全部会话。
        let Ok(files) = std::fs::read_dir(&path) else {
            continue;
        };
        for file in files.flatten() {
            let candidate = file.path();
            let is_log = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("session.v") && name.ends_with(".jsonl.zstd"));
            if is_log {
                logs.push(candidate);
            }
        }
    }
    // 稳定顺序：预算耗尽时下一轮从同一处接续，不会因为目录顺序抖动而饿死某些会话。
    logs.sort();
    Ok(logs)
}

#[cfg(test)]
#[path = "session_governance_tests.rs"]
mod tests;
