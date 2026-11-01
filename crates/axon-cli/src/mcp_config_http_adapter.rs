//! MCP 配置 HTTP 适配器：读写运行时根下的 `.mcp.json`。
//!
//! 落盘路径与桌面端 `documents.rs` 完全一致：运行时根目录下的 `.mcp.json`。
//! 载荷同时给出原文与解析后的服务器清单，清单字段名沿用桌面端 `McpServerDto`
//! （`name`、`kind`、`command`、`args`、`url`、`env_keys`）。
//!
//! 安全边界：**服务器清单只透出环境变量名**，绝不回传已有的环境变量值。
//! `raw` 是 `.mcp.json` 全文的忠实镜像，因此它按文件原样包含用户自己写的键值：
//! 这条通道整条都在网关鉴权之后，不接受匿名读取。写入时原样落盘客户端提交的全文，
//! 因此用户自己写的键值不会因为适配器不认识而被丢掉。
//!
//! 写入走 `axon_config` 的原子替换原语，与桌面端文档写入同一策略。

use std::path::PathBuf;

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde::Serialize;
use serde_json::{Value, json};

/// MCP 配置路由。
const MCP_CONFIG_PATH: &str = "/api/v1/mcp/config";
/// MCP 配置文件相对运行时根目录的名称。
const MCP_CONFIG_FILE: &str = ".mcp.json";
/// 服务器表字段名。
const SERVERS_FIELD: &str = "mcpServers";
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 请求体不是合法 JSON 的错误码。
const INVALID_JSON: &str = "invalid_json";
/// 缺少 `raw` 字段的错误码。
const MISSING_RAW: &str = "missing_raw";
/// MCP 配置内容非法的错误码。
const INVALID_CONFIG: &str = "invalid_config";
/// 读取失败的错误码。
const READ_FAILED: &str = "read_failed";
/// 写入失败的错误码。
const WRITE_FAILED: &str = "write_failed";

/// 一台 MCP 服务器的界面视图；字段名与桌面端 `McpServerDto` 逐字一致。
#[derive(Debug, Serialize)]
struct McpServerView {
    /// 服务器名称。
    name: String,
    /// 传输类型：stdio 或 http。
    kind: String,
    /// stdio 启动命令。
    command: String,
    /// stdio 启动参数。
    args: Vec<String>,
    /// http 端点地址。
    url: String,
    /// 环境变量名清单，值不回传。
    env_keys: Vec<String>,
}

/// MCP 配置适配器。
pub(crate) struct McpConfigHttpAdapter {
    /// Axon 运行时根目录。
    axon_root: PathBuf,
}

impl McpConfigHttpAdapter {
    /// 以运行时根目录构造。
    pub(crate) fn new(axon_root: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
        }
    }

    /// `GET /api/v1/mcp/config`：回原文与解析后的清单。
    fn read(&self) -> GatewayHttpResponse {
        let path = self.config_file_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => self.payload(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // 文件不存在不是错误：如实回空原文与空清单。
                self.payload("")
            }
            Err(error) => GatewayHttpResponse::json(
                500,
                &json!({ "error": READ_FAILED, "message": format!("读取 MCP 配置失败：{error}") }),
            ),
        }
    }

    /// `PUT /api/v1/mcp/config`：校验 JSON 合法性后原样落盘，回执与读取同构。
    fn write(&self, body: &[u8]) -> GatewayHttpResponse {
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return GatewayHttpResponse::json(400, &json!({ "error": INVALID_JSON }));
        };
        let Some(raw) = request.get("raw").and_then(Value::as_str) else {
            return GatewayHttpResponse::json(400, &json!({ "error": MISSING_RAW }));
        };
        // BOM 不属于 JSON 正文，落盘前统一去掉，避免下一次读取解析失败。
        let normalized = raw.trim_start_matches('\u{feff}');
        if normalized.trim().is_empty() {
            return GatewayHttpResponse::json(
                400,
                &json!({ "error": INVALID_CONFIG, "message": "MCP 配置内容不能为空" }),
            );
        }
        if let Err(message) = parse_servers(normalized) {
            return GatewayHttpResponse::json(
                400,
                &json!({ "error": INVALID_CONFIG, "message": message }),
            );
        }
        if let Err(error) = axon_config::atomic_write::replace_file_atomically(
            &self.config_file_path(),
            normalized.as_bytes(),
        ) {
            return GatewayHttpResponse::json(
                500,
                &json!({ "error": WRITE_FAILED, "message": format!("写入 MCP 配置失败：{error:?}") }),
            );
        }
        // 回执重新读一遍磁盘：客户端拿到的就是此刻的真实状态。
        self.read()
    }

    /// 组装载荷：原文加解析后的服务器清单。
    fn payload(&self, raw: &str) -> GatewayHttpResponse {
        let normalized = raw.trim_start_matches('\u{feff}');
        match parse_servers(normalized) {
            Ok(servers) => {
                GatewayHttpResponse::json(200, &json!({ "raw": normalized, "servers": servers }))
            }
            Err(message) => GatewayHttpResponse::json(
                500,
                &json!({ "error": INVALID_CONFIG, "message": message }),
            ),
        }
    }

    /// `.mcp.json` 的绝对路径。
    fn config_file_path(&self) -> PathBuf {
        self.axon_root.join(MCP_CONFIG_FILE)
    }
}

impl GatewayHttpRouteHandler for McpConfigHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        // 精确匹配：未命中一律交回组合器，不得返回统一 404 截断后续适配器。
        if path != MCP_CONFIG_PATH {
            return None;
        }
        Some(match method {
            "GET" => self.read(),
            "PUT" => self.write(body),
            _ => method_not_allowed(),
        })
    }
}

/// 解析服务器清单：空原文视为没有配置。
fn parse_servers(raw: &str) -> Result<Vec<McpServerView>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Value =
        serde_json::from_str(trimmed).map_err(|error| format!("MCP 配置不是合法 JSON：{error}"))?;
    let Some(servers) = parsed.get(SERVERS_FIELD) else {
        return Err(format!("MCP 配置缺少 {SERVERS_FIELD} 字段"));
    };
    let Some(servers) = servers.as_object() else {
        return Err(format!("MCP 配置的 {SERVERS_FIELD} 必须是对象"));
    };
    let mut views = servers
        .iter()
        .map(|(name, entry)| project_server(name, entry))
        .collect::<Vec<_>>();
    views.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(views)
}

/// 把一台服务器投影成界面视图。
fn project_server(name: &str, entry: &Value) -> McpServerView {
    let string_of = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let args = entry
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut env_keys = entry
        .get("env")
        .and_then(Value::as_object)
        .map(|items| items.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    env_keys.sort();

    McpServerView {
        name: name.to_owned(),
        kind: {
            let declared = string_of("type");
            if declared.is_empty() {
                "stdio".to_owned()
            } else {
                declared
            }
        },
        command: string_of("command"),
        args,
        url: string_of("url"),
        env_keys,
    }
}

/// 方法不匹配响应。
fn method_not_allowed() -> GatewayHttpResponse {
    GatewayHttpResponse::json(405, &json!({ "error": METHOD_NOT_ALLOWED }))
}

#[cfg(test)]
#[path = "mcp_config_http_adapter_tests.rs"]
mod mcp_config_http_adapter_tests;
