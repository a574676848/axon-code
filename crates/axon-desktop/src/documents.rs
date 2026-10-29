//! 设置内的文档与目录管理：MCP 配置、技能目录、人设与行为准则。
//!
//! 安全边界：MCP 配置里可能含明文密钥，因此**读取路径只透出环境变量名**，
//! 绝不把已有值回传给界面；写入时界面只提交用户本次输入的键值，
//! 已有键在未重新输入时保留原值，旧值不会被空串覆盖。
//!
//! 原子写与配置系统同源，一律复用
//! `axon_config::atomic_write::replace_file_atomically`。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use axon_config::ConfigLoader;
use serde::{Deserialize, Serialize};

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
/// 技能描述文件名。
const SKILL_ENTRY_FILE: &str = "SKILL.md";
/// 技能描述里单个字段允许的最大长度。
const SKILL_FIELD_MAX_CHARS: usize = 160;

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

/// 把绝对路径原子写入，父目录按需创建。
fn write_text(path: &Path, text: &str) -> Result<(), String> {
    axon_config::atomic_write::replace_file_atomically(path, text.as_bytes())
        .map_err(|error| format!("原子写入失败：{error:?}"))
}

/// 设置内四个可打开的目标。
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

/// MCP 配置文件路径。
fn mcp_config_path() -> Result<PathBuf, String> {
    Ok(axon_root()?.join(MCP_CONFIG_FILE))
}

/// 读取 MCP 配置根对象。
fn read_mcp_document(path: &Path) -> Result<serde_json::Value, String> {
    if !path.exists() {
        return Ok(serde_json::json!({ "mcpServers": {} }));
    }
    let text = fs::read_to_string(path).map_err(|error| format!("读取 MCP 配置失败：{error}"))?;
    let trimmed = text.trim_start_matches('\u{feff}');
    if trimmed.trim().is_empty() {
        return Ok(serde_json::json!({ "mcpServers": {} }));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|error| format!("MCP 配置不是合法 JSON：{error}"))?;
    if parsed.get("mcpServers").is_none() {
        return Err("MCP 配置缺少 mcpServers 字段".to_string());
    }
    Ok(parsed)
}

/// 取服务器表。
fn servers_map(document: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    document
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// 把一台服务器投影成界面视图。
fn project_server(name: &str, entry: &serde_json::Value) -> McpServerDto {
    let string_of = |key: &str| {
        entry
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let args = entry
        .get("args")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut env_keys: Vec<String> = entry
        .get("env")
        .and_then(serde_json::Value::as_object)
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
#[tauri::command(async)]
pub fn mcp_list() -> Result<Vec<McpServerDto>, String> {
    let path = mcp_config_path()?;
    let document = read_mcp_document(&path)?;
    let mut servers: Vec<McpServerDto> = servers_map(&document)
        .iter()
        .map(|(name, entry)| project_server(name, entry))
        .collect();
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(servers)
}

/// 由入参构建一条服务器配置；已有同名条目用于保留未重输的密钥。
fn build_server(
    input: &McpServerInput,
    previous: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("MCP 服务器名称不能为空".to_string());
    }
    let kind = if input.kind.trim().is_empty() {
        "stdio"
    } else {
        input.kind.trim()
    };

    let mut entry = serde_json::Map::new();
    entry.insert(
        "type".to_string(),
        serde_json::Value::String(kind.to_string()),
    );

    if kind == "http" {
        let url = input.url.trim();
        if url.is_empty() {
            return Err("HTTP 类型的 MCP 服务器必须填写地址".to_string());
        }
        entry.insert(
            "url".to_string(),
            serde_json::Value::String(url.to_string()),
        );
    } else {
        let command = input.command.trim();
        if command.is_empty() {
            return Err("stdio 类型的 MCP 服务器必须填写启动命令".to_string());
        }
        entry.insert(
            "command".to_string(),
            serde_json::Value::String(command.to_string()),
        );
        let args: Vec<serde_json::Value> = input
            .args
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(|value| serde_json::Value::String(value.to_string()))
            .collect();
        if !args.is_empty() {
            entry.insert("args".to_string(), serde_json::Value::Array(args));
        }
    }

    // 环境变量：本次输入非空则覆盖，留空则沿用原值，避免把已配好的密钥写成空串。
    let previous_env = previous
        .and_then(|value| value.get("env"))
        .and_then(serde_json::Value::as_object);
    let mut env = serde_json::Map::new();
    for (key, value) in &input.env {
        let trimmed_key = key.trim();
        if trimmed_key.is_empty() {
            continue;
        }
        let trimmed_value = value.trim();
        if !trimmed_value.is_empty() {
            env.insert(
                trimmed_key.to_string(),
                serde_json::Value::String(trimmed_value.to_string()),
            );
            continue;
        }
        if let Some(existing) = previous_env.and_then(|items| items.get(trimmed_key)) {
            env.insert(trimmed_key.to_string(), existing.clone());
        }
    }
    if !env.is_empty() {
        entry.insert("env".to_string(), serde_json::Value::Object(env));
    }

    Ok(serde_json::Value::Object(entry))
}

/// Tauri IPC 命令：新增或更新一台 MCP 服务器。
#[tauri::command(async)]
pub fn mcp_upsert(input: McpServerInput, original_name: Option<String>) -> Result<(), String> {
    let path = mcp_config_path()?;
    let mut document = read_mcp_document(&path)?;
    let mut servers = servers_map(&document);

    let name = input.name.trim().to_string();
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

    document["mcpServers"] = serde_json::Value::Object(servers);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("序列化 MCP 配置失败：{error}"))?;
    write_text(&path, &format!("{text}\n"))
}

/// Tauri IPC 命令：移除一台 MCP 服务器。
#[tauri::command(async)]
pub fn mcp_remove(name: String) -> Result<(), String> {
    let path = mcp_config_path()?;
    let mut document = read_mcp_document(&path)?;
    let mut servers = servers_map(&document);
    if servers.remove(name.trim()).is_none() {
        return Err(format!("MCP 服务器不存在：{name}"));
    }
    document["mcpServers"] = serde_json::Value::Object(servers);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("序列化 MCP 配置失败：{error}"))?;
    write_text(&path, &format!("{text}\n"))
}

/// Tauri IPC 命令：以完整 JSON 配置新增或更新一台 MCP 服务器。
///
/// 面向熟悉原始配置的使用者：直接给出该服务器的完整 JSON 片段，
/// 因此不做字段级改写。仍然校验它必须是 JSON 对象，
/// 并且**不会**因为键名不认识就丢掉内容。
#[tauri::command(async)]
pub fn mcp_upsert_raw(name: String, entry_json: String) -> Result<(), String> {
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err("MCP 服务器名称不能为空".to_string());
    }
    let parsed: serde_json::Value = serde_json::from_str(entry_json.trim())
        .map_err(|error| format!("配置不是合法 JSON：{error}"))?;
    if !parsed.is_object() {
        return Err("配置必须是 JSON 对象".to_string());
    }

    let path = mcp_config_path()?;
    let mut document = read_mcp_document(&path)?;
    let mut servers = servers_map(&document);
    servers.insert(trimmed_name.to_string(), parsed);
    document["mcpServers"] = serde_json::Value::Object(servers);

    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("序列化 MCP 配置失败：{error}"))?;
    write_text(&path, &format!("{text}\n"))
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

/// 从 SKILL.md 的 YAML 前置块里取一个字段。
///
/// 支持两种写法：单行值，以及 YAML 的块标量（`description: |` 后面跟缩进正文）。
/// 技能说明经常是一整段话，写成块标量很自然；只认单行会把 `|` 本身当成说明，
/// 斜杠指令里就会出现一个只有竖线的候选。
fn front_matter_field(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let prefix = format!("{key}:");
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let Some(rest) = trimmed.strip_prefix(&prefix) else {
            continue;
        };
        let value = rest.trim().trim_matches(['"', '\'']).trim();
        if is_block_scalar_marker(value) {
            return collect_block_scalar(&mut lines, line);
        }
        if !value.is_empty() {
            return Some(clamp_field(value));
        }
    }
    None
}

/// 该取值是否表示「正文在后面的缩进块里」。`|-`、`|+`、`>` 同理。
fn is_block_scalar_marker(value: &str) -> bool {
    matches!(value, "|" | "|-" | "|+" | ">" | ">-" | ">+")
}

/// 收集块标量正文：比 `key:` 那一行更深的缩进行，直到回到同级或块结束。
fn collect_block_scalar<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    key_line: &str,
) -> Option<String> {
    let key_indent = key_line.len() - key_line.trim_start().len();
    let mut parts: Vec<String> = Vec::new();
    for line in lines.by_ref() {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let indent = line.len() - line.trim_start().len();
        // 空行属于块内断行；缩进回到 key 同级则块已结束。
        if trimmed.is_empty() {
            if !parts.is_empty() {
                parts.push(String::new());
            }
            continue;
        }
        if indent <= key_indent {
            break;
        }
        parts.push(trimmed.to_string());
    }
    while parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    let joined = parts.join("\n").trim().to_string();
    (!joined.is_empty()).then(|| clamp_field(&joined))
}

/// 截断到展示上限。
fn clamp_field(value: &str) -> String {
    value.chars().take(SKILL_FIELD_MAX_CHARS).collect()
}

/// Tauri IPC 命令：列出已安装技能。
#[tauri::command(async)]
pub fn skills_list() -> Result<Vec<SkillDto>, String> {
    let root = axon_root()?.join(SKILLS_DIR);
    let Ok(entries) = fs::read_dir(&root) else {
        return Ok(Vec::new());
    };

    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if id.starts_with('.') {
            continue;
        }
        let entry_path = path.join(SKILL_ENTRY_FILE);
        let text = fs::read_to_string(&entry_path).unwrap_or_default();
        let name = front_matter_field(&text, "name").unwrap_or_else(|| id.to_string());
        let description = front_matter_field(&text, "description").unwrap_or_default();
        skills.push(SkillDto {
            id: id.to_string(),
            name,
            description,
            path: path.to_string_lossy().to_string(),
            entry_path: if entry_path.exists() {
                entry_path.to_string_lossy().to_string()
            } else {
                String::new()
            },
        });
    }
    skills.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(skills)
}

// ------------------------------------------------------------------ 文档

/// 文本类文档的读取结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentTextDto {
    /// 文档内容；文件不存在时为空串。
    pub content: String,
    /// 文档绝对路径。
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

/// Tauri IPC 命令：读取人设或行为准则文档。
#[tauri::command(async)]
pub fn document_read(id: String) -> Result<DocumentTextDto, String> {
    let path = axon_root()?.join(document_file(&id)?);
    let exists = path.exists();
    let content = if exists {
        fs::read_to_string(&path)
            .map_err(|error| format!("读取文档失败：{error}"))?
            .trim_start_matches('\u{feff}')
            .to_string()
    } else {
        String::new()
    };
    Ok(DocumentTextDto {
        content,
        path: path.to_string_lossy().to_string(),
        exists,
    })
}

/// Tauri IPC 命令：写入人设或行为准则文档。
#[tauri::command(async)]
pub fn document_write(id: String, content: String) -> Result<(), String> {
    if content.trim().is_empty() {
        return Err("文档内容不能为空".to_string());
    }
    write_text(&axon_root()?.join(document_file(&id)?), &content)
}

#[cfg(test)]
mod tests {
    use super::{front_matter_field, project_server};

    #[test]
    fn front_matter_reads_name_and_description() {
        let text = "---\nname: 示例技能\ndescription: 一句话说明\n---\n正文";
        assert_eq!(
            front_matter_field(text, "name").as_deref(),
            Some("示例技能")
        );
        assert_eq!(
            front_matter_field(text, "description").as_deref(),
            Some("一句话说明")
        );
        assert!(front_matter_field("没有前置块", "name").is_none());
    }

    #[test]
    fn front_matter_reads_block_scalar_descriptions() {
        // 技能说明常写成 YAML 块标量：只认单行会把 `|` 本身当成说明，
        // 斜杠指令里就会出现一个只有竖线的候选。
        let text = "---\nname: browser-skill\ndescription: |\n  Use when the user asks to automate\n  their logged-in browser.\n---\n正文";
        assert_eq!(
            front_matter_field(text, "description").as_deref(),
            Some("Use when the user asks to automate\ntheir logged-in browser.")
        );

        // 折叠标量（`>`）与截断标记（`|-`）同样按块处理。
        let folded = "---\ndescription: >-\n  第一行\n  第二行\n---\n";
        assert_eq!(
            front_matter_field(folded, "description").as_deref(),
            Some("第一行\n第二行")
        );

        // 块结束于同级字段：后面的字段不能被吃进说明里。
        let with_tail = "---\ndescription: |\n  说明\nname: 之后的字段\n---\n";
        assert_eq!(
            front_matter_field(with_tail, "description").as_deref(),
            Some("说明")
        );
        assert_eq!(
            front_matter_field(with_tail, "name").as_deref(),
            Some("之后的字段")
        );
    }

    #[test]
    fn server_projection_never_exposes_env_values() {
        let entry = serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "pkg"],
            "env": { "SECRET_KEY": "sk-should-never-leak" }
        });
        let dto = project_server("demo", &entry);
        assert_eq!(dto.env_keys, vec!["SECRET_KEY".to_string()]);
        let serialized = serde_json::to_string(&dto).unwrap();
        assert!(
            !serialized.contains("sk-should-never-leak"),
            "投影结果绝不能携带环境变量值"
        );
    }
}
