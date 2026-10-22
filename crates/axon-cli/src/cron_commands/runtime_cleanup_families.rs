//! Daemon runtime 运行态事实的按族清理（R1~R7）。
//!
//! 每个族遵循同一模式：在 IMMEDIATE 事务内选出有界批次主键，按依赖顺序删除
//! 派生数据与根记录后提交。所有族共享同一墙上时钟预算，预算耗尽即停止，剩余
//! 数据由下一轮 Cron 幂等继续。

use std::time::Instant;

use rusqlite::{Connection, params_from_iter};

use super::runtime_cleanup::{
    INTERACTION_TERMINAL_STATUSES, LEDGER_EVENT_MIN_RETAINED, RuntimeCleanupCounts,
    SESSION_ACTIVE_STATUSES, delete_family, delete_ids_in, placeholders, quoted_list,
    session_event_expiry_filter, task_expiry_filter,
};
use axon_session::NotificationDeliveryState;

/// 删除长期过期且不再代表任何在线客户端的 presence 记录。
///
/// presence 只表达「客户端此刻能否接收投递」。过期行没有任何历史价值，但会持续
/// 参与投递选路与清理扫描，因此必须随保留策略回收，避免表随客户端重启次数增长。
pub(super) fn delete_expired_client_presence(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &expired_client_presence_selection_sql(),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                client_presence: delete_ids_in(
                    transaction,
                    "jarvis_client_presence",
                    "client_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

/// 删除已经终结且超出保留期的完成通知。
///
/// 只回收 `delivered` 与 `expired`：`pending` 与 `dispatched` 代表用户尚未看到的
/// 完成消息，删除它们等同于静默丢弃 Result，属于产品定义禁止的行为。
pub(super) fn delete_settled_completion_notifications(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    let mut counts = RuntimeCleanupCounts::default();
    for state in [
        NotificationDeliveryState::Delivered,
        NotificationDeliveryState::Expired,
    ] {
        counts.merge(delete_settled_completion_notifications_by_state(
            connection, cutoff_ms, deadline, state,
        )?);
    }
    Ok(counts)
}

fn delete_settled_completion_notifications_by_state(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
    state: NotificationDeliveryState,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &settled_completion_notifications_selection_sql_for_state(state),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                completion_notifications: delete_ids_in(
                    transaction,
                    "jarvis_completion_notifications",
                    "notification_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

/// 客户端 presence 保留谓词（`?1` 为保留截止时间）。
///
/// dry-run 统计与实际删除共用同一份谓词，避免口径漂移。
pub(super) fn expired_client_presence_filter() -> &'static str {
    "FROM jarvis_client_presence
     INDEXED BY jarvis_client_presence_expiry
     WHERE expires_at_ms < ?1"
}

/// 完成通知保留谓词（`?1` 为保留截止时间）：只回收已经终结的通知。
///
/// 终结 = 渠道确认送达（`delivered`）或隔天未读自动失效（`expired`）。
/// `pending` / `dispatched` 一律保留，删除它们等同于静默丢弃完成结论。
pub(super) fn settled_completion_notifications_filter() -> String {
    format!(
        "FROM jarvis_completion_notifications
         INDEXED BY jarvis_completion_notifications_retention
         WHERE state IN ('{delivered}','{expired}') AND updated_at_ms < ?1
           AND NOT EXISTS (
               SELECT 1 FROM jarvis_completion_deliveries AS d
               WHERE d.notification_id = jarvis_completion_notifications.notification_id
                 AND d.state IN ('{pending}','{dispatched}')
           )",
        delivered = NotificationDeliveryState::Delivered.as_str(),
        expired = NotificationDeliveryState::Expired.as_str(),
        pending = NotificationDeliveryState::Pending.as_str(),
        dispatched = NotificationDeliveryState::Dispatched.as_str(),
    )
}

/// 渠道投递记录保留谓词（`?1` 为保留截止时间）：只回收已经终结的渠道投递。
pub(super) fn settled_completion_deliveries_filter() -> String {
    format!(
        "FROM jarvis_completion_deliveries
         INDEXED BY jarvis_completion_deliveries_retention
         WHERE state IN ('{delivered}','{expired}') AND updated_at_ms < ?1",
        delivered = NotificationDeliveryState::Delivered.as_str(),
        expired = NotificationDeliveryState::Expired.as_str(),
    )
}

pub(super) fn settled_completion_notifications_selection_sql_for_state(
    state: NotificationDeliveryState,
) -> String {
    format!(
        "SELECT notification_id FROM jarvis_completion_notifications
         INDEXED BY jarvis_completion_notifications_retention
         WHERE state='{}' AND updated_at_ms < ?1
           AND NOT EXISTS (
               SELECT 1 FROM jarvis_completion_deliveries AS d
               WHERE d.notification_id = jarvis_completion_notifications.notification_id
                 AND d.state IN ('pending','dispatched')
           )
         ORDER BY updated_at_ms ASC, notification_id ASC LIMIT ?2",
        state.as_str(),
    )
}

/// 客户端 presence 清理的批次选择 SQL：按过期时间索引有序取有界批次。
pub(super) fn expired_client_presence_selection_sql() -> String {
    format!(
        "SELECT client_id {} ORDER BY expires_at_ms ASC, client_id ASC LIMIT ?2",
        expired_client_presence_filter(),
    )
}

/// 删除已终结且超出保留期的渠道投递记录。
///
/// 只回收 `delivered` 与 `expired`：`pending` 与 `dispatched` 代表用户尚未看到的
/// 完成消息，删除它们等同于静默丢弃完成结论，属于产品定义禁止的行为。渠道投递
/// 记录是完成通知的派生数据，必须与通知本体使用同一保留口径，否则通知删除后
/// 这些行会永久滞留并持续参与收件箱扫描。
pub(super) fn delete_settled_completion_deliveries(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    let mut counts = RuntimeCleanupCounts::default();
    for state in [
        NotificationDeliveryState::Delivered,
        NotificationDeliveryState::Expired,
    ] {
        counts.merge(delete_settled_completion_deliveries_by_state(
            connection, cutoff_ms, deadline, state,
        )?);
    }
    Ok(counts)
}

fn delete_settled_completion_deliveries_by_state(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
    state: NotificationDeliveryState,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &settled_completion_deliveries_selection_sql_for_state(state),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                completion_deliveries: delete_ids_in(
                    transaction,
                    "jarvis_completion_deliveries",
                    "rowid",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

pub(super) fn settled_completion_deliveries_selection_sql_for_state(
    state: NotificationDeliveryState,
) -> String {
    format!(
        "SELECT CAST(rowid AS TEXT) FROM jarvis_completion_deliveries
         INDEXED BY jarvis_completion_deliveries_retention
         WHERE state='{}' AND updated_at_ms < ?1
         ORDER BY updated_at_ms ASC LIMIT ?2",
        state.as_str(),
    )
}

/// 删除已过交接截止期的升级准入记录。
pub(super) fn delete_expired_upgrade_admissions(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &expired_upgrade_admissions_selection_sql(),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                upgrade_admissions: delete_ids_in(
                    transaction,
                    "daemon_upgrade_admissions",
                    "admission_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

/// 在一个短 `IMMEDIATE` 事务中删除指定 Session family 的运行态数据。
pub(super) fn delete_session_families_by_ids(
    connection: &mut Connection,
    ids: &[String],
) -> Result<RuntimeCleanupCounts, String> {
    if ids.is_empty() {
        return Ok(RuntimeCleanupCounts::default());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "Daemon runtime Session family 清理事务启动失败".to_owned())?;
    let placeholders = placeholders(ids.len());
    let active_sql = format!(
        "SELECT COUNT(*) FROM session_runtimes
         WHERE session_id IN ({placeholders}) AND status IN ({active})",
        active = quoted_list(&SESSION_ACTIVE_STATUSES),
    );
    let active: i64 = transaction
        .query_row(&active_sql, params_from_iter(ids.iter()), |row| row.get(0))
        .map_err(|_| "Daemon runtime Session family 活跃状态复核失败".to_owned())?;
    let execution_sql =
        format!("SELECT COUNT(*) FROM jarvis_executions WHERE session_id IN ({placeholders})");
    let executions: i64 = transaction
        .query_row(&execution_sql, params_from_iter(ids.iter()), |row| {
            row.get(0)
        })
        .map_err(|_| "Daemon runtime Session family Execution 引用复核失败".to_owned())?;
    if active > 0 || executions > 0 {
        return Err("Daemon runtime Session family 已重新活跃，保留 rollout 重试锚点".to_owned());
    }
    let counts = RuntimeCleanupCounts {
        session_events: delete_ids_in(
            &transaction,
            "session_events",
            "session_id",
            &placeholders,
            ids,
        )?,
        session_checkpoints: delete_ids_in(
            &transaction,
            "session_checkpoints",
            "session_id",
            &placeholders,
            ids,
        )?,
        runtime_schedules: delete_ids_in(
            &transaction,
            "runtime_schedules",
            "session_id",
            &placeholders,
            ids,
        )?,
        sessions: delete_ids_in(
            &transaction,
            "session_runtimes",
            "session_id",
            &placeholders,
            ids,
        )?,
        ..RuntimeCleanupCounts::default()
    };
    transaction
        .commit()
        .map_err(|_| "Daemon runtime Session family 清理事务提交失败".to_owned())?;
    Ok(counts)
}

/// 删除 retained Session 中超出保留期且位于 sequence 保底线之前的旧事件。
pub(super) fn delete_expired_session_events(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    let sql = expired_session_events_selection_sql();
    delete_family(connection, cutoff_ms, deadline, &sql, |transaction, ids| {
        let placeholders = placeholders(ids.len());
        Ok(RuntimeCleanupCounts {
            session_events: delete_ids_in(
                transaction,
                "session_events",
                "rowid",
                &placeholders,
                ids,
            )?,
            ..RuntimeCleanupCounts::default()
        })
    })
}

/// retained Session Event 清理的批次选择 SQL：按 emitted_at 索引有序推进。
pub(super) fn expired_session_events_selection_sql() -> String {
    format!(
        "SELECT CAST(rowid AS TEXT) {} ORDER BY emitted_at_ms ASC LIMIT ?2",
        session_event_expiry_filter()
    )
}

/// 删除终态且没有未读 Result 的 Task 家族：读回执、Result、Execution、Task。
pub(super) fn delete_terminal_goals(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    let sql = terminal_goals_selection_sql();
    delete_family(connection, cutoff_ms, deadline, &sql, |transaction, ids| {
        let placeholders = placeholders(ids.len());
        let reads_sql = format!(
            "DELETE FROM jarvis_result_reads WHERE result_id IN (
                 SELECT result_id FROM jarvis_results WHERE goal_id IN ({placeholders})
             )"
        );
        let mut reads_statement = transaction
            .prepare_cached(&reads_sql)
            .map_err(|_| "Daemon runtime jarvis_result_reads 清理失败".to_owned())?;
        let result_reads = reads_statement
            .execute(params_from_iter(ids.iter()))
            .map_err(|_| "Daemon runtime jarvis_result_reads 清理失败".to_owned())?;
        Ok(RuntimeCleanupCounts {
            result_reads,
            results: delete_ids_in(transaction, "jarvis_results", "goal_id", &placeholders, ids)?,
            executions: delete_ids_in(
                transaction,
                "jarvis_executions",
                "goal_id",
                &placeholders,
                ids,
            )?,
            goals: delete_ids_in(transaction, "jarvis_goals", "goal_id", &placeholders, ids)?,
            ..RuntimeCleanupCounts::default()
        })
    })
}

/// 删除终态且超出保留期的 Interaction。
///
/// `jarvis_interactions_retention` 以 `received_at_ms` 为前导列：强制走该索引
/// 可同时满足范围过滤与 `ORDER BY`，避免优化器改走覆盖索引后追加临时排序。
pub(super) fn delete_terminal_interactions(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    let sql = terminal_interactions_selection_sql();
    delete_family(connection, cutoff_ms, deadline, &sql, |transaction, ids| {
        let placeholders = placeholders(ids.len());
        Ok(RuntimeCleanupCounts {
            interactions: delete_ids_in(
                transaction,
                "jarvis_interactions",
                "interaction_id",
                &placeholders,
                ids,
            )?,
            ..RuntimeCleanupCounts::default()
        })
    })
}

/// 删除已过期的 Decision；无过期时间的权威决策长期保留。
///
/// 过期行由部分索引 `jarvis_decisions_expiry` 直接按 `expires_at_ms` 有序定位，
/// 避免在权威决策越积越多时退化为「全索引扫描 + 临时排序」。
pub(super) fn delete_expired_decisions(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &expired_decisions_selection_sql(),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                decisions: delete_ids_in(
                    transaction,
                    "jarvis_decisions",
                    "decision_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

/// 删除超出保留期的 Ledger Event，同时保底保留最新一段以避免 sequence 回退。
pub(super) fn delete_expired_events(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    let sql = expired_events_selection_sql();
    delete_family(connection, cutoff_ms, deadline, &sql, |transaction, ids| {
        let placeholders = placeholders(ids.len());
        Ok(RuntimeCleanupCounts {
            events: delete_ids_in(transaction, "jarvis_events", "event_id", &placeholders, ids)?,
            ..RuntimeCleanupCounts::default()
        })
    })
}

/// 删除租约长期过期且不再被 Session 或升级准入引用的 Daemon Host 记录。
pub(super) fn delete_expired_daemon_hosts(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &expired_daemon_hosts_selection_sql(),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                daemon_hosts: delete_ids_in(
                    transaction,
                    "daemon_hosts",
                    "host_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

/// 升级准入清理的批次选择 SQL：dry-run 统计、实际删除与执行计划审计共用。
pub(super) fn expired_upgrade_admissions_selection_sql() -> String {
    "SELECT admission_id FROM daemon_upgrade_admissions
     WHERE deadline_at_ms < ?1
     ORDER BY deadline_at_ms ASC
     LIMIT ?2"
        .to_owned()
}

/// Task 清理的批次选择 SQL：终态、无活跃 Execution 且无未读 Result。
pub(super) fn terminal_goals_selection_sql() -> String {
    format!("SELECT goal_id {} LIMIT ?2", task_expiry_filter())
}

/// Interaction 清理的批次选择 SQL：固定走保留期索引，避免临时排序。
pub(super) fn terminal_interactions_selection_sql() -> String {
    format!(
        "SELECT interaction_id FROM jarvis_interactions
         INDEXED BY jarvis_interactions_retention
         WHERE status IN ({terminal})
           AND received_at_ms < ?1
         ORDER BY received_at_ms ASC
         LIMIT ?2",
        terminal = quoted_list(&INTERACTION_TERMINAL_STATUSES),
    )
}

/// Decision 清理的批次选择 SQL：过期行由部分索引按到期时间有序定位。
pub(super) fn expired_decisions_selection_sql() -> String {
    "SELECT decision_id FROM jarvis_decisions
     INDEXED BY jarvis_decisions_expiry
     WHERE expires_at_ms IS NOT NULL AND expires_at_ms < ?1
     ORDER BY expires_at_ms ASC
     LIMIT ?2"
        .to_owned()
}

/// Ledger Event 清理的批次选择 SQL：按 sequence 索引推进并保留最新一段。
pub(super) fn expired_events_selection_sql() -> String {
    format!(
        "SELECT event_id FROM jarvis_events
         WHERE occurred_at_ms < ?1
           AND sequence <= (SELECT COALESCE(MAX(sequence), 0) FROM jarvis_events) - {retained}
         ORDER BY sequence ASC
         LIMIT ?2",
        retained = LEDGER_EVENT_MIN_RETAINED,
    )
}

/// Daemon Host 清理的批次选择 SQL：租约过期索引 + 两条引用保护子查询。
pub(super) fn expired_daemon_hosts_selection_sql() -> String {
    "SELECT host_id FROM daemon_hosts
     WHERE lease_expires_at_ms < ?1
       AND NOT EXISTS (
           SELECT 1 FROM session_runtimes
           WHERE session_runtimes.owner_host_id = daemon_hosts.host_id
       )
       AND NOT EXISTS (
           SELECT 1 FROM daemon_upgrade_admissions
           INDEXED BY daemon_upgrade_admissions_owner
           WHERE daemon_upgrade_admissions.old_host_id = daemon_hosts.host_id
       )
     LIMIT ?2"
        .to_owned()
}

/// 删除越过保留窗口的 agent 间消息。
///
/// 消息的保留窗口由自身 `expires_at_ms` 表达（`MAILBOX_MESSAGE_RETENTION`）：到期后
/// 既不可能再被投递（重试扫描按同一时刻过滤），也不可能被读取，因此它是权威的
/// 「不再有意义」边界。Cron 的 cutoff 是更长的一道水位，两者同时满足才回收，
/// 从而不会把仍可被 `axon debug mailbox` 观察到的待投递事实提前删掉。
pub(super) fn delete_expired_agent_messages(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &expired_agent_messages_selection_sql(),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                agent_messages: delete_ids_in(
                    transaction,
                    "jarvis_agent_messages",
                    "message_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

/// agent 间消息保留谓词（`?1` 为保留截止时间）：dry-run 与实际删除共用。
pub(super) fn expired_agent_messages_filter() -> &'static str {
    "FROM jarvis_agent_messages
     INDEXED BY jarvis_agent_messages_retention
     WHERE expires_at_ms < ?1"
}

/// agent 间消息清理的批次选择 SQL：走保留索引按到期时间有序推进。
pub(super) fn expired_agent_messages_selection_sql() -> String {
    format!(
        "SELECT message_id {} ORDER BY expires_at_ms ASC, message_id ASC LIMIT ?2",
        expired_agent_messages_filter()
    )
}

/// 删除越过保留窗口的子代理终态引用。
///
/// 与消息同理：`expires_at_ms` 是 continuation 自身的保留窗口
/// （`CONTINUATION_RETENTION`），越过之后待装配扫描不会再取到它，消费侧也永远
/// 无法再产生有效执行。**未装配的 pending 记录同样参与回收**，且只因
/// 「自身窗口 + Cron 水位」双双越过才被删除，不存在静默丢弃仍可装配事实的路径。
pub(super) fn delete_expired_continuations(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &expired_continuations_selection_sql(),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                continuations: delete_ids_in(
                    transaction,
                    "jarvis_continuations",
                    "continuation_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

/// continuation 保留谓词（`?1` 为保留截止时间）：dry-run 与实际删除共用。
pub(super) fn expired_continuations_filter() -> &'static str {
    "FROM jarvis_continuations
     INDEXED BY jarvis_continuations_retention
     WHERE expires_at_ms < ?1"
}

/// continuation 清理的批次选择 SQL：走保留索引按到期时间有序推进。
pub(super) fn expired_continuations_selection_sql() -> String {
    format!(
        "SELECT continuation_id {} ORDER BY expires_at_ms ASC, continuation_id ASC LIMIT ?2",
        expired_continuations_filter()
    )
}

/// 删除越过保留窗口的用户提问记录。
pub(super) fn delete_expired_user_questions(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    delete_family(
        connection,
        cutoff_ms,
        deadline,
        &expired_user_questions_selection_sql(),
        |transaction, ids| {
            let placeholders = placeholders(ids.len());
            Ok(RuntimeCleanupCounts {
                user_questions: delete_ids_in(
                    transaction,
                    "jarvis_user_questions",
                    "question_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

pub(super) fn expired_user_questions_filter() -> &'static str {
    "FROM jarvis_user_questions WHERE expires_at_ms < ?1"
}

pub(super) fn expired_user_questions_selection_sql() -> String {
    format!(
        "SELECT question_id {} ORDER BY expires_at_ms ASC, question_id ASC LIMIT ?2",
        expired_user_questions_filter()
    )
}
