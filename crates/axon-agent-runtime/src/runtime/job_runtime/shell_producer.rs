//! Shell 后台作业生产者：统一生命周期的第一个实现（合同 §5.1.1）。
//!
//! 本文件是原 `job_runtime.rs` 中 Shell 专有动作的迁移落点：进程派生、取消标记、
//! 墙钟超时、增量输出与完整输出归档。生命周期顺序、owner 校验与保留窗口仍在注册表，
//! 本文件不重复实现它们。

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axon_domain::{AxonError, AxonResult};

use super::{
    producer::{
        JOB_REASON_CANCELLED, JOB_REASON_IDLE, JOB_REASON_TIMEOUT, JobExecution, JobExecutionBatch,
        JobProducer, JobProducerKind, JobTerminalState, bounded_job_reason,
    },
    registry::{JobId, JobStatus},
};
use crate::{
    ShellCommandCancelToken,
    builtin_shell_tool::{
        output_buffer::SharedProcessOutput, process_backend::ProcessBackend,
        process_manager::create_artifact,
    },
    runtime_limits::SHELL_ARTIFACT_MAX_BYTES,
};

/// 空命令拒绝原因。
pub(crate) const JOB_COMMAND_EMPTY_ERROR: &str = "Job 命令不能为空";
const JOB_ARTIFACT_ERROR: &str = "Job 输出归档创建失败";
/// Shell 作业正常结束的退出原因。
const JOB_REASON_COMPLETED: &str = "正常结束";

/// Shell 后台作业的启动载荷。
///
/// 不派生 `Debug`：`cancel_token` 是 trait object，无法实现 `Debug`，
/// 而作业规格只用于启动路径，不需要调试打印。
#[derive(Clone)]
pub struct JobSpec {
    /// 当前 Agent run 的内部所有者 ID，不由模型提供。
    pub owner_id: String,
    /// 作业展示标签，通常为原始命令。
    pub label: String,
    /// 待执行命令。
    pub command: Vec<String>,
    /// 工作目录。
    pub cwd: PathBuf,
    /// 可选墙钟超时。
    pub timeout_ms: Option<u64>,
    /// 可选取消标记；作业持有独立生命周期，不绑定 tool call。
    pub cancel_token: Option<ShellCommandCancelToken>,
    /// 输出归档根目录；`None` 时使用系统临时目录。
    pub tee_root: Option<PathBuf>,
}

/// Shell 后台作业生产者。
pub(crate) struct ShellJobProducer;

/// 进程级 Shell 作业生产者。
pub(crate) static SHELL_JOB_PRODUCER: ShellJobProducer = ShellJobProducer;

impl JobProducer<JobSpec> for ShellJobProducer {
    fn kind(&self) -> JobProducerKind {
        JobProducerKind::Shell
    }

    fn owner_id<'a>(&self, payload: &'a JobSpec) -> &'a str {
        &payload.owner_id
    }

    fn label<'a>(&self, payload: &'a JobSpec) -> &'a str {
        &payload.label
    }

    fn start(&self, job_id: JobId, payload: JobSpec) -> AxonResult<Box<dyn JobExecution>> {
        if payload.command.is_empty() {
            return Err(AxonError::tool_execution_failed(JOB_COMMAND_EMPTY_ERROR));
        }
        let artifact_path = open_job_artifact(
            payload.tee_root.as_deref(),
            &format!("job-{job_id}"),
            &payload.command,
            &payload.cwd,
        )?;
        let artifact = std::fs::OpenOptions::new()
            .append(true)
            .open(&artifact_path)
            .map_err(|_| AxonError::tool_execution_failed(JOB_ARTIFACT_ERROR))?;
        let output = Arc::new(SharedProcessOutput::new(artifact, SHELL_ARTIFACT_MAX_BYTES));
        let backend = ProcessBackend::spawn(
            &payload.command,
            &payload.cwd,
            false,
            Arc::clone(&output),
            &[],
        )
        .map_err(|error| AxonError::tool_execution_failed(error.to_string()))?;
        let now = Instant::now();
        Ok(Box::new(ShellJobExecution {
            backend,
            output,
            artifact_path,
            deadline: payload
                .timeout_ms
                .map(|timeout| now + Duration::from_millis(timeout)),
            cancel_token: payload.cancel_token,
            terminal: None,
        }))
    }
}

/// 创建作业输出归档；路径与 Shell 会话共用同一归档目录与后缀。
pub(crate) fn open_job_artifact(
    tee_root: Option<&std::path::Path>,
    artifact_name: &str,
    command: &[String],
    cwd: &std::path::Path,
) -> AxonResult<PathBuf> {
    create_artifact(tee_root, artifact_name, command, cwd, false)
        .map_err(|_| AxonError::tool_execution_failed(JOB_ARTIFACT_ERROR))
}

/// 单个 Shell 后台作业的执行体。
struct ShellJobExecution {
    backend: ProcessBackend,
    output: Arc<SharedProcessOutput>,
    artifact_path: PathBuf,
    deadline: Option<Instant>,
    cancel_token: Option<ShellCommandCancelToken>,
    terminal: Option<JobTerminalState>,
}

impl ShellJobExecution {
    /// 终止进程并投影终态。
    fn terminate_with(&mut self, status: JobStatus, reason: &str) -> std::io::Result<()> {
        self.backend.terminate()?;
        self.mark_terminal(status, None, reason);
        Ok(())
    }

    fn mark_terminal(&mut self, status: JobStatus, exit_code: Option<i32>, reason: &str) {
        self.terminal = Some(JobTerminalState {
            status,
            exit_code,
            reason: Some(bounded_job_reason(reason)),
        });
        self.output.notify_change();
    }
}

impl JobExecution for ShellJobExecution {
    fn poll(&mut self, now: Instant, idle_deadline: Instant) -> std::io::Result<()> {
        if self.terminal.is_some() {
            return Ok(());
        }
        // 三类终止的**原因不同**，因此各自投影到独立终态，不再合并成同一个 Killed。
        if self.cancel_token.as_ref().is_some_and(|token| token()) {
            return self.terminate_with(JobStatus::Killed, JOB_REASON_CANCELLED);
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            return self.terminate_with(JobStatus::Timeout, JOB_REASON_TIMEOUT);
        }
        if now >= idle_deadline {
            return self.terminate_with(JobStatus::Unresponsive, JOB_REASON_IDLE);
        }
        if let Some(exit_code) = self.backend.try_wait()? {
            if exit_code == 0 {
                self.mark_terminal(JobStatus::Completed, Some(exit_code), JOB_REASON_COMPLETED);
            } else {
                self.mark_terminal(
                    JobStatus::Failed,
                    Some(exit_code),
                    &format!("退出码 {exit_code}"),
                );
            }
        }
        Ok(())
    }

    fn terminate_with_reason(&mut self, reason: &str) -> std::io::Result<()> {
        if self.terminal.is_some() {
            return Ok(());
        }
        self.terminate_with(JobStatus::Killed, reason)
    }

    fn terminal(&self) -> Option<JobTerminalState> {
        self.terminal.clone()
    }

    fn change_generation(&self) -> u64 {
        self.output.change_generation()
    }

    fn wait_for_change(&self, observed: u64, timeout: Duration) {
        self.output.wait_for_change(observed, timeout);
    }

    fn take_batch(&self) -> JobExecutionBatch {
        let batch = self.output.take_receipt(None);
        JobExecutionBatch {
            stdout: batch.stdout,
            stderr: batch.stderr,
        }
    }

    fn artifact_path(&self) -> PathBuf {
        self.artifact_path.clone()
    }

    fn full_output_available(&self) -> bool {
        !self.output.artifact_truncated() && !self.output.artifact_failed()
    }
}
