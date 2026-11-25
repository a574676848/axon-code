//! 常驻写句柄注册表：同一会话在一段活跃期内只付一次打开成本。
//!
//! 为什么需要它：事件日志的每次追加都会重开写句柄，打开过程要修复撕裂尾（读尾部
//! 窗口）并把整份旁挂索引读进内存。会话越长、批越密，这两笔固定开销越大，而它们
//! 全部落在会话写锁里，与只读请求争同一块盘。
//!
//! 这里把句柄留在进程内复用：打开只付一次，之后每次追加只做「短暂取锁、按文件长度
//! 核对、写一帧、追加一条索引、释放锁」。跨进程语义不变：锁仍然只覆盖一次追加，
//! 别的进程插写会被长度核对发现并按尾号校验，不符时照旧返回
//! [`crate::EventLogError::SequenceTailMismatch`] 交给上层补偿。
//!
//! 注册表按日志路径分桶，条目上限 [`MAX_WRITERS`] 个；淘汰只丢弃句柄，不删除磁盘
//! 索引，下次打开按磁盘重建即可。日志被整体重写（事件正文分层归档）时必须显式
//! [`forget_session_log`]：它同时推进该路径的**代际**，让仍在途的旧句柄在下一次追加
//! 时以 [`crate::EventLogError::StaleHandle`] 自证失效，而不是继续往被替换的文件里写。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use crate::EventLog;
use crate::append::EventLogWriter;
use crate::error::EventLogError;
use crate::record::RuntimeEventRecord;

/// 进程内最多常驻多少个会话写句柄。
///
/// 句柄本身只占一个文件句柄与一份索引，但索引随会话帧数增长；上限按「同时活跃的
/// 会话数」定，实测一个 daemon 同时写的会话不超过十余条，64 足够覆盖多轮切换。
const MAX_WRITERS: usize = 64;
/// 常驻句柄的空闲存活时长：超过它没有写入就释放。
///
/// 句柄持有一份完整的内存帧索引（每帧 24 字节），长会话下单个句柄可达数 MB。只按
/// 条数上限淘汰会让「写完就闲置」的会话一直占着内存，这里按空闲时间再收一道。
const IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// 常驻写句柄与它们的最近使用顺序。
#[derive(Default)]
struct Registry {
    writers: HashMap<PathBuf, Arc<Mutex<EventLogWriter>>>,
    /// 每个路径的句柄代际：重写日志时推进，旧句柄据此失效。
    generations: HashMap<PathBuf, u64>,
    /// 最近使用的在前；淘汰从尾部开始。
    order: VecDeque<PathBuf>,
    /// 每个路径最后一次被取用的时刻，用于空闲淘汰。
    last_used: HashMap<PathBuf, std::time::Instant>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

/// 追加一批事件到某个会话日志，复用进程内常驻的写句柄。
///
/// 语义与一次性 [`crate::append_once`] 完全等价：取锁、核对尾号、写一帧、释放锁。
/// 区别只在于打开成本被摊到同一句柄的多次追加上。
pub(crate) fn append_to_session(
    log: &EventLog,
    expected_next: u64,
    created_at_ms: u64,
    events: &[RuntimeEventRecord],
) -> Result<u64, EventLogError> {
    let path = log.path();
    let mut retried_after_stale = false;
    loop {
        let (writer, generation) = writer_for(&path, log, expected_next, created_at_ms)?;
        let mut guard = writer.lock().unwrap_or_else(PoisonError::into_inner);
        match guard.append_batch_reusing(expected_next, generation, events) {
            Ok(written) => return Ok(written),
            Err(error) => {
                drop(guard);
                /*
                 * 失败一律丢弃句柄，让下一次追加回到一次性打开语义。
                 *
                 * 这不只是清理状态：两个写者在同一会话上并发抢号时，补偿逻辑会把控制面
                 * 水位反复拉回「已落盘的尾号」，常驻句柄的快速重试会让两边严格同步，
                 * 每一轮都有一方恰好晚一步，形成活锁。丢弃句柄让重试重新付出一次打开
                 * 代价，时序自然错开；正常路径仍然是常驻的，不为此变慢。
                 */
                forget_session_log(&path);
                /*
                 * 句柄代际失效说明日志刚被整体重写：立刻重建一次句柄再试。这是唯一
                 * 需要「丢弃后马上重来」的错误，其它失败按原样上报，由上层决定重试。
                 */
                if matches!(error, EventLogError::StaleHandle) && !retried_after_stale {
                    retried_after_stale = true;
                    continue;
                }
                return Err(error);
            }
        }
    }
}

/// 丢弃某个日志路径的常驻句柄并推进代际；日志被整体重写后必须调用。
///
/// 推进代际是必须的：`Arc` 可能仍被在途追加持有，句柄不会立即关闭；只有代际校验
/// 才能让那次追加在写入前发现「我手里的文件已经被换掉了」。
pub(crate) fn forget_session_log(path: &Path) {
    let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    registry.writers.remove(path);
    registry.order.retain(|candidate| candidate != path);
    registry.last_used.remove(path);
    let generation = registry.generations.entry(path.to_path_buf()).or_insert(0);
    *generation = generation.wrapping_add(1);
}

/// 淘汰空闲太久的句柄；返回被丢弃的条数。
///
/// 只在新增句柄时调用一次：新增路径本来就少，顺手做一次全表清理比每次追加都扫一遍便宜。
fn evict_idle(registry: &mut Registry, now: std::time::Instant, ttl: std::time::Duration) -> usize {
    let idle: Vec<PathBuf> = registry
        .last_used
        .iter()
        .filter(|(_, used)| now.duration_since(**used) >= ttl)
        .map(|(path, _)| path.clone())
        .collect();
    for path in &idle {
        registry.writers.remove(path);
        registry.order.retain(|candidate| candidate != path);
        registry.last_used.remove(path);
    }
    idle.len()
}

/// 某个日志路径是否还有常驻句柄，供测试断言重写后的失效行为。
#[cfg(test)]
pub(crate) fn has_resident_writer(path: &Path) -> bool {
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .writers
        .contains_key(path)
}

fn writer_for(
    path: &Path,
    log: &EventLog,
    expected_next: u64,
    created_at_ms: u64,
) -> Result<(Arc<Mutex<EventLogWriter>>, u64), EventLogError> {
    if let Some(found) = lookup(path) {
        return Ok(found);
    }
    let generation = current_generation(path);
    /*
     * 创建在锁外进行：打开要抢跨进程文件锁并读盘，持着注册表锁做这些会把其它会话的
     * 追加全部堵住。并发创建同一路径时后者发现已存在，直接用先到的那一个。
     */
    let created = Arc::new(Mutex::new(EventLogWriter::open_reusable(
        log,
        expected_next,
        created_at_ms,
        generation,
    )?));
    let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = registry.writers.get(path) {
        return Ok((
            Arc::clone(existing),
            current_generation_locked(&registry, path),
        ));
    }
    registry
        .writers
        .insert(path.to_path_buf(), Arc::clone(&created));
    registry.order.push_front(path.to_path_buf());
    registry
        .last_used
        .insert(path.to_path_buf(), std::time::Instant::now());
    // 新增路径时顺手收一次空闲句柄：写完就闲置的会话不该一直占着整份内存索引。
    evict_idle(&mut registry, std::time::Instant::now(), IDLE_TTL);
    while registry.writers.len() > MAX_WRITERS {
        let Some(evicted) = registry.order.pop_back() else {
            break;
        };
        registry.writers.remove(&evicted);
        registry.last_used.remove(&evicted);
    }
    Ok((created, current_generation_locked(&registry, path)))
}

fn lookup(path: &Path) -> Option<(Arc<Mutex<EventLogWriter>>, u64)> {
    let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    let writer = registry.writers.get(path).map(Arc::clone)?;
    let generation = current_generation_locked(&registry, path);
    registry.order.retain(|candidate| candidate != path);
    registry.order.push_front(path.to_path_buf());
    registry
        .last_used
        .insert(path.to_path_buf(), std::time::Instant::now());
    Some((writer, generation))
}

fn current_generation(path: &Path) -> u64 {
    let registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    current_generation_locked(&registry, path)
}

fn current_generation_locked(registry: &Registry, path: &Path) -> u64 {
    registry.generations.get(path).copied().unwrap_or(0)
}

#[cfg(test)]
#[path = "writer_registry_tests.rs"]
mod tests;
