//! `inbox` 的车道聚合查询实现。
//!
//! 一条车道由 Interaction 的复合归属决定，因此车道列表就是按 `session_id`
//! 分组后的汇总。这里用一条窗口函数查询同时取出每组的总数、最早接收时间、
//! 最近活动时间与最近一条 Interaction 的全部上下文列，避免逐组回查。

use super::{SqliteRuntimeStore, helpers::u64_value};
use crate::ports::{InteractionStoreError, SessionQueryStore, SessionSummaryRow};
use rusqlite::Row;

/// 按车道取一条汇总：车道清单来自索引去重，每个聚合都走 `inbox_session_recent`。
///
/// 不使用「先按 `session_id` 开窗再筛 rank=1」的写法：那条路径会把整张 `inbox`
/// 物化进临时 B 树并按会话分区，代价随历史总行数增长（实测 4.8 万行 8 条车道
/// 需 182 ms）。这里由车道数驱动，每个会话只碰自己的索引区间（同规模 17 ms），
/// 结果逐字段一致；车道仍取自 `inbox` 而不是 `sessions`，因此「有入站事实但会话行
/// 缺失」的车道照样能列出，与旧口径相同。
const SELECT_SESSION_SUMMARIES: &str = "SELECT ids.session_id, latest.content, latest.source_channel, \
        latest.scope_json, latest.audience_json, latest.reply_route_json, latest.updated_at_ms, \
        (SELECT COUNT(*) FROM inbox AS c WHERE c.session_id = ids.session_id), \
        (SELECT COUNT(*) FROM inbox AS c JOIN work_items AS w ON w.inbox_id = c.interaction_id \
         WHERE c.session_id = ids.session_id AND w.status = 'running'), \
        (SELECT MIN(f.received_at_ms) FROM inbox AS f WHERE f.session_id = ids.session_id), \
        COALESCE(s.title, '') \
     FROM (SELECT DISTINCT session_id FROM inbox \
           WHERE session_id IS NOT NULL AND session_id <> '') AS ids \
     JOIN inbox AS latest ON latest.session_id = ids.session_id \
      AND latest.interaction_id = (SELECT x.interaction_id FROM inbox AS x \
                                   WHERE x.session_id = ids.session_id \
                                   ORDER BY x.received_at_ms DESC, x.interaction_id DESC LIMIT 1) \
     LEFT JOIN sessions AS s ON s.session_id = ids.session_id \
     ORDER BY latest.updated_at_ms DESC";

/// 把底层持久化错误收敛为端口错误。
fn store_error(error: impl std::fmt::Display) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

/// 读取一行聚合结果。
fn read_session_summary(row: &Row<'_>) -> rusqlite::Result<SessionSummaryRow> {
    Ok(SessionSummaryRow {
        session_id: row.get(0)?,
        latest_content: row.get(1)?,
        latest_channel: row.get(2)?,
        scope_json: row.get(3)?,
        audience_json: row.get(4)?,
        reply_route_json: row.get(5)?,
        last_updated_ms: u64_value(row.get(6)?)?,
        interaction_count: u64_value(row.get(7)?)?,
        processing_count: u64_value(row.get(8)?)?,
        first_received_ms: u64_value(row.get(9)?)?,
        stored_title: row.get(10)?,
    })
}

impl SessionQueryStore for SqliteRuntimeStore {
    fn list_session_summaries(&self) -> Result<Vec<SessionSummaryRow>, InteractionStoreError> {
        let connection = self.connection().map_err(store_error)?;
        let mut statement = connection
            .prepare(SELECT_SESSION_SUMMARIES)
            .map_err(store_error)?;
        let rows = statement
            .query_map([], read_session_summary)
            .map_err(store_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_error)?;
        Ok(rows)
    }
}
