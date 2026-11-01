//! 输入区的附件：落地与预览。
//!
//! 粘贴进来的图片与文件必须先成为真实文件，才能作为附件路径提交给网关：后端把附件
//! 投影为 Provider turn 的 `transportContext.files`，图片走 vision 输入，文件走可读
//! 附件。这里只做两件事：把附件落到可提交的位置，把图片读回成可渲染的数据地址；
//! 不做任何格式转换，也不改写用户原始文件。
//!
//! 落地位置按生效档案分流（长官裁决：本机档案直读、云档案经网关）：
//!
//! - **本机档案**：落到客户端自有根，原路径直接交出去，省掉一次读放大；
//! - **云档案**：本机路径在远端不存在，必须把字节 multipart 上传到
//!   `POST /api/v1/attachments`，用回执里的路径作为提交给会话的附件路径。
//!
//! 上传侧遵守合同 §15.2 P6：base64 先解码成字节并立刻丢掉原串，multipart 按块写，
//! 整份附件不会同时以原文与编码两种形式驻留内存。

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::workspace_files::{FileSource, file_source};
use crate::workspace_files_gateway::UploadBody;

/// 单个附件的字节上限；超过即拒绝，避免把整条 IPC 通道拖垮。
///
/// 与后端 `/meta.uploadLimitBytes` 同源同值（64 MiB），两个上限必须一起改。
const MAX_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;
/// 预览回传的字节上限。
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;

/// 当前生效档案的附件数据来源。
fn current_source() -> FileSource {
    file_source(crate::connection::active_is_cloud())
}

/// 超过上限的固定文案；`subject` 区分「附件」（粘贴）与「文件」（选择）两种入口。
fn too_large(subject: &str) -> String {
    format!(
        "{subject}超过 {} MiB，请改用更小的文件",
        MAX_ATTACHMENT_BYTES / 1024 / 1024
    )
}

/// 附件落地回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedAttachment {
    /// 落地后的绝对路径，直接用于提交。
    pub path: String,
    /// 展示用文件名。
    pub name: String,
    /// 是否为可预览的图片。
    pub image: bool,
    /// 附件字节数；界面用它显示 `Excel · 5KB` 这类体量信息。
    pub bytes: u64,
}

/// 附件落地目录：客户端自有根下的 `attachments/`。
///
/// 附件是**桌面端自己的**数据：它由客户端暂存后随正文提交给网关，不属于后端运行时
/// 目录。改到客户端根之后，云端档案的附件也不会被误当成后端本机文件。
fn attachment_root() -> PathBuf {
    crate::connection::store::attachments_dir()
}

/// 把文件名收敛成安全的单段名字，去掉任何目录成分。
///
/// 附件落地与正文导出共用这一条规则：两处都要「单段、无非法字符」的文件名，
/// 分开实现只会让其中一处漏掉某个平台字符。
///
/// 只替换三平台真正非法的字符（`< > : " / \ | ? *` 与控制字符）与结尾的点、空格：
/// 中文、括号、空格都是合法文件名字符，替换掉只会让用户认不出自己的文件。
pub(crate) fn safe_name(name: &str) -> String {
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
        "attachment".to_string()
    } else {
        cleaned
    }
}

/// 图片扩展名到 MIME 的映射。
fn image_mime(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("svg") => Some("image/svg+xml"),
        _ => None,
    }
}

/// 图片最小边长：低于该值的图对任何视觉模型都没有信息量。
const MIN_IMAGE_SIDE: u32 = 16;
/// 读取图片头部的字节上限：JPEG 的 SOF 段可能靠后，但不会超过这个范围。
const IMAGE_HEADER_BYTES: usize = 64 * 1024;

/// 校验图片像素尺寸，过小直接拒绝。
///
/// Provider 自己也会拒（`Image too small (area=64px), minimum required area: >256px`），
/// 但那是白跑一次往返；尺寸在落地阶段就能读出来，因此在这里 fail fast，并给出中文原因。
/// 尺寸读不出来（例如 SVG 只有 viewBox）时不拦：宁可交给 Provider 判，也不误杀正常图片。
fn ensure_usable_image(name: &str, bytes: &[u8], mime: &str) -> Result<(), String> {
    let Some((width, height)) = image_dimensions(bytes, mime) else {
        return Ok(());
    };
    if width >= MIN_IMAGE_SIDE && height >= MIN_IMAGE_SIDE {
        return Ok(());
    }
    Err(format!(
        "图片太小：{name} 是 {width}×{height}，模型至少需要 {MIN_IMAGE_SIDE}×{MIN_IMAGE_SIDE} 像素"
    ))
}

/// 从图片字节里读出像素尺寸；认不出格式或数据不足时返回 None。
fn image_dimensions(bytes: &[u8], mime: &str) -> Option<(u32, u32)> {
    match mime {
        "image/png" => png_dimensions(bytes),
        "image/gif" => gif_dimensions(bytes),
        "image/bmp" => bmp_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes(slice.try_into().ok()?))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}

fn be_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_be_bytes(slice.try_into().ok()?))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    Some((be_u32(bytes, 16)?, be_u32(bytes, 20)?))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return None;
    }
    Some((u32::from(le_u16(bytes, 6)?), u32::from(le_u16(bytes, 8)?)))
}

fn bmp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"BM") {
        return None;
    }
    let width = le_u32(bytes, 18)?;
    let height = le_u32(bytes, 22)?;
    Some((width, height))
}

/// JPEG：扫到 SOFn 段后，段内第 1 个 u16 是高、第 2 个是宽。
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut offset = 2_usize;
    // SOF 段里要读到 offset+8（宽度低字节），因此这里必须留够 9 个字节。
    while offset + 9 <= bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = bytes[offset + 1];
        // 填充字节与无载荷标记：跳过。
        if marker == 0xff || marker == 0x01 || (0xd0..=0xd9).contains(&marker) {
            offset += 2;
            continue;
        }
        let length = usize::from(be_u16(bytes, offset + 2)?);
        if length < 2 {
            return None;
        }
        let is_start_of_frame = matches!(
            marker,
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf
        );
        if is_start_of_frame {
            let height = u32::from(be_u16(bytes, offset + 5)?);
            let width = u32::from(be_u16(bytes, offset + 7)?);
            return Some((width, height));
        }
        offset += 2 + length;
    }
    None
}

/// WebP：VP8X 给 24 位宽高减一，VP8L 与 VP8 给 14 位宽高。
fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    match bytes.get(12..16)? {
        b"VP8X" => {
            let width = le_u32(bytes, 24)? & 0x00ff_ffff;
            let height = le_u32(bytes, 27)? & 0x00ff_ffff;
            Some((width + 1, height + 1))
        }
        b"VP8L" => {
            let bits = le_u32(bytes, 21)?;
            let width = (bits & 0x3fff) + 1;
            let height = ((bits >> 14) & 0x3fff) + 1;
            Some((width, height))
        }
        b"VP8 " => {
            let width = u32::from(le_u16(bytes, 26)? & 0x3fff);
            let height = u32::from(le_u16(bytes, 28)? & 0x3fff);
            Some((width, height))
        }
        _ => None,
    }
}

/// 读取文件头部字节，用于校验图片尺寸。
fn read_header(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|error| format!("读取文件失败：{error}"))?;
    let mut buffer = vec![0_u8; IMAGE_HEADER_BYTES];
    let mut filled = 0_usize;
    while filled < buffer.len() {
        let read = file
            .read(&mut buffer[filled..])
            .map_err(|error| format!("读取文件失败：{error}"))?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// 把 base64 内容解成字节，并做空值与上限校验。
///
/// 本机与云档案共用这一关：上限、空内容与编码错误三类结论必须一致，否则同一个文件
/// 换个档案就会给出不同的拒绝文案。
fn decode_payload(data_base64: &str) -> Result<Vec<u8>, String> {
    let payload = data_base64.trim();
    if payload.is_empty() {
        return Err("附件内容为空".to_string());
    }
    // 先按编码长度粗判上限：每 4 个 base64 字符对应 3 字节。这一步只是拦住必然被拒的
    // 巨型负载，别为一个注定失败的请求先分配一整块解码缓冲；精确判定在解码之后。
    if payload.len() / 4 * 3 > MAX_ATTACHMENT_BYTES {
        return Err(too_large("附件"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| format!("附件内容不是合法的 base64：{error}"))?;
    if bytes.is_empty() {
        return Err("附件内容为空".to_string());
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(too_large("附件"));
    }
    Ok(bytes)
}

/// 落地一个粘贴进来的附件。
///
/// 云档案下走网关上传（调试桥按命令分派表调用这一条，入参是借用）；桌面窗口的命令另有
/// [`stage_remote_owned`]，那条路径才能在解出字节后立刻丢弃原串。
///
/// # Errors
///
/// 内容为空、base64 非法、超过上限，或云档案下上传失败时返回原因。
pub fn stage(name: &str, data_base64: &str) -> Result<StagedAttachment, String> {
    if current_source() == FileSource::Gateway {
        let bytes = decode_payload(data_base64)?;
        return stage_remote(name, &bytes);
    }
    stage_locally(name, data_base64)
}

/// 本机档案下的附件落地：写进客户端自有根。
fn stage_locally(name: &str, data_base64: &str) -> Result<StagedAttachment, String> {
    let bytes = decode_payload(data_base64)?;

    let file_name = safe_name(name);
    // 过小的图片 Provider 一定会拒（真机回执：Image too small (area=64px)），
    // 在落地阶段就拦住，省掉一次注定失败的 Provider 往返。
    if let Some(mime) = image_mime(Path::new(&file_name)) {
        ensure_usable_image(&file_name, &bytes, mime)?;
    }
    let directory = attachment_root();
    // 一次附件一个子目录：同名文件不会互相覆盖，文件自身的名字也不带任何前缀，
    // 界面按路径取基名显示时看到的就是用户给的那个名字。
    let path = directory
        .join(crate::workspace::generate_uuid())
        .join(&file_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建附件目录失败：{error}"))?;
    }
    std::fs::write(&path, &bytes).map_err(|error| format!("写入附件失败：{error}"))?;

    Ok(StagedAttachment {
        image: image_mime(&path).is_some(),
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        path: path.to_string_lossy().to_string(),
        // 展示用名保留用户原始写法：落盘名要安全，卡片上要认得出来。
        name: display_name(name),
    })
}

/// 云档案下的附件落地：字节上传到远端，用回执路径作为附件路径。
fn stage_remote(name: &str, bytes: &[u8]) -> Result<StagedAttachment, String> {
    let display = display_name(name);
    // 与粘贴路径同一条判据：过小的图片在这里就拒，别等 Provider 回 400。
    if let Some(mime) = image_mime(Path::new(&display)) {
        ensure_usable_image(&display, bytes, mime)?;
    }
    let reply =
        crate::workspace_files_gateway::upload_attachment(&display, UploadBody::Memory(bytes))?;
    Ok(StagedAttachment {
        image: image_mime(Path::new(&display)).is_some(),
        bytes: reply.bytes,
        path: reply.path,
        name: display,
    })
}

/// 云档案下的粘贴落地：持有 base64 串的调用方把所有权交进来。
///
/// 「丢原串」必须发生在发起上传之前，而不是函数结束时：合同 §15.2 P6 不允许附件同时
/// 以原文与编码两种形式驻留内存，留到最后等于让两种形式在整个上传期间并存。
fn stage_remote_owned(name: &str, data_base64: String) -> Result<StagedAttachment, String> {
    let bytes = decode_payload(data_base64.as_str())?;
    drop(data_base64);
    stage_remote(name, &bytes)
}

/// 展示用文件名：去掉目录成分，其余原样保留。
///
/// 与 [`safe_name`] 分工明确：磁盘名必须收敛成安全单段（括号、冒号等会被替换），
/// 而界面显示的是用户自己的文件，被替换过的名字会让人认不出这是哪个文件。
fn display_name(name: &str) -> String {
    let trimmed = name.trim();
    let leaf = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .trim_start_matches('.');
    if leaf.is_empty() {
        "attachment".to_string()
    } else {
        leaf.to_string()
    }
}

/// 校验一个待预览的图片路径，返回真实路径与 MIME。
///
/// 预览按图片路径本身放行，不限定所在目录：对话正文里的 `![说明](路径)`、工具回执里
/// 的截图路径与附件目录都在同一台机器的同一个文件系统上，按目录设卡只会让工作区里的
/// 图看不出来，挡不住任何真实风险。真正收口的是两条硬判据：后缀必须是已知图片类型，
/// 体积必须在预览上限内，读进来的字节只回给本机渲染层。
pub fn resolve_preview(path: &str) -> Result<(PathBuf, &'static str), String> {
    let candidate = Path::new(path.trim());
    let mime = image_mime(candidate).ok_or_else(|| "该附件不是可预览的图片".to_string())?;
    // 规范化同时消掉 `..` 与符号链接，后续读的就是这一个真实路径。
    let canonical = candidate
        .canonicalize()
        .map_err(|error| format!("读取附件信息失败：{error}"))?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|error| format!("读取附件信息失败：{error}"))?;
    if !metadata.is_file() {
        return Err("附件不存在或不是文件".to_string());
    }
    if metadata.len() > MAX_PREVIEW_BYTES {
        return Err("图片过大，无法预览".to_string());
    }
    Ok((canonical, mime))
}

/// 读取一张待预览图片的字节与 MIME。
///
/// 本机档案直读本机文件；云档案经网关取字节。两条分支共用同一个 MIME 判据，返回给
/// 自定义协议与 IPC 的都是同一份事实。
///
/// # Errors
///
/// 不是已知图片类型、目标不存在、超出预览上限，或云档案下网关失败。
pub fn read_preview(path: &str) -> Result<(Vec<u8>, &'static str), String> {
    // 先判类型再取字节：后缀不是已知图片时，两条分支给出的都是同一句话。
    let mime =
        image_mime(Path::new(path.trim())).ok_or_else(|| "该附件不是可预览的图片".to_string())?;
    if current_source() == FileSource::Gateway {
        // 远端附件不在本机，读本机同名文件只会显示成另一张图。
        let bytes = crate::workspace_files_gateway::preview_bytes(path, MAX_PREVIEW_BYTES)?;
        return Ok((bytes, mime));
    }
    let (canonical, mime) = resolve_preview(path)?;
    let bytes = std::fs::read(&canonical).map_err(|error| format!("读取附件失败：{error}"))?;
    Ok((bytes, mime))
}

/// 把一张待预览图片读成可直接渲染的数据地址。
///
/// 自定义协议在各平台的访问形式不一致（Windows 与 macOS/Linux 的宿主形式不同），
/// 而附件的可用性不该依赖这种平台细节；数据地址在所有平台行为一致，
/// 因此桌面窗口走这条路径，字节路由只留给浏览器调试桥。
pub fn read_preview_data_url(path: &str) -> Result<String, String> {
    let (bytes, mime) = read_preview(path)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

/// Tauri IPC 命令：把图片附件读成数据地址。
///
/// 云档案下由 [`read_preview`] 经网关取字节，绝不直读本机文件。
#[tauri::command(async)]
pub fn attachment_preview(path: String) -> Result<String, String> {
    read_preview_data_url(&path)
}

/// 百分号解码。
///
/// 附件路径经 URL 传给渲染层，Windows 路径里的盘符与反斜杠必须编码；
/// 这里只解 `%XX`，不把 `+` 当空格（路径里的 `+` 是合法字符）。
pub fn percent_decode(value: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut chars = value.bytes();
    while let Some(byte) = chars.next() {
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let high = chars.next().ok_or_else(|| "路径编码不完整".to_string())?;
        let low = chars.next().ok_or_else(|| "路径编码不完整".to_string())?;
        let digits = [high, low];
        let text = std::str::from_utf8(&digits).map_err(|_| "路径编码无效".to_string())?;
        let value = u8::from_str_radix(text, 16).map_err(|_| "路径编码无效".to_string())?;
        bytes.push(value);
    }
    String::from_utf8(bytes).map_err(|_| "路径不是合法 UTF-8".to_string())
}

/// 附件预览的自定义协议名。
pub const ATTACHMENT_SCHEME: &str = "axon-attachment";

/// 从自定义协议的请求 URI 里取出附件路径。
pub fn path_from_scheme_uri(uri: &str) -> Result<String, String> {
    let tail = uri
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| "附件地址无效".to_string())?;
    let encoded = tail
        .split_once('/')
        .map(|(_, path)| path)
        .ok_or_else(|| "附件地址缺少路径".to_string())?;
    percent_decode(encoded)
}

/// Tauri IPC 命令：落地一个粘贴进来的附件。
#[tauri::command(async)]
pub fn attachment_stage(name: String, data_base64: String) -> Result<StagedAttachment, String> {
    if current_source() == FileSource::Gateway {
        // 云档案：远端没有本机路径可提交，只能把字节传上去。
        return stage_remote_owned(&name, data_base64);
    }
    stage_locally(&name, &data_base64)
}

/// Tauri IPC 命令：选择本地文件并直接落地为附件。
///
/// 桌面窗口内不需要把字节绕一圈 base64：本机档案下选中的就是磁盘上的真实文件，
/// 直接把路径作为附件交出去即可，省掉一次读放大。
#[tauri::command(async)]
pub fn attachment_pick() -> Result<Vec<StagedAttachment>, String> {
    let selected = crate::process::pick_files(None)?;
    selected
        .into_iter()
        .map(|path| adopt(&path))
        .collect::<Result<Vec<_>, _>>()
}

/// 把用户选中的文件登记为附件。
///
/// 本机档案：附件目录在客户端根，用户文件不搬移，只做存在性与体积校验，原路径直接
/// 交给后端，避免在本地留下第二份副本。云档案：远端拿不到这个路径，必须把文件读出去
/// 上传，且按块读，几十 MiB 的文件不整份驻留内存。
pub fn adopt(path: &str) -> Result<StagedAttachment, String> {
    let candidate = Path::new(path.trim());
    let metadata =
        std::fs::metadata(candidate).map_err(|error| format!("读取文件失败：{error}"))?;
    if !metadata.is_file() {
        return Err("只能附加文件".to_string());
    }
    if metadata.len() > u64::try_from(MAX_ATTACHMENT_BYTES).unwrap_or(u64::MAX) {
        return Err(too_large("文件"));
    }
    let name = candidate
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "attachment".to_string());
    // 与粘贴路径同一条判据：过小的图片在这里就拒，别等 Provider 回 400。
    if let Some(mime) = image_mime(candidate) {
        let header = read_header(candidate)?;
        ensure_usable_image(&name, &header, mime)?;
    }
    if current_source() == FileSource::Gateway {
        let file =
            std::fs::File::open(candidate).map_err(|error| format!("打开文件失败：{error}"))?;
        let reply = crate::workspace_files_gateway::upload_attachment(
            &name,
            UploadBody::Disk {
                file,
                len: metadata.len(),
            },
        )?;
        return Ok(StagedAttachment {
            image: image_mime(candidate).is_some(),
            bytes: reply.bytes,
            path: reply.path,
            name,
        });
    }
    Ok(StagedAttachment {
        image: image_mime(candidate).is_some(),
        bytes: metadata.len(),
        path: candidate.to_string_lossy().to_string(),
        name,
    })
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
