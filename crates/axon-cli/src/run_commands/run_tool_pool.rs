//! 运行会话级固定工具工作池（PWC-1 `S-3`）。
//!
//! 池大小等于 `CapacitySnapshot.call_width`，按会话复用、跨批次存活：
//! 注册表持有强引用，批次结束不会销毁池；不再为每个调用起一个 OS 线程，
//! 也不再每批重建 `thread::scope`。

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use super::run_capacity::CapacitySnapshot;

type PoolTask = Box<dyn FnOnce() + Send + 'static>;

/// 注册表最多保留的活跃会话池数量；超出后回收最早插入的池。
const MAX_LIVE_POOLS: usize = 32;

pub(super) struct ToolWorkerPool {
    sender: mpsc::Sender<PoolTask>,
    width: usize,
    live_workers: Arc<AtomicUsize>,
}

impl ToolWorkerPool {
    fn spawn(width: usize) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel::<PoolTask>();
        let receiver = Arc::new(Mutex::new(receiver));
        let live_workers = Arc::new(AtomicUsize::new(0));
        for index in 0..width.max(1) {
            let receiver = Arc::clone(&receiver);
            let alive = Arc::clone(&live_workers);
            let spawned = thread::Builder::new()
                .name(format!("axon-tool-worker-{index}"))
                .spawn(move || {
                    let _exit = WorkerExitGuard(alive);
                    loop {
                        let task = {
                            let guard = receiver
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            guard.recv()
                        };
                        match task {
                            Ok(task) => task(),
                            Err(_) => break,
                        }
                    }
                });
            if spawned.is_ok() {
                live_workers.fetch_add(1, Ordering::SeqCst);
            }
        }
        Arc::new(Self {
            sender,
            width,
            live_workers,
        })
    }

    /// 投递一个执行任务；池满时任务在通道内排队，不会新建线程。
    ///
    /// 返回错误表示任务不会被任何 worker 执行，调用方必须据此产出失败回执，
    /// 不能记成"用户取消"。
    pub(super) fn submit(&self, task: impl FnOnce() + Send + 'static) -> Result<(), String> {
        if self.live_workers.load(Ordering::SeqCst) == 0 {
            return Err("工具工作池没有可用线程".to_owned());
        }
        self.sender
            .send(Box::new(task))
            .map_err(|_| "工具工作池已关闭".to_owned())
    }

    #[cfg(test)]
    pub(super) const fn width(&self) -> usize {
        self.width
    }

    #[cfg(test)]
    pub(super) fn live_workers(&self) -> usize {
        self.live_workers.load(Ordering::SeqCst)
    }
}

/// 线程退出时把存活计数减回去，避免池以为自己还有 worker。
struct WorkerExitGuard(Arc<AtomicUsize>);

impl Drop for WorkerExitGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 注册表项：插入序号用于按 LRU 回收，避免误淘汰正在使用的池。
struct PoolEntry {
    sequence: u64,
    pool: Arc<ToolWorkerPool>,
}

/// 按运行会话取池；注册表持强引用，跨批次复用。
pub(super) fn session_tool_pool(session: &str, capacity: CapacitySnapshot) -> Arc<ToolWorkerPool> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, PoolEntry>>> = OnceLock::new();
    static NEXT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let mut registry = REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = registry.get(session)
        && entry.pool.width == capacity.call_width
    {
        return Arc::clone(&entry.pool);
    }
    // 会话数受治理上限约束：优先回收"注册表是唯一持有者"的最老池；
    // 全部在用（正常不会发生）时退化为回收最老池，保证注册表不超上限。
    if registry.len() >= MAX_LIVE_POOLS {
        let stale = registry
            .iter()
            .filter(|(_, entry)| Arc::strong_count(&entry.pool) == 1)
            .min_by_key(|(_, entry)| entry.sequence)
            .or_else(|| registry.iter().min_by_key(|(_, entry)| entry.sequence))
            .map(|(session, _)| session.clone());
        if let Some(stale) = stale {
            registry.remove(&stale);
        }
    }
    let pool = ToolWorkerPool::spawn(capacity.call_width);
    registry.insert(
        session.to_owned(),
        PoolEntry {
            sequence: NEXT_SEQUENCE.fetch_add(1, Ordering::SeqCst) as u64,
            pool: Arc::clone(&pool),
        },
    );
    pool
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use super::*;

    #[test]
    fn pool_survives_between_batches_for_the_same_session() {
        let capacity = CapacitySnapshot {
            call_width: 2,
            ..CapacitySnapshot::preset()
        };
        let first = session_tool_pool("pool-lifetime-test", capacity);
        // 模拟批次结束：释放本批持有的强引用，注册表仍应保住同一个池。
        drop(first);
        let second = session_tool_pool("pool-lifetime-test", capacity);
        let third = session_tool_pool("pool-lifetime-test", capacity);
        assert!(Arc::ptr_eq(&second, &third), "同一会话必须复用同一个池");
        assert_eq!(second.live_workers(), 2, "复用后不得重建线程");
    }

    #[test]
    fn pool_never_runs_more_tasks_than_its_width() {
        let capacity = CapacitySnapshot {
            call_width: 2,
            ..CapacitySnapshot::preset()
        };
        let pool = session_tool_pool("pool-width-test", capacity);
        assert_eq!(pool.width(), 2);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        for _ in 0..8 {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            let finished = Arc::clone(&finished);
            pool.submit(move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(20));
                active.fetch_sub(1, Ordering::SeqCst);
                finished.fetch_add(1, Ordering::SeqCst);
            })
            .expect("池可用时投递必须成功");
        }
        for _ in 0..200 {
            if finished.load(Ordering::SeqCst) == 8 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(finished.load(Ordering::SeqCst), 8, "任务必须全部执行");
        assert!(peak.load(Ordering::SeqCst) <= 2, "并发不得超过池宽度");
        assert!(peak.load(Ordering::SeqCst) >= 1, "池必须真正执行任务");
    }

    #[test]
    fn saturated_batch_reaches_but_never_exceeds_call_width() {
        let capacity = CapacitySnapshot::preset();
        let width = capacity.call_width;
        let pool = session_tool_pool("pool-saturation-test", capacity);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        // A-03 夹具形状：32 个调用、每个 200ms，峰值不得超过 call_width。
        let total = 32usize;
        for _ in 0..total {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            let finished = Arc::clone(&finished);
            pool.submit(move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(200));
                active.fetch_sub(1, Ordering::SeqCst);
                finished.fetch_add(1, Ordering::SeqCst);
            })
            .expect("池可用时投递必须成功");
        }
        for _ in 0..400 {
            if finished.load(Ordering::SeqCst) == total {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(finished.load(Ordering::SeqCst), total, "任务必须全部执行");
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak <= width, "峰值并发不得超过 call_width");
        assert!(peak >= 2, "池必须真正并行（实测峰值 {peak}）");
    }

    #[test]
    fn same_session_reuses_pool_and_new_width_rebuilds_it() {
        let capacity = CapacitySnapshot {
            call_width: 3,
            ..CapacitySnapshot::preset()
        };
        let first = session_tool_pool("pool-reuse-test", capacity);
        let second = session_tool_pool("pool-reuse-test", capacity);
        assert!(Arc::ptr_eq(&first, &second));
        let rebuilt = session_tool_pool(
            "pool-reuse-test",
            CapacitySnapshot {
                call_width: 4,
                ..capacity
            },
        );
        assert!(!Arc::ptr_eq(&first, &rebuilt));
        assert_eq!(rebuilt.width(), 4);
    }
}
