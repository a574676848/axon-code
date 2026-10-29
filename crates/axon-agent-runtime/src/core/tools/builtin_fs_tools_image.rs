//! `read_file` 的图片格式判定。
//!
//! 图片是 `read_file` 的一个内容分支：扩展名只作声明，内容签名才是事实；
//! provider 请求阶段同样按签名重建 data URL，两处判定必须一致。

use std::path::Path;

use crate::{AgentRunToolReceiptProjection, ProviderToolCall, agent_run_completed_tool_receipt};

/// `read_file` 接受的图片 MIME。
pub const READ_FILE_IMAGE_FORMATS: [&str; 4] =
    ["image/png", "image/jpeg", "image/webp", "image/gif"];
/// 单张源图字节上限（与附件策略同源）。
pub const READ_FILE_IMAGE_SOURCE_MAX_BYTES: u64 = crate::IMAGE_SOURCE_MAX_BYTES;
/// 单张源图总像素上限。
pub const READ_FILE_IMAGE_SOURCE_MAX_PIXELS: u64 = crate::IMAGE_SOURCE_MAX_PIXELS;
/// 单张源图长边上限。
pub const READ_FILE_IMAGE_SOURCE_MAX_DIMENSION: u32 = crate::IMAGE_SOURCE_MAX_DIMENSION;
/// 归一化后的字节预算。
pub const READ_FILE_IMAGE_NORMALIZED_MAX_BYTES: usize = crate::IMAGE_NORMALIZED_MAX_BYTES;
/// 图片归档来源追踪前缀。
pub const READ_FILE_IMAGE_SOURCE_PREFIX: &str = "tool://builtin.read_file/";
/// 图片格式契约文案，错误与工具描述共用同一份。
pub const READ_FILE_IMAGE_FORMAT_CONTRACT: &str = "PNG/JPEG/WebP/GIF";

const READ_FILE_IMAGE_EXTENSIONS: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
];
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG_SIGNATURE: [u8; 3] = [0xff, 0xd8, 0xff];
const GIF87A_SIGNATURE: &[u8] = b"GIF87a";
const GIF89A_SIGNATURE: &[u8] = b"GIF89a";
const WEBP_RIFF_SIGNATURE: &[u8] = b"RIFF";
const WEBP_FORMAT_SIGNATURE: &[u8] = b"WEBP";
/// 判定签名需要读取的最大头部字节数。
pub const READ_FILE_IMAGE_SNIFF_BYTES: usize = 12;

/// 按扩展名给出图片声明的 MIME；非图片扩展名或无扩展名返回 `None`。
#[must_use]
pub fn image_mime_type_for_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    READ_FILE_IMAGE_EXTENSIONS
        .iter()
        .find(|(candidate, _)| *candidate == extension)
        .map(|(_, mime_type)| *mime_type)
}

/// 按文件签名判定受支持的图片 MIME；非图片内容返回 `None`。
#[must_use]
pub fn sniff_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        return Some("image/png");
    }
    if bytes.starts_with(&JPEG_SIGNATURE) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(GIF87A_SIGNATURE) || bytes.starts_with(GIF89A_SIGNATURE) {
        return Some("image/gif");
    }
    if bytes.starts_with(WEBP_RIFF_SIGNATURE) && bytes.get(8..12) == Some(WEBP_FORMAT_SIGNATURE) {
        return Some("image/webp");
    }
    None
}

/// 一次图片读取的投影输入。
///
/// 只承载已经归一化并归档完成的图片事实；字节来源（归档引用）是投影成功的硬前提。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFileImageReceipt {
    /// 工作区相对展示路径。
    pub path: String,
    /// 规范化绝对路径。
    pub absolute_path: String,
    /// 归一化后的图片 MIME。
    pub mime_type: String,
    /// 归一化后的字节数。
    pub byte_len: usize,
    /// 归一化后的宽度。
    pub width: u32,
    /// 归一化后的高度。
    pub height: u32,
    /// 归一化前的宽度。
    pub original_width: u32,
    /// 归一化前的高度。
    pub original_height: u32,
    /// 图片内容 SHA-256。
    pub sha256: String,
    /// 附件归档相对引用。
    pub archive_ref: String,
    /// 附件来源追踪。
    pub source_ref: String,
}

/// 把图片读取结果投影成 `read_file` 的 provider 可见回执。
///
/// 图片本体走 `localImages`，由运行时投影为紧随工具结果的图片消息；摘要只保留
/// 一行机器可读头部与图片信封，模型据此知道图片已经附上。
#[must_use]
pub fn agent_run_read_file_image_receipt(
    tool_call: &ProviderToolCall,
    receipt: &ReadFileImageReceipt,
) -> AgentRunToolReceiptProjection {
    let metadata = serde_json::json!({
        "kind": "read_file",
        "contentKind": "image",
        "path": receipt.path,
        "absolutePath": receipt.absolute_path,
        "mimeType": receipt.mime_type,
        "byteLen": receipt.byte_len,
        "width": receipt.width,
        "height": receipt.height,
        "originalDimensions": {
            "width": receipt.original_width,
            "height": receipt.original_height,
        },
        "sha256": receipt.sha256,
        "archiveRef": receipt.archive_ref,
        "localImages": [{
            "path": receipt.absolute_path,
            "mimeType": receipt.mime_type,
            "byteLen": receipt.byte_len,
            "width": receipt.width,
            "height": receipt.height,
            "sha256": receipt.sha256,
            "archiveRef": receipt.archive_ref,
            "sourceRef": receipt.source_ref,
        }],
    });
    agent_run_completed_tool_receipt(
        tool_call,
        read_file_image_summary(receipt),
        Some(metadata),
        false,
    )
}

/// 图片回执的模型可见摘要：一行头部加图片信封。
#[must_use]
pub fn read_file_image_summary(receipt: &ReadFileImageReceipt) -> String {
    let scaling =
        if receipt.width == receipt.original_width && receipt.height == receipt.original_height {
            String::new()
        } else {
            let x = f64::from(receipt.original_width) / f64::from(receipt.width.max(1));
            let y = f64::from(receipt.original_height) / f64::from(receipt.height.max(1));
            if format!("{x:.2}") == format!("{y:.2}") {
                format!(
                    "（由 {}x{} 降采样；在原图上定位时坐标乘以 {x:.2}）",
                    receipt.original_width, receipt.original_height
                )
            } else {
                format!(
                    "（由 {}x{} 降采样；在原图上定位时 x 坐标乘以 {x:.2}、y 坐标乘以 {y:.2}）",
                    receipt.original_width, receipt.original_height
                )
            }
        };
    format!(
        "read_file path={} content_kind=image mime={} bytes={} size={}x{}\n<path>{}</path>\n<type>image</type>\n<content>\n{} 图片 {}x{} px，{} 字节{}；图片已随本条工具结果附上，请直接依据图片像素判断。\n</content>",
        receipt.path,
        receipt.mime_type,
        receipt.byte_len,
        receipt.width,
        receipt.height,
        receipt.path,
        receipt.mime_type,
        receipt.width,
        receipt.height,
        receipt.byte_len,
        scaling
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing_recognizes_every_supported_signature() {
        assert_eq!(sniff_image_mime_type(&png_bytes()), Some("image/png"));
        assert_eq!(
            sniff_image_mime_type(&[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_image_mime_type(b"GIF87a\x01\x00"), Some("image/gif"));
        assert_eq!(sniff_image_mime_type(b"GIF89a\x01\x00"), Some("image/gif"));
        assert_eq!(
            sniff_image_mime_type(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(sniff_image_mime_type(b"plain text, not an image"), None);
        assert_eq!(sniff_image_mime_type(&[]), None);
        // RIFF 容器但不是 WEBP（例如 WAVE）不得被误判为图片。
        assert_eq!(sniff_image_mime_type(b"RIFF\x00\x00\x00\x00WAVE"), None);
    }

    #[test]
    fn supported_extensions_map_to_declared_media_types() {
        for (path, expected) in [
            ("shot.png", Some("image/png")),
            ("shot.PNG", Some("image/png")),
            ("photo.jpg", Some("image/jpeg")),
            ("photo.JPEG", Some("image/jpeg")),
            ("anim.gif", Some("image/gif")),
            ("logo.webp", Some("image/webp")),
            ("icon.bmp", None),
            ("archive.tar", None),
            ("no-extension", None),
        ] {
            assert_eq!(
                image_mime_type_for_path(Path::new(path)),
                expected,
                "扩展名解析不符：{path}"
            );
        }
    }

    fn png_bytes() -> Vec<u8> {
        vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0,
            0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
        ]
    }
}
