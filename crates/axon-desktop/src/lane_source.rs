//! 车道数据源：经网关 HTTP 协议读取工作区与车道。
//!
//! 依赖倒置：表现层只依赖本模块声明的 [`LaneSource`]，真实实现
//! [`GatewayLaneSource`] 在组合根装配，因此 IPC 层看不到传输细节，
//! 桌面端也不链接会话子系统。
//!
//! 车道等于 Interaction 的复合归属：同一 `(workspace, trust scope, channel,
//! destination)` 下的 Interaction 共享一条车道。**车道标识的派生属于服务端口径**，
//! 客户端通过网关请求派生结果，绝不在本地复刻哈希算法，否则同一身份会出现
//! 两份实现并随口径漂移而各算各的车道。
//!
//! 本应用自身产生的车道渠道取 [`DESKTOP_LANE_CHANNEL`]。

use serde::{Deserialize, Serialize};

use crate::gateway;

/// 本应用产生的车道所属渠道。
pub const DESKTOP_LANE_CHANNEL: &str = "desktop";

/// 工作区条目，对应网关 `GET /api/v1/workspaces` 的载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    /// Workspace 标识。
    pub workspace_id: String,
    /// 显示名称。
    pub display_name: String,
    /// 工作区根目录。
    pub workspace_root: String,
    /// Git remote。
    #[serde(default)]
    pub git_remote: Option<String>,
    /// 默认分支。
    #[serde(default)]
    pub default_branch: Option<String>,
    /// 是否固定保留。
    #[serde(default)]
    pub pinned: bool,
    /// 最近访问时间（毫秒）。
    #[serde(default)]
    pub last_seen_at_ms: u64,
}

/// 车道条目，对应网关 `GET /api/v1/lanes` 的载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneEntry {
    /// 车道标识。
    pub lane_id: String,
    /// 所属 Workspace 标识。
    #[serde(default)]
    pub workspace_id: String,
    /// 会话表面渠道。
    pub channel: String,
    /// 会话表面目标。
    pub destination: String,
    /// 受众边界标签。
    #[serde(default)]
    pub audience: String,
    /// 车道标题。
    pub title: String,
    /// Interaction 总数。
    #[serde(default)]
    pub interaction_count: u64,
    /// 待答复提问数量；字段缺失时按无待答处理。
    #[serde(default)]
    pub pending_question_count: u64,
    /// 是否有一条 Interaction 正在处理中。
    #[serde(default)]
    pub running: bool,
    /// 最早活动时间（毫秒）。
    #[serde(default)]
    pub created_at_ms: u64,
    /// 最近活动时间（毫秒）。
    #[serde(default)]
    pub updated_at_ms: u64,
}

/// 表现层消费的车道数据端口。
pub trait LaneSource: Send + Sync {
    /// 注册表中全部工作区。
    fn workspaces(&self) -> Result<Vec<WorkspaceEntry>, String>;

    /// 全部真实车道。
    fn lanes(&self) -> Result<Vec<LaneEntry>, String>;

    /// 派生一条新车道的标识；派生在服务端完成。
    fn derive_lane_id(
        &self,
        workspace_id: &str,
        channel: &str,
        destination: &str,
    ) -> Result<String, String>;

    /// 注册（或刷新）一个工作区根目录，返回其权威身份。
    ///
    /// 未注册的目录不能直接拿路径当工作区标识使用：车道身份是
    /// `(workspace, audience, channel, destination)` 的复合哈希，路径标识派生出的
    /// 车道无法与其它渠道对齐，也会被提交侧的注册校验拒绝。
    fn ensure_workspace(&self, root: &str) -> Result<WorkspaceEntry, String>;
}

/// 网关实现：所有读写都走网关 HTTP 协议。
#[derive(Debug, Clone, Default)]
pub struct GatewayLaneSource;

impl GatewayLaneSource {
    /// 构造数据源；地址与凭据在每次请求时按权威配置解析。
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// 从网关载荷里取出条目数组。
fn entries<T: for<'de> Deserialize<'de>>(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Vec<T>, String> {
    let items = payload
        .get(field)
        .ok_or_else(|| format!("网关响应缺少 {field} 字段"))?;
    serde_json::from_value(items.clone()).map_err(|error| format!("解析 {field} 失败：{error}"))
}

impl LaneSource for GatewayLaneSource {
    fn workspaces(&self) -> Result<Vec<WorkspaceEntry>, String> {
        let payload = gateway::get_json("/workspaces")?;
        entries(&payload, "workspaces")
    }

    fn lanes(&self) -> Result<Vec<LaneEntry>, String> {
        let payload = gateway::get_json("/lanes")?;
        entries(&payload, "lanes")
    }

    fn derive_lane_id(
        &self,
        workspace_id: &str,
        channel: &str,
        destination: &str,
    ) -> Result<String, String> {
        let payload = gateway::post_json(
            "/lanes/identity",
            &serde_json::json!({
                "workspaceId": workspace_id,
                "channel": channel,
                "destination": destination,
            }),
        )?;
        payload
            .get("laneId")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| "网关响应缺少 laneId 字段".to_string())
    }

    fn ensure_workspace(&self, root: &str) -> Result<WorkspaceEntry, String> {
        let payload = gateway::post_json("/workspaces", &serde_json::json!({ "path": root }))?;
        let workspace = payload
            .get("workspace")
            .ok_or_else(|| "网关响应缺少 workspace 字段".to_string())?;
        serde_json::from_value(workspace.clone())
            .map_err(|error| format!("解析工作区身份失败：{error}"))
    }
}
