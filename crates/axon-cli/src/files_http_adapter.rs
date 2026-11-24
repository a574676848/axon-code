//! 服务端文件列表与文件内容只读路由。
//!
//! 两条路由都只读，边界一律以「已注册的工作区根」为准：`root` 是根，
//! `path` 是根内相对路径（缺省为空即根自身，也接受落在根内的绝对路径）。
//!
//! 性能契约（合同 §15.2 P5、P11）落在两处：列表一次只读一层，且单层最多 500 条；
//! 内容读取先看元数据长度再决定读不读，绝不把超大文件整体读进内存。
//!
//! 鉴权不在这里做：网关的 `custom()` 对 `/api/v1/*` 无条件鉴权，适配器拿不到豁免权。
//!
//! 附件目录另有一条硬规则（合同 §6.3.1）：它自成一类根，不得被当作工作区根浏览。
//! 因此**列表**路由命中附件目录时直接 403；**内容**路由不设此限，因为云端档案的
//! 附件预览本来就要按已知路径向服务端取字节。

use std::path::{Path, PathBuf};

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde_json::{Value, json};

use crate::server_path::{
    PathDialect, Rejection, is_attachments_path, query_parameter, resolve_within,
};

/// 文件列表路由。
const LIST_PATH: &str = "/api/v1/files";
/// 文件内容路由。
const CONTENT_PATH: &str = "/api/v1/files/content";
/// 单层目录最多返回的条目数，与既有基线一致。
const MAX_DIRECTORY_ENTRIES: usize = 500;
/// 文本读取上限 2 MiB，与既有基线一致。
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
/// 二进制读取上限 12 MiB，与既有基线一致。
const MAX_BINARY_BYTES: u64 = 12 * 1024 * 1024;
/// 文本模式。
const ENCODING_TEXT: &str = "text";
/// 二进制模式。
const ENCODING_BASE64: &str = "base64";
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 缺少必填查询参数的错误码。
const MISSING_PARAMETER: &str = "missing_parameter";
/// 查询参数取值非法的错误码。
const INVALID_PARAMETER: &str = "invalid_parameter";
/// 读不到目标的错误码。
const NOT_FOUND: &str = "not_found";
/// 无权读取目标的错误码。
const FORBIDDEN: &str = "forbidden";
/// 目标超过读取上限的错误码。
const TOO_LARGE: &str = "too_large";
/// 读取失败的兜底错误码。
const READ_FAILED: &str = "read_failed";

/// 文件列表与文件内容适配器。
pub(crate) struct FilesHttpAdapter {
    /// Axon 运行时根目录；附件目录据此判定。
    axon_root: PathBuf,
}

impl FilesHttpAdapter {
    /// 以运行时根目录构造。
    pub(crate) fn new(axon_root: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
        }
    }

    /// 列出根内某一层目录的直接子项。
    fn list(&self, query: Option<&str>) -> GatewayHttpResponse {
        let root = match required_parameter(query, "root") {
            Ok(root) => root,
            Err(response) => return response,
        };
        let relative = query_parameter(query, "path").unwrap_or_default();
        let dialect = PathDialect::host();
        let root_directory = match resolve_within(Path::new(&root), "", dialect) {
            Ok(directory) => directory,
            Err(rejection) => return rejection_response(rejection),
        };
        let directory = match resolve_within(Path::new(&root), &relative, dialect) {
            Ok(directory) => directory,
            Err(rejection) => return rejection_response(rejection),
        };
        if is_attachments_path(&directory, &self.axon_root, dialect) {
            return forbidden("附件目录自成一类根，不得被当作工作区根浏览");
        }
        if !directory.is_dir() {
            return not_found("目标不是目录");
        }

        let reader = match std::fs::read_dir(&directory) {
            Ok(reader) => reader,
            Err(error) => return read_failure("读取目录", error),
        };
        let mut entries: Vec<(String, Value)> = Vec::new();
        for item in reader {
            // 单层上限加一条：多读一条只为判断是否存在被截断的条目。
            if entries.len() > MAX_DIRECTORY_ENTRIES {
                break;
            }
            let Ok(item) = item else {
                continue;
            };
            let name = item.file_name().to_string_lossy().to_string();
            let is_dir = item.file_type().is_ok_and(|kind| kind.is_dir());
            // 目录不带 size：目录项的元数据长度与界面语义无关，给 0 而不是编造值。
            let metadata = item.metadata().ok();
            let size = if is_dir {
                0
            } else {
                metadata.as_ref().map_or(0, std::fs::Metadata::len)
            };
            let modified_ms = metadata.as_ref().and_then(modified_millis);
            let path = relative_display(&root_directory, &item.path(), &name);
            entries.push((
                name.clone(),
                json!({
                    "name": name,
                    "path": path,
                    "isDir": is_dir,
                    "size": size,
                    "modifiedMs": modified_ms,
                }),
            ));
        }
        let truncated = entries.len() > MAX_DIRECTORY_ENTRIES;
        // 按目录项名称升序；截断时只回前 500 条，不回整棵子树。
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries.truncate(MAX_DIRECTORY_ENTRIES);
        GatewayHttpResponse::json(
            200,
            &json!({
                "entries": entries.into_iter().map(|(_, entry)| entry).collect::<Vec<Value>>(),
                "truncated": truncated,
            }),
        )
    }

    /// 读取根内一个文件的内容或原始字节。
    fn content(&self, query: Option<&str>) -> GatewayHttpResponse {
        let root = match required_parameter(query, "root") {
            Ok(root) => root,
            Err(response) => return response,
        };
        let relative = match required_parameter(query, "path") {
            Ok(relative) => relative,
            Err(response) => return response,
        };
        let encoding =
            query_parameter(query, "encoding").unwrap_or_else(|| ENCODING_TEXT.to_owned());
        if encoding != ENCODING_TEXT && encoding != ENCODING_BASE64 {
            return invalid_parameter("encoding 只接受 text 或 base64");
        }

        let dialect = PathDialect::host();
        let path = match resolve_within(Path::new(&root), &relative, dialect) {
            Ok(path) => path,
            Err(rejection) => return rejection_response(rejection),
        };
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => return read_failure("读取文件信息", error),
        };
        if !metadata.is_file() {
            return not_found("目标不是文件");
        }
        let length = metadata.len();
        // 先按元数据长度判定上限：超限连读都不读，内存不随文件体积增长。
        let limit = if encoding == ENCODING_TEXT {
            MAX_TEXT_BYTES
        } else {
            MAX_BINARY_BYTES
        };
        if length > limit {
            return GatewayHttpResponse::json(
                413,
                &json!({
                    "error": TOO_LARGE,
                    "message": format!(
                        "文件超过 {limit} 字节读取上限（当前 {length} 字节），请改用另一种模式或分段读取"
                    ),
                    "limit": limit,
                    "bytes": length,
                }),
            );
        }

        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => return read_failure("读取文件", error),
        };
        let byte_count = bytes.len() as u64;
        if encoding == ENCODING_TEXT {
            let text = String::from_utf8_lossy(&bytes).to_string();
            return GatewayHttpResponse::json(
                200,
                &json!({ "text": text, "bytes": byte_count, "truncated": false }),
            );
        }
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        GatewayHttpResponse::json(
            200,
            &json!({
                "base64": encoded,
                "mime": mime_of(&path),
                "bytes": byte_count,
            }),
        )
    }
}

impl GatewayHttpRouteHandler for FilesHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        _body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let is_list = path == LIST_PATH;
        if !is_list && path != CONTENT_PATH {
            // 未命中一律交回组合器，不得返回统一 404 截断后续适配器。
            return None;
        }
        if method != "GET" {
            return Some(GatewayHttpResponse::json(
                405,
                &json!({ "error": METHOD_NOT_ALLOWED }),
            ));
        }
        Some(if is_list {
            self.list(query)
        } else {
            self.content(query)
        })
    }
}

/// 目录项相对根的展示路径，统一用正斜杠。
fn relative_display(root: &Path, entry: &Path, fallback: &str) -> String {
    let relative = entry.strip_prefix(root).unwrap_or(entry);
    let text = relative.to_string_lossy();
    if text.is_empty() {
        return fallback.to_owned();
    }
    text.replace('\\', "/")
}

/// 文件的修改时间（Unix 毫秒）；平台给不出来时回 `null`，不编造时间戳。
fn modified_millis(metadata: &std::fs::Metadata) -> Option<u64> {
    let modified = metadata.modified().ok()?;
    let elapsed = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// 按扩展名判定 MIME 类型；判不出来时给通用二进制类型。
fn mime_of(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "txt" | "md" | "log" => "text/plain",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// 读取必填查询参数；缺失或为空一律 400。
fn required_parameter(query: Option<&str>, key: &str) -> Result<String, GatewayHttpResponse> {
    match query_parameter(query, key) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(GatewayHttpResponse::json(
            400,
            &json!({
                "error": MISSING_PARAMETER,
                "message": format!("缺少必填查询参数 {key}"),
            }),
        )),
    }
}

/// 归一化拒绝结论 -> 响应：越界与非法形态按拒绝码给出，理由一律走中文。
fn rejection_response(rejection: Rejection) -> GatewayHttpResponse {
    let status = rejection.denied_status();
    let code = match status {
        403 => FORBIDDEN,
        404 => NOT_FOUND,
        _ => INVALID_PARAMETER,
    };
    GatewayHttpResponse::json(
        status,
        &json!({ "error": code, "message": rejection.message() }),
    )
}

/// 目标不存在。
fn not_found(message: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(404, &json!({ "error": NOT_FOUND, "message": message }))
}

/// 目标不允许访问。
fn forbidden(message: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(403, &json!({ "error": FORBIDDEN, "message": message }))
}

/// 查询参数取值非法。
fn invalid_parameter(message: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(
        400,
        &json!({ "error": INVALID_PARAMETER, "message": message }),
    )
}

/// 文件系统错误 -> 响应：不存在是 404，无权限是 403，其余如实报 500。
fn read_failure(action: &str, error: std::io::Error) -> GatewayHttpResponse {
    let message = format!("{action}失败：{error}");
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            GatewayHttpResponse::json(404, &json!({ "error": NOT_FOUND, "message": message }))
        }
        std::io::ErrorKind::PermissionDenied => {
            GatewayHttpResponse::json(403, &json!({ "error": FORBIDDEN, "message": message }))
        }
        _ => GatewayHttpResponse::json(500, &json!({ "error": READ_FAILED, "message": message })),
    }
}

#[cfg(test)]
#[path = "files_http_adapter_tests.rs"]
mod files_http_adapter_tests;
