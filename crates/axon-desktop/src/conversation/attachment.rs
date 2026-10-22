//! 输入区的附件：落地与预览。
//!
//! 粘贴进来的图片与文件必须先成为磁盘上的真实文件，才能作为附件路径提交给网关：
//! 后端把附件投影为 Provider turn 的 `transportContext.files`，图片走 vision 输入，
//! 文件走可读附件。这里只做两件事：把字节落到运行态目录，把图片读回成可渲染的
//! 数据地址；不做任何格式转换，也不改写用户原始文件。

use std::path::{Path, PathBuf};

use axon_config::ConfigLoader;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// 单个附件的字节上限；超过即拒绝，避免把整条 IPC 通道拖垮。
const MAX_ATTACHMENT_BYTES: usize = 16 * 1024 * 1024;
/// 预览回传的字节上限。
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
/// 附件落地目录名。
const ATTACHMENT_DIR: &str = "attachments";

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

/// 记录运行态根目录。
fn axon_root() -> Result<PathBuf, String> {
    ConfigLoader::default_user_axon_root_path()
        .map_err(|error| format!("无法解析 Axon 运行时根目录：{error:?}"))
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

/// 落地一个粘贴进来的附件。
pub fn stage(name: &str, data_base64: &str) -> Result<StagedAttachment, String> {
    let payload = data_base64.trim();
    if payload.is_empty() {
        return Err("附件内容为空".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| format!("附件内容不是合法的 base64：{error}"))?;
    if bytes.is_empty() {
        return Err("附件内容为空".to_string());
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "附件超过 {} MiB，请改用更小的文件",
            MAX_ATTACHMENT_BYTES / 1024 / 1024
        ));
    }

    let file_name = safe_name(name);
    // 过小的图片 Provider 一定会拒（真机回执：Image too small (area=64px)），
    // 在落地阶段就拦住，省掉一次注定失败的 Provider 往返。
    if let Some(mime) = image_mime(Path::new(&file_name)) {
        ensure_usable_image(&file_name, &bytes, mime)?;
    }
    let directory = axon_root()?.join("data").join(ATTACHMENT_DIR);
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
pub fn read_preview(path: &str) -> Result<(Vec<u8>, &'static str), String> {
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
#[tauri::command]
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
#[tauri::command]
pub fn attachment_stage(name: String, data_base64: String) -> Result<StagedAttachment, String> {
    stage(&name, &data_base64)
}

/// Tauri IPC 命令：选择本地文件并直接落地为附件。
///
/// 桌面窗口内不需要把字节绕一圈 base64：选中的就是磁盘上的真实文件，
/// 直接把路径作为附件交出去即可，省掉一次读放大。
#[tauri::command]
pub fn attachment_pick() -> Result<Vec<StagedAttachment>, String> {
    let selected = crate::process::pick_files(None)?;
    selected
        .into_iter()
        .map(|path| adopt(&path))
        .collect::<Result<Vec<_>, _>>()
}

/// 把用户选中的文件登记为附件。
///
/// 附件目录是运行态目录，用户文件不搬移：只做存在性与体积校验，
/// 原路径直接交给后端，避免在本地留下第二份副本。
pub fn adopt(path: &str) -> Result<StagedAttachment, String> {
    let candidate = Path::new(path.trim());
    let metadata =
        std::fs::metadata(candidate).map_err(|error| format!("读取文件失败：{error}"))?;
    if !metadata.is_file() {
        return Err("只能附加文件".to_string());
    }
    if metadata.len() > u64::try_from(MAX_ATTACHMENT_BYTES).unwrap_or(u64::MAX) {
        return Err(format!(
            "文件超过 {} MiB，请改用更小的文件",
            MAX_ATTACHMENT_BYTES / 1024 / 1024
        ));
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
    Ok(StagedAttachment {
        image: image_mime(candidate).is_some(),
        bytes: metadata.len(),
        path: candidate.to_string_lossy().to_string(),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_reduced_to_a_single_safe_segment() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\tmp\\shot.png"), "shot.png");
        assert_eq!(safe_name("   "), "attachment");
        assert!(!safe_name("a:b*c?.png").contains([':', '*', '?']));
    }

    #[test]
    fn display_names_keep_the_user_wording_and_drop_directories() {
        // 落盘名会把全角括号等替换成下划线，但卡片上必须还是用户那个名字。
        assert_eq!(
            display_name("实时语音识别 V2（WebSocket）.md"),
            "实时语音识别 V2（WebSocket）.md"
        );
        assert_eq!(display_name("C:\\tmp\\截图（1）.png"), "截图（1）.png");
        assert_eq!(display_name("   "), "attachment");
    }

    #[test]
    fn staging_rejects_invalid_payloads_before_touching_disk() {
        assert!(stage("a.png", "").is_err());
        assert!(stage("a.png", "不是 base64").is_err());
    }

    /// 造一个只带有效 PNG 头的字节串：宽高按 IHDR 写入。
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    #[test]
    fn image_dimensions_are_read_from_the_header() {
        assert_eq!(
            image_dimensions(&png_bytes(1_200, 800), "image/png"),
            Some((1_200, 800))
        );
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&40_u16.to_le_bytes());
        gif.extend_from_slice(&30_u16.to_le_bytes());
        assert_eq!(image_dimensions(&gif, "image/gif"), Some((40, 30)));
        // JPEG：SOI + SOF0 段，高在前宽在后。
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x02,
            0x58, 0x03, 0x20,
        ];
        assert_eq!(image_dimensions(&jpeg, "image/jpeg"), Some((800, 600)));
    }

    #[test]
    fn tiny_images_are_rejected_before_reaching_the_provider() {
        // 真机回执：8×8 的图被 Provider 拒（Image too small (area=64px)），这里提前拦住。
        let error = ensure_usable_image("tiny.png", &png_bytes(8, 8), "image/png")
            .expect_err("过小的图片必须被拒");
        assert!(error.contains("图片太小"), "{error}");
        assert!(error.contains("8×8"), "{error}");
        assert!(ensure_usable_image("ok.png", &png_bytes(16, 16), "image/png").is_ok());
        // 尺寸读不出来时不拦：宁可交给 Provider 判，也不误杀正常图片。
        assert!(
            ensure_usable_image(
                "vector.svg",
                b"<svg viewBox=\"0 0 4 4\"></svg>",
                "image/svg+xml"
            )
            .is_ok()
        );
    }

    #[test]
    fn preview_serves_any_image_path_and_still_rejects_non_images() {
        assert!(resolve_preview("notes.txt").is_err());
        assert!(resolve_preview("missing.png").is_err());
    }

    #[test]
    fn preview_reads_an_image_outside_the_runtime_root() {
        // 判据只有「图片后缀 + 体积」两条：工作区里的图片与控制台临时目录里的图片
        // 走同一条路径，不再按目录设卡。
        let root = axon_test_support::test_temp_root().join("preview-scope");
        std::fs::create_dir_all(&root).expect("建临时目录");
        let path = root.join("shot.png");
        let bytes = png_bytes(8, 8);
        std::fs::write(&path, &bytes).expect("写临时图片");
        let (resolved, mime) =
            resolve_preview(&path.to_string_lossy()).expect("运行态目录之外的图片必须可预览");
        assert_eq!(mime, "image/png");
        assert_eq!(std::fs::read(&resolved).expect("读回"), bytes);
    }

    #[test]
    fn percent_decoding_round_trips_windows_paths() {
        let encoded = "C%3A%5CUsers%5Cexample%5C.axon%5Cdata%5Ct.png";
        assert_eq!(
            percent_decode(encoded).expect("解码"),
            r"C:\Users\example\.axon\data\t.png"
        );
        assert!(percent_decode("%zz").is_err());
        assert!(percent_decode("%4").is_err());
        assert_eq!(
            path_from_scheme_uri("axon-attachment://local/C%3A%5Ca%5Cb.png").expect("解析"),
            r"C:\a\b.png"
        );
    }
}
