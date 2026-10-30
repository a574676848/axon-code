use std::path::Path;

use axon_crypto::sha256_hex_prefix;
use axon_domain::{
    CronJob, CronJobInput, CronJobStatus, CronJobUpdate, CronRunRecord, CronWakeTarget,
};
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::cli_error,
    remote_commands::{FeishuCronAdapter, load_remote_context},
    usage_statistics::{
        USAGE_ROLLUP_CRON_JOB_ID, USAGE_ROLLUP_CRON_JOB_NAME, USAGE_ROLLUP_CRON_SCRIPT,
        USAGE_ROLLUP_INTERVAL_MS,
    },
};

use super::{
    MEMORY_CONSOLIDATION_CRON_JOB_ID, MEMORY_CONSOLIDATION_CRON_NAME,
    MEMORY_CONSOLIDATION_CRON_SCRIPT, SESSION_CLEANUP_CRON_JOB_ID, SESSION_CLEANUP_CRON_NAME,
    SESSION_CLEANUP_CRON_SCRIPT, SESSION_CLEANUP_INTERVAL_MS,
    args::{
        CronAction, CronCommand, CronCreateCommand, CronListCommand, CronRunCommand,
        CronTickCommand, CronUpdateCommand,
    },
    execution::{CronExecutionAdapter, execute_cron_job},
    render::render_cron,
    repository::{CronClaim, CronRepository},
    schedule::{next_run_after, parse_schedule},
    wake::LEGACY_DELIVERY_ARGS_HINT,
};

const CRON_DEFAULT_LIMIT: usize = 20;
const CRON_MAX_LIMIT: usize = 100;
const CRON_STATUS_CREATED: &str = "cron_created";
const CRON_STATUS_UPDATED: &str = "cron_updated";
const CRON_STATUS_READY: &str = "cron_ready";
const CRON_STATUS_STATE_UPDATED: &str = "cron_status_updated";
const CRON_STATUS_RUN_READY: &str = "cron_run_ready";
const CRON_STATUS_TICK_READY: &str = "cron_tick_ready";

#[derive(Debug, Clone, Copy)]
struct SystemCronSpec {
    job_id: &'static str,
    name: &'static str,
    script: &'static str,
    interval_ms: u64,
    error_label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CronReport {
    pub(super) status: &'static str,
    pub(super) store_path: String,
    pub(super) total_jobs: usize,
    pub(super) listed_jobs: usize,
    pub(super) executed_jobs: usize,
    pub(super) success_runs: usize,
    pub(super) failed_runs: usize,
    pub(super) delivery_executed: bool,
    pub(super) jobs: Vec<CronJobView>,
    pub(super) runs: Vec<CronRunRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CronJobView {
    pub(super) job_id: String,
    pub(super) name: String,
    pub(super) schedule: String,
    pub(super) status: String,
    pub(super) no_agent: bool,
    pub(super) next_run_at_ms: Option<u64>,
    pub(super) last_run_at_ms: Option<u64>,
    pub(super) revision: u64,
    /// 唤醒目标展示文本；未声明目标时为 `None`。
    pub(super) wake: Option<String>,
}

pub(crate) fn run_cron(
    command: &CronCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let (_, context) = match load_remote_context(config_path) {
        Ok(context) => context,
        Err(error) => return cli_error(error),
    };
    let adapter = FeishuCronAdapter::new(context);
    let report = match &command.action {
        CronAction::Create(command) => create_job(&adapter, command),
        CronAction::Update(command) => update_job(&adapter, command),
        CronAction::List(command) => list_jobs(&adapter, command),
        CronAction::Pause(command) => set_status(&adapter, &command.job_id, CronJobStatus::Paused),
        CronAction::Resume(command) => set_status(&adapter, &command.job_id, CronJobStatus::Active),
        CronAction::Remove(command) => {
            set_status(&adapter, &command.job_id, CronJobStatus::Removed)
        }
        CronAction::Run(command) => run_job(&adapter, command),
        CronAction::Tick(command) => tick_jobs(&adapter, command),
    };
    match report {
        Ok(report) => render_cron(format, &report),
        Err(error) => cli_error(error),
    }
}

pub(crate) fn ensure_memory_consolidation_cron(axon_root: &Path) -> Result<String, String> {
    ensure_system_cron(
        axon_root,
        SystemCronSpec {
            job_id: MEMORY_CONSOLIDATION_CRON_JOB_ID,
            name: MEMORY_CONSOLIDATION_CRON_NAME,
            script: MEMORY_CONSOLIDATION_CRON_SCRIPT,
            interval_ms: axon_agent_runtime::MEMORY_CONSOLIDATION_DEFAULT_INTERVAL_MS,
            error_label: "Memory consolidation",
        },
    )
}

pub(crate) fn ensure_usage_rollup_cron(axon_root: &Path) -> Result<String, String> {
    ensure_system_cron(
        axon_root,
        SystemCronSpec {
            job_id: USAGE_ROLLUP_CRON_JOB_ID,
            name: USAGE_ROLLUP_CRON_JOB_NAME,
            script: USAGE_ROLLUP_CRON_SCRIPT,
            interval_ms: USAGE_ROLLUP_INTERVAL_MS,
            error_label: "Usage rollup",
        },
    )
}

pub(crate) fn ensure_session_cleanup_cron(axon_root: &Path) -> Result<String, String> {
    ensure_system_cron(
        axon_root,
        SystemCronSpec {
            job_id: SESSION_CLEANUP_CRON_JOB_ID,
            name: SESSION_CLEANUP_CRON_NAME,
            script: SESSION_CLEANUP_CRON_SCRIPT,
            interval_ms: SESSION_CLEANUP_INTERVAL_MS,
            error_label: "Session storage cleanup",
        },
    )
}

/// 模型费率自动同步任务的目标状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PricingSyncCronState {
    pub(crate) enabled: bool,
    pub(crate) job_exists: bool,
    pub(crate) status: String,
    pub(crate) schedule: String,
    pub(crate) last_run_at_ms: Option<u64>,
    pub(crate) next_run_at_ms: Option<u64>,
    pub(crate) last_run_success: Option<bool>,
    pub(crate) last_run_summary: Option<String>,
}

/// 读取自动同步任务的当前状态；任务不存在时如实报「未创建」。
pub(crate) fn pricing_sync_cron_state(axon_root: &Path) -> Result<PricingSyncCronState, String> {
    let repository = CronRepository::open(axon_root)?;
    let job = repository.find_job(crate::usage_statistics::PRICING_SYNC_CRON_JOB_ID)?;
    let Some(job) = job else {
        return Ok(PricingSyncCronState {
            enabled: false,
            job_exists: false,
            status: "missing".to_owned(),
            schedule: format!(
                "interval:{}ms",
                crate::usage_statistics::PRICING_SYNC_INTERVAL_MS
            ),
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_run_success: None,
            last_run_summary: None,
        });
    };
    let latest = repository.latest_run_for_job(&job.job_id)?;
    Ok(PricingSyncCronState {
        enabled: job.status == CronJobStatus::Active,
        job_exists: true,
        status: job_status_text(job.status).to_owned(),
        schedule: job.schedule.clone(),
        last_run_at_ms: latest
            .as_ref()
            .map(|run| run.finished_at_ms)
            .or(job.last_run_at_ms),
        next_run_at_ms: job.next_run_at_ms,
        last_run_success: latest.as_ref().map(|run| run.success),
        last_run_summary: latest
            .as_ref()
            .and_then(|run| run.error_summary.clone())
            .filter(|summary| !summary.is_empty()),
    })
}

/// 打开或关闭自动同步：打开时创建（或重新启用）系统任务，关闭时转为暂停。
///
/// 关闭不删除任务：任务的历史运行记录是「上次同步是什么时候、结果如何」的事实来源，
/// 删掉它只会让下一次开启变成一片空白。
pub(crate) fn set_pricing_sync_cron_enabled(
    axon_root: &Path,
    enabled: bool,
) -> Result<PricingSyncCronState, String> {
    if enabled {
        let _ = ensure_system_cron(
            axon_root,
            SystemCronSpec {
                job_id: crate::usage_statistics::PRICING_SYNC_CRON_JOB_ID,
                name: crate::usage_statistics::PRICING_SYNC_CRON_JOB_NAME,
                script: crate::usage_statistics::PRICING_SYNC_CRON_SCRIPT,
                interval_ms: crate::usage_statistics::PRICING_SYNC_INTERVAL_MS,
                error_label: "Models.dev pricing sync",
            },
        )?;
    } else {
        let repository = CronRepository::open(axon_root)?;
        if let Some(job) =
            repository.find_job_any_status(crate::usage_statistics::PRICING_SYNC_CRON_JOB_ID)?
            && job.status != CronJobStatus::Paused
        {
            let paused = job.with_status(CronJobStatus::Paused, now_ms());
            repository.save_job(&paused)?;
        }
    }
    pricing_sync_cron_state(axon_root)
}

fn job_status_text(status: CronJobStatus) -> &'static str {
    match status {
        CronJobStatus::Active => "active",
        CronJobStatus::Paused => "paused",
        CronJobStatus::Completed => "completed",
        CronJobStatus::Removed => "removed",
    }
}

fn ensure_system_cron(axon_root: &Path, spec: SystemCronSpec) -> Result<String, String> {
    let repository = CronRepository::open(axon_root)?;
    let now = now_ms();
    let schedule = format!("interval:{}ms", spec.interval_ms);
    let parsed_schedule = parse_schedule(&schedule)?;
    let next_run_at_ms = next_run_after(&parsed_schedule, now);
    let existing = repository.find_job_any_status(spec.job_id)?;
    let desired = CronJob::new(
        CronJobInput {
            job_id: spec.job_id.to_owned(),
            name: spec.name.to_owned(),
            schedule,
            prompt: String::new(),
            skills: Vec::new(),
            wake: None,
            workdir: None,
            no_agent: true,
            script: Some(spec.script.to_owned()),
        },
        now,
        next_run_at_ms,
    )
    .map_err(|error| format!("创建 {} Cron 失败：{error}", spec.error_label))?;
    let Some(existing) = existing else {
        repository.save_job(&desired)?;
        return Ok("created".to_owned());
    };
    if system_cron_matches(&existing, &desired) {
        return Ok("unchanged".to_owned());
    }
    let mut updated = desired;
    updated.revision = existing.revision.saturating_add(1);
    updated.created_at_ms = existing.created_at_ms;
    updated.last_run_at_ms = existing.last_run_at_ms;
    repository.save_job(&updated)?;
    Ok("updated".to_owned())
}

fn system_cron_matches(existing: &CronJob, desired: &CronJob) -> bool {
    existing.name == desired.name
        && existing.schedule == desired.schedule
        && existing.prompt == desired.prompt
        && existing.skills == desired.skills
        && existing.wake == desired.wake
        && existing.workdir == desired.workdir
        && existing.no_agent == desired.no_agent
        && existing.script == desired.script
        && existing.status == CronJobStatus::Active
}

fn create_job(
    adapter: &dyn CronExecutionAdapter,
    command: &CronCreateCommand,
) -> Result<CronReport, String> {
    reject_legacy_delivery_args(
        command.feishu_chat_id.as_ref(),
        command.feishu_thread_id.as_ref(),
    )?;
    let repository = CronRepository::open(adapter.axon_root())?;
    let created_at_ms = now_ms();
    let schedule = parse_schedule(&command.schedule)?;
    let job = CronJob::new(
        CronJobInput {
            job_id: stable_job_id(&command.name, created_at_ms),
            name: command.name.clone(),
            schedule: command.schedule.clone(),
            prompt: command.prompt.clone(),
            skills: command.skills.clone(),
            wake: command
                .wake
                .as_deref()
                .map(super::wake::parse_wake)
                .transpose()?,
            workdir: path_text(command.workdir.as_deref())?,
            no_agent: command.no_agent,
            script: command.script.clone(),
        },
        created_at_ms,
        next_run_after(&schedule, created_at_ms),
    )
    .map_err(|error| error.to_string())?;
    repository.save_job(&job)?;
    report(adapter, CRON_STATUS_CREATED, &[job], Vec::new(), false)
}

fn update_job(
    adapter: &dyn CronExecutionAdapter,
    command: &CronUpdateCommand,
) -> Result<CronReport, String> {
    let repository = CronRepository::open(adapter.axon_root())?;
    let job = required_job(&repository, &command.job_id)?;
    let updated_at_ms = now_ms();
    let schedule_text = command.schedule.as_deref().unwrap_or(&job.schedule);
    let schedule = parse_schedule(schedule_text)?;
    let updated = job
        .with_update(
            update_input(command)?,
            updated_at_ms,
            next_run_after(&schedule, updated_at_ms),
        )
        .map_err(|error| error.to_string())?;
    repository.save_job(&updated)?;
    report(adapter, CRON_STATUS_UPDATED, &[updated], Vec::new(), false)
}

fn list_jobs(
    adapter: &dyn CronExecutionAdapter,
    command: &CronListCommand,
) -> Result<CronReport, String> {
    let limit = bounded_limit(command.limit)?;
    let repository = CronRepository::open(adapter.axon_root())?;
    let mut visible = repository.jobs(command.include_removed)?;
    visible.truncate(limit);
    report(adapter, CRON_STATUS_READY, &visible, Vec::new(), false)
}

fn set_status(
    adapter: &dyn CronExecutionAdapter,
    job_id: &str,
    status: CronJobStatus,
) -> Result<CronReport, String> {
    if status == CronJobStatus::Removed && super::is_system_job(job_id) {
        return Err("系统 Cron 无法删除".to_owned());
    }
    let repository = CronRepository::open(adapter.axon_root())?;
    let updated = required_job(&repository, job_id)?.with_status(status, now_ms());
    repository.save_job(&updated)?;
    report(
        adapter,
        CRON_STATUS_STATE_UPDATED,
        &[updated],
        Vec::new(),
        false,
    )
}

fn run_job(
    adapter: &dyn CronExecutionAdapter,
    command: &CronRunCommand,
) -> Result<CronReport, String> {
    let repository = CronRepository::open(adapter.axon_root())?;
    let claim = repository.claim_manual(&command.job_id, now_ms())?;
    let run = execute_and_persist(adapter, &repository, &claim, false)?;
    let job = required_job(&repository, &command.job_id)?;
    report(adapter, CRON_STATUS_RUN_READY, &[job], vec![run], false)
}

fn tick_jobs(
    adapter: &dyn CronExecutionAdapter,
    command: &CronTickCommand,
) -> Result<CronReport, String> {
    let limit = bounded_limit(command.limit)?;
    let (jobs, runs) = execute_due_jobs(adapter, limit)?;
    report(adapter, CRON_STATUS_TICK_READY, &jobs, runs, false)
}

fn execute_due_jobs(
    adapter: &dyn CronExecutionAdapter,
    limit: usize,
) -> Result<(Vec<CronJob>, Vec<CronRunRecord>), String> {
    let repository = CronRepository::open(adapter.axon_root())?;
    let mut jobs = Vec::with_capacity(limit);
    let mut runs = Vec::with_capacity(limit);
    while jobs.len() < limit {
        let claim = repository.claim_due(now_ms())?;
        let Some(claim) = claim else {
            break;
        };
        jobs.push(claim.job.clone());
        runs.push(execute_and_persist(adapter, &repository, &claim, true)?);
    }
    Ok((jobs, runs))
}

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
        claim.job.after_run(
            finished_at_ms,
            next_run_after(&schedule, finished_at_ms),
            schedule.is_one_shot(),
        )
    } else {
        // 手动执行只更新执行事实，不消费一次性计划，也不推迟下一次定时执行。
        claim
            .job
            .after_run(finished_at_ms, claim.job.next_run_at_ms, false)
    };
    heartbeat.finish()?;
    repository.complete_claim(claim, &run, &next_job)?;
    Ok(run)
}

fn required_job(repository: &CronRepository, job_id: &str) -> Result<CronJob, String> {
    repository
        .find_job(job_id)?
        .ok_or_else(|| "未找到用户级 Agent Cron 任务".to_owned())
}

fn update_input(command: &CronUpdateCommand) -> Result<CronJobUpdate, String> {
    reject_legacy_delivery_args(
        command.feishu_chat_id.as_ref(),
        command.feishu_thread_id.as_ref(),
    )?;
    Ok(CronJobUpdate {
        name: command.name.clone(),
        schedule: command.schedule.clone(),
        prompt: command.prompt.clone(),
        skills: if command.clear_skills {
            Some(Vec::new())
        } else if command.skills.is_empty() {
            None
        } else {
            Some(command.skills.clone())
        },
        wake: command
            .wake
            .as_deref()
            .map(super::wake::parse_wake)
            .transpose()?,
        replace_wake: command.clear_wake || command.wake.is_some(),
        workdir: path_text(command.workdir.as_deref())?,
        replace_workdir: command.clear_workdir || command.workdir.is_some(),
        no_agent: command.no_agent,
        script: command.script.clone(),
        replace_script: command.clear_script || command.script.is_some(),
    })
}

fn report(
    adapter: &dyn CronExecutionAdapter,
    status: &'static str,
    jobs: &[CronJob],
    runs: Vec<CronRunRecord>,
    delivery_executed: bool,
) -> Result<CronReport, String> {
    let repository = CronRepository::open(adapter.axon_root())?;
    let success_runs = runs.iter().filter(|run| run.success).count();
    let failed_runs = runs.iter().filter(|run| !run.success).count();
    Ok(CronReport {
        status,
        store_path: axon_display::display_path(repository.path()),
        total_jobs: repository.jobs(false)?.len(),
        listed_jobs: jobs.len(),
        executed_jobs: runs.len(),
        success_runs,
        failed_runs,
        delivery_executed,
        jobs: jobs.iter().map(job_view).collect(),
        runs,
    })
}

fn job_view(job: &CronJob) -> CronJobView {
    CronJobView {
        job_id: job.job_id.clone(),
        name: job.name.clone(),
        schedule: job.schedule.clone(),
        status: job.status.as_str().to_owned(),
        no_agent: job.no_agent,
        next_run_at_ms: job.next_run_at_ms,
        last_run_at_ms: job.last_run_at_ms,
        revision: job.revision,
        wake: job.wake.as_ref().map(wake_text),
    }
}

/// 唤醒目标的命令行展示文本；与 `--wake` 的输入格式一致。
pub(crate) fn wake_text(wake: &CronWakeTarget) -> String {
    match wake {
        CronWakeTarget::Feishu(chat_id) => format!("feishu:{chat_id}"),
        CronWakeTarget::Session {
            channel,
            destination,
        } => format!("session:{channel}:{destination}"),
    }
}

/// 拒绝已移除的投递参数，并给出中文迁移路径。
fn reject_legacy_delivery_args(
    chat_id: Option<&String>,
    thread_id: Option<&String>,
) -> Result<(), String> {
    if chat_id.is_some() || thread_id.is_some() {
        return Err(LEGACY_DELIVERY_ARGS_HINT.to_owned());
    }
    Ok(())
}

fn path_text(path: Option<&Path>) -> Result<Option<String>, String> {
    path.map(|path| {
        path.to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| "cron workdir 必须是有效 UTF-8 路径".to_owned())
    })
    .transpose()
}

fn bounded_limit(limit: Option<usize>) -> Result<usize, String> {
    let limit = limit.unwrap_or(CRON_DEFAULT_LIMIT);
    if limit == 0 || limit > CRON_MAX_LIMIT {
        return Err(format!("cron limit 必须在 1..={CRON_MAX_LIMIT} 范围内"));
    }
    Ok(limit)
}

fn stable_job_id(name: &str, now_ms: u64) -> String {
    let source = format!("{name}:{now_ms}");
    format!("cron-{}", sha256_hex_prefix(source.as_bytes(), 16))
}

fn now_ms() -> u64 {
    axon_time::unix_now_ms()
}

pub(crate) fn run_due_cron_jobs(
    adapter: &dyn CronExecutionAdapter,
    limit: usize,
) -> Result<String, String> {
    let (jobs, runs) = execute_due_jobs(adapter, limit)?;
    let failed = runs.iter().filter(|run| !run.success).count();
    Ok(format!(
        "claimed={} completed={} failed={failed}",
        jobs.len(),
        runs.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_consolidation_cron_is_created_idempotently_and_reactivated() {
        let root = axon_test_support::test_temp_dir();
        assert_eq!(
            ensure_memory_consolidation_cron(root.path()).expect("系统 Cron 应可创建"),
            "created"
        );
        let repository = CronRepository::open(root.path()).expect("Cron 仓库应可打开");
        let created = repository
            .jobs(true)
            .expect("系统 Cron 应可读取")
            .into_iter()
            .find(|job| job.job_id == MEMORY_CONSOLIDATION_CRON_JOB_ID)
            .expect("系统 Cron 应存在");
        assert_eq!(created.wake, None, "系统 Cron 不得声明唤醒目标");
        assert!(created.no_agent);
        assert_eq!(
            ensure_memory_consolidation_cron(root.path()).expect("重复启动应幂等"),
            "unchanged"
        );

        repository
            .save_job(&created.with_status(CronJobStatus::Paused, now_ms()))
            .expect("暂停状态应可写入");
        assert_eq!(
            ensure_memory_consolidation_cron(root.path()).expect("启动应恢复系统 Cron"),
            "updated"
        );
        let reactivated = repository
            .jobs(true)
            .expect("系统 Cron 应可读取")
            .into_iter()
            .find(|job| job.job_id == MEMORY_CONSOLIDATION_CRON_JOB_ID)
            .expect("系统 Cron 应存在");
        assert_eq!(reactivated.status, CronJobStatus::Active);
        assert!(reactivated.revision > created.revision);
    }

    #[test]
    fn usage_rollup_cron_is_created_without_scanning_other_jobs() {
        let root = axon_test_support::test_temp_dir();
        let status = ensure_usage_rollup_cron(root.path()).expect("usage Cron 应可创建");
        assert_eq!(status, "created");
        let repository = CronRepository::open(root.path()).expect("Cron 仓库应可打开");
        let job = repository
            .find_job_any_status(USAGE_ROLLUP_CRON_JOB_ID)
            .expect("usage Cron 应可按主键读取")
            .expect("usage Cron 应存在");
        assert!(job.no_agent);
        assert_eq!(job.script.as_deref(), Some(USAGE_ROLLUP_CRON_SCRIPT));
        assert_eq!(
            ensure_usage_rollup_cron(root.path()).expect("重复启动应幂等"),
            "unchanged"
        );
    }

    #[test]
    fn pricing_sync_cron_starts_absent_then_created_paused_and_reactivated() {
        let root = axon_test_support::test_temp_dir();

        // 默认不存在：自动同步是用户打开的，不是安装时就跑的。
        let initial = pricing_sync_cron_state(root.path()).expect("初始状态应可读");
        assert!(!initial.job_exists);
        assert!(!initial.enabled);
        assert_eq!(initial.status, "missing");

        let enabled = set_pricing_sync_cron_enabled(root.path(), true).expect("开启应创建任务");
        assert!(enabled.job_exists);
        assert!(enabled.enabled);
        assert_eq!(enabled.status, "active");
        assert_eq!(
            enabled.schedule,
            format!(
                "interval:{}ms",
                crate::usage_statistics::PRICING_SYNC_INTERVAL_MS
            ),
            "自动同步周期固定为一天"
        );

        let repository = CronRepository::open(root.path()).expect("Cron 仓库应可打开");
        let job = repository
            .find_job_any_status(crate::usage_statistics::PRICING_SYNC_CRON_JOB_ID)
            .expect("任务应可按主键读取")
            .expect("任务应存在");
        assert!(job.no_agent);
        assert_eq!(
            job.script.as_deref(),
            Some(crate::usage_statistics::PRICING_SYNC_CRON_SCRIPT)
        );

        // 重复开启是幂等的：任务已经在跑就不该再动它。
        let again = set_pricing_sync_cron_enabled(root.path(), true).expect("重复开启应幂等");
        assert!(again.enabled);
        assert_eq!(again.status, "active");

        let disabled = set_pricing_sync_cron_enabled(root.path(), false).expect("关闭应转暂停");
        assert!(!disabled.enabled);
        assert_eq!(disabled.status, "paused");
        assert!(disabled.job_exists, "关闭只暂停，不删除任务与历史");
        let paused = repository
            .find_job_any_status(crate::usage_statistics::PRICING_SYNC_CRON_JOB_ID)
            .expect("任务应可按主键读取")
            .expect("暂停后任务仍应存在");
        assert_eq!(paused.status, CronJobStatus::Paused);

        let reenabled = set_pricing_sync_cron_enabled(root.path(), true).expect("再次开启应恢复");
        assert!(reenabled.enabled);
        assert_eq!(reenabled.status, "active");
    }
}
