//! 本机附件入口、MIME sniffing 与 drag/drop manifest。

use axon_domain::{AttachmentKind, AttachmentSensitivity, AttachmentSource, PaneId};

use crate::{
    AttachmentBlobRecord, AttachmentBlobStore, AttachmentIngressError, AttachmentIngressRequest,
    AttachmentIngressRuntime,
};

const MIME_IMAGE_PNG: &str = "image/png";
const MIME_IMAGE_JPEG: &str = "image/jpeg";
const MIME_IMAGE_WEBP: &str = "image/webp";
const MIME_TEXT_PLAIN: &str = "text/plain";
const MIME_APPLICATION_PDF: &str = "application/pdf";
const MIME_APPLICATION_OCTET_STREAM: &str = "application/octet-stream";
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG_MAGIC: &[u8] = b"\xff\xd8\xff";
const PDF_MAGIC: &[u8] = b"%PDF-";
const WEBP_RIFF_MAGIC: &[u8] = b"RIFF";
const WEBP_FORMAT_MAGIC: &[u8] = b"WEBP";
const PNG_IHDR_OFFSET: usize = 16;
const JPEG_START_OF_FRAME_BASELINE: u8 = 0xc0;
const JPEG_START_OF_FRAME_PROGRESSIVE: u8 = 0xc2;
const WEBP_VP8X_CHUNK: &[u8] = b"VP8X";

/// 本机上传请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentLocalUploadRequest {
    /// 文件名。
    pub file_name: String,
    /// MIME hint。
    pub mime_hint: Option<String>,
    /// 原始字节。
    pub bytes: Vec<u8>,
    /// 来源追踪。
    pub source_trace: String,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
}

/// 剪贴板图片入口请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentClipboardImageRequest {
    /// 剪贴板内容 ID。
    pub content_id: String,
    /// MIME hint。
    pub mime_hint: Option<String>,
    /// 图片原始字节。
    pub bytes: Vec<u8>,
    /// 来源追踪。
    pub source_trace: String,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
}

/// 剪贴板图片写入结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentClipboardImageRecord {
    /// BlobStore 记录。
    pub blob: AttachmentBlobRecord,
    /// 图片格式。
    pub image_format: String,
    /// 宽度。
    pub width: u32,
    /// 高度。
    pub height: u32,
    /// 剪贴板内容 ID。
    pub content_id: String,
}

/// 截图入口请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentScreenshotRequest {
    /// 窗口 ID。
    pub window_id: u64,
    /// 会话 ID。
    pub session_id: String,
    /// pane ID。
    pub pane_id: PaneId,
    /// MIME hint。
    pub mime_hint: Option<String>,
    /// 截图原始字节。
    pub bytes: Vec<u8>,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
}

/// 截图写入结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentScreenshotRecord {
    /// BlobStore 记录。
    pub blob: AttachmentBlobRecord,
    /// 图片格式。
    pub image_format: String,
    /// 宽度。
    pub width: u32,
    /// 高度。
    pub height: u32,
    /// 窗口 ID。
    pub window_id: u64,
    /// 会话 ID。
    pub session_id: String,
    /// pane ID。
    pub pane_id: PaneId,
}

/// 远控上传请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRemoteUploadRequest {
    /// 远控 client ID。
    pub client_id: String,
    /// 上传 ID。
    pub upload_id: String,
    /// 文件名。
    pub file_name: String,
    /// MIME hint。
    pub mime_hint: Option<String>,
    /// 原始字节。
    pub bytes: Vec<u8>,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
}

/// drag/drop 路径类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentPathKind {
    /// 文件。
    File,
    /// 目录。
    Directory,
}

/// drag/drop manifest。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentDragDropManifest {
    /// 条目。
    pub entries: Vec<AttachmentDragDropEntry>,
    /// 是否允许递归读取目录。
    pub recursive_read_allowed: bool,
}

/// drag/drop manifest 条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentDragDropEntry {
    /// 路径。
    pub path: String,
    /// 条目类型。
    pub kind: AttachmentDragDropEntryKind,
}

/// drag/drop manifest 条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentDragDropEntryKind {
    /// 文件引用，可进入后续显式读取。
    FileRef,
    /// 目录 manifest，不递归读取原文。
    DirectoryManifest,
}

impl AttachmentIngressRuntime {
    /// 接收本机上传，按 bytes 与扩展名 sniff MIME 后写入 BlobStore。
    pub fn ingest_local_upload_with_blob_store(
        &mut self,
        request: AttachmentLocalUploadRequest,
        blob_store: &mut AttachmentBlobStore,
        received_at_ms: u64,
    ) -> Result<AttachmentBlobRecord, AttachmentIngressError> {
        let mime_type = sniff_mime_type(
            request.mime_hint.as_deref(),
            &request.file_name,
            &request.bytes,
        );
        let kind = attachment_kind_for_mime(&mime_type);
        self.ingest_with_blob_store(
            AttachmentIngressRequest {
                source: AttachmentSource::LocalUpload,
                kind,
                mime_type,
                bytes: request.bytes,
                source_trace: request.source_trace,
                sensitivity: request.sensitivity,
            },
            blob_store,
            received_at_ms,
        )
    }

    /// 接收剪贴板图片，保留格式、尺寸、来源 trace 与 BlobStore 记录。
    pub fn ingest_clipboard_image_with_blob_store(
        &mut self,
        request: AttachmentClipboardImageRequest,
        blob_store: &mut AttachmentBlobStore,
        received_at_ms: u64,
    ) -> Result<AttachmentClipboardImageRecord, AttachmentIngressError> {
        let mime_type = sniff_mime_type(
            request.mime_hint.as_deref(),
            &request.content_id,
            &request.bytes,
        );
        let dimensions = image_dimensions(&mime_type, &request.bytes);
        let blob = self.ingest_with_blob_store(
            AttachmentIngressRequest {
                source: AttachmentSource::ClipboardImage,
                kind: AttachmentKind::Image,
                mime_type: mime_type.clone(),
                bytes: request.bytes,
                source_trace: request.source_trace,
                sensitivity: request.sensitivity,
            },
            blob_store,
            received_at_ms,
        )?;
        let (width, height) = dimensions.unwrap_or((0, 0));
        Ok(AttachmentClipboardImageRecord {
            blob,
            image_format: mime_type,
            width,
            height,
            content_id: request.content_id,
        })
    }

    /// 接收截图，绑定 window/session/pane source trace 并写入 BlobStore。
    pub fn ingest_screenshot_with_blob_store(
        &mut self,
        request: AttachmentScreenshotRequest,
        blob_store: &mut AttachmentBlobStore,
        received_at_ms: u64,
    ) -> Result<AttachmentScreenshotRecord, AttachmentIngressError> {
        let mime_type = sniff_mime_type(request.mime_hint.as_deref(), "screenshot", &request.bytes);
        let dimensions = image_dimensions(&mime_type, &request.bytes);
        let source_trace =
            screenshot_source_trace(request.window_id, &request.session_id, request.pane_id);
        let blob = self.ingest_with_blob_store(
            AttachmentIngressRequest {
                source: AttachmentSource::Screenshot,
                kind: AttachmentKind::Image,
                mime_type: mime_type.clone(),
                bytes: request.bytes,
                source_trace,
                sensitivity: request.sensitivity,
            },
            blob_store,
            received_at_ms,
        )?;
        let (width, height) = dimensions.unwrap_or((0, 0));
        Ok(AttachmentScreenshotRecord {
            blob,
            image_format: mime_type,
            width,
            height,
            window_id: request.window_id,
            session_id: request.session_id,
            pane_id: request.pane_id,
        })
    }

    /// 接收远控上传，默认进入本机复核且只允许引用进入上下文。
    pub fn ingest_remote_upload_with_blob_store(
        &mut self,
        request: AttachmentRemoteUploadRequest,
        blob_store: &mut AttachmentBlobStore,
        received_at_ms: u64,
    ) -> Result<AttachmentBlobRecord, AttachmentIngressError> {
        let mime_type = sniff_mime_type(
            request.mime_hint.as_deref(),
            &request.file_name,
            &request.bytes,
        );
        let kind = attachment_kind_for_mime(&mime_type);
        self.ingest_with_blob_store(
            AttachmentIngressRequest {
                source: AttachmentSource::RemoteUpload,
                kind,
                mime_type,
                bytes: request.bytes,
                source_trace: remote_upload_source_trace(&request.client_id, &request.upload_id),
                sensitivity: request.sensitivity,
            },
            blob_store,
            received_at_ms,
        )
    }

    /// 生成 drag/drop manifest；目录只登记 manifest，不递归读取原文。
    #[must_use]
    pub fn build_drag_drop_manifest(
        entries: Vec<(String, AttachmentPathKind)>,
    ) -> Option<AttachmentDragDropManifest> {
        if entries.is_empty() {
            return None;
        }
        Some(AttachmentDragDropManifest {
            entries: entries
                .into_iter()
                .map(|(path, kind)| AttachmentDragDropEntry {
                    path,
                    kind: match kind {
                        AttachmentPathKind::File => AttachmentDragDropEntryKind::FileRef,
                        AttachmentPathKind::Directory => {
                            AttachmentDragDropEntryKind::DirectoryManifest
                        }
                    },
                })
                .collect(),
            recursive_read_allowed: false,
        })
    }
}

fn sniff_mime_type(mime_hint: Option<&str>, file_name: &str, bytes: &[u8]) -> String {
    if bytes.starts_with(PNG_MAGIC) {
        return MIME_IMAGE_PNG.to_owned();
    }
    if bytes.starts_with(JPEG_MAGIC) {
        return MIME_IMAGE_JPEG.to_owned();
    }
    if is_webp(bytes) {
        return MIME_IMAGE_WEBP.to_owned();
    }
    if bytes.starts_with(PDF_MAGIC) {
        return MIME_APPLICATION_PDF.to_owned();
    }
    if is_likely_utf8_text(bytes) {
        return MIME_TEXT_PLAIN.to_owned();
    }
    if let Some(mime_hint) = mime_hint.filter(|value| !value.trim().is_empty()) {
        return mime_hint.trim().to_owned();
    }
    mime_from_extension(file_name)
        .unwrap_or(MIME_APPLICATION_OCTET_STREAM)
        .to_owned()
}

fn attachment_kind_for_mime(mime_type: &str) -> AttachmentKind {
    if mime_type.starts_with("image/") {
        AttachmentKind::Image
    } else if mime_type.starts_with("text/") {
        AttachmentKind::Text
    } else if mime_type == MIME_APPLICATION_PDF {
        AttachmentKind::File
    } else {
        AttachmentKind::Binary
    }
}

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12
        && bytes.starts_with(WEBP_RIFF_MAGIC)
        && bytes.get(8..12) == Some(WEBP_FORMAT_MAGIC)
}

fn is_likely_utf8_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
        && !bytes
            .iter()
            .any(|byte| *byte < 0x09 || (*byte > 0x0d && *byte < 0x20))
}

fn mime_from_extension(file_name: &str) -> Option<&'static str> {
    let extension = file_name.rsplit_once('.')?.1.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some(MIME_IMAGE_PNG),
        "jpg" | "jpeg" => Some(MIME_IMAGE_JPEG),
        "webp" => Some(MIME_IMAGE_WEBP),
        "txt" | "md" | "rs" | "json" | "toml" => Some(MIME_TEXT_PLAIN),
        "pdf" => Some(MIME_APPLICATION_PDF),
        _ => None,
    }
}

fn image_dimensions(mime_type: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    match mime_type {
        MIME_IMAGE_PNG => png_dimensions(bytes),
        MIME_IMAGE_JPEG => jpeg_dimensions(bytes),
        MIME_IMAGE_WEBP => webp_dimensions(bytes),
        _ => None,
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let width = u32::from_be_bytes(
        bytes
            .get(PNG_IHDR_OFFSET..PNG_IHDR_OFFSET + 4)?
            .try_into()
            .ok()?,
    );
    let height = u32::from_be_bytes(
        bytes
            .get(PNG_IHDR_OFFSET + 4..PNG_IHDR_OFFSET + 8)?
            .try_into()
            .ok()?,
    );
    Some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut index = 2;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            return None;
        }
        let marker = bytes[index + 1];
        let length = u16::from_be_bytes(bytes.get(index + 2..index + 4)?.try_into().ok()?) as usize;
        if marker == JPEG_START_OF_FRAME_BASELINE || marker == JPEG_START_OF_FRAME_PROGRESSIVE {
            let height = u16::from_be_bytes(bytes.get(index + 5..index + 7)?.try_into().ok()?);
            let width = u16::from_be_bytes(bytes.get(index + 7..index + 9)?.try_into().ok()?);
            return Some((u32::from(width), u32::from(height)));
        }
        if length < 2 {
            return None;
        }
        index = index.saturating_add(2 + length);
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(12..16) != Some(WEBP_VP8X_CHUNK) {
        return None;
    }
    let width = 1 + read_u24_le(bytes.get(24..27)?)?;
    let height = 1 + read_u24_le(bytes.get(27..30)?)?;
    Some((width, height))
}

fn read_u24_le(bytes: &[u8]) -> Option<u32> {
    Some(
        u32::from(bytes.first().copied()?)
            | (u32::from(bytes.get(1).copied()?) << 8)
            | (u32::from(bytes.get(2).copied()?) << 16),
    )
}

fn screenshot_source_trace(window_id: u64, session_id: &str, pane_id: PaneId) -> String {
    format!(
        "screenshot://window/{window_id}/session/{session_id}/pane/{}",
        pane_id.0
    )
}

fn remote_upload_source_trace(client_id: &str, upload_id: &str) -> String {
    format!("remote://client/{client_id}/upload/{upload_id}")
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod local_tests;
