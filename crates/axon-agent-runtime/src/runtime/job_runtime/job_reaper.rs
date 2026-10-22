//! 作业回收与保留策略：终态判定、宽限让出、后台收割与墓碑裁剪。
//!
//! 从 `registry` 拆出：这一组只关心「什么时候可以丢掉一个作业记录」，
//! 与注册表的增删查是两件事，混在一起会让保留窗口的规则淹没在读写逻辑里。

use super::*;

pub(super) fn job_unreachable_reason(
    tombstones: &VecDeque<JobTombstone>,
    id: JobId,
) -> &'static str {
    if tombstones.iter().any(|tombstone| tombstone.job_id == id) {
        JOB_EXPIRED_ERROR
    } else {
        JOB_NOT_FOUND_ERROR
    }
}

pub(super) fn is_terminal(job: &Arc<Mutex<Job>>) -> bool {
    job.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .terminal
        .is_some()
}

/// 把模型给出的等待时长收敛到上界内。
pub(super) fn bounded_yield(yield_ms: Option<u64>) -> Duration {
    yield_ms
        .map(Duration::from_millis)
        .unwrap_or_default()
        .min(JOB_MAX_READ_YIELD)
}

/// 后台回收线程。
pub(super) fn spawn_reaper(jobs: JobRegistry, tombstones: JobTombstones) {
    thread::spawn(move || {
        loop {
            thread::sleep(JOB_REAPER_INTERVAL);
            reap_once(
                &jobs,
                &tombstones,
                Instant::now(),
                JOB_TERMINAL_RETENTION,
                JOB_MAX_READ_YIELD,
            );
        }
    });
}

/// 单次回收：本轮判定全部以注入的 `now` 与保留窗口为准，便于确定性测试。
pub(super) fn reap_once(
    jobs: &JobRegistry,
    tombstones: &JobTombstones,
    now: Instant,
    terminal_retention: Duration,
    idle_timeout: Duration,
) {
    let snapshot = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|(id, job)| (*id, Arc::clone(job)))
        .collect::<Vec<_>>();
    let mut expired = Vec::new();
    let mut notices = Vec::new();
    let mut pending_notices = Vec::new();
    for (id, job) in snapshot {
        let mut job = job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let poll_result = job.poll(now, idle_timeout);
        let poll_failed = poll_result.is_err();
        let mut newly_terminal = poll_result.unwrap_or(false);
        if poll_failed && job.terminal.is_none() {
            // 只有推进失败才强制收口；仍在运行的作业必须继续运行。
            let _ = job.execution.terminate();
            newly_terminal |= job.mark_terminal(JobTerminalState {
                status: JobStatus::Failed,
                exit_code: None,
                reason: Some(bounded_job_reason(JOB_REASON_EXECUTION_ERROR)),
            });
        }
        if newly_terminal {
            notices.push((id, job.notice_facts()));
        }
        if job
            .terminal
            .as_ref()
            .is_some_and(|terminal| now.duration_since(terminal.ended_at) >= terminal_retention)
        {
            expired.push(id);
        }
    }
    for (id, facts) in notices {
        if let Some(facts) = facts {
            pending_notices.extend(build_job_notice(id, facts));
        }
    }
    if !pending_notices.is_empty() {
        // 一轮回收里完成的多个作业合并为一次投递，避免逐条唤醒主 turn。
        publish_completion_notices(pending_notices);
    }
    if expired.is_empty() {
        prune_tombstones(tombstones, now);
        return;
    }
    jobs.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|id, _| !expired.contains(id));
    let mut tombstones = tombstones
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tombstones.extend(expired.into_iter().map(|job_id| JobTombstone {
        job_id,
        retired_at: now,
    }));
    while tombstones.len() > JOB_TOMBSTONE_CAPACITY {
        tombstones.pop_front();
    }
    prune_tombstones_locked(&mut tombstones, now);
}

pub(super) fn prune_tombstones(tombstones: &JobTombstones, now: Instant) {
    let mut tombstones = tombstones
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    prune_tombstones_locked(&mut tombstones, now);
}

pub(super) fn prune_tombstones_locked(tombstones: &mut VecDeque<JobTombstone>, now: Instant) {
    while tombstones.front().is_some_and(|tombstone| {
        now.duration_since(tombstone.retired_at) >= JOB_TOMBSTONE_RETENTION
    }) {
        tombstones.pop_front();
    }
}
