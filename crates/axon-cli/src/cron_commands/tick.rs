//! 到期 Cron 的领取、触发点判定与结算。
//!
//! 调度器只执行「触发点仍然有效」的到期任务：服务停机期间错过的触发点不补跑，
//! 直接顺延到计划给出的下一次，避免长时间停机恢复后把历史触发点当成现在该跑。

use axon_domain::{CronJob, CronRunRecord};

use super::{
    execution::{CronExecutionAdapter, execute_cron_job},
    now_ms,
    repository::{CronClaim, CronRepository},
    schedule::{next_run_from, parse_schedule},
};

/// 到期触发点允许的迟到容忍窗口。
///
/// 调度器按秒推进，正常迟到只在秒级；窗口用于吸收「同一批任务里前一个跑得久」造成
/// 的排队延迟与短暂重启。超过该窗口的触发点视为已经错过：停机几小时后恢复时，
/// 昨天的触发点不得被当作现在该跑。
pub(super) const DUE_LATE_ALLOWANCE_MS: u64 = 5 * 60 * 1_000;

/// 一次 tick 的结算结果。
#[derive(Debug, Default)]
pub(super) struct CronTickBatch {
    /// 本轮真正执行的到期任务。
    pub(super) jobs: Vec<CronJob>,
    /// 与 `jobs` 一一对应的执行记录。
    pub(super) runs: Vec<CronRunRecord>,
    /// 本轮判定为已经错过、只顺延未执行的数量。
    pub(super) skipped: usize,
}

/// 领取并结算本轮到期任务。
///
/// `scheduler_alive_since_ms` 是调度器本次连续在线的起点，判定规则见 [`should_fire`]。
/// 顺延同样计入 `limit`：一次 tick 的领取次数有上界，剩余事项由下一轮续跑。
pub(super) fn execute_due_jobs(
    adapter: &dyn CronExecutionAdapter,
    limit: usize,
    scheduler_alive_since_ms: u64,
) -> Result<CronTickBatch, String> {
    let repository = CronRepository::open(adapter.axon_root())?;
    let mut batch = CronTickBatch::default();
    while batch.jobs.len() + batch.skipped < limit {
        let now = now_ms();
        let Some(claim) = repository.claim_due(now)? else {
            break;
        };
        let scheduled_at_ms = claim.job.next_run_at_ms.unwrap_or(now);
        if !should_fire(scheduled_at_ms, scheduler_alive_since_ms, now) {
            reschedule_missed(&repository, &claim, now)?;
            batch.skipped += 1;
            continue;
        }
        // 同一触发点已经跑过（上一轮租约失守后重领）：只推进计划，不再执行一次。
        // 没有这一步时，任务体超过租约（120s）就会让同一次触发点被执行两遍。
        if repository.has_run_for_trigger(&claim.job.job_id, scheduled_at_ms)? {
            reschedule_missed(&repository, &claim, now)?;
            batch.skipped += 1;
            continue;
        }
        batch.jobs.push(claim.job.clone());
        batch
            .runs
            .push(execute_and_persist(adapter, &repository, &claim, true)?);
    }
    Ok(batch)
}

/// 判断一个已到期的触发点现在是否仍然该执行。
///
/// **绝对上限优先**：逾期超过 [`DUE_LATE_ALLOWANCE_MS`] 的触发点一律视为已经错过。
/// 只按「是否晚于本次上线」判断会漏掉休眠与长时间卡顿：线程不重启，休眠期间的所有
/// 触发点都「晚于上线」，于是 `@daily` 会在下午三点跑一次。
fn should_fire(scheduled_at_ms: u64, _scheduler_alive_since_ms: u64, now_ms: u64) -> bool {
    if scheduled_at_ms > now_ms {
        return false;
    }
    now_ms.saturating_sub(scheduled_at_ms) <= DUE_LATE_ALLOWANCE_MS
}

/// 错过的触发点不执行：只推进计划，不写执行事实。
fn reschedule_missed(
    repository: &CronRepository,
    claim: &CronClaim,
    now_ms: u64,
) -> Result<(), String> {
    let schedule = parse_schedule(&claim.job.schedule)?;
    let scheduled_at_ms = claim.job.next_run_at_ms.unwrap_or(now_ms);
    let next_run_at_ms = next_run_from(&schedule, scheduled_at_ms, now_ms);
    let next_job = claim.job.after_missed_run(now_ms, next_run_at_ms);
    // 顺延既可能是停机导致的正常结果，也可能暴露计划写错，必须留下可回溯的证据。
    // 默认日志级别是 warn，这里用 warning 才能让运维真的看见「这个任务今天没跑」。
    axon_logger::warning(&format!(
        "Cron 任务跳过已错过的触发点：job={} schedule={} 触发点={:?} 顺延到={:?}",
        claim.job.job_id, claim.job.schedule, claim.job.next_run_at_ms, next_run_at_ms
    ));
    repository.reschedule_claim(claim, &next_job)
}

/// 执行一次认领并把结果与下一次计划落库。
///
/// `scheduled` 为真表示这是一次计划触发，需要推进 `next_run_at_ms`；手动执行只写
/// 执行事实，不消费一次性计划，也不推迟下一次定时执行。
pub(crate) fn execute_and_persist(
    adapter: &dyn CronExecutionAdapter,
    repository: &CronRepository,
    claim: &CronClaim,
    scheduled: bool,
) -> Result<CronRunRecord, String> {
    let started_at_ms = now_ms();
    let heartbeat = repository.start_heartbeat(claim.claim_id.clone());
    let run = execute_cron_job(adapter, &claim.job, started_at_ms);
    let finished_at_ms = now_ms();
    let next_job = if scheduled {
        let schedule = parse_schedule(&claim.job.schedule)?;
        let scheduled_at_ms = claim.job.next_run_at_ms.unwrap_or(finished_at_ms);
        claim.job.after_run(
            finished_at_ms,
            next_run_from(&schedule, scheduled_at_ms, finished_at_ms),
            schedule.is_one_shot(),
        )
    } else {
        claim
            .job
            .after_run(finished_at_ms, claim.job.next_run_at_ms, false)
    };
    heartbeat.finish()?;
    repository.complete_claim(claim, &run, &next_job)?;
    Ok(run)
}

/// Daemon 侧的一次到期扫描：只回结算摘要，失败留给下一轮重试。
pub(crate) fn run_due_cron_jobs(
    adapter: &dyn CronExecutionAdapter,
    limit: usize,
    scheduler_alive_since_ms: u64,
) -> Result<String, String> {
    let batch = execute_due_jobs(adapter, limit, scheduler_alive_since_ms)?;
    let failed = batch.runs.iter().filter(|run| !run.success).count();
    Ok(format!(
        "claimed={} completed={} failed={failed} skipped={}",
        batch.jobs.len(),
        batch.runs.len(),
        batch.skipped
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use axon_domain::{CronJobInput, CronJobStatus};
    use chrono::{Local, TimeZone as _};

    use super::*;

    /// 只统计执行次数、不触达外部系统的 adapter。
    struct CountingAdapter {
        root: PathBuf,
        executed: AtomicUsize,
    }

    impl CountingAdapter {
        fn new(root: &Path) -> Self {
            Self {
                root: root.to_path_buf(),
                executed: AtomicUsize::new(0),
            }
        }

        fn executed(&self) -> usize {
            self.executed.load(Ordering::SeqCst)
        }
    }

    impl CronExecutionAdapter for CountingAdapter {
        fn axon_root(&self) -> &Path {
            &self.root
        }

        fn config_path(&self) -> &Path {
            &self.root
        }

        fn execute_agent(&self, _job: &CronJob, _prompt: &str) -> Result<String, String> {
            self.executed.fetch_add(1, Ordering::SeqCst);
            Ok(String::new())
        }

        fn wake_session(
            &self,
            _job: &CronJob,
            _session_id: &str,
            _body: &str,
            _now_ms: u64,
        ) -> Result<String, String> {
            Ok("interaction-test".to_owned())
        }
    }

    #[test]
    fn trigger_point_from_downtime_is_rescheduled_without_executing() {
        let root = axon_test_support::test_temp_dir();
        let repository = CronRepository::open(root.path()).expect("cron 仓库应可打开");
        let now = now_ms();
        let downtime = 8 * 60 * 60 * 1_000;
        let job = test_job("interval:1h", now - downtime);
        repository.save_job(&job).expect("任务应可保存");
        let adapter = CountingAdapter::new(root.path());

        let batch = execute_due_jobs(&adapter, 10, now).expect("到期扫描应成功");

        assert_eq!(batch.skipped, 1, "停机期间错过的触发点必须跳过");
        assert!(batch.runs.is_empty(), "跳过不得产生执行记录");
        assert_eq!(adapter.executed(), 0, "跳过不得真的执行任务");
        let reloaded = repository
            .find_job(&job.job_id)
            .expect("任务应可读取")
            .expect("任务应仍存在");
        assert!(
            reloaded.next_run_at_ms.expect("应顺延出下一次") > now,
            "跳过必须把计划推进到未来"
        );
        assert_eq!(
            reloaded.last_run_at_ms, None,
            "没有执行就不能写上次执行时间"
        );
        assert_eq!(reloaded.status, CronJobStatus::Active);
    }

    /// 同一触发点已经跑过（租约失守后重领）时不得再执行一次：任务体超过租约就会让脚本
    /// 与 agent 副作用各来一遍。判据落在执行事实上。
    #[test]
    fn already_settled_trigger_point_is_not_executed_twice() {
        let root = axon_test_support::test_temp_dir();
        let repository = CronRepository::open(root.path()).expect("cron 仓库应可打开");
        let now = now_ms();
        let scheduled_at_ms = now - 30 * 1_000;
        let job = test_job("interval:1h", scheduled_at_ms);
        repository.save_job(&job).expect("任务应可保存");
        let adapter = CountingAdapter::new(root.path());

        let batch = execute_due_jobs(&adapter, 10, now).expect("到期扫描应成功");
        assert_eq!(batch.runs.len(), 1);

        assert!(
            repository
                .has_run_for_trigger(&job.job_id, scheduled_at_ms)
                .expect("查询执行事实应成功"),
            "同一触发点必须判成已执行，重领时只顺延"
        );
        assert!(
            !repository
                .has_run_for_trigger(&job.job_id, now + 60 * 60 * 1_000)
                .expect("查询执行事实应成功"),
            "未发生的触发点不得判成已执行"
        );
    }

    #[test]
    fn trigger_point_overdue_beyond_allowance_is_skipped_even_when_scheduler_looks_alive() {
        let root = axon_test_support::test_temp_dir();
        let repository = CronRepository::open(root.path()).expect("cron 仓库应可打开");
        let now = now_ms();
        let job = test_job("interval:1h", now - 40 * 60 * 1_000);
        repository.save_job(&job).expect("任务应可保存");
        let adapter = CountingAdapter::new(root.path());

        let batch = execute_due_jobs(&adapter, 10, now - 60 * 60 * 1_000).expect("到期扫描应成功");

        assert_eq!(batch.skipped, 1, "逾期 40 分钟的触发点必须顺延");
        assert_eq!(adapter.executed(), 0);
    }

    #[test]
    fn short_restart_inside_allowance_still_runs() {
        let root = axon_test_support::test_temp_dir();
        let repository = CronRepository::open(root.path()).expect("cron 仓库应可打开");
        let now = now_ms();
        let job = test_job("interval:1h", now - 30 * 1_000);
        repository.save_job(&job).expect("任务应可保存");
        let adapter = CountingAdapter::new(root.path());

        let batch = execute_due_jobs(&adapter, 10, now).expect("到期扫描应成功");

        assert_eq!(batch.skipped, 0, "容忍窗口内的迟到必须照常执行");
        assert_eq!(adapter.executed(), 1);
    }

    #[test]
    fn missed_one_shot_completes_without_executing() {
        let root = axon_test_support::test_temp_dir();
        let repository = CronRepository::open(root.path()).expect("cron 仓库应可打开");
        let now = now_ms();
        let missed_at_ms = now - 2 * 60 * 60 * 1_000;
        let job = test_job(&once_schedule(missed_at_ms), missed_at_ms);
        repository.save_job(&job).expect("任务应可保存");
        let adapter = CountingAdapter::new(root.path());

        let batch = execute_due_jobs(&adapter, 10, now).expect("到期扫描应成功");

        assert_eq!(batch.skipped, 1);
        assert_eq!(adapter.executed(), 0);
        let reloaded = repository
            .find_job_any_status(&job.job_id)
            .expect("任务应可读取")
            .expect("任务应仍存在");
        assert_eq!(
            reloaded.status,
            CronJobStatus::Completed,
            "错过的一次性任务没有下一次，只能收口"
        );
        assert_eq!(reloaded.next_run_at_ms, None);
    }

    fn once_schedule(at_ms: u64) -> String {
        let local = Local
            .timestamp_millis_opt(i64::try_from(at_ms).expect("时间应在范围内"))
            .single()
            .expect("时间应有效");
        format!("once:{}", local.format("%Y-%m-%dT%H:%M:%S"))
    }

    fn test_job(schedule: &str, next_run_at_ms: u64) -> CronJob {
        CronJob::new(
            CronJobInput {
                job_id: "cron-due-window".to_owned(),
                name: "到期窗口".to_owned(),
                schedule: schedule.to_owned(),
                prompt: "执行".to_owned(),
                skills: Vec::new(),
                wake: None,
                workdir: None,
                no_agent: false,
                script: None,
            },
            next_run_at_ms,
            Some(next_run_at_ms),
        )
        .expect("测试任务应有效")
    }
}
