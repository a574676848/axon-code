//! 会话派生投影的进程内单线程队列。
//!
//! 投影（逐轮事实、上下文快照、会话累计指标）原本在会话写锁内同步执行，写路径因此要等
//! 完三次派生写入才放锁。这里把执行挪到进程内一条单线程队列：写路径只投递拥有所有权的
//! 副本，worker 按投递顺序逐批执行。
//!
//! 顺序由单 worker 的先进先出保证：同一会话的批在会话写锁内按序号顺序投递，worker 也按
//! 这个顺序消费，水位因此单调推进，累计型投影不会算错。水位闸门留在 worker 内判定，投递
//! 时不判定：投递时刻的水位与执行时刻可能不同。
//!
//! 水位只在三个投影全部成功后推进，记的是「已投影到的最后一条序号」。失败批次不推进，
//! 也不就地重试：三个投影不是同一事务，整批重放会把已成功的累加投影算第二遍。失败事实
//! 写进日志，由 [`SqliteRuntimeStore::rebuild_session_projections`] 这个重建入口兜底。
//!
//! 闸门命中时整批跳过，并记一条带水位与批首序号的 debug 日志：序号回退这类故障里，整批
//! 跳过是唯一会静默丢投影的路径，必须留痕。
//!
//! 队列满不阻塞写路径：把该会话在队列里的积压取出来，连同本批在调用线程按原序执行。选
//! 同步兜底而不是丢弃，是因为投影丢失比延迟更糟：累计型投影漏一批就永久偏小，只能靠
//! [`SqliteRuntimeStore::rebuild_session_metrics`] 整体重放纠正；同步兜底只让这一批多付
//! 几次写入。
//!
//! 进程退出不要求排空队列：崩溃丢掉的那批投影由 `rebuild_session_metrics` 兜底，读侧在
//! 投影缺行时也会回源事件日志。
//!
//! 单批失败不终止 worker：执行包在 `catch_unwind` 里，失败只计数并记日志，随后继续消费。

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(not(test))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
#[cfg(not(test))]
use std::sync::{Once, OnceLock};
#[cfg(not(test))]
use std::thread;

use axon_domain::DaemonSessionId;

use super::super::SqliteRuntimeStore;
use super::projection_watermark;
use crate::ports::RuntimeStoreError;

/// 投影队列容量（按批计）。
///
/// 一批对应一次事件追加。正常情况下 worker 几乎立刻消费完，1024 批的积压意味着写路径已经
/// 远远跑赢投影，这时的正确动作是同步兜底，而不是继续排队吃内存。
#[cfg(not(test))]
const PROJECTION_QUEUE_CAPACITY: usize = 1024;

/// worker 线程名：日志与调试器里要能一眼认出这条线程。
#[cfg(not(test))]
const PROJECTION_WORKER_THREAD: &str = "axon-projection";

/// worker 线程是否已经起来；没起来时投影退回同步执行。
#[cfg(not(test))]
static WORKER_READY: AtomicBool = AtomicBool::new(false);

/// 单测构建里同步执行过的批数：`drain_for_tests` 用它回答「已处理的批次数」。
#[cfg(test)]
static SYNC_PROCESSED: AtomicU64 = AtomicU64::new(0);

/// 一批投影的处置结果。
///
/// 跳过与失败分开表达：跳过是闸门的正常结论，失败才需要告警，两者都不能被当成成功。
#[derive(Debug)]
enum ProjectionOutcome {
    /// 三个投影都成功，水位已推进。
    Applied,
    /// 水位闸门判定整批已投影过，本批没有执行；`watermark` 是挡住它的水位。
    Skipped { watermark: u64 },
    /// 执行失败，水位未推进。
    Failed { error: RuntimeStoreError },
}

/// 一批待投影事件的完整输入。
///
/// 事件必须拥有所有权：投递发生在会话写锁内，执行发生在锁外的另一个线程上，借用活不过
/// 写路径。
struct ProjectionJob {
    store: SqliteRuntimeStore,
    session_id: DaemonSessionId,
    start_sequence: u64,
    events: Vec<(String, String)>,
    origin_channel: Option<String>,
    now_ms: u64,
    /// 本批落盘序号小于分配序号：事件日志被清空、修复重排或截断过。
    ///
    /// 水位这时停在旧的高序号上，闸门会把这一批以及之后每一批都当成已投影而整批跳过。
    /// 标记随批投递，由 worker 在执行前先重置水位：排队中的旧批仍按旧水位判定，不会被
    /// 重复投影。
    log_rewound: bool,
}

/// 投递一批投影。
///
/// 调用方必须持有该会话的事件面写入锁：同一会话的投递顺序就是投影顺序。
pub(super) fn submit(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
    events: &[(String, String)],
    origin_channel: Option<&str>,
    now_ms: u64,
    log_rewound: bool,
) {
    run(ProjectionJob {
        store: store.clone(),
        session_id: session_id.clone(),
        start_sequence,
        events: events.to_vec(),
        origin_channel: origin_channel.map(str::to_owned),
        now_ms,
        log_rewound,
    });
}

/// 单测构建下同步执行：库内用例依赖「投递后立刻可读」，且测试进程里不起线程。
#[cfg(test)]
fn run(job: ProjectionJob) {
    project(&job);
    SYNC_PROCESSED.fetch_add(1, Ordering::Relaxed);
}

/// 把一批投影交给进程内队列。
#[cfg(not(test))]
fn run(job: ProjectionJob) {
    let queue = global_queue();
    if WORKER_READY.load(Ordering::Acquire) {
        queue.enqueue_or_run(job);
    } else {
        // 线程没起来（创建失败）：退回同步执行，不能让投影永远躺在队列里。
        queue.execute(job);
    }
}

/// 执行一批事件的三个投影；任一失败即返回错误，由调用方决定水位。
///
/// 这是写路径（worker）与重建入口共用的实现，不复制逻辑。重建时把整条日志按批喂进来，
/// 因此签名只依赖「一批事件」，不依赖投递上下文。
pub(super) fn project_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
    events: &[(String, String)],
    origin_channel: Option<&str>,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    /*
     * 三个投影、水位与日志身份放进同一次 IMMEDIATE 提交：任一失败整批回滚，不会留下
     * 「逐轮读数写了、累计指标没写」的半批状态，水位也不会先于投影推进。
     */
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| RuntimeStoreError::Storage(error.to_string()))?;
    super::super::turn_activity_projection::project_turn_activity_in(
        store,
        &transaction,
        session_id,
        events,
        now_ms,
    )?;
    super::super::context_snapshot_projection::project_context_snapshot_in(
        &transaction,
        session_id,
        events,
        now_ms,
    )?;
    for (event_type, payload_json) in events {
        if event_type != "turn_metrics" {
            continue;
        }
        let Ok(metrics) = serde_json::from_str::<axon_eventlog::TurnMetrics>(payload_json) else {
            continue;
        };
        super::super::session_metrics_repository::apply_turn_metrics_in(
            &transaction,
            session_id,
            &metrics,
            origin_channel,
            now_ms,
        )?;
    }
    /*
     * 水位记「已投影到的最后一条序号」：一批事件是连续序号的一段，记末尾才能让
     * 「是否落后」判定精确到批。水位与身份和三个投影同一次提交，失败整批回滚。
     */
    let batch_end =
        start_sequence.saturating_add(u64::try_from(events.len().saturating_sub(1)).unwrap_or(0));
    super::projection_watermark::advance_in(&transaction, store, session_id, batch_end, now_ms)?;
    transaction
        .commit()
        .map_err(|error| RuntimeStoreError::Storage(error.to_string()))
}

/// 一批投影的完整执行：闸门判定、三个投影、成功后推进水位。
///
/// 闸门判据保持原样：批首序号不大于水位即整批跳过（同一次提交的派生结果不能只跳一半）。
/// 留痕与计数在 worker 侧做：跳过与失败都要区分开，不能静默当成成功。
fn project(job: &ProjectionJob) -> ProjectionOutcome {
    let store = &job.store;
    let session_id = &job.session_id;
    // 日志序号回退过：先清水位，否则下面的闸门会把这批当成已投影而整批跳过。
    if job.log_rewound {
        projection_watermark::reset(store, session_id);
    }
    if let Some(watermark) =
        projection_watermark::already_projected(store, session_id, job.start_sequence)
    {
        return ProjectionOutcome::Skipped { watermark };
    }
    /*
     * 不做就地重试：三个投影不是同一个事务，整批重放会把已经成功的 `turn_metrics` 再
     * 累加一次（累计型投影不幂等）。失败只留痕、不推进水位，由重建入口兜底。
     */
    match project_events(
        store,
        session_id,
        job.start_sequence,
        &job.events,
        job.origin_channel.as_deref(),
        job.now_ms,
    ) {
        Ok(()) => ProjectionOutcome::Applied,
        Err(error) => ProjectionOutcome::Failed { error },
    }
}

/// 队列里的执行函数：生产路径是 [`project`]，单测可以换成只记录顺序的替身。
type Apply = Box<dyn Fn(ProjectionJob) -> ProjectionOutcome + Send + Sync>;

/// 进程内投影队列：有界、先进先出、单 worker 消费。
///
/// 阻塞相关成员（`drained` 与等待侧方法）只被 `cfg(not(test))` 的路径使用：单测不起线程，
/// 队列由 `run_pending_for_tests` 在调用线程跑完。
#[cfg_attr(test, allow(dead_code))]
struct ProjectionQueue {
    capacity: usize,
    apply: Apply,
    state: Mutex<QueueState>,
    wake: Condvar,
    drained: Condvar,
    /// 执行互斥：worker 与同步兜底不得同时投影，否则水位判定会交错。
    run_lock: Mutex<()>,
    /// 已执行的批数（含跳过与失败）。
    processed: AtomicU64,
    /// 队列满而改在调用线程执行的批数。
    overflowed: AtomicU64,
    /// 闸门判定整批已投影过而跳过的批数。
    skipped: AtomicU64,
    /// 执行失败的批数。
    failed: AtomicU64,
}

/// 队列的共享状态。
struct QueueState {
    jobs: VecDeque<ProjectionJob>,
    /// worker 正在执行一批：排空判定必须等它结束。
    busy: bool,
}

#[cfg_attr(test, allow(dead_code))]
impl ProjectionQueue {
    fn new(capacity: usize, apply: Apply) -> Self {
        Self {
            capacity,
            apply,
            state: Mutex::new(QueueState {
                jobs: VecDeque::new(),
                busy: false,
            }),
            wake: Condvar::new(),
            drained: Condvar::new(),
            run_lock: Mutex::new(()),
            processed: AtomicU64::new(0),
            overflowed: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_run(&self) -> MutexGuard<'_, ()> {
        self.run_lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 投递一批；队列已满时把该会话的积压取出来，连同本批按原序在调用线程执行。
    ///
    /// 只捞本会话的积压：别的会话留在队列里，它们的相对顺序不受影响，写路径也不会因为
    /// 一次溢出被拖成「清空整个队列」的长尾。
    fn enqueue_or_run(&self, job: ProjectionJob) {
        let mut backlog = Vec::new();
        {
            let mut state = self.lock_state();
            if state.jobs.len() < self.capacity {
                state.jobs.push_back(job);
                self.wake.notify_one();
                return;
            }
            let session_id = job.session_id.clone();
            let mut kept = VecDeque::with_capacity(state.jobs.len());
            while let Some(queued) = state.jobs.pop_front() {
                if queued.session_id == session_id {
                    backlog.push(queued);
                } else {
                    kept.push_back(queued);
                }
            }
            state.jobs = kept;
        }
        let overflow = self.overflowed.fetch_add(1, Ordering::Relaxed) + 1;
        axon_logger::warning(&format!(
            "投影队列已满（容量 {}，累计同步兜底 {overflow} 次），本批改在写线程执行: session={} start={}",
            self.capacity,
            job.session_id.as_str(),
            job.start_sequence
        ));
        // 同一个会话的批必须按投递顺序执行：积压先跑，本批最后跑。
        let _guard = self.lock_run();
        for queued in backlog {
            self.execute_locked(queued);
        }
        self.execute_locked(job);
    }

    /// 在 worker 线程执行一批。
    fn execute(&self, job: ProjectionJob) {
        let _guard = self.lock_run();
        self.execute_locked(job);
    }

    /// 已持有执行互斥时执行一批；失败只计数并记日志，不向上传播。
    fn execute_locked(&self, job: ProjectionJob) {
        let session = job.session_id.as_str().to_owned();
        let start_sequence = job.start_sequence;
        let started = std::time::Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| (self.apply)(job)));
        // 投影已移出写路径，这里只留「单批偏慢」的归因线索，不影响写延迟。
        let elapsed_ms = started.elapsed().as_millis();
        if elapsed_ms >= 100 {
            axon_logger::warning(&format!(
                "单批投影偏慢：session={session} start={start_sequence} 耗时={elapsed_ms}ms"
            ));
        }
        self.processed.fetch_add(1, Ordering::Relaxed);
        match outcome {
            Ok(ProjectionOutcome::Applied) => {}
            Ok(ProjectionOutcome::Skipped { watermark }) => {
                let skipped = self.skipped.fetch_add(1, Ordering::Relaxed) + 1;
                /*
                 * 整批跳过是唯一会静默丢投影的路径：序号回退（重排、并发写者）时后一批先把
                 * 水位推高，前一批到这里就会被整批越过。日志带上水位与累计次数，事后才能
                 * 反推是哪一批被越过。
                 */
                axon_logger::debug(&format!(
                    "投影整批跳过（批首序号不大于水位，累计 {skipped} 批）: session={session} \
                     start={start_sequence} watermark={watermark}"
                ));
            }
            Ok(ProjectionOutcome::Failed { error }) => {
                let failed = self.failed.fetch_add(1, Ordering::Relaxed) + 1;
                axon_logger::warning(&format!(
                    "投影执行失败，水位不推进，派生读数可重建（累计 {failed} 批）: \
                     session={session} start={start_sequence} error={error}"
                ));
            }
            Err(payload) => {
                let failed = self.failed.fetch_add(1, Ordering::Relaxed) + 1;
                report(&format!(
                    "投影执行 panic（累计 {failed} 批），worker 继续消费下一批: session={session} \
                     start={start_sequence} panic={}",
                    panic_message(payload.as_ref())
                ));
            }
        }
    }

    /// worker 主循环：按投递顺序消费，单批失败不退出。
    ///
    /// 出队与执行都在 `run_lock` 内完成：否则「已出队、尚未取锁」的那一批既不在队列里，
    /// 溢出兜底也扫不到它，后批就能先跑并推高水位，前批随后被闸门整批跳过。
    fn run_worker(&self) {
        loop {
            /*
             * 等活时不持执行互斥：否则队列被溢出兜底摘空后 worker 会持着 `run_lock` 睡在
             * 条件变量上，而兜底正需要这把锁，写线程可能永久等锁。取活则在锁内完成，
             * 「已出队未执行」的窗口仍然不存在。
             */
            self.wait_for_work();
            let run_guard = self.lock_run();
            let Some(job) = self.take_job() else {
                drop(run_guard);
                continue;
            };
            self.execute_locked(job);
            drop(run_guard);
            let mut state = self.lock_state();
            state.busy = false;
            self.drained.notify_all();
        }
    }

    /// 阻塞到队列里有批可拿；等待期间不持执行互斥。
    fn wait_for_work(&self) {
        let mut state = self.lock_state();
        while state.jobs.is_empty() {
            self.drained.notify_all();
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// 在持有执行互斥时取一批；队列已被溢出兜底摘空时返回 `None`。
    fn take_job(&self) -> Option<ProjectionJob> {
        let mut state = self.lock_state();
        let job = state.jobs.pop_front();
        if job.is_some() {
            state.busy = true;
        }
        job
    }

    /// 阻塞到队列排空且 worker 手上没有在跑的批，返回累计已执行的批数。
    fn drain(&self) -> u64 {
        let mut state = self.lock_state();
        while !(state.jobs.is_empty() && !state.busy) {
            state = self
                .drained
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        self.processed.load(Ordering::Relaxed)
    }

    /// 单测用：把队列里剩下的批按原序在调用线程跑完。
    ///
    /// 单测不启线程，投递只会入队，由本函数模拟 worker 的消费顺序。
    #[cfg(test)]
    fn run_pending_for_tests(&self) {
        loop {
            let job = { self.lock_state().jobs.pop_front() };
            let Some(job) = job else {
                break;
            };
            self.execute(job);
        }
    }

    /// 队列满而改在调用线程执行的批数。
    #[cfg(test)]
    fn overflow_count(&self) -> u64 {
        self.overflowed.load(Ordering::Relaxed)
    }

    /// 已执行的批数（含跳过与失败）。
    #[cfg(test)]
    fn processed_count(&self) -> u64 {
        self.processed.load(Ordering::Relaxed)
    }

    /// 闸门跳过的批数。
    #[cfg(test)]
    fn skipped_count(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    /// 执行失败的批数。
    #[cfg(test)]
    fn failed_count(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }
}

/// 取进程内唯一的投影队列，并在第一次取用时拉起 worker 线程。
#[cfg(not(test))]
fn global_queue() -> &'static ProjectionQueue {
    static QUEUE: OnceLock<ProjectionQueue> = OnceLock::new();
    static SPAWNED: Once = Once::new();
    let queue = QUEUE.get_or_init(|| {
        ProjectionQueue::new(PROJECTION_QUEUE_CAPACITY, Box::new(|job| project(&job)))
    });
    SPAWNED.call_once(|| {
        let spawned = thread::Builder::new()
            .name(PROJECTION_WORKER_THREAD.to_owned())
            .spawn(|| {
                let Some(queue) = QUEUE.get() else {
                    report("投影队列尚未初始化，worker 无法启动");
                    return;
                };
                queue.run_worker();
            });
        match spawned {
            Ok(_) => WORKER_READY.store(true, Ordering::Release),
            Err(error) => report(&format!(
                "投影 worker 线程启动失败，投影退回同步执行: {error}"
            )),
        }
    });
    queue
}

impl SqliteRuntimeStore {
    /// 等待投影队列排空，返回进程内累计已执行的批数。
    ///
    /// 只给测试与诊断用：生产写路径不等投影，读侧也不依赖它。单测构建里投影本来就在调用
    /// 线程同步执行，返回的是本次进程里同步执行过的批数。
    ///
    /// 计数是进程级的，断言时只应比较下界（同一测试进程里其他用例也会投递）。
    pub fn drain_for_tests(&self) -> u64 {
        #[cfg(test)]
        {
            SYNC_PROCESSED.load(Ordering::Relaxed)
        }
        #[cfg(not(test))]
        {
            global_queue().drain()
        }
    }
}

/// 失败事实同时写日志文件与 stderr：日志文件是线上定位依据，stderr 是不依赖日志初始化的
/// 兜底通道（CLI 直跑时也能立刻看到）。
fn report(message: &str) {
    axon_logger::error(message);
    eprintln!("{message}");
}

/// 取 panic 载荷里的文本，取不到时给一句占位说明。
fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("未知 panic")
}

#[cfg(test)]
#[path = "projection_worker_tests.rs"]
mod tests;
