//! 统一 claim 与 lease：所有工作项的领取 SQL 只在这里定义。
//!
//! 三种工作项共用 `work_items` 一张在途表，初看领取条件各不相同，实质是同一条规则：
//! 先按状态与退避时间挑候选，再用带状态守卫的 UPDATE 抢占，抢占成功才发租约。
//! 把这条规则收在一个文件里，避免三处各写一份 SQL 后逐渐分叉。
//!
//! 并发语义由「状态守卫 + 单条 UPDATE」保证：两个 worker 同时领同一条，
//! 只有一个能把 `status` 从候选状态改成 `running`，另一个的 `changed` 为 0。

use rusqlite::types::ToSql;
use rusqlite::{Transaction, params, params_from_iter};

/// 领取一条入站交互对应的工作项：不存在就新建，已存在则接管过期租约。
///
/// `running` 与 `queued` 由调用方传入状态字面量，避免本模块依赖仓储层的常量。
pub(super) fn start_interaction_item(
    transaction: &Transaction<'_>,
    interaction_id: &str,
    work_id: String,
    running: &str,
    queued: &str,
    lease_token: &str,
    now: i64,
    expires: i64,
) -> rusqlite::Result<bool> {
    let existing: Option<(String, String, Option<String>)> = transaction
        .query_row(
            "SELECT execution_id,status,terminal_kind FROM work_items WHERE inbox_id=?1",
            params![interaction_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some((work_id, status, terminal_kind)) = existing else {
        // 已经终结并搬进历史表的入站事实不能再领：归档后它在途表里没有行，
        // 只看在途表会把它当成「从未处理过」。
        let archived: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_item_history WHERE inbox_id=?1)",
            params![interaction_id],
            |row| row.get(0),
        )?;
        if archived {
            return Ok(false);
        }
        transaction.execute(
            // session_id 取自入站事实本身：写死 NULL 会让取消、会话族清理与会话保护
            // 谓词全部漏判这条工作项。未绑定 Workspace 的输入在 `inbox` 里是空串，
            // 这里折回 NULL；同时要求 `sessions` 已有对应行，避免触发外键约束。
            "INSERT INTO work_items(execution_id,goal_id,session_id,kind,payload_json,attempt,priority,status,lease_token,claimed_by,lease_expires_at_ms,started_at_ms,created_at_ms,updated_at_ms,inbox_id) \
             VALUES(?1,NULL,NULLIF((SELECT i.session_id FROM inbox AS i JOIN sessions AS s ON s.session_id=i.session_id WHERE i.interaction_id=?6),''),'turn','{}',1,0,?2,?3,NULL,?4,?5,?5,?5,?6)",
            params![work_id, running, lease_token, expires, now, interaction_id],
        )?;
        return Ok(true);
    };
    if terminal_kind.is_some() {
        return Ok(false);
    }
    let changed = if status == running {
        // 只有过期租约才可被接管，避免两个 worker 同时持有同一条输入。
        transaction.execute(
            "UPDATE work_items SET lease_token=?2,lease_expires_at_ms=?3,updated_at_ms=?4,started_at_ms=?4,attempt=attempt+1 \
             WHERE execution_id=?1 AND status=?5 AND lease_expires_at_ms<=?4",
            params![work_id, lease_token, expires, now, running],
        )?
    } else {
        transaction.execute(
            "UPDATE work_items SET status=?2,lease_token=?3,lease_expires_at_ms=?4,updated_at_ms=?5,started_at_ms=?5,attempt=attempt+1 \
             WHERE execution_id=?1 AND status=?6",
            params![work_id, running, lease_token, expires, now, queued],
        )?
    };
    Ok(changed == 1)
}

/// 批量领取执行行：只领 `queued` 且退避已到期的那些。
///
/// 返回实际改动的行数；调用方据此判断是否在事务内被别人抢先。
pub(super) fn claim_execution_rows(
    transaction: &Transaction<'_>,
    ids: &[String],
    lease_json: &str,
    owner: &str,
    generation: &str,
    expires: i64,
    now: i64,
) -> rusqlite::Result<usize> {
    let placeholders = (6..(6 + ids.len()))
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE work_items SET status='running',lease_json=?1,claimed_by=?2,claimed_generation=?3,lease_expires_at_ms=?4,retry_at_ms=NULL,updated_at_ms=?5 WHERE execution_id IN ({placeholders}) AND status='queued' AND (retry_at_ms IS NULL OR retry_at_ms<=?5)"
    );
    let mut bindings: Vec<&dyn ToSql> = vec![&lease_json, &owner, &generation, &expires, &now];
    bindings.extend(ids.iter().map(|id| id as &dyn ToSql));
    transaction.execute(&sql, params_from_iter(bindings))
}

/// 领取一条动作行：`queued` 直接领，`waiting` 只在租约过期后接管。
pub(super) fn claim_action_item(
    transaction: &Transaction<'_>,
    action_id: &str,
    owner: &str,
    generation: &str,
    lease_token: &str,
    expires: i64,
    now: i64,
) -> rusqlite::Result<usize> {
    transaction.execute(
        "UPDATE work_items SET status='running',claimed_by=?1,claimed_generation=?2,lease_expires_at_ms=?3,attempt=attempt+1,started_at_ms=COALESCE(started_at_ms,?4),retry_at_ms=NULL,lease_json=json_object('owner',?1,'token',?5,'expires_at_ms',?3) WHERE execution_id=?6 AND (status='queued' OR (status='waiting' AND lease_expires_at_ms <= ?4))",
        params![owner, generation, expires, now, lease_token, action_id],
    )
}
