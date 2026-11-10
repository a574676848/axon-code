//! 技能清单与移除 HTTP 适配器。
//!
//! 清单：只读，列出运行时根下 `skills/` 里的技能目录，从每个 `SKILL.md` 的 YAML 前置块
//! 读取名称与说明。字段名沿用桌面端 `skills_list` 的 `SkillDto`
//! （`id`、`name`、`description`、`path`、`entry_path`），因此同一份界面代码在
//! 直连宿主与经网关两条通道上拿到的是同一形状。
//!
//! 移除：`DELETE /api/v1/skills/{id}` 删除技能根下的**一个真实子目录**。技能的本体就是
//! 磁盘目录，删掉即不再加载，因此这条路由是破坏性的，删除范围被钉死在 `skills/` 之内：
//! 标识只接受单层普通文件名，目标必须是技能根的直接子目录，软链接与根外目标一律拒绝。
//!
//! 取值口径与桌面端一致：前置块缺失或字段缺失时，名称回退为目录名、说明为空串，
//! 不因为某个技能写得潦草就把它从清单里藏起来。清单只回元数据，**不回技能正文**。

use std::path::PathBuf;

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde::Serialize;
use serde_json::json;

/// 技能清单路由。
const SKILLS_PATH: &str = "/api/v1/skills";
/// 技能移除路由前缀；其后一段是技能目录名。
const SKILLS_ID_PREFIX: &str = "/api/v1/skills/";
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
/// 移除失败的错误码。
const REMOVE_FAILED: &str = "remove_failed";
/// 技能目录不存在（含已经移除）的错误码。
const SKILL_NOT_FOUND: &str = "skill_not_found";
/// 技能标识不合法的错误码。
const INVALID_SKILL_ID: &str = "invalid_skill_id";

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
            Err(message) => error(500, READ_FAILED, &message),
        }
    }

    /// `DELETE /api/v1/skills/{id}`：删除一个技能目录。
    ///
    /// 三道闸门缺一不可：标识必须是单层普通文件名，目标必须是真实目录（软链接不算），
    /// 规范化后的父目录必须是技能根本身。删除用的是规范化之后的路径，因此判定与删除
    /// 指向同一个对象，标识里的 `..` 一类写法无法把范围带出 `skills/`。
    fn remove(&self, id: &str) -> GatewayHttpResponse {
        if !is_safe_skill_id(id) {
            return error(
                400,
                INVALID_SKILL_ID,
                "技能标识不合法：只接受技能根下的单层目录名",
            );
        }
        let root = self.axon_root.join(SKILLS_DIR);
        let directory = root.join(id);
        let Ok(metadata) = std::fs::symlink_metadata(&directory) else {
            return error(404, SKILL_NOT_FOUND, "技能不存在或已被移除");
        };
        // 软链接与 junction 都指向别处：删它等于删别处的数据，不代劳。
        if metadata.file_type().is_symlink() {
            return error(
                400,
                INVALID_SKILL_ID,
                "该技能是软链接，界面不代为删除，请在技能目录里手工处理",
            );
        }
        if !metadata.is_dir() {
            return error(404, SKILL_NOT_FOUND, "技能不存在或已被移除");
        }
        // 技能根自身也可能是软链接，只有两侧同时规范化，父目录才对得上。
        let (Ok(real_root), Ok(real_directory)) = (
            std::fs::canonicalize(&root),
            std::fs::canonicalize(&directory),
        ) else {
            return error(404, SKILL_NOT_FOUND, "技能不存在或已被移除");
        };
        if real_directory.parent() != Some(real_root.as_path()) {
            return error(400, INVALID_SKILL_ID, "技能标识不合法：目标不在技能目录内");
        }
        match std::fs::remove_dir_all(&real_directory) {
            Ok(()) => GatewayHttpResponse::json(200, &json!({ "removed": id })),
            Err(cause) => error(500, REMOVE_FAILED, &format!("删除技能目录失败：{cause}")),
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
        // 精确匹配：只认「技能集合」与「集合下的单个技能」两条路径。未命中一律交回
        // 组合器，不得返回统一 404 截断后续适配器。
        let id = skill_id(path);
        if path != SKILLS_PATH && id.is_none() {
            return None;
        }
        Some(match (id, method) {
            (None, "GET") => self.list(),
            (Some(id), "DELETE") => self.remove(&id),
            _ => GatewayHttpResponse::json(405, &json!({ "error": METHOD_NOT_ALLOWED })),
        })
    }
}

/// 从请求路径里取出技能标识；不属于移除路由时为 None。
///
/// 与网关的每路由上限表同一口径：`{id}` 只匹配一段非空路径，因此 `/api/v1/skills/`
/// 与 `/api/v1/skills/a/b` 都不属于本族。
fn skill_id(path: &str) -> Option<String> {
    let encoded = path.strip_prefix(SKILLS_ID_PREFIX)?;
    if encoded.is_empty() || encoded.contains('/') {
        return None;
    }
    let decoded = percent_decode(encoded);
    (!decoded.is_empty()).then_some(decoded)
}

/// 技能标识的合法形态：技能根下的单层普通目录名。
///
/// 拒绝路径分隔符（含 Windows 的反斜杠）、点开头的运行态目录（`.`、`..` 与
/// `.revisions` 一类都由这一条挡掉），以及首尾空白：Windows 会把尾随的点与空格
/// 规范化掉，带着它们通过校验的标识与磁盘上的名字并不是同一个。
fn is_safe_skill_id(id: &str) -> bool {
    !id.is_empty() && id == id.trim() && !id.starts_with('.') && !id.contains(['/', '\\', '\0'])
}

/// 最小百分号解码：`%XX` 还原为字节，非法序列原样保留。
///
/// 只服务路由里的技能标识：目录名可能带空格与中文，客户端按 URL 规则编码后送来，
/// 这里解回磁盘上的原名再比对。网关不提供解码器，各适配器自带一份同名口径。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// 单个十六进制字符的取值。
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 统一错误回执：错误码加一句可执行的中文原因。
fn error(status: u16, code: &str, message: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &json!({ "error": code, "message": message }))
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
