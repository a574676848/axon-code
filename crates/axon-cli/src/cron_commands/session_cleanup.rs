//! 会话存储保留策略与 Cron 内置清理。

use std::path::Path;

use axon_session::RuntimeStore;
use serde::{Deserialize, Serialize};

use self::session_cleanup_files::{FileCleanupCounts, cleanup_runtime_files, count_runtime_files};
use self::session_cleanup_metrics::{
    cleanup_channel_metrics, dry_run_channel_metrics_count, file_retention_ms, retention_ms,
};
use self::session_cleanup_sqlite::vacuum_session_sqlite_files;
use self::session_family_cleanup::{
    cleanup_session_family_data, count_session_family_data, stale_session_family_ids,
};
use super::repository::CronRepository;
use super::runtime_cleanup::{RuntimeCleanupCounts, cleanup_runtime_data, count_runtime_cleanup};
use crate::run_commands::resolved_axon_root;
use crate::usage_statistics::cleanup_usage_data;

/// 由 Axon Cron 直接识别的跨平台会话清理任务标识。
pub(crate) const SESSION_CLEANUP_CRON_SCRIPT: &str = "axon.internal.brain-cleanup";
/// 只生成完整清理报告、不执行删除的内部任务标识。
pub(crate) const SESSION_CLEANUP_DRY_RUN_CRON_SCRIPT: &str = "axon.internal.brain-cleanup-dry-run";
/// 默认会话保留天数。
pub(crate) const SESSION_RETENTION_DAYS: u64 = 7;
/// 文件面（logs / tmp / archive / attachments）的保留天数。
///
/// 与 `SESSION_RETENTION_DAYS` 刻意解耦：会话与 SQLite 事实表仍按 7 天回收，
/// 四个文件目录按用户口径统一在 3 天之外整体清空，避免两套策略互相牵制。
pub(crate) const FILE_RETENTION_DAYS: u64 = 3;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SessionCleanupReport {
    pub(crate) run_id: Option<String>,
    pub(crate) status: String,
    pub(crate) phase: String,
    pub(crate) dry_run: bool,
    pub(crate) protection_snapshot: Option<String>,
    pub(crate) memory_consistency_report: Option<serde_json::Value>,
    pub(crate) deleted_session_count: usize,
    pub(crate) deleted_thread_file_count: usize,
    pub(crate) deleted_governance_count: usize,
    pub(crate) deleted_runtime_session_count: usize,
    pub(crate) deleted_channel_metrics_sample_count: usize,
    pub(crate) deleted_cron_run_count: usize,
    pub(crate) deleted_cron_job_count: usize,
    pub(crate) deleted_session_notes_count: usize,
    pub(crate) deleted_note_file_count: usize,
    pub(crate) deleted_archived_shard_count: usize,
    pub(crate) vacuumed_sqlite_count: usize,
    pub(crate) deleted_session_event_count: usize,
    pub(crate) deleted_session_checkpoint_count: usize,
    pub(crate) deleted_runtime_schedule_count: usize,
    pub(crate) deleted_interaction_count: usize,
    pub(crate) deleted_task_count: usize,
    pub(crate) deleted_execution_count: usize,
    pub(crate) deleted_result_count: usize,
    pub(crate) deleted_result_read_count: usize,
    pub(crate) deleted_decision_count: usize,
    pub(crate) deleted_event_count: usize,
    pub(crate) deleted_daemon_host_count: usize,
    pub(crate) deleted_upgrade_admission_count: usize,
    pub(crate) deleted_client_presence_count: usize,
    pub(crate) deleted_completion_notification_count: usize,
    pub(crate) deleted_completion_delivery_count: usize,
    pub(crate) deleted_agent_message_count: usize,
    pub(crate) deleted_continuation_count: usize,
    pub(crate) deleted_usage_fact_count: u64,
    pub(crate) deleted_log_file_count: usize,
    pub(crate) deleted_temp_entry_count: usize,
    pub(crate) deleted_mcp_schema_cache_count: usize,
    pub(crate) deleted_archive_file_count: usize,
    pub(crate) deleted_attachment_file_count: usize,
    pub(crate) elapsed_ms: u64,
}

pub(crate) fn run_session_cleanup_once(config_path: &Path) -> Result<SessionCleanupReport, String> {
    run_session_cleanup(config_path, false)
}

pub(crate) fn run_session_cleanup(
    config_path: &Path,
    dry_run: bool,
) -> Result<SessionCleanupReport, String> {
    session_cleanup_run::run_session_cleanup(config_path, dry_run)
}

pub(super) fn run_session_cleanup_data(
    config_path: &Path,
    dry_run: bool,
) -> Result<SessionCleanupReport, String> {
    let usage_fact_count = cleanup_usage_data(config_path, dry_run)?;
    let root = resolved_axon_root(config_path)?;
    let cutoff_ms = axon_time::unix_now_ms().saturating_sub(retention_ms());
    let stale_sessions = stale_session_family_ids(&root, cutoff_ms)?;
    if dry_run {
        return run_cleanup_dry_run(&root, cutoff_ms, &stale_sessions, usage_fact_count);
    }
    run_cleanup_deletion(&root, cutoff_ms, &stale_sessions, usage_fact_count)
}

/// 文件面清理的截止时间：四个文件目录按独立保留期回收。
fn file_cutoff_ms() -> u64 {
    axon_time::unix_now_ms().saturating_sub(file_retention_ms())
}

fn run_cleanup_dry_run(
    root: &Path,
    cutoff_ms: u64,
    stale_sessions: &[String],
    usage_fact_count: u64,
) -> Result<SessionCleanupReport, String> {
    let mut report = SessionCleanupReport {
        dry_run: true,
        status: "completed".to_owned(),
        phase: "data_completed".to_owned(),
        deleted_session_count: stale_sessions.len(),
        deleted_usage_fact_count: usage_fact_count,
        deleted_channel_metrics_sample_count: dry_run_channel_metrics_count(root, cutoff_ms)?,
        ..SessionCleanupReport::default()
    };
    count_session_family_data(root, stale_sessions, cutoff_ms, &mut report)?;
    let (cron_runs, cron_jobs) = dry_run_cron_history_count(root, cutoff_ms)?;
    report.deleted_cron_run_count = cron_runs;
    report.deleted_cron_job_count = cron_jobs;
    apply_runtime_cleanup_counts(&mut report, count_runtime_cleanup(root, cutoff_ms)?);
    apply_file_cleanup_counts(&mut report, count_runtime_files(root, file_cutoff_ms())?);
    Ok(report)
}

fn run_cleanup_deletion(
    root: &Path,
    cutoff_ms: u64,
    stale_sessions: &[String],
    usage_fact_count: u64,
) -> Result<SessionCleanupReport, String> {
    let mut report = SessionCleanupReport {
        deleted_usage_fact_count: usage_fact_count,
        ..SessionCleanupReport::default()
    };
    let (cron_runs, cron_jobs) = cleanup_cron_history(root, cutoff_ms)?;
    report.deleted_cron_run_count = cron_runs;
    report.deleted_cron_job_count = cron_jobs;
    initialize_and_cleanup_runtime(root, cutoff_ms, &mut report)?;
    apply_file_cleanup_counts(&mut report, cleanup_runtime_files(root, file_cutoff_ms())?);
    cleanup_session_family_data(root, cutoff_ms, stale_sessions, &mut report)?;
    report.vacuumed_sqlite_count = vacuum_sqlite_files(root)?;
    report.status = "completed".to_owned();
    Ok(report)
}

fn dry_run_cron_history_count(root: &Path, cutoff_ms: u64) -> Result<(usize, usize), String> {
    let path = root.join("cron").join("cron.sqlite3");
    if !path.exists() {
        return Ok((0, 0));
    }
    CronRepository::open(root)?.history_counts(cutoff_ms)
}

fn cleanup_cron_history(root: &Path, cutoff_ms: u64) -> Result<(usize, usize), String> {
    CronRepository::open(root)?.cleanup_history(cutoff_ms)
}

fn vacuum_sqlite_files(root: &Path) -> Result<usize, String> {
    vacuum_session_sqlite_files(root)
}

fn initialize_and_cleanup_runtime(
    root: &Path,
    cutoff_ms: u64,
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    let runtime_store =
        axon_session::SqliteRuntimeStore::from_root(root.join(axon_session::AXON_RUNTIME_DATA_DIR));
    runtime_store
        .initialize()
        .map_err(|error| format!("初始化 Daemon runtime 清理 schema 失败：{error}"))?;
    apply_runtime_cleanup_counts(report, cleanup_runtime_data(root, cutoff_ms)?);
    report.deleted_channel_metrics_sample_count = cleanup_channel_metrics(root, cutoff_ms)?;
    Ok(())
}

fn apply_runtime_cleanup_counts(report: &mut SessionCleanupReport, counts: RuntimeCleanupCounts) {
    report.deleted_runtime_session_count = report
        .deleted_runtime_session_count
        .saturating_add(counts.sessions);
    report.deleted_session_event_count = report
        .deleted_session_event_count
        .saturating_add(counts.session_events);
    // 检查点表已删除：报告字段保留（对外结构稳定），读数恒为 0。
    report.deleted_runtime_schedule_count = report
        .deleted_runtime_schedule_count
        .saturating_add(counts.schedules);
    report.deleted_interaction_count = report
        .deleted_interaction_count
        .saturating_add(counts.interactions);
    report.deleted_task_count = report.deleted_task_count.saturating_add(counts.goals);
    report.deleted_execution_count = report
        .deleted_execution_count
        .saturating_add(counts.executions);
    report.deleted_result_count = report.deleted_result_count.saturating_add(counts.results);
    report.deleted_result_read_count = report
        .deleted_result_read_count
        .saturating_add(counts.result_reads);
    report.deleted_decision_count = report
        .deleted_decision_count
        .saturating_add(counts.decisions);
    report.deleted_event_count = report.deleted_event_count.saturating_add(counts.events);
    report.deleted_daemon_host_count = report
        .deleted_daemon_host_count
        .saturating_add(counts.daemon_instances);
    report.deleted_upgrade_admission_count = report
        .deleted_upgrade_admission_count
        .saturating_add(counts.upgrade_admissions);
    report.deleted_client_presence_count = report
        .deleted_client_presence_count
        .saturating_add(counts.client_presence);
    report.deleted_completion_notification_count = report
        .deleted_completion_notification_count
        .saturating_add(counts.completion_notifications);
    report.deleted_completion_delivery_count = report
        .deleted_completion_delivery_count
        .saturating_add(counts.completion_deliveries);
    report.deleted_agent_message_count = report
        .deleted_agent_message_count
        .saturating_add(counts.agent_messages);
    report.deleted_continuation_count = report
        .deleted_continuation_count
        .saturating_add(counts.continuations);
}

fn apply_file_cleanup_counts(report: &mut SessionCleanupReport, counts: FileCleanupCounts) {
    report.deleted_log_file_count = counts.log_files;
    report.deleted_temp_entry_count = counts.temp_entries;
    report.deleted_mcp_schema_cache_count = counts.mcp_schema_rows;
    report.deleted_archive_file_count = counts.archive_files;
    report.deleted_attachment_file_count = counts.attachment_files;
}

#[path = "session_cleanup_files.rs"]
mod session_cleanup_files;
#[path = "session_cleanup_metrics.rs"]
mod session_cleanup_metrics;
#[path = "session_cleanup_run.rs"]
mod session_cleanup_run;
#[path = "session_cleanup_sqlite.rs"]
mod session_cleanup_sqlite;
#[path = "session_family_cleanup.rs"]
mod session_family_cleanup;
