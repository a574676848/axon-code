//! MCP 运行态路由：把 daemon 已加载的 MCP 服务器暴露给工作台。
//!
//! 与 TUI 的 `/mcp` 同源：服务器名单来自配置发现，加载结论来自 daemon 的启动快照
//! （`StartupSnapshotCache`，与 turn 用的是同一份），因此这里不另开一套探测，
//! 也不会为了报一次数凭空多连一遍 MCP。
//!
//! - `POST /api/v1/mcp/status`：读当前快照；冷启动会现场构建一次。
//! - `POST /api/v1/mcp/refresh`：清掉 MCP 健康负缓存并强制重建快照，返回重建之后的同一份载荷。
//!
//! 两台入口都从 JSON body 取 `cwd`，不放进查询串：工作区路径里有空格与中文，
//! 走查询串就得再加一层编解码，而 JSON 本身就是安全的。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axon_agent_runtime::MCP_TOOL_NAMESPACE_PREFIX;
use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde::Serialize;

use crate::run_commands::{RunStartupSnapshot, StartupSnapshotCache};

/// daemon 网关上的 MCP 路由前缀。
pub const MCP_PATH: &str = "/api/v1/mcp";

/// 单台 MCP 服务器的运行态读数。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    /// MCP server 稳定 ID。
    pub server_id: String,
    /// 该服务器注册的可用工具数。
    pub tool_count: usize,
    /// 注册的可用工具名（已排序去重）；`/mcp` 的列表据此展开。
    pub tools: Vec<String>,
    /// 是否没加载起来：零工具即视为异常，与 TUI `/mcp` 同一口径。
    pub failed: bool,
    /// 失败原因（已脱敏）；成功时为缺省。
    pub failure: Option<String>,
    /// 快照里记录的 schema 处置（hit / stale / miss / refreshed / empty）。
    pub disposition: String,
}

/// MCP 运行态适配器。
pub struct McpHttpAdapter {
    /// Axon 运行时根（用户级 `.mcp.json` 与健康缓存都在这里）。
    root: PathBuf,
    config_path: PathBuf,
    /// 未指定 `cwd` 时的工作区；daemon 启动时声明的那个。
    default_cwd: PathBuf,
    snapshots: StartupSnapshotCache,
}

impl McpHttpAdapter {
    /// 构造适配器；`snapshots` 必须是 daemon 正在用的那一份，否则刷新只重建一份影子快照。
    pub fn new(
        root: PathBuf,
        config_path: PathBuf,
        default_cwd: PathBuf,
        snapshots: StartupSnapshotCache,
    ) -> Self {
        Self {
            root,
            config_path,
            default_cwd,
            snapshots,
        }
    }

    /// 读当前快照下的运行态。
    fn status(&self, cwd: Option<&str>) -> Result<serde_json::Value, String> {
        let cwd = self.resolve_cwd(cwd)?;
        match self.snapshots.acquire(&self.config_path, &cwd) {
            Some(snapshot) => Ok(self.payload(&cwd, &snapshot)),
            None => Ok(unavailable_payload(
                &cwd,
                "构建启动快照失败，暂时读不到 MCP 运行态",
            )),
        }
    }

    /// 清健康负缓存并强制重建快照，返回重建之后的结论。
    fn refresh(&self, cwd: Option<&str>) -> Result<serde_json::Value, String> {
        let cwd = self.resolve_cwd(cwd)?;
        let cleared = axon_mcp_runtime::clear_runtime_mcp_health_cache(&self.root);
        let Some(snapshot) = self.snapshots.force_rebuild(&self.config_path, &cwd) else {
            return Ok(unavailable_payload(
                &cwd,
                "重新探测 MCP 失败，保留上一次结论",
            ));
        };
        let mut payload = self.payload(&cwd, &snapshot);
        payload["healthCacheCleared"] = serde_json::json!(cleared);
        payload["refreshed"] = serde_json::json!(true);
        Ok(payload)
    }

    /// 组装载荷：发现名单决定「有哪些」，注册表决定「加载成没成」。
    fn payload(&self, cwd: &Path, snapshot: &RunStartupSnapshot) -> serde_json::Value {
        let home = crate::runtime_tools::runtime_home();
        let discovered = match axon_mcp_runtime::discover_runtime_mcp_servers(
            &self.root,
            Some(cwd),
            home.as_deref(),
        ) {
            Ok(servers) => servers,
            Err(error) => return unavailable_payload(cwd, &format!("读取 MCP 配置失败：{error}")),
        };

        let mut tools_by_server = tools_per_server(snapshot);
        let dispositions = dispositions_by_server(snapshot);

        let mut servers = Vec::with_capacity(discovered.len());
        for server_id in discovered.keys() {
            let tools = tools_by_server.remove(server_id).unwrap_or_default();
            let (disposition, failure) = dispositions
                .get(server_id)
                .cloned()
                .unwrap_or_else(|| ("unknown".to_owned(), None));
            servers.push(McpServerStatus {
                server_id: server_id.clone(),
                tool_count: tools.len(),
                failed: tools.is_empty(),
                failure,
                tools,
                disposition,
            });
        }
        // 注册表里有、发现名单里没有的服务器（配置刚被删掉、快照还没跟上）照实列出：
        // 报数是事实汇总，不替用户把看不见的那一台藏起来。
        for (server_id, tools) in tools_by_server {
            servers.push(McpServerStatus {
                server_id,
                tool_count: tools.len(),
                failed: tools.is_empty(),
                failure: None,
                tools,
                disposition: "unknown".to_owned(),
            });
        }

        let failed_count = servers.iter().filter(|server| server.failed).count();
        serde_json::json!({
            "cwd": cwd.to_string_lossy(),
            "servers": servers,
            "totalCount": servers.len(),
            "loadedCount": servers.len() - failed_count,
            "failedCount": failed_count,
            "checkedAtMs": axon_time::unix_now_ms(),
        })
    }

    /// 解析 `cwd`：缺省用 daemon 声明的工作区，给了就必须是一个真目录。
    fn resolve_cwd(&self, raw: Option<&str>) -> Result<PathBuf, String> {
        let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(self.default_cwd.clone());
        };
        let path = PathBuf::from(value);
        if !path.is_dir() {
            return Err(format!("工作目录不可访问：{value}"));
        }
        Ok(path)
    }
}

impl GatewayHttpRouteHandler for McpHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let suffix = path.strip_prefix(MCP_PATH)?;
        let cwd = body_cwd(body);
        let result = match (method, suffix) {
            ("POST", "/status") => self.status(cwd.as_deref()),
            ("POST", "/refresh") => self.refresh(cwd.as_deref()),
            _ => return None,
        };
        Some(match result {
            Ok(payload) => GatewayHttpResponse::json(200, &payload),
            Err(error) => GatewayHttpResponse::json(400, &serde_json::json!({ "error": error })),
        })
    }
}

/// 读不到结论时的载荷：带着原因，让界面显式告警而不是画一个 0。
fn unavailable_payload(cwd: &Path, error: &str) -> serde_json::Value {
    serde_json::json!({
        "cwd": cwd.to_string_lossy(),
        "servers": [],
        "totalCount": 0,
        "loadedCount": 0,
        "failedCount": 0,
        "error": error,
        "checkedAtMs": axon_time::unix_now_ms(),
    })
}

/// 从 body 里取 `cwd`；没有 body 或没有这个字段都当作未指定。
fn body_cwd(body: &[u8]) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_slice(body).ok()?;
    parsed
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// 注册表里每台 MCP 服务器注册了哪些工具。
fn tools_per_server(snapshot: &RunStartupSnapshot) -> BTreeMap<String, Vec<String>> {
    let mut tools = BTreeMap::<String, Vec<String>>::new();
    for tool in snapshot.execution_tools.tools() {
        let Some(namespace) = tool.namespace.strip_prefix(MCP_TOOL_NAMESPACE_PREFIX) else {
            continue;
        };
        tools
            .entry(namespace.to_owned())
            .or_default()
            .push(tool.name.clone());
    }
    for names in tools.values_mut() {
        names.sort();
        names.dedup();
    }
    tools
}

/// 快照记录的 schema 处置与失败原因，按服务器归并。
fn dispositions_by_server(
    snapshot: &RunStartupSnapshot,
) -> BTreeMap<String, (String, Option<String>)> {
    snapshot
        .timeline
        .mcp_schema_cache
        .iter()
        .map(|report| {
            (
                report.server_id.clone(),
                (report.disposition.clone(), report.failure.clone()),
            )
        })
        .collect()
}
