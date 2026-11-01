//! 附件上传路由：`POST /api/v1/attachments`。
//!
//! 请求体是 `multipart/form-data`，取名为 `file` 的那一段；文件本体落到运行时根下的
//! `data/attachments/<uuid>/<安全文件名>`，与桌面端既有布局一致。返回的是**绝对路径**
//! 与展示名，客户端据此再走内容路由取字节。
//!
//! 两条硬约束来自合同 §6.1.1 与 §15.2：
//!
//! - 文件本体上限 64 MiB。声明长度由网关在读体前判定，这里只负责「声明未超但文件超」
//!   这一档：逐块写入并累计判定，超限立刻 413，并把临时文件与目录一并删掉；
//! - P6：整份上传不得在内存里出现两份。因此全程对着请求体的切片逐块写临时文件，
//!   最后原子改名，既不做 base64，也不整份复制。注意正文本身由网关 `custom()` 按
//!   该路由上限一次性缓冲（合同 §6.1.1 第 3 步的规定），因此峰值内存约等于请求体
//!   体积，这里省掉的是「再复制一份」而不是那份缓冲。
//!
//! 关于 `Content-Type`：网关只把 方法、路径、查询串、正文 交给路由处理器，适配器
//! 拿不到请求头。因此「不支持的 Content-Type」按**正文形态**判定：正文不是以
//! `--<分隔符>` 开头的 multipart 报文时回 400 并给出中文原因。

use std::io::Write as _;
use std::path::{Path, PathBuf};

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde_json::json;

use crate::server_path::{attachments_dir, strip_verbatim_prefix};

/// 附件上传路由。
const ATTACHMENTS_PATH: &str = "/api/v1/attachments";
/// 文件本体上限 64 MiB（67108864 字节），与 `/meta.uploadLimitBytes` 同源同值。
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// multipart 分隔符的最大长度，RFC 2046 规定不超过 70 字节。
const MAX_BOUNDARY_BYTES: usize = 70;
/// 写临时文件的块大小：一次 64 KiB，内存占用与文件体积无关。
const WRITE_CHUNK_BYTES: usize = 64 * 1024;
/// 上传字段名。
const FILE_FIELD: &str = "file";
/// 临时文件前缀与后缀；与最终文件名区分开，避免半成品被当成附件。
const TEMP_PREFIX: &str = ".upload-";
/// 临时文件后缀。
const TEMP_SUFFIX: &str = ".part";
/// 文件名收敛失败时的兜底名字。
const FALLBACK_NAME: &str = "attachment";
/// 方法不匹配的错误码。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 正文不是 multipart 的错误码。
const UNSUPPORTED_CONTENT_TYPE: &str = "unsupported_content_type";
/// 请求体形态错误的错误码。
const INVALID_MULTIPART: &str = "invalid_multipart";
/// 缺少 file 段的错误码。
const MISSING_FILE_FIELD: &str = "missing_file_field";
/// 文件超过上限的错误码。
const TOO_LARGE: &str = "too_large";
/// 落盘失败的错误码。
const WRITE_FAILED: &str = "write_failed";

/// 附件上传适配器。
pub(crate) struct AttachmentsHttpAdapter {
    /// Axon 运行时根目录；附件目录位于其下。
    axon_root: PathBuf,
}

impl AttachmentsHttpAdapter {
    /// 以运行时根目录构造。
    pub(crate) fn new(axon_root: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
        }
    }

    /// 解析请求体并把 `file` 段落盘。
    fn upload(&self, body: &[u8]) -> GatewayHttpResponse {
        let Some(boundary) = boundary_of(body) else {
            return GatewayHttpResponse::json(
                400,
                &json!({
                    "error": UNSUPPORTED_CONTENT_TYPE,
                    "message": "请求体不是 multipart/form-data：正文没有以 --<分隔符> 开头",
                }),
            );
        };
        let Some(part) = find_file_part(body, &boundary) else {
            return GatewayHttpResponse::json(
                400,
                &json!({
                    "error": MISSING_FILE_FIELD,
                    "message": format!("请求体里没有名为 {FILE_FIELD} 的 multipart 段"),
                }),
            );
        };
        let Some(original_name) = part.file_name else {
            return GatewayHttpResponse::json(
                400,
                &json!({
                    "error": INVALID_MULTIPART,
                    "message": format!("{FILE_FIELD} 段缺少 filename，无法确定展示名"),
                }),
            );
        };

        // 每次上传一个独立目录：同名附件互不覆盖，落盘路径可直接当标识用。
        let upload_id = uuid::Uuid::new_v4().to_string();
        let directory = attachments_dir(&self.axon_root).join(&upload_id);
        if let Err(error) = std::fs::create_dir_all(&directory) {
            return write_failure("创建附件目录", &error);
        }
        let temp_path = directory.join(format!("{TEMP_PREFIX}{upload_id}{TEMP_SUFFIX}"));
        let file_name = safe_name(&original_name);
        let final_path = directory.join(&file_name);

        let written = match write_in_chunks(&temp_path, part.content) {
            Ok(written) => written,
            Err(UploadFailure::TooLarge { bytes }) => {
                discard(&temp_path, &directory);
                return GatewayHttpResponse::json(
                    413,
                    &json!({
                        "error": TOO_LARGE,
                        "message": format!(
                            "附件超过 {MAX_FILE_BYTES} 字节上传上限（已读到 {bytes} 字节），未落盘"
                        ),
                        "limit": MAX_FILE_BYTES,
                    }),
                );
            }
            Err(UploadFailure::Write(error)) => {
                discard(&temp_path, &directory);
                return write_failure("写入附件", &error);
            }
        };
        // 改名在同一目录内完成，因此是原子的：要么没有这个附件，要么是完整的一份。
        if let Err(error) = std::fs::rename(&temp_path, &final_path) {
            discard(&temp_path, &directory);
            return write_failure("重命名附件", &error);
        }
        let display_path = strip_verbatim_prefix(&final_path.to_string_lossy()).to_owned();
        GatewayHttpResponse::json(
            200,
            &json!({
                "path": display_path,
                "name": original_name,
                "bytes": written,
                "image": mime_of(&final_path).starts_with("image/"),
            }),
        )
    }
}

impl GatewayHttpRouteHandler for AttachmentsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        if path != ATTACHMENTS_PATH {
            return None;
        }
        if method != "POST" {
            return Some(GatewayHttpResponse::json(
                405,
                &json!({ "error": METHOD_NOT_ALLOWED }),
            ));
        }
        Some(self.upload(body))
    }
}

/// 写临时文件时的失败分类。
enum UploadFailure {
    /// 超过文件本体上限，`bytes` 是已写入的字节数。
    TooLarge { bytes: u64 },
    /// 其它 IO 失败。
    Write(std::io::Error),
}

/// 逐块写临时文件并累计判定上限。
///
/// 上限在写入过程中判定而不是先看声明长度：文件字节数只能边读边算，这里就是那条边界。
fn write_in_chunks(path: &Path, content: &[u8]) -> Result<u64, UploadFailure> {
    let mut file = std::fs::File::create(path).map_err(UploadFailure::Write)?;
    let mut written: u64 = 0;
    for chunk in content.chunks(WRITE_CHUNK_BYTES) {
        written = written.saturating_add(chunk.len() as u64);
        if written > MAX_FILE_BYTES {
            return Err(UploadFailure::TooLarge { bytes: written });
        }
        file.write_all(chunk).map_err(UploadFailure::Write)?;
    }
    file.flush().map_err(UploadFailure::Write)?;
    Ok(written)
}

/// 清理半成品：先删临时文件，再删这次上传的目录，删不掉也不覆盖原始错误。
fn discard(temp_path: &Path, directory: &Path) {
    let _ = std::fs::remove_file(temp_path);
    let _ = std::fs::remove_dir(directory);
}

/// multipart 报文里的一段。
struct Part<'a> {
    /// 段正文。
    content: &'a [u8],
    /// 段声明的文件名；非文件段为 `None`。
    file_name: Option<String>,
}

/// 从报文首行取出分隔符。
fn boundary_of(body: &[u8]) -> Option<Vec<u8>> {
    if !body.starts_with(b"--") {
        return None;
    }
    let line_end = find(body, b"\r\n", 2)?;
    let boundary = body.get(2..line_end)?;
    if boundary.is_empty() || boundary.len() > MAX_BOUNDARY_BYTES {
        return None;
    }
    if !boundary
        .iter()
        .all(|value| value.is_ascii_graphic() && *value != b'"')
    {
        return None;
    }
    Some(boundary.to_vec())
}

/// 找出名为 `file` 的那一段。
fn find_file_part<'a>(body: &'a [u8], boundary: &[u8]) -> Option<Part<'a>> {
    let mut delimiter = b"\r\n--".to_vec();
    delimiter.extend_from_slice(boundary);
    let first_line_end = find(body, b"\r\n", 2)?;
    let mut cursor = first_line_end + 2;
    loop {
        let header_end = find(body, b"\r\n\r\n", cursor)?;
        let headers = body.get(cursor..header_end)?;
        let content_start = header_end + 4;
        let content_end = find(body, &delimiter, content_start)?;
        let content = body.get(content_start..content_end)?;
        let (name, file_name) = part_headers(headers);
        // 命中字段名即返回，缺 filename 由调用方给出专门的中文原因。
        if name.as_deref() == Some(FILE_FIELD) {
            return Some(Part { content, file_name });
        }
        // 跳过 `\r\n` + 分隔符；分隔符之后若是 `--` 则报文到此结束。
        let next = content_end + delimiter.len();
        if body.get(next..next + 2)? == b"--" {
            return None;
        }
        cursor = next + 2;
    }
}

/// 解析段头，取出字段名与文件名。
fn part_headers(headers: &[u8]) -> (Option<String>, Option<String>) {
    let text = String::from_utf8_lossy(headers);
    let mut name = None;
    let mut file_name = None;
    for line in text.split("\r\n") {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("content-disposition") {
            continue;
        }
        for parameter in value.split(';').skip(1) {
            let Some((key, value)) = parameter.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').to_owned();
            match key.trim().to_ascii_lowercase().as_str() {
                "name" => name = Some(value),
                "filename" => file_name = Some(value),
                _ => {}
            }
        }
    }
    (name, file_name)
}

/// 在 `haystack` 中从 `from` 起查找 `needle`。
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() || haystack.len() - from < needle.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| offset + from)
}

/// 把文件名收敛成安全的单段名字，去掉任何目录成分。
///
/// 只替换三平台真正非法的字符（`< > : " / \ | ? *` 与控制字符）与结尾的点、空格：
/// 中文、括号、空格都是合法文件名字符，替换掉只会让用户认不出自己的文件。
fn safe_name(name: &str) -> String {
    let trimmed = name.trim();
    let leaf = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .trim_start_matches('.');
    let cleaned: String = leaf
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        FALLBACK_NAME.to_owned()
    } else {
        cleaned
    }
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
        _ => "application/octet-stream",
    }
}

/// 落盘失败。
fn write_failure(action: &str, error: &std::io::Error) -> GatewayHttpResponse {
    GatewayHttpResponse::json(
        500,
        &json!({
            "error": WRITE_FAILED,
            "message": format!("{action}失败：{error}"),
        }),
    )
}

#[cfg(test)]
#[path = "attachments_http_adapter_tests.rs"]
mod attachments_http_adapter_tests;
