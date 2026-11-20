//! 运行态清理的 dry-run 统计。
//!
//! 统计与真实删除共享同一份保留谓词（Task），其余分表沿用
//! 与删除完全一致的过滤条件，保证 dry-run 报告可以安全用于容量评估。
//!
//! 两个库各自统计：控制面库缺失时队列库的统计仍然成立，反之亦然。

use rusqlite::Connection;

use super::runtime_cleanup::{
    INTERACTION_TERMINAL_KINDS, LEDGER_EVENT_MIN_RETAINED, RuntimeCleanupCounts, count_rows,
    quoted_list, task_expiry_filter,
};
use super::runtime_cleanup_families::{
    expired_client_presence_filter,
    retention::{
        expired_agent_messages_filter, expired_continuations_filter, expired_user_questions_filter,
    },
    settled_completion_deliveries_filter, settled_completion_notifications_filter,
};

/// 统计即将被清理的行数，不产生任何写入。
pub(super) fn count_expired_rows(
    control: Option<&Connection>,
    outbox: Option<&Connection>,
    cutoff_ms: i64,
) -> Result<RuntimeCleanupCounts, String> {
    let task_filter = task_expiry_filter();
    let mut counts = RuntimeCleanupCounts::default();

    if let Some(connection) = control {
        counts.interactions = count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM inbox AS i
                 JOIN work_item_history AS w ON w.inbox_id = i.interaction_id
                 WHERE w.terminal_kind IN ({terminal}) AND i.received_at_ms < ?1",
                terminal = quoted_list(&INTERACTION_TERMINAL_KINDS),
            ),
            cutoff_ms,
        )?;
        // 派发轮没有工作项，终态直接落在入站行上：dry-run 必须与删除路径口径一致。
        counts.interactions = counts.interactions.saturating_add(count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM inbox
                 WHERE terminal_kind IN ({terminal}) AND received_at_ms < ?1",
                terminal = quoted_list(&INTERACTION_TERMINAL_KINDS),
            ),
            cutoff_ms,
        )?);
        counts.goals = count_rows(
            connection,
            &format!("SELECT COUNT(*) {task_filter}"),
            cutoff_ms,
        )?;
        counts.decisions = count_rows(
            connection,
            "SELECT COUNT(*) FROM decisions
             WHERE expires_at_ms IS NOT NULL AND expires_at_ms < ?1",
            cutoff_ms,
        )?;
        counts.events = count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM ledger_events
                 WHERE occurred_at_ms < ?1
                   AND sequence <= (SELECT COALESCE(MAX(sequence), 0) FROM ledger_events) - {retained}",
                retained = LEDGER_EVENT_MIN_RETAINED,
            ),
            cutoff_ms,
        )?;
        counts.daemon_instances = count_rows(
            connection,
            "SELECT COUNT(*) FROM daemon_instances
             WHERE lease_expires_at_ms < ?1
               AND NOT EXISTS (
                   SELECT 1 FROM sessions
                   WHERE sessions.owner_host_id = daemon_instances.host_id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM upgrade_handoffs
                   INDEXED BY upgrade_handoffs_owner
                   WHERE upgrade_handoffs.old_host_id = daemon_instances.host_id
               )",
            cutoff_ms,
        )?;
        counts.upgrade_admissions = count_rows(
            connection,
            "SELECT COUNT(*) FROM upgrade_handoffs WHERE deadline_at_ms < ?1",
            cutoff_ms,
        )?;
        counts.executions = count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM work_items_all WHERE goal_id IN (SELECT goal_id {task_filter})"
            ),
            cutoff_ms,
        )?;
        counts.results = count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM goal_results WHERE goal_id IN (SELECT goal_id {task_filter})"
            ),
            cutoff_ms,
        )?;
        counts.result_reads = count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) FROM goal_result_reads WHERE result_id IN (
                     SELECT result_id FROM goal_results WHERE goal_id IN (SELECT goal_id {task_filter})
                 )"
            ),
            cutoff_ms,
        )?;
    }

    if let Some(connection) = outbox {
        counts.client_presence = count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_client_presence_filter()),
            cutoff_ms,
        )?;
        counts.completion_notifications = count_rows(
            connection,
            &format!(
                "SELECT COUNT(*) {}",
                settled_completion_notifications_filter()
            ),
            cutoff_ms,
        )?;
        counts.completion_deliveries = count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", settled_completion_deliveries_filter()),
            cutoff_ms,
        )?;
        counts.agent_messages = count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_agent_messages_filter()),
            cutoff_ms,
        )?;
        counts.continuations = count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_continuations_filter()),
            cutoff_ms,
        )?;
        counts.user_questions = count_rows(
            connection,
            &format!("SELECT COUNT(*) {}", expired_user_questions_filter()),
            cutoff_ms,
        )?;
    }

    Ok(counts)
}
