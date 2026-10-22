//! 运行态清理的 dry-run 统计。
//!
//! 统计与真实删除共享同一份保留谓词（Task），其余分表沿用
//! 与删除完全一致的过滤条件，保证 dry-run 报告可以安全用于容量评估。

use rusqlite::Connection;

use super::runtime_cleanup::{
    INTERACTION_TERMINAL_STATUSES, LEDGER_EVENT_MIN_RETAINED, RuntimeCleanupCounts, count_rows,
    quoted_list, session_event_expiry_filter, task_expiry_filter,
};
use super::runtime_cleanup_families::{
    expired_agent_messages_filter, expired_client_presence_filter, expired_continuations_filter,
    expired_user_questions_filter, settled_completion_deliveries_filter,
    settled_completion_notifications_filter,
};

/// 统计即将被清理的行数，不产生任何写入。
pub(super) fn count_expired_rows(
    connection: &Connection,
    cutoff_ms: i64,
) -> Result<RuntimeCleanupCounts, String> {
    let task_filter = task_expiry_filter();
    let session_events = count_rows(
        connection,
        &format!("SELECT COUNT(*) {}", session_event_expiry_filter()),
        cutoff_ms,
    )?;
    let counts = RuntimeCleanupCounts {
        interactions: count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM jarvis_interactions
                 WHERE status IN ({terminal}) AND received_at_ms < ?1",
                terminal = quoted_list(&INTERACTION_TERMINAL_STATUSES),
            ),
            cutoff_ms,
        )?,
        goals: count_rows(
            connection,
            &format!("SELECT COUNT(*) {task_filter}"),
            cutoff_ms,
        )?,
        decisions: count_rows(
            connection,
            "SELECT COUNT(*) FROM jarvis_decisions
             WHERE expires_at_ms IS NOT NULL AND expires_at_ms < ?1",
            cutoff_ms,
        )?,
        events: count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM jarvis_events
                 WHERE occurred_at_ms < ?1
                   AND sequence <= (SELECT COALESCE(MAX(sequence), 0) FROM jarvis_events) - {retained}",
                retained = LEDGER_EVENT_MIN_RETAINED,
            ),
            cutoff_ms,
        )?,
        daemon_hosts: count_rows(
            connection,
            "SELECT COUNT(*) FROM daemon_hosts
             WHERE lease_expires_at_ms < ?1
               AND NOT EXISTS (
                   SELECT 1 FROM session_runtimes
                   WHERE session_runtimes.owner_host_id = daemon_hosts.host_id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM daemon_upgrade_admissions
                   INDEXED BY daemon_upgrade_admissions_owner
                   WHERE daemon_upgrade_admissions.old_host_id = daemon_hosts.host_id
               )",
            cutoff_ms,
        )?,
        upgrade_admissions: count_rows(
            connection,
            "SELECT COUNT(*) FROM daemon_upgrade_admissions WHERE deadline_at_ms < ?1",
            cutoff_ms,
        )?,
        session_events,
        executions: count_rows(
            connection,
            &format!("SELECT COUNT(*) FROM jarvis_executions WHERE goal_id IN (SELECT goal_id {task_filter})"),
            cutoff_ms,
        )?,
        results: count_rows(
            connection,
            &format!("SELECT COUNT(*) FROM jarvis_results WHERE goal_id IN (SELECT goal_id {task_filter})"),
            cutoff_ms,
        )?,
        result_reads: count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM jarvis_result_reads WHERE result_id IN (
                     SELECT result_id FROM jarvis_results WHERE goal_id IN (SELECT goal_id {task_filter})
                 )"
            ),
            cutoff_ms,
        )?,
        client_presence: count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_client_presence_filter()),
            cutoff_ms,
        )?,
        completion_notifications: count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", settled_completion_notifications_filter()),
            cutoff_ms,
        )?,
        completion_deliveries: count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", settled_completion_deliveries_filter()),
            cutoff_ms,
        )?,
        agent_messages: count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_agent_messages_filter()),
            cutoff_ms,
        )?,
        continuations: count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_continuations_filter()),
            cutoff_ms,
        )?,
        user_questions: count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_user_questions_filter()),
            cutoff_ms,
        )?,
        ..RuntimeCleanupCounts::default()
    };
    Ok(counts)
}
