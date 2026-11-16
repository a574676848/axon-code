//! 工作项终态归档：把已结束的行从在途表搬到历史表。
//!
//! 在途表 `work_items` 只放排队与运行中的行：它是 claim、lease 与积压统计的事实源，
//! 行数随活跃度而不是随历史增长。终态行整体搬到 append-only 的 `work_item_history`，
//! 由视图 `work_items_all` 保持「按 id 读、按 goal 列历史」的既有语义。

use rusqlite::{Connection, params};

/// 把一条已结束的工作项搬进历史表；不存在时什么都不做。
///
/// 调用方必须先在同一事务里把 `work_items.status`/`terminal_kind` 写成终态，
/// 本函数只做搬运，不再判断状态，避免「以为已经结束、实际还在跑」的行被提前归档。
pub(super) fn archive_work_item(
    connection: &Connection,
    execution_id: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    connection.execute(
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
         WHERE execution_id=?1
         ON CONFLICT(work_id) DO NOTHING",
        params![execution_id, now_ms],
    )?;
    connection
        .execute(
            "DELETE FROM work_items WHERE execution_id=?1",
            params![execution_id],
        )
        .map(|_| ())
}
