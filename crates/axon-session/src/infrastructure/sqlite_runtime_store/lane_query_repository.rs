//! `jarvis_interactions` 的车道聚合查询实现。
//!
//! 一条车道由 Interaction 的复合归属决定，因此车道列表就是按 `lane_id`
//! 分组后的汇总。这里用一条窗口函数查询同时取出每组的总数、最早接收时间、
//! 最近活动时间与最近一条 Interaction 的全部上下文列，避免逐组回查。

use super::{SqliteRuntimeStore, helpers::u64_value};
use crate::ports::{InteractionStoreError, LaneQueryStore, LaneSummaryRow};
use rusqlite::Row;

/// 按车道聚合 Interaction；只取每组排名第一的行。
const SELECT_LANE_SUMMARIES: &str = "WITH ranked AS (\
       SELECT lane_id, content, source_channel, scope_json, audience_json, reply_route_json, \
              received_at_ms, updated_at_ms, \
              ROW_NUMBER() OVER (PARTITION BY lane_id ORDER BY received_at_ms DESC, interaction_id DESC) AS rank_in_lane, \
              COUNT(*) OVER (PARTITION BY lane_id) AS interaction_count, \
              SUM(CASE WHEN status = 'processing' THEN 1 ELSE 0 END) OVER (PARTITION BY lane_id) AS processing_count, \
              MIN(received_at_ms) OVER (PARTITION BY lane_id) AS first_received_ms \
       FROM jarvis_interactions \
       WHERE lane_id IS NOT NULL AND lane_id <> ''\
     ) \
     SELECT lane_id, content, source_channel, scope_json, audience_json, reply_route_json, \
            updated_at_ms, interaction_count, processing_count, first_received_ms \
     FROM ranked WHERE rank_in_lane = 1 ORDER BY updated_at_ms DESC";

/// 把底层持久化错误收敛为端口错误。
fn store_error(error: impl std::fmt::Display) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

/// 读取一行聚合结果。
fn read_lane_summary(row: &Row<'_>) -> rusqlite::Result<LaneSummaryRow> {
    Ok(LaneSummaryRow {
        lane_id: row.get(0)?,
        latest_content: row.get(1)?,
        latest_channel: row.get(2)?,
        scope_json: row.get(3)?,
        audience_json: row.get(4)?,
        reply_route_json: row.get(5)?,
        last_updated_ms: u64_value(row.get(6)?)?,
        interaction_count: u64_value(row.get(7)?)?,
        processing_count: u64_value(row.get(8)?)?,
        first_received_ms: u64_value(row.get(9)?)?,
    })
}

impl LaneQueryStore for SqliteRuntimeStore {
    fn list_lane_summaries(&self) -> Result<Vec<LaneSummaryRow>, InteractionStoreError> {
        let connection = self.connection().map_err(store_error)?;
        let mut statement = connection
            .prepare(SELECT_LANE_SUMMARIES)
            .map_err(store_error)?;
        let rows = statement
            .query_map([], read_lane_summary)
            .map_err(store_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_error)?;
        Ok(rows)
    }
}
