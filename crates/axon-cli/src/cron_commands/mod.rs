mod agent_config;
mod application;
pub(crate) use agent_config::cron_agent_config;
mod args;
mod execution;
mod render;
pub(crate) mod repository;
mod runtime_cleanup;
mod runtime_cleanup_counts;
mod runtime_cleanup_families;
pub(crate) mod schedule;
mod session_cleanup;
mod wake;

pub use args::CronCommand;

pub(crate) use application::wake_text;
pub(crate) use application::{
    PricingSyncCronState, ensure_memory_consolidation_cron, ensure_session_cleanup_cron,
    ensure_usage_rollup_cron, execute_and_persist, pricing_sync_cron_state, run_cron,
    run_due_cron_jobs, set_pricing_sync_cron_enabled,
};
pub(crate) use execution::{
    CRON_AGENT_CHANNEL, CRON_AGENT_DESTINATION_PREFIX, CronExecutionAdapter,
    execute_local_cron_agent, local_cron_session_key,
};
pub(crate) use wake::{CRON_WAKE_SOURCE_PREFIX, parse_wake, suggested_feishu_chat_id};

pub(crate) const MEMORY_CONSOLIDATION_CRON_JOB_ID: &str = "axon.system.memory-consolidation";
pub(crate) const MEMORY_CONSOLIDATION_CRON_NAME: &str = "Memory consolidation";
pub(crate) const MEMORY_CONSOLIDATION_CRON_SCRIPT: &str = "axon.internal.memory-consolidation";
pub(crate) use session_cleanup::{run_session_cleanup, run_session_cleanup_once};
pub(crate) const SESSION_CLEANUP_CRON_SCRIPT: &str = session_cleanup::SESSION_CLEANUP_CRON_SCRIPT;
pub(crate) const SESSION_CLEANUP_CRON_JOB_ID: &str = "axon.system.brain-cleanup";
pub(crate) const SESSION_CLEANUP_CRON_NAME: &str = "Axon brain cleanup";
pub(crate) const SESSION_CLEANUP_INTERVAL_MS: u64 = 86_400_000;

pub(crate) fn is_system_job(job_id: &str) -> bool {
    matches!(
        job_id,
        MEMORY_CONSOLIDATION_CRON_JOB_ID
            | SESSION_CLEANUP_CRON_JOB_ID
            | crate::usage_statistics::USAGE_ROLLUP_CRON_JOB_ID
            | crate::usage_statistics::PRICING_SYNC_CRON_JOB_ID
    )
}
