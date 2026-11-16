//! 工作项终态归档：把已结束的行从在途表搬到历史表。
//!
//! 在途表 `work_items` 只放排队与运行中的行：它是 claim、lease 与积压统计的事实源，
//! 行数随活跃度而不是随历史增长。终态行整体搬到 append-only 的 `work_item_history`，
//! 由视图 `work_items_all` 保持「按 id 读、按 goal 列历史」的既有语义。

use rusqlite::Connection;

/// 已落定终态：这些状态的行必须搬进历史表。
///
/// `uncertain` 不在其中：它表示外部结果未知、仍待裁决，留在在途表才能继续拦住
/// Goal 的重新激活。
const SETTLED_STATUSES: &str = "'completed','failed','cancelled','dead_letter'";

/// 把一条已结束的工作项搬进历史表；不存在时什么都不做。
///
/// 调用方必须先在同一事务里把 `work_items.status`/`terminal_kind` 写成终态，
/// 本函数只做搬运，不再判断状态，避免「以为已经结束、实际还在跑」的行被提前归档。
pub(super) fn archive_work_item(
    connection: &Connection,
    execution_id: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    archive_where(connection, "execution_id=?1", execution_id, now_ms)
}

/// 按入站事实（`inbox_id`）归档：交互型工作项的主键是随机 UUID，调用方只有 `interaction_id`。
pub(super) fn archive_work_item_by_inbox(
    connection: &Connection,
    inbox_id: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    archive_where(connection, "inbox_id=?1", inbox_id, now_ms)
}

/// 归档命中条件且已落定终态的行，返回搬运行数。
///
/// 终态写点的统一出口：终态 UPDATE 不管有没有真的把行改到终态（可能因守卫失效
/// 而 0 行、也可能是重试排队态），都可以无条件调用本函数，由状态谓词决定搬不搬。
pub(super) fn archive_settled_work_item(
    connection: &Connection,
    condition: &str,
    work_key: &str,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    archive_where_count(
        connection,
        &format!("{condition} AND status IN ({SETTLED_STATUSES})"),
        work_key,
        now_ms,
    )
}

fn archive_where(
    connection: &Connection,
    condition: &str,
    work_key: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    archive_where_count(connection, condition, work_key, now_ms).map(|_| ())
}

fn archive_where_count(
    connection: &Connection,
    condition: &str,
    work_key: &str,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    let moved = connection.execute(
        &format!(
            "INSERT INTO work_item_history(
             work_id, goal_id, session_id, kind, payload_json, goal_revision, context_version,
             attempt, priority, dedupe_key, inbox_id, terminal_kind, status, started_at_ms,
             committed_at_ms, cancelled_at_ms, checkpoint_json, cancellation_reason,
             created_at_ms, updated_at_ms, finished_at_ms
         )
         SELECT execution_id, goal_id, session_id, kind, payload_json, goal_revision,
                context_version, attempt, priority, dedupe_key, inbox_id,
                COALESCE(terminal_kind, status), status, started_at_ms, committed_at_ms,
                cancelled_at_ms, checkpoint_json, cancellation_reason, created_at_ms,
                updated_at_ms, COALESCE(committed_at_ms, cancelled_at_ms, ?2)
         FROM work_items
         WHERE {condition}"
        ),
        rusqlite::params![work_key, now_ms],
    )?;
    connection.execute(
        &format!("DELETE FROM work_items WHERE {condition}"),
        rusqlite::params![work_key],
    )?;
    Ok(moved)
}
