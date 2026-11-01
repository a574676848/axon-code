//! 技能清单 HTTP 适配器。
//!
//! 只读：列出运行时根下 `skills/` 里的技能目录，从每个 `SKILL.md` 的 YAML 前置块
//! 读取名称与说明。字段名沿用桌面端 `skills_list` 的 `SkillDto`
//! （`id`、`name`、`description`、`path`、`entry_path`），因此同一份界面代码在
//! 直连宿主与经网关两条通道上拿到的是同一形状。
//!
//! 取值口径与桌面端一致：前置块缺失或字段缺失时，名称回退为目录名、说明为空串，
//! 不因为某个技能写得潦草就把它从清单里藏起来。清单只回元数据，**不回技能正文**。

use std::path::PathBuf;

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde::Serialize;
use serde_json::json;

/// 技能清单路由。
const SKILLS_PATH: &str = "/api/v1/skills";
/// 技能目录相对运行时根目录的名称。
const SKILLS_DIR: &str = "skills";
/// 技能描述文件名。
const SKILL_ENTRY_FILE: &str = "SKILL.md";
/// 技能说明里单个字段允许的最大长度。
const FIELD_MAX_CHARS: usize = 160;
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 读取失败的错误码。
const READ_FAILED: &str = "read_failed";

/// 一项技能的界面视图；字段名与桌面端 `SkillDto` 逐字一致。
#[derive(Debug, Serialize)]
struct SkillView {
    /// 目录名，同时作为稳定标识。
    id: String,
    /// 技能显示名称。
    name: String,
    /// 技能说明。
    description: String,
    /// 技能目录绝对路径。
    path: String,
    /// 技能描述文件绝对路径；不存在时为空。
    entry_path: String,
}

/// 技能清单适配器。
pub(crate) struct SkillsHttpAdapter {
    /// Axon 运行时根目录。
    axon_root: PathBuf,
}

impl SkillsHttpAdapter {
    /// 以运行时根目录构造。
    pub(crate) fn new(axon_root: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
        }
    }

    /// `GET /api/v1/skills`。
    fn list(&self) -> GatewayHttpResponse {
        match self.collect() {
            Ok(skills) => GatewayHttpResponse::json(200, &json!({ "skills": skills })),
            Err(message) => {
                GatewayHttpResponse::json(500, &json!({ "error": READ_FAILED, "message": message }))
            }
        }
    }

    /// 扫描技能目录并投影成清单。
    fn collect(&self) -> Result<Vec<SkillView>, String> {
        let root = self.axon_root.join(SKILLS_DIR);
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            // 目录不存在等于「一个技能也没装」，不是错误。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("读取技能目录失败：{error}")),
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
            // 以点开头的目录是运行态数据（例如技能演进的修订目录），不是技能本体。
            if id.starts_with('.') {
                continue;
            }
            let entry_path = path.join(SKILL_ENTRY_FILE);
            let text = std::fs::read_to_string(&entry_path).unwrap_or_default();
            let name = front_matter_field(&text, "name").unwrap_or_else(|| id.to_owned());
            let description = front_matter_field(&text, "description").unwrap_or_default();
            skills.push(SkillView {
                id: id.to_owned(),
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
}

impl GatewayHttpRouteHandler for SkillsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        _body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        // 精确匹配：未命中一律交回组合器，不得返回统一 404 截断后续适配器。
        if path != SKILLS_PATH {
            return None;
        }
        Some(if method == "GET" {
            self.list()
        } else {
            GatewayHttpResponse::json(405, &json!({ "error": METHOD_NOT_ALLOWED }))
        })
    }
}

/// 从 `SKILL.md` 的 YAML 前置块里取一个字段。
///
/// 支持两种写法：单行值，以及 YAML 的块标量（`description: |` 后面跟缩进正文）。
/// 技能说明经常是一整段话，写成块标量很自然；只认单行会把 `|` 本身当成说明。
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
    let key_indent = key_line.len().saturating_sub(key_line.trim_start().len());
    let mut parts: Vec<String> = Vec::new();
    for line in lines.by_ref() {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let indent = line.len().saturating_sub(line.trim_start().len());
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
        parts.push(trimmed.to_owned());
    }
    while parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    let joined = parts.join("\n").trim().to_owned();
    (!joined.is_empty()).then(|| clamp_field(&joined))
}

/// 截断到展示上限。
fn clamp_field(value: &str) -> String {
    value.chars().take(FIELD_MAX_CHARS).collect()
}

#[cfg(test)]
#[path = "skills_http_adapter_tests.rs"]
mod skills_http_adapter_tests;
