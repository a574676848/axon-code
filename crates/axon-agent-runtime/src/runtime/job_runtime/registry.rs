//! 作业注册表：身份分配、owner 校验、读取、终止、终态保留与墓碑回收。
//!
//! 注册表是**唯一**的作业生命周期所有者：它固定七个阶段的顺序与判据，
//! 通过 [`JobExecution`] 触达各生产者的执行体，不感知进程或子代理会话等具体机制。
//!
//! | 能力 | 触发条件 | 通道 | 并发策略 |
//! |---|---|---|---|
//! | PTY 交互式会话 | `tty: true` | `session_id` + `write_stdin` | `Exclusive` |
//! | 后台长任务 | `run_in_background: true` | `job_id` + `job_output` / `job_kill` | `ParallelSafe` |
//! | 前台短命令 | 默认 | 直接返回回执 | `WorkspaceCoordinated` |
//!
//! **不存在双通道**：作业注册表与会话注册表相互独立，作业 ID 永远不会被
//! `write_stdin` 接受，`session_id` 也永远不会被 `job_output` 接受。

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use axon_domain::{AxonError, AxonResult};

use super::{
    notice::{CompletionNotice, publish_completion_notice, publish_completion_notices},
    producer::{
        JOB_REASON_EXECUTION_ERROR, JobExecution, JobProducer, JobProducerKind, JobTerminalState,
        bounded_job_reason,
    },
    shell_producer::SHELL_JOB_PRODUCER,
};
use crate::{
    ShellCommandBoundedStream,
    runtime_limits::{
        JOB_LABEL_MAX_CHARS, JOB_MAX_READ_YIELD, JOB_POLL_INTERVAL, JOB_REAPER_INTERVAL,
        JOB_TERMINAL_RETENTION, JOB_TOMBSTONE_CAPACITY, JOB_TOMBSTONE_RETENTION,
    },
};

pub use super::shell_producer::JobSpec;

#[cfg(test)]
pub(crate) use super::shell_producer::JOB_COMMAND_EMPTY_ERROR;

const JOB_NOT_FOUND_ERROR: &str = "Job 不存在";
const JOB_EXPIRED_ERROR: &str = "Job 已结束并超出保留窗口，请重新执行命令";
const JOB_OWNER_ERROR: &str = "Job 不属于当前 Agent run";
const JOB_OWNER_EMPTY_ERROR: &str = "Job owner_id 不能为空";
const JOB_KILLED_STATUS_ERROR: &str = "Job 已进入终态";

/// 作业 ID。
///
/// 与 `session_id`（UUID）刻意不同形：作业 ID 是进程内单调递增的十进制数，
/// 便于模型逐字复制且不会被误当成会话 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// 作业状态。
///
/// 六类退出**互不合并**：`Refusal` 只表示执行前的准入裁决拒绝了这次任务，
/// `Killed` 只表示显式终止，墙钟超时与空闲超时各有独立状态，
/// 否则「为什么停了」在回执里不可诊断（合同 §8.1 第 6 条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// 仍在运行。
    Running,
    /// 正常结束，退出码已知。
    Completed,
    /// 任务未被接受：执行前的准入裁决（策略门 / 治理门）拒绝了这次任务。
    Refusal,
    /// 结束但退出码非零。
    Failed,
    /// 被 `job_kill` 显式终止。
    Killed,
    /// 越过墙钟超时被终止。
    Timeout,
    /// 越过空闲窗口被终止。
    Unresponsive,
}

/// 作业读取结果。
#[derive(Debug, Clone)]
pub struct JobOutput {
    /// 作业 ID。
    pub job_id: JobId,
    /// 作业展示标签，通常为原始命令或子代理任务路径。
    pub label: String,
    /// 从启动作业到本次读取的已运行毫秒数。
    pub elapsed_ms: u64,
    /// 当前状态。
    pub status: JobStatus,
    /// 终态退出码；仍在运行或结局无退出码时为 `None`。
    pub exit_code: Option<i32>,
    /// 本次读取的 stdout 增量批次。
    pub stdout: ShellCommandBoundedStream,
    /// 本次读取的 stderr 增量批次。
    pub stderr: ShellCommandBoundedStream,
    /// 完整输出归档路径；作业启动时即创建。
    pub artifact_path: PathBuf,
    /// 归档是否完整（未被容量上限截断、写入未失败）。
    pub full_output_available: bool,
}

/// 作业终态记录。
struct JobTerminal {
    status: JobStatus,
    exit_code: Option<i32>,
    reason: Option<String>,
    ended_at: Instant,
}

/// 单个后台作业。
struct Job {
    owner_id: String,
    label: String,
    producer: JobProducerKind,
    started: Instant,
    last_activity: Instant,
    execution: Box<dyn JobExecution>,
    terminal: Option<JobTerminal>,
}

impl Job {
    /// 推进作业状态：取消、超时、空闲超时与自然结束。
    ///
    /// 返回本次推进是否**首次**进入终态；调用方据此在释放作业锁之后再产出完成通知，
    /// 避免持锁回调。
    fn poll(&mut self, now: Instant, idle_timeout: Duration) -> std::io::Result<bool> {
        if self.terminal.is_some() {
            return Ok(false);
        }
        let outcome = self.execution.poll(now, self.last_activity + idle_timeout);
        if outcome.is_err() && self.execution.terminal().is_none() {
            // 执行体自身推进失败时不留下半途状态：终止它并投影为失败终态。
            let _ = self.execution.terminate();
        }
        let terminal = self.execution.terminal();
        let mut newly_terminal = false;
        if let Some(terminal) = terminal {
            newly_terminal = self.mark_terminal(terminal);
        }
        outcome.map(|()| newly_terminal)
    }

    fn mark_terminal(&mut self, state: JobTerminalState) -> bool {
        if self.terminal.is_some() {
            return false;
        }
        self.terminal = Some(JobTerminal {
            status: state.status,
            exit_code: state.exit_code,
            reason: state.reason,
            ended_at: Instant::now(),
        });
        true
    }

    /// 组装完成通知所需的全部事实；未终态时为 `None`。
    ///
    /// 结算正文与完整结果引用分开取：引用优先使用生产者给出的结果归档，
    /// 缺失时退回作业自身的输出归档：前者指向落盘的完整结果，
    /// 后者指向流式输出归档，两者都不参与正文截断（§4 不变量 6）。
    fn notice_facts(&self) -> Option<JobNoticeFacts> {
        let terminal = self.terminal.as_ref()?;
        let settlement = self.execution.notice_settlement();
        Some(JobNoticeFacts {
            owner_id: self.owner_id.clone(),
            producer: self.producer,
            label: self.label.clone(),
            terminal: JobTerminalState {
                status: terminal.status,
                exit_code: terminal.exit_code,
                reason: terminal.reason.clone(),
            },
            summary: settlement
                .as_ref()
                .and_then(|settlement| settlement.final_message.clone()),
            artifact_ref: settlement
                .as_ref()
                .and_then(|settlement| settlement.final_artifact_ref.clone())
                .map_or_else(|| self.execution.artifact_path(), PathBuf::from),
        })
    }

    fn status(&self) -> JobStatus {
        self.terminal
            .as_ref()
            .map_or(JobStatus::Running, |terminal| terminal.status)
    }

    fn exit_code(&self) -> Option<i32> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.exit_code)
    }
}

/// 已回收作业的墓碑；仅用于区分「曾存在但已回收」与「从未存在」。
struct JobTombstone {
    job_id: JobId,
    retired_at: Instant,
}

/// 作业注册表。
type JobRegistry = Arc<Mutex<HashMap<JobId, Arc<Mutex<Job>>>>>;
/// 已回收作业的墓碑队列。
type JobTombstones = Arc<Mutex<VecDeque<JobTombstone>>>;

static JOB_RUNTIME: LazyLock<JobRuntime> = LazyLock::new(JobRuntime::new);

/// 进程级运行代次：`job_id` 只是进程内计数，重启后会从 1 重新开始，
/// 因此通知去重键必须带上它，否则新进程的作业会与上一进程的旧通知撞键（R-10）。
static JOB_RUNTIME_EPOCH: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// 当前进程的运行代次。
#[must_use]
pub fn runtime_epoch() -> &'static str {
    &JOB_RUNTIME_EPOCH
}

/// 临时 Job 的引用文本：`{进程代际}:{作业 ID}`。
///
/// 该引用**只作可读线索**：进程代际随进程消亡，跨进程必然失效，因此任何解析路径
/// 都不得以它为主键或查询条件（合同 §5.3.3 要求引用重启后可解析：那由
/// artifact 磁盘路径与落盘摘要承担，不由本引用承担）。
#[must_use]
pub fn job_reference(job_id: JobId) -> String {
    format!("{}:{job_id}", runtime_epoch())
}

/// 后台作业运行时（进程级单例，与 Shell 会话管理器同级）。
pub struct JobRuntime {
    jobs: JobRegistry,
    tombstones: JobTombstones,
    next_id: AtomicU64,
}

impl JobRuntime {
    fn new() -> Self {
        let jobs: JobRegistry = Arc::new(Mutex::new(HashMap::new()));
        let tombstones: JobTombstones = Arc::new(Mutex::new(VecDeque::new()));
        spawn_reaper(Arc::clone(&jobs), Arc::clone(&tombstones));
        Self {
            jobs,
            tombstones,
            next_id: AtomicU64::new(1),
        }
    }

    /// 返回进程级作业运行时。
    #[must_use]
    pub fn global() -> &'static Self {
        &JOB_RUNTIME
    }

    /// 提交一个 Shell 后台作业并立即返回 ID。
    ///
    /// 返回 ID 之后作业的生命周期归作业系统所有：它不再绑定发起它的 tool call，
    /// 只能通过 [`Self::kill`] 或取消标记终止。
    pub fn start(&self, spec: JobSpec) -> AxonResult<JobId> {
        self.submit(&SHELL_JOB_PRODUCER, spec)
    }

    /// 通用提交入口：分配身份、校验 owner，再交给生产者启动执行体。
    ///
    /// owner 由生产者载荷注入（运行时来源），**不由模型提供**；校验统一在注册表完成，
    /// 因此所有生产者共享同一条归属规则。
    pub(crate) fn submit<P>(&self, producer: &dyn JobProducer<P>, payload: P) -> AxonResult<JobId> {
        let owner_id = producer.owner_id(&payload).to_owned();
        if owner_id.trim().is_empty() {
            return Err(AxonError::tool_execution_failed(JOB_OWNER_EMPTY_ERROR));
        }
        let label = bounded_job_label(producer.label(&payload));
        let id = JobId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let execution = producer.start(id, payload)?;
        let now = Instant::now();
        let job = Job {
            owner_id,
            label,
            producer: producer.kind(),
            started: now,
            last_activity: now,
            execution,
            terminal: None,
        };
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, Arc::new(Mutex::new(job)));
        Ok(id)
    }

    /// 读取作业的增量输出，必要时等待到终态或等待窗口结束。
    pub fn read(&self, id: JobId, owner: &str, yield_ms: Option<u64>) -> AxonResult<JobOutput> {
        let job = self.owned_job(id, owner)?;
        let deadline = Instant::now() + bounded_yield(yield_ms);
        let mut newly_terminal = false;
        loop {
            let (generation, now) = {
                let mut job = job
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let now = Instant::now();
                newly_terminal |= job
                    .poll(now, JOB_MAX_READ_YIELD)
                    .map_err(|error| AxonError::tool_execution_failed(error.to_string()))?;
                job.last_activity = now;
                (job.execution.change_generation(), now)
            };
            if now >= deadline {
                break;
            }
            let observed = generation;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = remaining.min(JOB_POLL_INTERVAL);
            job.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .execution
                .wait_for_change(observed, timeout);
            if is_terminal(&job) {
                break;
            }
        }
        if newly_terminal {
            self.publish_notice(id, &job);
        }
        self.project(id, &job)
    }

    /// 终止作业；原因按 Agent 自己发起的 `job_kill` 记录。
    pub fn kill(&self, id: JobId, owner: &str) -> AxonResult<JobStatus> {
        self.kill_with_reason(id, owner, None)
    }

    /// 终止作业并指定终态原因。
    ///
    /// `reason` 为 `None` 时交给生产者按默认原因收口（Shell 作业是 `job_kill`，
    /// 子代理作业是自己的中断措辞）；用户在界面上手动停止时传入
    /// [`JOB_REASON_USER_STOP`]，模型因此能区分终止来源。
    pub fn kill_with_reason(
        &self,
        id: JobId,
        owner: &str,
        reason: Option<&str>,
    ) -> AxonResult<JobStatus> {
        let job = self.owned_job(id, owner)?;
        let (status, newly_terminal) = {
            let mut job = job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.terminal.is_some() {
                return Err(AxonError::tool_execution_failed(JOB_KILLED_STATUS_ERROR));
            }
            match reason {
                Some(reason) => job
                    .execution
                    .terminate_with_reason(reason)
                    .map_err(|error| AxonError::tool_execution_failed(error.to_string()))?,
                None => job
                    .execution
                    .terminate()
                    .map_err(|error| AxonError::tool_execution_failed(error.to_string()))?,
            }
            let mut newly_terminal = false;
            let terminal = job.execution.terminal();
            if let Some(terminal) = terminal {
                newly_terminal = job.mark_terminal(terminal);
            }
            (job.status(), newly_terminal)
        };
        if newly_terminal {
            self.publish_notice(id, &job);
        }
        Ok(status)
    }

    /// 作业的归属 owner；作业不可达时为 `None`。
    ///
    /// 只读入口，供宿主在校验归属前解析「这个作业属于谁」；真正的归属裁决仍在
    /// [`Self::kill`] 一侧，调用方拿到 owner 之后必须把它作为 owner 传回去。
    #[must_use]
    pub fn owner_of(&self, id: JobId) -> Option<String> {
        let job = self.job_handle(id)?;
        let job = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(job.owner_id.clone())
    }

    /// 当前 owner 范围内未终态的作业 ID；按 ID 升序返回，供 pre-final 守卫判定。
    #[must_use]
    pub fn pending_job_ids(&self, owner: &str) -> Vec<JobId> {
        let snapshot = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(id, job)| (*id, Arc::clone(job)))
            .collect::<Vec<_>>();
        let mut pending = snapshot
            .into_iter()
            .filter(|(_, job)| {
                let job = job
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                job.owner_id == owner && job.terminal.is_none()
            })
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        pending.sort_unstable();
        pending
    }

    /// 作业的生产者类型；作业不可达时为 `None`。
    #[must_use]
    pub fn producer_kind(&self, id: JobId) -> Option<JobProducerKind> {
        let job = self.job_handle(id)?;
        let job = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(job.producer)
    }

    /// 作业的终态退出原因；仍在运行或作业不可达时为 `None`。
    #[must_use]
    pub fn terminal_reason(&self, id: JobId) -> Option<String> {
        let job = self.job_handle(id)?;
        let job = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        job.terminal
            .as_ref()
            .and_then(|terminal| terminal.reason.clone())
    }

    fn job_handle(&self, id: JobId) -> Option<Arc<Mutex<Job>>> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .cloned()
    }

    /// 产出一次完成通知；同一终态因事件键去重只会投递一次。
    fn publish_notice(&self, id: JobId, job: &Arc<Mutex<Job>>) {
        let facts = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .notice_facts();
        if let Some(facts) = facts {
            publish_job_notice(id, facts);
        }
    }

    /// 按 owner 立即释放已进入终态的作业（父 turn 结束时调用）。
    ///
    /// 合同裁决：终态保留窗口服务于「父 turn 存活期间的 job_output 查询」；父 turn
    /// 结束后查询消费者不复存在，结果只经完成主动通知交付。已回收作业登记墓碑，
    /// 后续读取仍可区分「已回收」与「从未存在」。
    ///
    /// 刚进入终态不足一个 reaper 周期的作业跳过：其完成通知可能尚未被回收线程投递，
    /// 立即移除会吞掉通知；它们由 reaper 按保留窗口正常处理。返回释放的作业数。
    pub fn release_terminal_for_owner(&self, owner: &str, now: Instant) -> usize {
        let snapshot = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, job)| {
                job.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .owner_id
                    == owner
            })
            .map(|(id, job)| (*id, Arc::clone(job)))
            .collect::<Vec<_>>();
        let mut released = Vec::new();
        for (id, job) in snapshot {
            let job = job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.terminal.as_ref().is_some_and(|terminal| {
                now.duration_since(terminal.ended_at) >= JOB_REAPER_INTERVAL
            }) {
                released.push(id);
            }
        }
        if released.is_empty() {
            return 0;
        }
        let released_count = released.len();
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|id, _| !released.contains(id));
        let mut tombstones = self
            .tombstones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tombstones.extend(released.into_iter().map(|job_id| JobTombstone {
            job_id,
            retired_at: now,
        }));
        while tombstones.len() > JOB_TOMBSTONE_CAPACITY {
            tombstones.pop_front();
        }
        prune_tombstones_locked(&mut tombstones, now);
        released_count
    }

    /// 按 ID 与 owner 取作业；不可达时区分「已回收」与「从未存在」。
    fn owned_job(&self, id: JobId, owner: &str) -> AxonResult<Arc<Mutex<Job>>> {
        let job = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .cloned();
        let Some(job) = job else {
            let tombstones = self
                .tombstones
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let reason = job_unreachable_reason(&tombstones, id);
            return Err(AxonError::tool_execution_failed(reason));
        };
        let job_owner = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .owner_id
            .clone();
        if job_owner != owner {
            return Err(AxonError::tool_execution_failed(JOB_OWNER_ERROR));
        }
        Ok(job)
    }

    /// 组装一次读取回执，并消费本批增量输出。
    fn project(&self, id: JobId, job: &Arc<Mutex<Job>>) -> AxonResult<JobOutput> {
        let job = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let batch = job.execution.take_batch();
        Ok(JobOutput {
            job_id: id,
            label: job.label.clone(),
            elapsed_ms: u64::try_from(job.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            status: job.status(),
            exit_code: job.exit_code(),
            stdout: batch.stdout,
            stderr: batch.stderr,
            artifact_path: job.execution.artifact_path(),
            full_output_available: job.execution.full_output_available(),
        })
    }
}

#[path = "job_notice.rs"]
mod job_notice;

use job_notice::{JobNoticeFacts, build_job_notice, publish_job_notice};

#[path = "job_reaper.rs"]
mod job_reaper;

use job_reaper::{
    bounded_yield, is_terminal, job_unreachable_reason, prune_tombstones_locked, spawn_reaper,
};
// 收割轮只在测试里被直接驱动：生产路径由 `spawn_reaper` 起的后台线程持有它，
// 因此这里按测试构建引入，免得非测试构建多出一条「从未使用」的告警。
#[cfg(test)]
use job_reaper::reap_once;

/// 截断作业标签以保持回执有界。
#[must_use]
pub fn bounded_job_label(label: &str) -> String {
    label.chars().take(JOB_LABEL_MAX_CHARS).collect()
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
