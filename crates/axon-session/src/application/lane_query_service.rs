//! 车道查询服务。
//!
//! 把端口返回的原始汇总整理成用户可见的车道：解析 Workspace 与回复路由、
//! 派生中文可读标题、按最近活动排序。业务口径集中在这里，表现层只做展示。

use axon_domain::{AudienceContext, ReplyRoute, ScopeRef};
use serde::{Deserialize, Serialize};

use crate::ports::{InteractionStoreError, LaneQueryStore, LaneSummaryRow};

/// 车道标题的最大字符数。
const LANE_TITLE_MAX_CHARS: usize = 32;

/// 车道标题兜底文案。
const LANE_TITLE_FALLBACK: &str = "未命名车道";

/// 用户可见的一条车道。
///
/// 实现 `Serialize` 是为了让网关等 adapter 直接把它作为协议载荷，
/// 避免每个 adapter 各自再抄一份同名字段而导致口径漂移。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSnapshot {
    /// 车道稳定标识。
    pub lane_id: String,
    /// 所属 Workspace 标识；未绑定时为空。
    pub workspace_id: String,
    /// 会话表面渠道。
    pub channel: String,
    /// 会话表面目标。
    pub destination: String,
    /// 受众边界标签。
    pub audience: String,
    /// 派生出的车道标题。
    pub title: String,
    /// 该车道下的 Interaction 总数。
    pub interaction_count: u64,
    /// 该车道当前是否有一条 Interaction 正在处理中。
    pub running: bool,
    /// 最早活动时间（毫秒）。
    pub created_at_ms: u64,
    /// 最近活动时间（毫秒）。
    pub updated_at_ms: u64,
}

/// 车道查询服务。
pub struct LaneQueryService<S> {
    store: S,
}

impl<S: LaneQueryStore> LaneQueryService<S> {
    /// 以给定存储构造服务。
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// 列出全部车道。
    pub fn list_lanes(&self) -> Result<Vec<LaneSnapshot>, InteractionStoreError> {
        let rows = self.store.list_lane_summaries()?;
        Ok(rows.iter().map(project_lane).collect())
    }
}

/// 把一行原始汇总投影成用户可见的车道。
fn project_lane(row: &LaneSummaryRow) -> LaneSnapshot {
    let workspace_id = serde_json::from_str::<ScopeRef>(&row.scope_json)
        .ok()
        .and_then(|scope| scope.workspace_id().map(|value| value.as_str().to_string()))
        .unwrap_or_default();

    let route = serde_json::from_str::<ReplyRoute>(&row.reply_route_json).ok();
    let audience = serde_json::from_str::<AudienceContext>(&row.audience_json).ok();

    LaneSnapshot {
        lane_id: row.lane_id.clone(),
        workspace_id,
        channel: route.as_ref().map_or_else(
            || row.latest_channel.clone(),
            |value| value.channel().to_string(),
        ),
        destination: route
            .as_ref()
            .map_or_else(String::new, |value| value.destination().to_string()),
        audience: audience.as_ref().map_or_else(String::new, audience_label),
        title: lane_title(&row.latest_content),
        interaction_count: row.interaction_count,
        running: row.processing_count > 0,
        created_at_ms: row.first_received_ms,
        updated_at_ms: row.last_updated_ms,
    }
}

/// 受众边界的展示标签。
fn audience_label(audience: &AudienceContext) -> String {
    match audience.kind() {
        axon_domain::AudienceKind::Private => "私密",
        axon_domain::AudienceKind::Group => "群组",
    }
    .to_string()
}

/// 从最近一条 Interaction 正文派生车道标题。
///
/// 取第一条非空行，按中文句读切出第一个短句，并限制长度；
/// 与运行时会话命名保持同一套可读性口径。
fn lane_title(content: &str) -> String {
    let source = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(LANE_TITLE_FALLBACK);
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let content = normalized
        .trim_start_matches(['#', '>', '-', '*', '+'])
        .trim();
    let clause = content
        .split(['。', '！', '？', '!', '?'])
        .next()
        .unwrap_or(content)
        .trim();
    let title = clause
        .chars()
        .take(LANE_TITLE_MAX_CHARS)
        .collect::<String>();
    if title.is_empty() {
        LANE_TITLE_FALLBACK.to_string()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::{LaneQueryService, lane_title};
    use crate::ports::{InteractionStoreError, LaneQueryStore, LaneSummaryRow};
    use axon_domain::{
        AudienceContext, AudienceKind, DisclosurePolicy, ReplyRoute, ScopeRef, WorkspaceId,
    };

    /// 固定测试数据的车道存储。
    struct FixedStore(Vec<LaneSummaryRow>);

    impl LaneQueryStore for FixedStore {
        fn list_lane_summaries(&self) -> Result<Vec<LaneSummaryRow>, InteractionStoreError> {
            Ok(self.0.clone())
        }
    }

    fn row(content: &str) -> LaneSummaryRow {
        // 上下文列全部由领域类型自身序列化生成，避免手写 JSON 与领域契约漂移。
        let scope = ScopeRef::Workspace(WorkspaceId::new("workspace-a").unwrap());
        let audience = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
        let route = ReplyRoute::new("tui", "terminal-1").unwrap();
        LaneSummaryRow {
            lane_id: "session-lane-test".to_string(),
            interaction_count: 3,
            first_received_ms: 100,
            last_updated_ms: 900,
            processing_count: 0,
            latest_content: content.to_string(),
            latest_channel: "tui".to_string(),
            scope_json: serde_json::to_string(&scope).unwrap(),
            audience_json: serde_json::to_string(&audience).unwrap(),
            reply_route_json: serde_json::to_string(&route).unwrap(),
        }
    }

    #[test]
    fn title_takes_first_clause_and_bounds_length() {
        assert_eq!(
            lane_title("  ## 修复 Provider 延迟。后续说明"),
            "修复 Provider 延迟"
        );
        assert_eq!(lane_title("\n\n"), "未命名车道");
        let long = lane_title(&"超长标题".repeat(20));
        assert_eq!(long.chars().count(), 32);
    }

    #[test]
    fn service_projects_lane_identity_and_counts() {
        let service = LaneQueryService::new(FixedStore(vec![row("设计工作台侧栏交互")]));
        let lanes = service.list_lanes().unwrap();
        assert_eq!(lanes.len(), 1);
        let lane = &lanes[0];
        assert_eq!(lane.title, "设计工作台侧栏交互");
        assert_eq!(lane.interaction_count, 3);
        assert!(!lane.running, "没有在途 Interaction 时不得标记为运行中");
        assert_eq!(lane.workspace_id, "workspace-a");
        assert_eq!(lane.channel, "tui");
        assert_eq!(lane.destination, "terminal-1");
        assert_eq!(lane.audience, "私密");
    }
}
