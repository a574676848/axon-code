//! 工作区文件与附件经网关的共用客户端。
//!
//! 只有云档案走这里（本机档案直读文件系统，理由见 [`crate::workspace_files`]）：
//! 目录列表 `GET /api/v1/files`，文本与原始字节 `GET /api/v1/files/content`，
//! 目录判定 `GET /api/v1/fs/dirs`，附件上传 `POST /api/v1/attachments`
//! （合同 §6.1 阶段 3）。
//!
//! 本模块只管三件事：拼查询串、发请求、把非 2xx 翻成可执行的中文结论。DTO 组装留在
//! [`crate::workspace_files`] 与 [`crate::conversation::attachment`]，因为「展示用路径」
//! 是界面语义，不是协议语义。
//!
//! 两条降级口径与配置类同源（[`crate::config_gateway`]）：路由缺失（404 且回执是网关的
//! 嵌套错误模型）给「后台服务版本过旧」；连不上、证书失败、401 各给一句可展示的中文
//! 原因，不 panic、不返回伪造数据。
//!
//! 路径边界由后台判定（合同 §6.3.1）：客户端只把 `root` 与 `path` 原样交出，归一化与
//! 越界形态的拒绝都在服务端，这里不实现第二份。
//!
//! 上传侧遵守合同 §15.2 P6：段头与收尾分隔符只有几十字节，文件本体要么是调用方已经
//! 持有的那一片内存，要么从磁盘按块读，整份附件不会在内存里出现两份。

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::connection::endpoint;

/// 目录列表路由。
pub(crate) const FILES_PATH: &str = "/api/v1/files";
/// 文件内容路由。
pub(crate) const FILE_CONTENT_PATH: &str = "/api/v1/files/content";
/// 目录浏览路由。
pub(crate) const FS_DIRS_PATH: &str = "/api/v1/fs/dirs";
/// 附件上传路由。
pub(crate) const ATTACHMENTS_PATH: &str = "/api/v1/attachments";
/// 文本读取的服务端上限，与后台 `files` 适配器同值。
///
/// 后台对超限文本回 413，这里据此改走字节路由再按本值截断：截断可见比整块读不到更有
/// 用，内存占用也仍有上限。
pub(crate) const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

/// 文本模式。
const ENCODING_TEXT: &str = "text";
/// 原始字节模式。
const ENCODING_BASE64: &str = "base64";
/// 上传的独立超时：几十 MiB 在慢链路上会超过通用的 15 秒预算。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// 上传声称的文件上限，与后端 `/meta.uploadLimitBytes` 同源同值。
const UPLOAD_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
/// multipart 段头里允许出现的文件名占位。
const FALLBACK_NAME: &str = "attachment";
/// 回执结构不符时的结论，与配置类的措辞一致。
const UNEXPECTED_BODY: &str = "后台服务回执不是预期的 JSON 结构";

/// 把一段查询参数值做百分号编码。
///
/// 只保留 RFC 3986 的 unreserved 集合，其余一律 `%XX`。服务端用
/// `application/x-www-form-urlencoded` 解析查询串，因此 `&` 与 `#` 不编码会截断参数
/// 边界、`+` 不编码会被解码成空格（路径里合法）、空格与中文按 UTF-8 逐字节编码。
pub(crate) fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    encoded
}

/// 组装查询串：键与值都编码；空参数表返回 None。
fn query_string(pairs: &[(&str, &str)]) -> Option<String> {
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
            .collect::<Vec<String>>()
            .join("&"),
    )
}

/// 发一次带鉴权的 GET，返回状态码与响应体。
///
/// 非 2xx 走 `Ok`：网关拒绝请求时会在响应体里给出中文原因，把状态码当异常抛出会丢掉
/// 那句话（`endpoint::http_agent` 已按这个口径关掉了 `http_status_as_error`）。
fn get(path: &str, pairs: &[(&str, &str)]) -> Result<(u16, String), String> {
    let target = match query_string(pairs) {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let url = crate::gateway::endpoint_at(&target)?;
    let agent = endpoint::http_agent(endpoint::REQUEST_TIMEOUT);
    let mut request = agent.get(&url).header("accept", "application/json");
    if let Some(token) = crate::gateway::api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            Ok((status, body))
        }
        Err(error) => Err(transport_failure(&error)),
    }
}

/// 传输层失败的中文结论。
///
/// 口径与 [`crate::config_gateway`] 逐字对齐：证书、超时、域名、拒连各自一句，因为三者
/// 的修复动作完全不同。那边的判定函数是私有的（该文件不在本阶段可改范围），因此这里
/// 按同一口径复述一遍；两份的措辞必须一起改。
fn transport_failure(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Tls(reason) => format!("证书有问题：{reason}"),
        ureq::Error::TlsRequired => "证书有问题：该地址要求 TLS，但地址写的是明文 http".to_string(),
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => {
            "连不上：网关没有在预算内应答，后台服务可能正忙".to_string()
        }
        ureq::Error::HostNotFound => "连不上：域名解析失败".to_string(),
        ureq::Error::ConnectionFailed => "连不上：连接被拒绝".to_string(),
        ureq::Error::Io(reason) => {
            let text = reason.to_string();
            let lowered = text.to_ascii_lowercase();
            if lowered.contains("certificate") || lowered.contains("tls") {
                format!("证书有问题：{text}")
            } else {
                format!("连不上：{text}")
            }
        }
        other => format!("连不上：{other}"),
    }
}

/// 非 2xx 的中文结论。
///
/// 404 有两种含义，修复动作完全不同，靠回执形状区分：
///
/// - 网关路由缺失：嵌套错误模型 `{"error":{"code":"NOT_FOUND",...}}`，如实报 404；
/// - 文件路由自己的结论：扁平 `{"error":"not_found","message":"..."}`，原样带出原因。
#[must_use]
pub(crate) fn failure_message(status: u16, body: &str) -> String {
    match (status, resource_reason(body)) {
        (404, None) => crate::config_gateway::failure_message(status, body),
        (404, Some(reason)) => format!("目标不存在或不可读：{reason}"),
        (403, Some(reason)) => format!("后台服务拒绝了这次操作：{reason}"),
        (400 | 415 | 422, Some(reason)) => format!("后台服务不接受这组参数：{reason}"),
        (413, Some(reason)) => format!("文件超过后台服务的读取上限：{reason}"),
        _ => crate::config_gateway::failure_message(status, body),
    }
}

/// 取资源级错误的说明：只有扁平错误（`error` 是字符串）才是路由自己的结论。
fn resource_reason(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let code = value.get("error")?.as_str()?;
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    Some(if message.is_empty() {
        code.to_owned()
    } else {
        message.to_owned()
    })
}

/// 把响应体解析成 JSON 值；结构不符时给中文结论。
fn parse_reply(body: &str) -> Result<Value, String> {
    serde_json::from_str(body).map_err(|_| UNEXPECTED_BODY.to_string())
}

/// `/files` 的单个条目。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryReply {
    /// 名称。
    name: String,
    /// 相对根目录的展示路径。
    path: String,
    /// 是否目录。
    is_dir: bool,
}

/// `/files` 的回执。
#[derive(Debug, Deserialize)]
struct ListReply {
    /// 本层条目。
    entries: Vec<EntryReply>,
}

/// `/files/content?encoding=text` 的回执。
#[derive(Debug, Deserialize)]
struct TextReply {
    /// 文本内容。
    text: String,
    /// 是否被后台截断。
    truncated: bool,
}

/// `/files/content?encoding=base64` 的回执。
#[derive(Debug, Deserialize)]
struct Base64Reply {
    /// base64 编码的字节。
    base64: String,
    /// 由扩展名判定的 MIME 类型。
    mime: String,
}

/// 附件上传的回执。
#[derive(Debug, Deserialize)]
pub(crate) struct UploadReply {
    /// 服务端落地后的绝对路径，直接作为提交给会话的附件路径。
    pub(crate) path: String,
    /// 落盘的字节数。
    pub(crate) bytes: u64,
}

/// 列出一层目录。
///
/// # Errors
///
/// 网关不可达、凭据不匹配、后台版本过旧、目标不可读或回执结构不符。
pub(crate) fn list_entries(
    root: &str,
    relative: &str,
) -> Result<Vec<crate::workspace_files::FileEntryDto>, String> {
    let (status, body) = get(FILES_PATH, &[("root", root), ("path", relative)])?;
    if !(200..300).contains(&status) {
        return Err(failure_message(status, &body));
    }
    let reply: ListReply = crate::config_gateway::decode(parse_reply(&body)?)?;
    Ok(reply
        .entries
        .into_iter()
        .map(|entry| crate::workspace_files::FileEntryDto {
            name: entry.name,
            path: entry.path,
            kind: if entry.is_dir { "dir" } else { "file" }.to_string(),
        })
        .collect())
}

/// 读取一个文本文件的内容与截断标记。
///
/// 超文本上限（413）时改走字节路由再按 [`MAX_TEXT_BYTES`] 截断：后台的文本模式对超限
/// 文件直接拒绝，而「截断后仍可读」是既有语义，界面据此显示「内容不完整」。
///
/// # Errors
///
/// 网关不可达、凭据不匹配、后台版本过旧、目标不可读或回执结构不符。
pub(crate) fn fetch_text(root: &str, relative: &str) -> Result<(String, bool), String> {
    let reply = get(
        FILE_CONTENT_PATH,
        &[
            ("root", root),
            ("path", relative),
            ("encoding", ENCODING_TEXT),
        ],
    )?;
    match reply {
        (status, body) if (200..300).contains(&status) => {
            let reply: TextReply = crate::config_gateway::decode(parse_reply(&body)?)?;
            Ok((reply.text, reply.truncated))
        }
        (413, _) => {
            let bytes = fetch_decoded(root, relative)?;
            let cut = bytes.len().min(MAX_TEXT_BYTES);
            let content = String::from_utf8_lossy(&bytes[..cut]).to_string();
            Ok((content, bytes.len() > cut))
        }
        (status, body) => Err(failure_message(status, &body)),
    }
}

/// 读取一个文件的 base64 回执与 MIME 类型。
///
/// # Errors
///
/// 同 [`fetch_text`]；文件超过后台的字节上限时给出上限结论。
pub(crate) fn fetch_bytes(root: &str, relative: &str) -> Result<(String, String), String> {
    let (status, body) = get(
        FILE_CONTENT_PATH,
        &[
            ("root", root),
            ("path", relative),
            ("encoding", ENCODING_BASE64),
        ],
    )?;
    if !(200..300).contains(&status) {
        return Err(failure_message(status, &body));
    }
    let reply: Base64Reply = crate::config_gateway::decode(parse_reply(&body)?)?;
    Ok((reply.base64, reply.mime))
}

/// 读取一个文件的原始字节。
///
/// # Errors
///
/// 同 [`fetch_bytes`]；回执里的 base64 不是合法编码时如实报错。
fn fetch_decoded(root: &str, relative: &str) -> Result<Vec<u8>, String> {
    let (encoded, _mime) = fetch_bytes(root, relative)?;
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| format!("后台回执的字节不是合法的 base64：{error}"))
}

/// 判定一个绝对路径是不是目录。
///
/// 目录路由对「不是目录」（含不存在）回 404，因此 `false` 是后台给出的确定结论而不是
/// 失败；其余非 2xx 与传输失败都按 `Err` 上报，由调用方按不可判定处理。
///
/// # Errors
///
/// 网关不可达、凭据不匹配、后台版本过旧，或后台以 400 / 403 / 5xx 拒绝该路径。
pub(crate) fn is_directory(absolute: &str) -> Result<bool, String> {
    let (status, body) = get(FS_DIRS_PATH, &[("path", absolute)])?;
    match status {
        200..=299 => Ok(true),
        404 => Ok(false),
        _ => Err(failure_message(status, &body)),
    }
}

/// 云档案下的附件预览：经网关取字节，绝不直读本机。
///
/// `limit` 是预览的字节上限；超过即拒绝。上限只能在拿到字节之后判，因为内容路由只
/// 按自己的 12 MiB 收体，预览上限比它更紧。
///
/// # Errors
///
/// 路径无法映射成内容路由的入参、网关失败或回执不是合法 base64。
pub(crate) fn preview_bytes(path: &str, limit: u64) -> Result<Vec<u8>, String> {
    let (root, name) = preview_target(path)?;
    let bytes = fetch_decoded(&root, &name)?;
    if bytes.len() as u64 > limit {
        return Err("图片过大，无法预览".to_string());
    }
    Ok(bytes)
}

/// 把预览路径映射成内容路由的入参：父目录作为根，文件名作为根内路径。
///
/// 云档案下不碰本机文件系统，因此这里只做形态判定：必须是绝对路径、必须带父目录与
/// 文件名，含上级跳转段的一律拒绝（越界判定权在后台，这里挡的是根本映射不出去的形态）。
fn preview_target(path: &str) -> Result<(String, String), String> {
    let normalized = path.trim().replace('\\', "/");
    let is_absolute = normalized.starts_with('/')
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if !is_absolute {
        return Err("云档案下的预览只接受服务端绝对路径".to_string());
    }
    let (parent, name) = normalized
        .rsplit_once('/')
        .ok_or_else(|| "附件路径缺少父目录，云档案下无法定位".to_string())?;
    if name.is_empty() || name == "." || name == ".." {
        return Err("附件路径缺少文件名，云档案下无法定位".to_string());
    }
    if parent.is_empty() {
        return Err("附件路径缺少父目录，云档案下无法定位".to_string());
    }
    if parent.split('/').any(|segment| segment == "..") {
        return Err("附件路径含上级跳转，已拒绝".to_string());
    }
    // 盘根（`C:/a.png`）的父段只有一个盘符：补一个分隔符，后台才认得出这是绝对路径。
    let parent = if parent.len() == 2 && parent.ends_with(':') {
        format!("{parent}/")
    } else {
        parent.to_string()
    };
    Ok((parent, name.to_string()))
}

/// 待上传的字节来源。
pub(crate) enum UploadBody<'a> {
    /// 已经在内存里的字节（粘贴路径：base64 解码后只剩下这一份）。
    Memory(&'a [u8]),
    /// 磁盘上的文件（选择器路径：边读边发，不整份读进内存）。
    Disk {
        /// 已经打开的文件。
        file: std::fs::File,
        /// 文件字节数，用于声明请求体长度。
        len: u64,
    },
}

/// 把一段字节以 multipart 上传为附件。
///
/// `file_name` 是落盘与展示用的文件名，会先收敛成能安全放进段头的形态。
///
/// # Errors
///
/// 网关不可达、凭据不匹配、后台版本过旧、超过上限被拒、写盘失败或回执结构不符。
pub(crate) fn upload_attachment(
    file_name: &str,
    body: UploadBody<'_>,
) -> Result<UploadReply, String> {
    let boundary = format!("axon-{}", uuid::Uuid::new_v4().simple());
    let head = multipart_head(&boundary, file_name);
    let tail = format!("\r\n--{boundary}--\r\n").into_bytes();
    let payload_len = match &body {
        UploadBody::Memory(bytes) => bytes.len() as u64,
        UploadBody::Disk { len, .. } => *len,
    };
    if payload_len > UPLOAD_LIMIT_BYTES {
        return Err(upload_too_large());
    }
    let total = (head.len() as u64)
        .saturating_add(payload_len)
        .saturating_add(tail.len() as u64);

    let url = crate::gateway::endpoint_at(ATTACHMENTS_PATH)?;
    let agent = endpoint::http_agent(UPLOAD_TIMEOUT);
    let mut request = agent
        .post(&url)
        .header("accept", "application/json")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        // 长度必须显式声明：网关对分块传输（无长度）一律回 411，并在读体前按这个
        // 数字挡住声明超限的请求（合同 §6.1.1）。
        .header("content-length", total.to_string());
    if let Some(token) = crate::gateway::api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let mut reader = MultipartReader::new(head, body, tail);
    let (status, text) = match request.send(ureq::SendBody::from_reader(&mut reader)) {
        Ok(response) => {
            let status = response.status().as_u16();
            let text = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, text)
        }
        Err(error) => return Err(transport_failure(&error)),
    };
    if !(200..300).contains(&status) {
        return Err(failure_message(status, &text));
    }
    crate::config_gateway::decode(parse_reply(&text)?)
}

/// 附件超过上限时的固定文案。
fn upload_too_large() -> String {
    format!(
        "附件超过 {} MiB，请改用更小的文件",
        UPLOAD_LIMIT_BYTES / 1024 / 1024
    )
}

/// multipart 段头：字段名固定 `file`，文件名经段头安全化。
fn multipart_head(boundary: &str, file_name: &str) -> Vec<u8> {
    let name = header_safe_name(file_name);
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes()
}

/// 把文件名收敛成能安全放进 multipart 段头的形态。
///
/// 后台按 `;` 与 `=` 切段头参数、按 `"` 剥引号，因此这几个字符与 CR/LF、控制字符都会
/// 改变解析边界，一律换成下划线。中文、空格、括号是合法文件名内容，保留。
fn header_safe_name(file_name: &str) -> String {
    let leaf = file_name
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let cleaned: String = leaf
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '"' | ';' | '=' | '\\') {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        FALLBACK_NAME.to_string()
    } else {
        cleaned
    }
}

/// multipart 请求体的分段游标。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// 段头。
    Head,
    /// 文件本体。
    Body,
    /// 收尾分隔符。
    Tail,
    /// 已读完。
    Done,
}

/// 文件本体的两种来源与各自的读游标。
enum Payload<'a> {
    /// 内存切片与已发出的偏移。
    Memory {
        /// 字节。
        bytes: &'a [u8],
        /// 已发出的字节数。
        offset: usize,
    },
    /// 磁盘文件。
    Disk(std::fs::File),
}

/// multipart 请求体的流式拼装。
///
/// 全程只有一份文件字节：内存来源直接切片输出，磁盘来源按块读，整份请求体从不组装成
/// 一个 `Vec`（合同 §15.2 P6 与 P11）。
struct MultipartReader<'a> {
    /// 段头字节。
    head: Vec<u8>,
    /// 段头已发出的字节数。
    head_offset: usize,
    /// 文件本体。
    payload: Payload<'a>,
    /// 收尾分隔符。
    tail: Vec<u8>,
    /// 收尾分隔符已发出的字节数。
    tail_offset: usize,
    /// 当前分段。
    stage: Stage,
}

impl<'a> MultipartReader<'a> {
    /// 按「段头、本体、收尾」组装一个读取器。
    fn new(head: Vec<u8>, body: UploadBody<'a>, tail: Vec<u8>) -> Self {
        let payload = match body {
            UploadBody::Memory(bytes) => Payload::Memory { bytes, offset: 0 },
            UploadBody::Disk { file, .. } => Payload::Disk(file),
        };
        Self {
            head,
            head_offset: 0,
            payload,
            tail,
            tail_offset: 0,
            stage: Stage::Head,
        }
    }
}

impl Read for MultipartReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            match self.stage {
                Stage::Head => {
                    let written = copy_segment(&self.head, &mut self.head_offset, buf);
                    if written > 0 {
                        return Ok(written);
                    }
                    self.stage = Stage::Body;
                }
                Stage::Body => {
                    let written = match &mut self.payload {
                        Payload::Memory { bytes, offset } => copy_segment(bytes, offset, buf),
                        Payload::Disk(file) => file.read(buf)?,
                    };
                    if written > 0 {
                        return Ok(written);
                    }
                    self.stage = Stage::Tail;
                }
                Stage::Tail => {
                    let written = copy_segment(&self.tail, &mut self.tail_offset, buf);
                    if written > 0 {
                        return Ok(written);
                    }
                    self.stage = Stage::Done;
                }
                Stage::Done => return Ok(0),
            }
        }
    }
}

/// 把 `source` 从 `cursor` 起拷进 `buf`，返回本次拷出的字节数（0 表示已经拷完）。
fn copy_segment(source: &[u8], cursor: &mut usize, buf: &mut [u8]) -> usize {
    let remaining = source.get(*cursor..).unwrap_or_default();
    let written = remaining.len().min(buf.len());
    if written == 0 {
        return 0;
    }
    buf[..written].copy_from_slice(&remaining[..written]);
    *cursor += written;
    written
}
#[cfg(test)]
#[path = "workspace_files_gateway_tests.rs"]
mod tests;
