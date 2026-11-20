//! 纯保留期族的清理（agent 间消息、子代理终态引用、用户提问）。
//!
//! 与 `runtime_cleanup_families` 分成两个文件，是因为它们在语义上只关心「自身保留窗口
//! 是否越过」：三张表各自带 `expires_at_ms`，越过之后既不可能再被消费，也没有别的
//! 引用路径；因此这里只需要按到期时间有界选批并删除，不涉及会话族、执行引用或终态保护。

use std::time::Instant;

use rusqlite::Connection;

use crate::cron_commands::runtime_cleanup::{
    RuntimeCleanupCounts, delete_family, delete_ids_in, placeholders,
};

/// 删除越过保留窗口的 agent 间消息。
///
/// 消息的保留窗口由自身 `expires_at_ms` 表达（`MAILBOX_MESSAGE_RETENTION`）：到期后
/// 既不可能再被投递（重试扫描按同一时刻过滤），也不可能被读取，因此它是权威的
/// 「不再有意义」边界。Cron 的 cutoff 是更长的一道水位，两者同时满足才回收，
/// 从而不会把仍可被 `axon debug mailbox` 观察到的待投递事实提前删掉。
pub(crate) fn delete_expired_agent_messages(
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
                    "agent_messages",
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
pub(crate) fn expired_agent_messages_filter() -> &'static str {
    "FROM agent_messages
     INDEXED BY agent_messages_cleanup
     WHERE expires_at_ms < ?1"
}

/// agent 间消息清理的批次选择 SQL：走保留索引按到期时间有序推进。
pub(crate) fn expired_agent_messages_selection_sql() -> String {
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
pub(crate) fn delete_expired_continuations(
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
                    "continuations",
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
pub(crate) fn expired_continuations_filter() -> &'static str {
    "FROM continuations
     INDEXED BY continuations_cleanup
     WHERE expires_at_ms < ?1"
}

/// continuation 清理的批次选择 SQL：走保留索引按到期时间有序推进。
pub(crate) fn expired_continuations_selection_sql() -> String {
    format!(
        "SELECT continuation_id {} ORDER BY expires_at_ms ASC, continuation_id ASC LIMIT ?2",
        expired_continuations_filter()
    )
}

/// 删除越过保留窗口的用户提问记录。
pub(crate) fn delete_expired_user_questions(
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
                    "user_questions",
                    "question_id",
                    &placeholders,
                    ids,
                )?,
                ..RuntimeCleanupCounts::default()
            })
        },
    )
}

pub(crate) fn expired_user_questions_filter() -> &'static str {
    "FROM user_questions
     INDEXED BY user_questions_cleanup
     WHERE expires_at_ms < ?1"
}

pub(crate) fn expired_user_questions_selection_sql() -> String {
    format!(
        "SELECT question_id {} ORDER BY expires_at_ms ASC, question_id ASC LIMIT ?2",
        expired_user_questions_filter()
    )
}
