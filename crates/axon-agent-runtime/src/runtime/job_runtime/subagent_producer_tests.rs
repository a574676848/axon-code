//! 子代理作业生产者的行为测试（合同 §5.1.2 / §5.1.3）。
//!
//! 两条测试路径刻意为：注册表侧用进程级运行时验证端到端（提交 → 读取 → 归档），
//! 执行体侧直接构造 `SubagentJobExecution` 验证超时投影：超时判定需要注入时刻，
//! 而注册表的回收线程按真实时间推进、无法注入时钟。两条路径都只用公开 API 或
//! 本模块自身的能力，不新增测试专用生产分支。

use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use super::*;

/// 记录中断次数的宿主句柄。
struct RecordingInterrupt {
    count: Arc<AtomicUsize>,
}

impl SubagentJobInterrupt for RecordingInterrupt {
    fn interrupt(&self, _terminal: JobTerminalKind) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

/// 构造载荷；owner 必须逐用例唯一，避免进程级注册表在并行用例间串扰。
fn spec(owner: &str, interrupt: Arc<dyn SubagentJobInterrupt>) -> SubagentJobSpec {
    SubagentJobSpec {
        owner_id: owner.to_owned(),
        label: "/root/review".to_owned(),
        tee_root: Some(axon_test_support::test_temp_root()),
        max_runtime_ms: None,
        interrupt,
    }
}

/// 构造一个不接宿主的中断句柄。
fn idle_interrupt() -> (Arc<dyn SubagentJobInterrupt>, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let interrupt: Arc<dyn SubagentJobInterrupt> = Arc::new(RecordingInterrupt {
        count: Arc::clone(&count),
    });
    (interrupt, count)
}

/// 直接构造执行体：仅用于注入时刻的超时断言。
fn execution_with_deadline(
    deadline: Option<Instant>,
    interrupt: Arc<dyn SubagentJobInterrupt>,
) -> SubagentJobExecution {
    SubagentJobExecution {
        state: Arc::new(SubagentJobState {
            inner: Mutex::new(SubagentJobStateInner {
                generation: 0,
                terminal: None,
                stdout: ShellCommandBoundedStream::empty(),
                stdout_pending: false,
                artifact_path: None,
                artifact_failed: false,
                interrupt,
                deadline,
                final_message: None,
                final_artifact_ref: None,
            }),
            changed: Condvar::new(),
        }),
    }
}

// ---------- 注册表端到端 ----------

#[test]
fn reported_result_reaches_the_job_receipt() {
    let (interrupt, _) = idle_interrupt();
    let owner = "subagent-producer-receipt";
    let (job_id, handle) =
        submit_subagent_job(JobRuntime::global(), spec(owner, interrupt)).expect("提交必须成功");

    let running = JobRuntime::global()
        .read(job_id, owner, Some(0))
        .expect("刚提交的作业必须可读");
    assert_eq!(running.status, JobStatus::Running);
    assert!(
        running.artifact_path.exists(),
        "子代理作业启动即创建归档：{}",
        running.artifact_path.display()
    );

    let summary = "结论：改动安全".to_owned();
    handle.report(SubagentJobOutcome {
        kind: JobTerminalKind::Completed,
        exit_reason: Some("子代理正常结束".to_owned()),
        summary: summary.clone(),
        final_message: Some(summary.clone()),
        final_artifact_ref: None,
    });

    let output = JobRuntime::global()
        .read(job_id, owner, Some(0))
        .expect("终态作业必须可读");
    assert_eq!(output.status, JobStatus::Completed);
    assert_eq!(output.exit_code, None, "子代理作业没有进程退出码");
    assert_eq!(output.stdout.preview, summary);
    assert!(!output.stdout.truncated);
    assert!(
        output.full_output_available,
        "归档写入成功时完整输出必须可用"
    );
    assert!(
        std::fs::read_to_string(&output.artifact_path)
            .expect("归档必须可读")
            .contains(&summary)
    );
    assert_eq!(
        JobRuntime::global().producer_kind(job_id),
        Some(JobProducerKind::Subagent)
    );
    assert_eq!(
        JobRuntime::global().terminal_reason(job_id).as_deref(),
        Some("子代理正常结束")
    );
}

#[test]
fn oversized_result_is_bounded_in_the_receipt_and_complete_in_the_archive() {
    let (interrupt, _) = idle_interrupt();
    let owner = "subagent-producer-oversized";
    let (job_id, handle) =
        submit_subagent_job(JobRuntime::global(), spec(owner, interrupt)).expect("提交必须成功");

    let summary = "结".repeat(JOB_SUMMARY_MAX_CHARS + 64);
    handle.report(SubagentJobOutcome {
        kind: JobTerminalKind::Completed,
        exit_reason: None,
        summary: summary.clone(),
        final_message: Some("短结束语".to_owned()),
        final_artifact_ref: Some("/tmp/subagent-result.md".to_owned()),
    });

    let output = JobRuntime::global()
        .read(job_id, owner, Some(0))
        .expect("终态作业必须可读");
    assert!(output.stdout.truncated, "超阈值结果不得整段进回执");
    assert_eq!(output.stdout.original_chars, summary.chars().count());
    assert!(output.stdout.retained_chars <= JOB_SUMMARY_MAX_CHARS);
    assert!(
        std::fs::read_to_string(&output.artifact_path)
            .expect("归档必须可读")
            .contains(&summary),
        "完整结果必须落盘归档"
    );
}

#[test]
fn duplicate_report_keeps_the_first_terminal_state() {
    let (interrupt, _) = idle_interrupt();
    let owner = "subagent-producer-idempotent";
    let (job_id, handle) =
        submit_subagent_job(JobRuntime::global(), spec(owner, interrupt)).expect("提交必须成功");

    handle.report(SubagentJobOutcome {
        kind: JobTerminalKind::Completed,
        exit_reason: None,
        summary: "第一次回写".to_owned(),
        final_message: None,
        final_artifact_ref: None,
    });
    handle.report(SubagentJobOutcome {
        kind: JobTerminalKind::Failed,
        exit_reason: None,
        summary: "迟到的第二次回写".to_owned(),
        final_message: None,
        final_artifact_ref: None,
    });

    let output = JobRuntime::global()
        .read(job_id, owner, Some(0))
        .expect("终态作业必须可读");
    assert_eq!(
        output.status,
        JobStatus::Completed,
        "同一终态重复回写不得改写结局"
    );
    assert_eq!(output.stdout.preview, "第一次回写");
}

#[test]
fn killing_a_subagent_job_interrupts_the_host_then_ignores_late_reports() {
    let (interrupt, interrupts) = idle_interrupt();
    let owner = "subagent-producer-kill";
    let (job_id, handle) =
        submit_subagent_job(JobRuntime::global(), spec(owner, interrupt)).expect("提交必须成功");

    let status = JobRuntime::global()
        .kill(job_id, owner)
        .expect("终止运行中的子代理作业必须成功");
    assert_eq!(status, JobStatus::Killed);
    assert_eq!(
        interrupts.load(Ordering::Relaxed),
        1,
        "job_kill 必须把完整中断交给宿主执行一次"
    );

    handle.report(SubagentJobOutcome {
        kind: JobTerminalKind::Completed,
        exit_reason: None,
        summary: "迟到结果".to_owned(),
        final_message: None,
        final_artifact_ref: None,
    });

    let output = JobRuntime::global()
        .read(job_id, owner, Some(0))
        .expect("终态作业必须可读");
    assert_eq!(output.status, JobStatus::Killed, "迟到回写不得覆盖显式终止");
}

// ---------- 超时投影（注入时刻） ----------

#[test]
fn idle_timeout_projects_unresponsive_and_interrupts_the_host() {
    let (interrupt, interrupts) = idle_interrupt();
    let mut execution = execution_with_deadline(None, interrupt);
    let now = Instant::now();

    execution.poll(now, now).expect("空闲超时判定不得返回错误");

    let terminal = execution.terminal().expect("空闲超时必须投影终态");
    assert_eq!(terminal.status, JobStatus::Unresponsive);
    assert_eq!(
        terminal.status.terminal_kind(),
        Some(JobTerminalKind::Unresponsive)
    );
    assert_eq!(interrupts.load(Ordering::Relaxed), 1);
}

#[test]
fn wall_clock_timeout_projects_timeout_before_idle_timeout() {
    let (interrupt, interrupts) = idle_interrupt();
    let now = Instant::now();
    let mut execution = execution_with_deadline(Some(now), interrupt);

    execution
        .poll(now, now + Duration::from_secs(60))
        .expect("墙钟超时判定不得返回错误");

    let terminal = execution.terminal().expect("墙钟超时必须投影终态");
    assert_eq!(
        terminal.status,
        JobStatus::Timeout,
        "墙钟超时优先于空闲超时，两者不得混为同一个结局"
    );
    assert_eq!(interrupts.load(Ordering::Relaxed), 1);
}

#[test]
fn running_execution_reports_no_terminal_and_no_output() {
    let (interrupt, _) = idle_interrupt();
    let mut execution = execution_with_deadline(None, interrupt);
    let now = Instant::now();

    execution
        .poll(now, now + Duration::from_secs(60))
        .expect("运行中推进不得返回错误");

    assert!(execution.terminal().is_none());
    assert_eq!(execution.change_generation(), 0);
    let batch = execution.take_batch();
    assert!(batch.stdout.preview.is_empty());
    assert!(batch.stderr.preview.is_empty());
    assert!(execution.full_output_available());
}
