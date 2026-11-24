//! 一次性后台子代理生产者（合同 §5.1.2 / §5.1.3）。
//!
//! 子代理与 Shell 作业注册进**同一**作业注册表：它因此获得同一套 owner 校验、
//! 终态保留、墓碑与回收，以及同一条 `job_output` / `job_kill` 消费通道。
//!
//! 边界：本文件只承载生命周期接线，不承载子代理执行本身。中止 turn 与关闭会话
//! 由宿主通过 [`SubagentJobInterrupt`] 提供，本文件不依赖 `axon-cli`。

use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use axon_domain::{AxonError, AxonResult};

use super::{
    producer::{
        JOB_REASON_IDLE, JOB_REASON_TIMEOUT, JobExecution, JobExecutionBatch, JobNoticeSettlement,
        JobProducer, JobProducerKind, JobTerminalKind, JobTerminalState, bounded_job_reason,
    },
    registry::{JobId, JobRuntime, JobStatus},
};
use crate::{ShellCommandBoundedStream, runtime_limits::JOB_SUMMARY_MAX_CHARS};

const JOB_HANDLE_MISSING_ERROR: &str = "子代理作业回写句柄缺失";
/// 显式中断导致的退出原因。
const JOB_REASON_SUBAGENT_INTERRUPTED: &str = "job_kill 终止子代理作业";
/// 归档写入失败时的原因后缀。
const JOB_REASON_ARCHIVE_FAILED: &str = "归档写入失败";

/// 子代理作业的中断句柄：由子代理宿主（`axon-cli` 执行层）提供。
///
/// 作业内核只调用它，不感知会话、cancel token 或 mailbox 等宿主设施。
pub trait SubagentJobInterrupt: Send + Sync {
    /// 完整中断：中止当前 turn 并关闭会话（合同 §5.1.2 步骤 1-2）。
    ///
    /// `terminal` 是内核已经判定的六类终态之一：显式终止、墙钟超时与空闲超时
    /// 都在这里交给宿主，宿主据此投影**同一个**终态结构，不必自行猜测原因。
    fn interrupt(&self, terminal: JobTerminalKind);
}

/// 子代理作业的终态回写载荷。
#[derive(Debug, Clone)]
pub struct SubagentJobOutcome {
    /// 六类终态之一。
    pub kind: JobTerminalKind,
    /// 有界退出原因。
    pub exit_reason: Option<String>,
    /// 子代理结果正文（未截断）；超阈值时落盘并只回传引用。
    pub summary: String,
    /// 结束语：最后一条非空 assistant 文本块；没有则为 `None`。
    ///
    /// 与 [`Self::summary`] 分开承载：`summary` 是给 `job_output` 的完整回执，
    /// `final_message` 是给完成通知的有界结算正文，两者的截断口径不同。
    pub final_message: Option<String>,
    /// 结算正文超过内联阈值时的完整结果归档引用。
    pub final_artifact_ref: Option<String>,
}

/// 子代理作业的启动载荷。
pub struct SubagentJobSpec {
    /// 运行时注入的所有者 ID，不由模型提供；与 Shell 作业同源。
    pub owner_id: String,
    /// 有界展示标签，通常为子代理任务路径。
    pub label: String,
    /// 输出归档根目录；`None` 时使用系统临时目录。
    pub tee_root: Option<PathBuf>,
    /// 可选墙钟上限；来自子代理执行输入的最大运行时长。
    pub max_runtime_ms: Option<u64>,
    /// 完整中断句柄。
    pub interrupt: Arc<dyn SubagentJobInterrupt>,
}

/// 子代理作业句柄：宿主用它回写终态与结果，克隆共享同一作业状态。
#[derive(Clone)]
pub struct SubagentJobHandle {
    state: Arc<SubagentJobState>,
}

impl SubagentJobHandle {
    /// 回写终态与结果。
    ///
    /// 幂等：首次回写生效，之后（含 `job_kill` 之后的迟到回写）一律忽略，
    /// 因此不存在「已终止的作业被事后改写终态」的窗口。
    pub fn report(&self, outcome: SubagentJobOutcome) {
        let SubagentJobOutcome {
            kind,
            exit_reason,
            summary,
            final_message,
            final_artifact_ref,
        } = outcome;
        let mut state = self
            .state
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.terminal.is_some() {
            return;
        }
        let mut reason = exit_reason.unwrap_or_else(|| kind.as_str().to_owned());
        if let Some(path) = state.artifact_path.as_ref()
            && let Err(error) = append_result(path, &summary)
        {
            // 归档失败必须可见：回执的 full_output_available 会随之转假。
            state.artifact_failed = true;
            reason = format!("{reason}；{JOB_REASON_ARCHIVE_FAILED}：{error}");
        }
        state.stdout =
            ShellCommandBoundedStream::from_bounded_text(&summary, JOB_SUMMARY_MAX_CHARS);
        state.stdout_pending = true;
        state.final_message = final_message;
        state.final_artifact_ref = final_artifact_ref;
        state.terminal = Some(JobTerminalState {
            status: kind.status(),
            exit_code: None,
            reason: Some(bounded_job_reason(&reason)),
        });
        state.generation = state.generation.saturating_add(1);
        drop(state);
        self.state.changed.notify_all();
    }
}

/// 子代理作业的共享状态。
struct SubagentJobState {
    inner: Mutex<SubagentJobStateInner>,
    changed: Condvar,
}

struct SubagentJobStateInner {
    generation: u64,
    terminal: Option<JobTerminalState>,
    stdout: ShellCommandBoundedStream,
    stdout_pending: bool,
    artifact_path: Option<PathBuf>,
    artifact_failed: bool,
    interrupt: Arc<dyn SubagentJobInterrupt>,
    deadline: Option<Instant>,
    final_message: Option<String>,
    final_artifact_ref: Option<String>,
}

/// 子代理作业的生产者。
pub(crate) struct SubagentJobProducer;

/// 进程级子代理作业生产者。
pub(crate) static SUBAGENT_JOB_PRODUCER: SubagentJobProducer = SubagentJobProducer;

/// 提交载荷：规格 + 回写句柄落点。
struct SubagentJobPayload {
    spec: SubagentJobSpec,
    handle_slot: Arc<Mutex<Option<SubagentJobHandle>>>,
}

/// 提交一次性后台子代理作业；返回作业 ID 与宿主持有的结果回写句柄。
pub fn submit_subagent_job(
    runtime: &JobRuntime,
    spec: SubagentJobSpec,
) -> AxonResult<(JobId, SubagentJobHandle)> {
    let handle_slot = Arc::new(Mutex::new(None));
    let payload = SubagentJobPayload {
        spec,
        handle_slot: Arc::clone(&handle_slot),
    };
    let job_id = runtime.submit(&SUBAGENT_JOB_PRODUCER, payload)?;
    let handle = handle_slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or_else(|| AxonError::tool_execution_failed(JOB_HANDLE_MISSING_ERROR))?;
    Ok((job_id, handle))
}

impl JobProducer<SubagentJobPayload> for SubagentJobProducer {
    fn kind(&self) -> JobProducerKind {
        JobProducerKind::Subagent
    }

    fn owner_id<'a>(&self, payload: &'a SubagentJobPayload) -> &'a str {
        &payload.spec.owner_id
    }

    fn label<'a>(&self, payload: &'a SubagentJobPayload) -> &'a str {
        &payload.spec.label
    }

    fn start(
        &self,
        job_id: JobId,
        payload: SubagentJobPayload,
    ) -> AxonResult<Box<dyn JobExecution>> {
        let SubagentJobPayload { spec, handle_slot } = payload;
        let now = Instant::now();
        let artifact_path = open_subagent_artifact(&spec, job_id)?;
        let state = Arc::new(SubagentJobState {
            inner: Mutex::new(SubagentJobStateInner {
                generation: 0,
                terminal: None,
                stdout: ShellCommandBoundedStream::empty(),
                stdout_pending: false,
                artifact_path: Some(artifact_path),
                artifact_failed: false,
                interrupt: spec.interrupt,
                deadline: spec
                    .max_runtime_ms
                    .map(|limit| now + Duration::from_millis(limit)),
                final_message: None,
                final_artifact_ref: None,
            }),
            changed: Condvar::new(),
        });
        *handle_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(SubagentJobHandle {
            state: Arc::clone(&state),
        });
        Ok(Box::new(SubagentJobExecution { state }))
    }
}

/// 为子代理作业创建输出归档并写入可识别头部。
fn open_subagent_artifact(spec: &SubagentJobSpec, job_id: JobId) -> AxonResult<PathBuf> {
    let header = format!("subagent={}\nowner={}\n", spec.label, spec.owner_id);
    crate::builtin_shell_tool::process_manager::create_artifact_with_header(
        spec.tee_root.as_deref(),
        &format!("job-{job_id}"),
        &header,
    )
    .map_err(|error| AxonError::tool_execution_failed(format!("Job 输出归档创建失败：{error}")))
}

/// 把完整结果追加进归档。
fn append_result(path: &PathBuf, summary: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    file.write_all(summary.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}

/// 子代理作业的执行体：状态由宿主回写，内核只做推进与投影。
struct SubagentJobExecution {
    state: Arc<SubagentJobState>,
}

impl SubagentJobExecution {
    fn lock(&self) -> std::sync::MutexGuard<'_, SubagentJobStateInner> {
        self.state
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 触发宿主的完整中断；中断本身不吞掉终态投影。
    fn interrupt(&self, terminal: JobTerminalKind) {
        let interrupt = Arc::clone(&self.lock().interrupt);
        interrupt.interrupt(terminal);
    }

    fn mark_terminal(&self, status: JobStatus, reason: &str) {
        let mut state = self.lock();
        if state.terminal.is_some() {
            return;
        }
        state.terminal = Some(JobTerminalState {
            status,
            exit_code: None,
            reason: Some(bounded_job_reason(reason)),
        });
        state.generation = state.generation.saturating_add(1);
        drop(state);
        self.state.changed.notify_all();
    }
}

impl JobExecution for SubagentJobExecution {
    fn poll(&mut self, now: Instant, idle_deadline: Instant) -> std::io::Result<()> {
        let (terminal, deadline) = {
            let state = self.lock();
            (state.terminal.is_some(), state.deadline)
        };
        if terminal {
            return Ok(());
        }
        if deadline.is_some_and(|deadline| now >= deadline) {
            self.interrupt(JobTerminalKind::Timeout);
            self.mark_terminal(JobStatus::Timeout, JOB_REASON_TIMEOUT);
            return Ok(());
        }
        if now >= idle_deadline {
            self.interrupt(JobTerminalKind::Unresponsive);
            self.mark_terminal(JobStatus::Unresponsive, JOB_REASON_IDLE);
        }
        Ok(())
    }

    /// 子代理的默认终止原因保留自己的措辞：它说的不只是「被终止」，
    /// 还点明了被终止的是一次子代理作业。
    fn terminate(&mut self) -> std::io::Result<()> {
        self.terminate_with_reason(JOB_REASON_SUBAGENT_INTERRUPTED)
    }

    fn terminate_with_reason(&mut self, reason: &str) -> std::io::Result<()> {
        if self.lock().terminal.is_some() {
            return Ok(());
        }
        self.interrupt(JobTerminalKind::Killed);
        self.mark_terminal(JobStatus::Killed, reason);
        Ok(())
    }

    fn terminal(&self) -> Option<JobTerminalState> {
        self.lock().terminal.clone()
    }

    fn change_generation(&self) -> u64 {
        self.lock().generation
    }

    fn wait_for_change(&self, observed: u64, timeout: Duration) {
        let state = self.lock();
        if state.generation != observed || state.terminal.is_some() {
            return;
        }
        let _unused = self
            .state
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.generation == observed && state.terminal.is_none()
            });
    }

    fn take_batch(&self) -> JobExecutionBatch {
        let mut state = self.lock();
        let stdout = if state.stdout_pending {
            state.stdout_pending = false;
            std::mem::replace(&mut state.stdout, ShellCommandBoundedStream::empty())
        } else {
            ShellCommandBoundedStream::empty()
        };
        JobExecutionBatch {
            stdout,
            stderr: ShellCommandBoundedStream::empty(),
        }
    }

    fn artifact_path(&self) -> PathBuf {
        self.lock().artifact_path.clone().unwrap_or_default()
    }

    fn full_output_available(&self) -> bool {
        !self.lock().artifact_failed
    }

    fn notice_settlement(&self) -> Option<JobNoticeSettlement> {
        let state = self.lock();
        state.terminal.as_ref()?;
        Some(JobNoticeSettlement {
            final_message: state.final_message.clone(),
            final_artifact_ref: state.final_artifact_ref.clone(),
        })
    }
}

#[cfg(test)]
#[path = "subagent_producer_tests.rs"]
mod tests;
