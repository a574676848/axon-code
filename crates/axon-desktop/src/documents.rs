//! 设置内的文档与目录管理：MCP 配置、技能目录、人设与行为准则。
//!
//! 三个数据面都经网关：MCP 配置（`GET` / `PUT /api/v1/mcp/config`）、技能清单
//! （`GET /api/v1/skills`）与人设/行为文档（`GET` / `PUT /api/v1/documents/{soul,agents}`）。
//! 桌面端因此不再解析本机的 `.mcp.json`、`skills/`、`SOUL.md` 与 `AGENTS.md`，
//! 同一份代码在云端档案下同样成立。
//!
//! 安全边界：MCP 配置里可能含明文密钥，因此**读取路径只透出环境变量名**，
//! 绝不把已有值回传给界面；写入时界面只提交用户本次输入的键值，已有键在未重新
//! 输入时保留原值，旧值不会被空串覆盖。配置原文是保留这些键值的唯一依据，
//! 它整条都在网关鉴权之后。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axon_config::ConfigLoader;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config_gateway::{self, DOCUMENTS_PATH_PREFIX, MCP_CONFIG_PATH, SKILLS_PATH};

/// 配置文件相对运行时根目录的名称。
const CONFIG_FILE: &str = "config.toml";
/// MCP 配置文件相对运行时根目录的路径。
const MCP_CONFIG_FILE: &str = ".mcp.json";
/// 技能目录相对运行时根目录的路径。
const SKILLS_DIR: &str = "skills";
/// 日志目录相对运行时根目录的路径。
const LOGS_DIR: &str = "data/logs";
/// 人设文件名。
const SOUL_FILE: &str = "SOUL.md";
/// 行为约束文件名。
const AGENTS_FILE: &str = "AGENTS.md";

/// 设置里可打开的目标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentDto {
    /// 稳定标识。
    pub id: String,
    /// 显示名称。
    pub label: String,
    /// 绝对路径。
    pub path: String,
    /// 目标是目录还是文件。
    pub kind: String,
    /// 目标当前是否存在。
    pub exists: bool,
}

/// 解析运行时根目录。
fn axon_root() -> Result<PathBuf, String> {
    ConfigLoader::default_user_axon_root_path()
        .map_err(|error| format!("无法解析 Axon 运行时根目录：{error:?}"))
}

/// 设置内四个可打开的目标。
///
/// 这是本机路径入口（用系统关联程序打开配置、目录与日志），不是配置读写通道：
/// 云端档案下这几条本机路径没有意义，等阶段 3 的文件路由接上后由网关提供。
fn catalog() -> Result<Vec<DocumentDto>, String> {
    let root = axon_root()?;
    let entries = [
        ("config", "打开配置文件", CONFIG_FILE, "file"),
        ("mcp", "打开 MCP 配置", MCP_CONFIG_FILE, "file"),
        ("skills", "打开技能目录", SKILLS_DIR, "dir"),
        ("soul", "打开人设文件", SOUL_FILE, "file"),
        ("agents", "打开行为文件", AGENTS_FILE, "file"),
        ("logs", "打开日志目录", LOGS_DIR, "dir"),
    ];
    Ok(entries
        .into_iter()
        .map(|(id, label, relative, kind)| {
            let path = root.join(relative);
            DocumentDto {
                id: id.to_string(),
                label: label.to_string(),
                exists: path.exists(),
                path: path.to_string_lossy().to_string(),
                kind: kind.to_string(),
            }
        })
        .collect())
}

/// Tauri IPC 命令：列出设置内可打开的目标及其存在状态。
///
/// 界面据此决定是否渲染「打开文件/打开目录」按钮：目标不存在时不显示按钮，
/// 而不是让用户点开一个报错。
#[tauri::command(async)]
pub fn document_catalog() -> Result<Vec<DocumentDto>, String> {
    catalog()
}

/// 用系统默认方式打开文件所在位置或目录本身。
fn open_native(path: &Path, _is_dir: bool) -> Result<(), String> {
    // 文件与目录都交给系统默认程序：文件用关联程序，目录在资源管理器中打开。
    // 这里不再区分平台分支，平台差异由统一入口承担。
    crate::process::open_path(path).map_err(|error| format!("无法打开目标：{error}"))
}

/// Tauri IPC 命令：按标识打开对应的文件或其所在目录。
#[tauri::command(async)]
pub fn open_document(id: String) -> Result<(), String> {
    let entry = catalog()?
        .into_iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| format!("未知的打开目标：{id}"))?;
    open_native(Path::new(&entry.path), entry.kind == "dir")
}

// ------------------------------------------------------------------ MCP

/// 一台 MCP 服务器的界面视图；只透出环境变量名，不含任何值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDto {
    /// 服务器名称。
    pub name: String,
    /// 传输类型：stdio 或 http。
    pub kind: String,
    /// stdio 启动命令。
    pub command: String,
    /// stdio 启动参数。
    pub args: Vec<String>,
    /// http 端点地址。
    pub url: String,
    /// 环境变量名清单，值不回传。
    pub env_keys: Vec<String>,
}

/// 一台 MCP 服务器的写入入参。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct McpServerInput {
    /// 服务器名称。
    pub name: String,
    /// 传输类型。
    pub kind: String,
    /// stdio 启动命令。
    pub command: String,
    /// stdio 启动参数，多行文本按行拆分。
    pub args: Vec<String>,
    /// http 端点地址。
    pub url: String,
    /// 本次输入的键值；值为空串表示保留已有值。
    pub env: BTreeMap<String, String>,
}

/// 读取 MCP 配置回执：原文（可缺）与服务器清单。
///
/// 兼容三种形状：后台当前回 `{"raw": ..., "servers": [...]}`；只回清单时按清单
/// 用；只回原文时在本地按同一套字段投影出清单。原文是写入路径保留用户自有键值
/// 的唯一依据，缺了它就不能安全保存。
fn decode_mcp_receipt(receipt: &Value) -> Result<(Option<String>, Vec<McpServerDto>), String> {
    let raw = receipt
        .get("raw")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let servers = match receipt.get("servers").and_then(Value::as_array) {
        Some(items) => decode_servers(items)?,
        None => match receipt.as_array() {
            Some(items) => decode_servers(items)?,
            None => match raw.as_deref() {
                Some(text) => project_document(&parse_mcp_document(text)?),
                None => {
                    return Err("后台服务回执不是预期的 JSON 结构：缺少 servers".to_string());
                }
            },
        },
    };
    Ok((raw, servers))
}

/// 把清单数组反序列化成既有 DTO。
fn decode_servers(items: &[Value]) -> Result<Vec<McpServerDto>, String> {
    items.iter().cloned().map(config_gateway::decode).collect()
}

/// 从原文投影出服务器清单。
fn project_document(document: &Value) -> Vec<McpServerDto> {
    servers_map(document)
        .iter()
        .map(|(name, entry)| project_server(name, entry))
        .collect()
}

/// 解析 MCP 配置原文。
fn parse_mcp_document(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim_start_matches('\u{feff}');
    if trimmed.trim().is_empty() {
        return Ok(json!({ "mcpServers": {} }));
    }
    let parsed: Value =
        serde_json::from_str(trimmed).map_err(|error| format!("MCP 配置不是合法 JSON：{error}"))?;
    if parsed.get("mcpServers").is_none() {
        return Err("MCP 配置缺少 mcpServers 字段".to_string());
    }
    Ok(parsed)
}

/// 取服务器表。
fn servers_map(document: &Value) -> Map<String, Value> {
    document
        .get("mcpServers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// 把一台服务器投影成界面视图。
fn project_server(name: &str, entry: &Value) -> McpServerDto {
    let string_of = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let args = entry
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut env_keys: Vec<String> = entry
        .get("env")
        .and_then(Value::as_object)
        .map(|items| items.keys().cloned().collect())
        .unwrap_or_default();
    env_keys.sort();

    McpServerDto {
        name: name.to_string(),
        kind: {
            let declared = string_of("type");
            if declared.is_empty() {
                "stdio".to_string()
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

/// Tauri IPC 命令：列出 MCP 服务器。
///
/// # Errors
///
/// 网关不可达、凭据不匹配或后台服务版本过旧。
#[tauri::command(async)]
pub fn mcp_list() -> Result<Vec<McpServerDto>, String> {
    let receipt = config_gateway::get_json(MCP_CONFIG_PATH)?;
    let (_, mut servers) = decode_mcp_receipt(&receipt)?;
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(servers)
}

/// 由入参构建一条服务器配置；已有同名条目用于保留未重输的密钥。
fn build_server(input: &McpServerInput, previous: Option<&Value>) -> Result<Value, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("MCP 服务器名称不能为空".to_string());
    }
    let kind = if input.kind.trim().is_empty() {
        "stdio"
    } else {
        input.kind.trim()
    };

    let mut entry = Map::new();
    entry.insert("type".to_string(), Value::String(kind.to_string()));

    if kind == "http" {
        let url = input.url.trim();
        if url.is_empty() {
            return Err("HTTP 类型的 MCP 服务器必须填写地址".to_string());
        }
        entry.insert("url".to_string(), Value::String(url.to_string()));
    } else {
        let command = input.command.trim();
        if command.is_empty() {
            return Err("stdio 类型的 MCP 服务器必须填写启动命令".to_string());
        }
        entry.insert("command".to_string(), Value::String(command.to_string()));
        let args: Vec<Value> = input
            .args
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(|value| Value::String(value.to_string()))
            .collect();
        if !args.is_empty() {
            entry.insert("args".to_string(), Value::Array(args));
        }
    }

    // 环境变量：本次输入非空则覆盖，留空则沿用原值，避免把已配好的密钥写成空串。
    let previous_env = previous
        .and_then(|value| value.get("env"))
        .and_then(Value::as_object);
    let mut env = Map::new();
    for (key, value) in &input.env {
        let trimmed_key = key.trim();
        if trimmed_key.is_empty() {
            continue;
        }
        let trimmed_value = value.trim();
        if !trimmed_value.is_empty() {
            env.insert(
                trimmed_key.to_string(),
                Value::String(trimmed_value.to_string()),
            );
            continue;
        }
        if let Some(existing) = previous_env.and_then(|items| items.get(trimmed_key)) {
            env.insert(trimmed_key.to_string(), existing.clone());
        }
    }
    if !env.is_empty() {
        entry.insert("env".to_string(), Value::Object(env));
    }

    Ok(Value::Object(entry))
}

/// 读原文、改服务器表、写回原文：MCP 编辑的唯一通道。
///
/// 读改写都在网关侧完成，桌面端只做一次转发；原文里用户自己写的键值原样保留，
/// 落盘格式与用户手写的一致。
fn edit_mcp_config(
    edit: impl FnOnce(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    let receipt = config_gateway::get_json(MCP_CONFIG_PATH)?;
    let (raw, _) = decode_mcp_receipt(&receipt)?;
    let Some(raw) = raw else {
        return Err(
            "后台服务没有提供 MCP 配置原文，无法安全保存（请升级后台服务后再试）".to_string(),
        );
    };
    let mut document = parse_mcp_document(&raw)?;
    let mut servers = servers_map(&document);
    edit(&mut servers)?;
    document["mcpServers"] = Value::Object(servers);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("序列化 MCP 配置失败：{error}"))?;
    let receipt =
        config_gateway::put_json(MCP_CONFIG_PATH, &json!({ "raw": format!("{text}\n") }))?;
    // 回执与读取同构：清单回得来才算这一份写成功。
    decode_mcp_receipt(&receipt).map(|_| ())
}

/// Tauri IPC 命令：新增或更新一台 MCP 服务器。
///
/// # Errors
///
/// 入参不合法、同名冲突、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn mcp_upsert(input: McpServerInput, original_name: Option<String>) -> Result<(), String> {
    edit_mcp_config(move |servers| {
        let name = input.name.trim().to_string();
        if name.is_empty() {
            return Err("MCP 服务器名称不能为空".to_string());
        }
        let previous_name = original_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| name.clone());

        if previous_name != name && servers.contains_key(&name) {
            return Err(format!("已存在同名 MCP 服务器：{name}"));
        }

        let built = build_server(&input, servers.get(&previous_name))?;
        servers.remove(&previous_name);
        servers.insert(name, built);
        Ok(())
    })
}

/// Tauri IPC 命令：移除一台 MCP 服务器。
///
/// # Errors
///
/// 服务器不存在、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn mcp_remove(name: String) -> Result<(), String> {
    edit_mcp_config(|servers| {
        if servers.remove(name.trim()).is_none() {
            return Err(format!("MCP 服务器不存在：{name}"));
        }
        Ok(())
    })
}

/// Tauri IPC 命令：以完整 JSON 配置新增或更新一台 MCP 服务器。
///
/// 面向熟悉原始配置的使用者：直接给出该服务器的完整 JSON 片段，
/// 因此不做字段级改写。仍然校验它必须是 JSON 对象，
/// 并且**不会**因为键名不认识就丢掉内容。
///
/// # Errors
///
/// 入参不是合法 JSON 对象、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn mcp_upsert_raw(name: String, entry_json: String) -> Result<(), String> {
    let trimmed_name = name.trim().to_string();
    if trimmed_name.is_empty() {
        return Err("MCP 服务器名称不能为空".to_string());
    }
    let parsed: Value = serde_json::from_str(entry_json.trim())
        .map_err(|error| format!("配置不是合法 JSON：{error}"))?;
    if !parsed.is_object() {
        return Err("配置必须是 JSON 对象".to_string());
    }
    edit_mcp_config(move |servers| {
        servers.insert(trimmed_name, parsed);
        Ok(())
    })
}

// ------------------------------------------------------------------ 技能

/// 一项技能。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDto {
    /// 目录名，同时作为稳定标识。
    pub id: String,
    /// 技能显示名称。
    pub name: String,
    /// 技能说明。
    pub description: String,
    /// 技能目录绝对路径。
    pub path: String,
    /// 技能描述文件绝对路径；不存在时为空。
    pub entry_path: String,
}

/// Tauri IPC 命令：列出已安装技能。
///
/// 技能目录在后台的运行时根下，解析 YAML 前置块也在后台完成；桌面端只取清单。
///
/// # Errors
///
/// 网关不可达、凭据不匹配或后台服务版本过旧。
#[tauri::command(async)]
pub fn skills_list() -> Result<Vec<SkillDto>, String> {
    let receipt = config_gateway::get_json(SKILLS_PATH)?;
    let items = receipt
        .get("skills")
        .and_then(Value::as_array)
        .or_else(|| receipt.as_array())
        .ok_or_else(|| "后台服务回执不是预期的 JSON 结构：缺少 skills".to_string())?;
    let mut skills: Vec<SkillDto> = items
        .iter()
        .cloned()
        .map(config_gateway::decode)
        .collect::<Result<Vec<SkillDto>, String>>()?;
    skills.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(skills)
}

// ------------------------------------------------------------------ 文档

/// 文本类文档的读取结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentTextDto {
    /// 文档内容；文件不存在时为空串。
    pub content: String,
    /// 文档在本机的绝对路径；云端档案下为空串。
    pub path: String,
    /// 文件是否已存在。
    pub exists: bool,
}

/// 支持的文本文档标识。
fn document_file(id: &str) -> Result<&'static str, String> {
    match id {
        "soul" => Ok(SOUL_FILE),
        "agents" => Ok(AGENTS_FILE),
        _ => Err(format!("不支持的文档：{id}")),
    }
}

/// 文档在本机的落地路径。
///
/// 只有本机档案下这条路径真实可用：云档案的文档在后台，桌面端没有也不该有它的
/// 本机路径，此时回空串，界面那一行就不会显示一个假路径。
fn document_local_path(file_name: &str) -> String {
    let is_local = crate::connection::runtime()
        .active()
        .is_some_and(|entry| !entry.profile.is_cloud());
    if !is_local {
        return String::new();
    }
    axon_root()
        .map(|root| root.join(file_name).to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Tauri IPC 命令：读取人设或行为准则文档。
///
/// # Errors
///
/// 不支持的文档标识、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn document_read(id: String) -> Result<DocumentTextDto, String> {
    let file_name = document_file(&id)?;
    let receipt = config_gateway::get_json(&format!("{DOCUMENTS_PATH_PREFIX}{id}"))?;
    let content = config_gateway::string_field(&receipt, "text")?;
    Ok(DocumentTextDto {
        // 网关只回全文，不回存在性；空正文等于这份文档还没有建好，
        // 因为空内容写入会被拒。
        exists: !content.is_empty(),
        content,
        path: document_local_path(file_name),
    })
}

/// Tauri IPC 命令：写入人设或行为准则文档。
///
/// # Errors
///
/// 不支持的文档标识、正文为空、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn document_write(id: String, content: String) -> Result<(), String> {
    document_file(&id)?;
    if content.trim().is_empty() {
        return Err("文档内容不能为空".to_string());
    }
    let receipt = config_gateway::put_json(
        &format!("{DOCUMENTS_PATH_PREFIX}{id}"),
        &json!({ "text": content }),
    )?;
    // 回执与读取同构：正文回得来才算这一份写成功。
    if config_gateway::string_field(&receipt, "text")?
        .trim()
        .is_empty()
    {
        return Err("后台服务回执的正文为空，写入可能未生效".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{McpServerInput, build_server, decode_mcp_receipt, project_server};

    #[test]
    fn server_projection_never_exposes_env_values() {
        let entry = json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "pkg"],
            "env": { "SECRET_KEY": "sk-should-never-leak" }
        });
        let dto = project_server("demo", &entry);
        assert_eq!(dto.env_keys, vec!["SECRET_KEY".to_string()]);
        let serialized = serde_json::to_string(&dto).expect("投影结果必须可序列化");
        assert!(
            !serialized.contains("sk-should-never-leak"),
            "投影结果绝不能携带环境变量值"
        );
    }

    #[test]
    fn receipt_accepts_both_the_list_and_the_raw_document() {
        // 后台当前回原文加清单；只回清单时按清单用；只回原文时本地投影。
        let both = json!({
            "raw": "{\"mcpServers\":{\"demo\":{\"type\":\"stdio\",\"command\":\"npx\"}}}",
            "servers": [{"name": "demo", "kind": "stdio", "command": "npx", "args": [], "url": "", "env_keys": []}]
        });
        let (raw, servers) = decode_mcp_receipt(&both).expect("带清单的回执必须可解析");
        assert!(raw.is_some(), "原文必须保留，写入路径要用它保值");
        assert_eq!(servers.len(), 1);

        let raw_only = json!({
            "raw": "{\"mcpServers\":{\"demo\":{\"type\":\"stdio\",\"command\":\"npx\"}}}"
        });
        let (_, servers) = decode_mcp_receipt(&raw_only).expect("只回原文时必须能投影出清单");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "demo");

        assert!(decode_mcp_receipt(&json!({ "other": 1 })).is_err());
    }

    #[test]
    fn empty_env_input_keeps_the_existing_secret() {
        // 界面留空表示「不修改这把密钥」：旧值必须原样保留，绝不能被空串覆盖。
        let mut env = BTreeMap::new();
        env.insert("SECRET_KEY".to_string(), String::new());
        let input = McpServerInput {
            name: "demo".to_string(),
            kind: "stdio".to_string(),
            command: "npx".to_string(),
            env,
            ..McpServerInput::default()
        };
        let previous = json!({ "env": { "SECRET_KEY": "sk-existing" } });
        let built = build_server(&input, Some(&previous)).expect("合法入参必须能建出条目");
        assert_eq!(built["env"]["SECRET_KEY"], "sk-existing");
    }

    #[test]
    fn http_server_requires_an_address() {
        let input = McpServerInput {
            name: "demo".to_string(),
            kind: "http".to_string(),
            ..McpServerInput::default()
        };
        assert!(build_server(&input, None).is_err());
    }
}
