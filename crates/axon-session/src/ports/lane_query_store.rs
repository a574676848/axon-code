//! 车道查询端口。
//!
//! 车道是 Interaction 的复合归属：同一条车道下的 Interaction 共享
//! `(workspace, trust scope, channel, destination)` 四元组。本端口只负责
//! 把「每条车道一行」的原始汇总取出来，标题、受众等业务口径由应用层决定。

use super::InteractionStoreError;

/// 一条车道的原始汇总行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneSummaryRow {
    /// 车道稳定标识，取值同 `session_lane_id`。
    pub lane_id: String,
    /// 该车道下的 Interaction 总数。
    pub interaction_count: u64,
    /// 该车道当前处于处理中的 Interaction 条数。
    pub processing_count: u64,
    /// 该车道最早一条 Interaction 的接收时间（毫秒）。
    pub first_received_ms: u64,
    /// 该车道最近一条 Interaction 的更新时间（毫秒）。
    pub last_updated_ms: u64,
    /// 最近一条 Interaction 的正文，用于派生车道标题。
    pub latest_content: String,
    /// 最近一条 Interaction 的来源渠道。
    pub latest_channel: String,
    /// 最近一条 Interaction 的 Workspace 范围 JSON。
    pub scope_json: String,
    /// 最近一条 Interaction 的受众上下文 JSON。
    pub audience_json: String,
    /// 最近一条 Interaction 的回复路由 JSON。
    pub reply_route_json: String,
}

/// 车道查询端口。
pub trait LaneQueryStore: Send + Sync {
    /// 列出全部车道汇总，按最近更新时间降序。
    fn list_lane_summaries(&self) -> Result<Vec<LaneSummaryRow>, InteractionStoreError>;
}
