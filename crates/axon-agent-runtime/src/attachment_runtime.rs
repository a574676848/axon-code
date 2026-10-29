//! 附件入口运行时。

#[path = "attachment_runtime_local.rs"]
mod local;

use std::{
    error::Error,
    fmt, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use axon_crypto::sha256_hex;
use axon_domain::{
    AttachmentId, AttachmentKind, AttachmentPromptPolicy, AttachmentRef, AttachmentReviewState,
    AttachmentSensitivity, AttachmentSource,
};

use crate::{AgentModelLocalFileRef, AgentModelLocalImageRef};

pub use local::{
    AttachmentClipboardImageRecord, AttachmentClipboardImageRequest, AttachmentDragDropEntry,
    AttachmentDragDropEntryKind, AttachmentDragDropManifest, AttachmentLocalUploadRequest,
    AttachmentPathKind, AttachmentRemoteUploadRequest, AttachmentScreenshotRecord,
    AttachmentScreenshotRequest,
};

/// 图片附件最大字节数。
pub const MAX_IMAGE_SIZE: usize = 5 * 1024 * 1024;
/// 文本、PDF 与工具输出附件最大字节数。
pub const MAX_TOOL_OUTPUT: usize = 100 * 1024;
/// 默认 BlobStore TTL，单位毫秒。
pub const BLOB_DEFAULT_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
/// 用户级附件归档目录名。
pub const ATTACHMENT_ARCHIVE_DIR_NAME: &str = "attachments";
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG_MAGIC: &[u8] = b"\xff\xd8\xff";
const GIF87A_MAGIC: &[u8] = b"GIF87a";
const GIF89A_MAGIC: &[u8] = b"GIF89a";
const PDF_MAGIC: &[u8] = b"%PDF-";
const WEBP_RIFF_MAGIC: &[u8] = b"RIFF";
const WEBP_FORMAT_MAGIC: &[u8] = b"WEBP";
const BLOB_DIR_NAME: &str = "blobs";
const PREVIEW_DIR_NAME: &str = "previews";
const BLOB_EXTENSION: &str = "blob";
const SHA256_HEX_LENGTH: usize = 64;
const PREVIEW_EXTENSION: &str = "preview.txt";

/// 附件入口策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentIngressPolicy {
    /// 图片附件最大字节数。
    pub max_image_bytes: usize,
    /// 文本、PDF 与工具输出附件最大字节数。
    pub max_tool_output_bytes: usize,
    /// 上传默认审核状态。
    pub upload_default_review_state: AttachmentReviewState,
    /// 默认 prompt 策略。
    pub default_prompt_policy: AttachmentPromptPolicy,
    /// BlobStore 记录 TTL，单位毫秒。
    pub blob_ttl_ms: Option<u64>,
}

impl Default for AttachmentIngressPolicy {
    fn default() -> Self {
        Self {
            max_image_bytes: MAX_IMAGE_SIZE,
            max_tool_output_bytes: MAX_TOOL_OUTPUT,
            upload_default_review_state: AttachmentReviewState::Ready,
            default_prompt_policy: AttachmentPromptPolicy::RefOnly,
            blob_ttl_ms: Some(BLOB_DEFAULT_TTL_MS),
        }
    }
}

/// 附件入口请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentIngressRequest {
    /// 附件来源。
    pub source: AttachmentSource,
    /// 附件类型。
    pub kind: AttachmentKind,
    /// MIME 类型。
    pub mime_type: String,
    /// 原始字节。
    pub bytes: Vec<u8>,
    /// 来源追踪。
    pub source_trace: String,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
}

/// 附件入口错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentIngressError {
    /// MIME 为空。
    EmptyMimeType,
    /// payload 为空。
    EmptyPayload,
    /// payload 超过配置上限。
    PayloadTooLarge {
        /// 最大字节数。
        limit: usize,
        /// 实际字节数。
        actual: usize,
    },
    /// 附件内容类型不支持。
    UnsupportedContent,
    /// 来源追踪为空。
    EmptySourceTrace,
    /// BlobStore 写入失败。
    BlobStoreWriteFailed {
        /// 错误消息。
        message: String,
    },
}

/// 将已归档附件投影为模型消息可携带的强类型引用时的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentProjectionError {
    /// 附件尚未通过本机复核。
    ReviewRequired,
    /// 附件策略禁止进入 prompt。
    PromptBlocked,
}

impl fmt::Display for AttachmentProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReviewRequired => write!(formatter, "附件尚未通过本机复核"),
            Self::PromptBlocked => write!(formatter, "附件策略禁止进入 prompt"),
        }
    }
}

impl Error for AttachmentProjectionError {}

/// Provider 边界前的统一附件投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentModelAttachmentProjection {
    /// 图片引用。
    Image(AgentModelLocalImageRef),
    /// 文件引用。
    File(AgentModelLocalFileRef),
}

impl fmt::Display for AttachmentIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMimeType => write!(formatter, "附件 MIME 不能为空"),
            Self::EmptyPayload => write!(formatter, "附件内容不能为空"),
            Self::PayloadTooLarge { limit, actual } => {
                write!(
                    formatter,
                    "附件大小超过上限：limit={limit}, actual={actual}"
                )
            }
            Self::UnsupportedContent => write!(formatter, "附件内容类型不支持"),
            Self::EmptySourceTrace => write!(formatter, "附件 source trace 不能为空"),
            Self::BlobStoreWriteFailed { message } => {
                write!(formatter, "附件 BlobStore 写入失败：{message}")
            }
        }
    }
}

impl Error for AttachmentIngressError {}

/// BlobStore 中的附件记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentBlobRecord {
    /// 附件引用。
    pub attachment: AttachmentRef,
    /// blob 文件路径。
    pub blob_path: PathBuf,
    /// preview 文件路径。
    pub preview_path: Option<PathBuf>,
    /// 记录创建时间戳，毫秒。
    pub created_at_ms: u64,
    /// 过期时间戳，毫秒。
    pub expires_at_ms: Option<u64>,
}

/// 本机附件 BlobStore。
#[derive(Debug, Clone)]
pub struct AttachmentBlobStore {
    root: PathBuf,
    records: Vec<AttachmentBlobRecord>,
}

/// 解析用户级附件归档目录：`~/.axon/data/attachments`。
#[must_use]
pub fn default_attachment_archive_root() -> Option<PathBuf> {
    let home = axon_config::user_home_dir().ok()?;
    Some(
        home.join(".axon")
            .join("data")
            .join(ATTACHMENT_ARCHIVE_DIR_NAME),
    )
}

impl AttachmentBlobStore {
    /// 创建 BlobStore。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            records: Vec::new(),
        }
    }

    /// BlobStore 根目录。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 已写入记录。
    #[must_use]
    pub fn records(&self) -> &[AttachmentBlobRecord] {
        &self.records
    }

    /// 按强类型附件引用读取归档正文，并校验长度与 SHA-256。
    pub fn read(&self, attachment: &AttachmentRef) -> io::Result<Vec<u8>> {
        self.read_verified(attachment.byte_len, &attachment.sha256)
    }

    /// 按持久化 receipt 的长度与 SHA-256 读取归档正文。
    pub fn read_verified(&self, byte_len: usize, sha256: &str) -> io::Result<Vec<u8>> {
        if sha256.len() != SHA256_HEX_LENGTH || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "附件 SHA-256 引用无效",
            ));
        }
        let path = self
            .root
            .join(BLOB_DIR_NAME)
            .join(format!("{sha256}.{BLOB_EXTENSION}"));
        let bytes = fs::read(path)?;
        if bytes.len() != byte_len {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "附件归档字节数校验失败",
            ));
        }
        if sha256_hex(&bytes) != sha256 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "附件归档 SHA-256 校验失败",
            ));
        }
        Ok(bytes)
    }

    fn write(
        &mut self,
        attachment: AttachmentRef,
        bytes: &[u8],
        created_at_ms: u64,
        expires_at_ms: Option<u64>,
    ) -> Result<AttachmentBlobRecord, io::Error> {
        let blob_dir = self.root.join(BLOB_DIR_NAME);
        let preview_dir = self.root.join(PREVIEW_DIR_NAME);
        fs::create_dir_all(&blob_dir)?;
        fs::create_dir_all(&preview_dir)?;

        let blob_path = blob_dir.join(format!("{}.{}", attachment.sha256, BLOB_EXTENSION));
        fs::write(&blob_path, bytes)?;

        let preview_path = if let Some(preview) = &attachment.preview {
            let path = preview_dir.join(format!("{}.{}", attachment.id.0, PREVIEW_EXTENSION));
            fs::write(&path, preview.as_bytes())?;
            Some(path)
        } else {
            None
        };

        let record = AttachmentBlobRecord {
            attachment,
            blob_path,
            preview_path,
            created_at_ms,
            expires_at_ms,
        };
        self.records.push(record.clone());
        Ok(record)
    }
}

/// 统一附件入口运行时。
#[derive(Debug, Clone)]
pub struct AttachmentIngressRuntime {
    policy: AttachmentIngressPolicy,
    next_attachment_id: u64,
}

impl AttachmentIngressRuntime {
    /// 将附件统一转换为模型消息投影，供 CLI、TUI 与恢复链复用。
    pub fn project_for_model(
        attachment: &AttachmentRef,
    ) -> Result<AgentModelAttachmentProjection, AttachmentProjectionError> {
        if attachment.review_state != AttachmentReviewState::Ready {
            return Err(AttachmentProjectionError::ReviewRequired);
        }
        if attachment.prompt_policy == AttachmentPromptPolicy::Block {
            return Err(AttachmentProjectionError::PromptBlocked);
        }
        let archive_ref = Some(format!("blobs/{}.blob", attachment.sha256));
        let source_ref = Some(attachment.source_trace.clone());
        match attachment.kind {
            AttachmentKind::Image => Ok(AgentModelAttachmentProjection::Image(
                AgentModelLocalImageRef {
                    path: String::new(),
                    mime_type: attachment.mime_type.clone(),
                    byte_len: Some(attachment.byte_len as u64),
                    sha256: Some(attachment.sha256.clone()),
                    archive_ref,
                    source_ref,
                },
            )),
            AttachmentKind::Text | AttachmentKind::File | AttachmentKind::Binary => Ok(
                AgentModelAttachmentProjection::File(AgentModelLocalFileRef {
                    name: format!("attachment-{}", attachment.id.0),
                    path: None,
                    mime_type: attachment.mime_type.clone(),
                    preview: attachment.preview.clone().unwrap_or_default(),
                    byte_len: Some(attachment.byte_len as u64),
                    sha256: Some(attachment.sha256.clone()),
                    archive_ref,
                    source_ref,
                }),
            ),
        }
    }

    /// 创建附件入口运行时。
    #[must_use]
    pub const fn new(policy: AttachmentIngressPolicy) -> Self {
        Self {
            policy,
            next_attachment_id: 0,
        }
    }

    /// 接收附件并生成 `AttachmentRef`。
    pub fn ingest(
        &mut self,
        request: AttachmentIngressRequest,
    ) -> Result<AttachmentRef, AttachmentIngressError> {
        self.validate(&request)?;
        Ok(self.create_attachment_ref(request))
    }

    /// 接收附件，写入 BlobStore，并返回持久化记录。
    pub fn ingest_with_blob_store(
        &mut self,
        request: AttachmentIngressRequest,
        blob_store: &mut AttachmentBlobStore,
        received_at_ms: u64,
    ) -> Result<AttachmentBlobRecord, AttachmentIngressError> {
        self.validate(&request)?;
        let bytes = request.bytes.clone();
        let attachment = self.create_attachment_ref(request);
        let expires_at_ms = self
            .policy
            .blob_ttl_ms
            .map(|ttl_ms| received_at_ms.saturating_add(ttl_ms));
        blob_store
            .write(attachment, &bytes, received_at_ms, expires_at_ms)
            .map_err(|error| AttachmentIngressError::BlobStoreWriteFailed {
                message: error.to_string(),
            })
    }

    fn create_attachment_ref(&mut self, request: AttachmentIngressRequest) -> AttachmentRef {
        self.next_attachment_id += 1;
        let byte_len = request.bytes.len();
        let preview = build_preview(
            &request.bytes,
            request.kind,
            self.policy.max_tool_output_bytes,
        );

        AttachmentRef {
            id: AttachmentId(self.next_attachment_id),
            source: request.source,
            kind: request.kind,
            mime_type: request.mime_type,
            byte_len,
            sha256: sha256_hex(&request.bytes),
            preview,
            source_trace: request.source_trace,
            sensitivity: request.sensitivity,
            review_state: review_state_for_source(request.source, &self.policy),
            prompt_policy: self.policy.default_prompt_policy,
        }
    }

    fn validate(&self, request: &AttachmentIngressRequest) -> Result<(), AttachmentIngressError> {
        if request.mime_type.trim().is_empty() {
            return Err(AttachmentIngressError::EmptyMimeType);
        }
        if request.bytes.is_empty() {
            return Err(AttachmentIngressError::EmptyPayload);
        }
        if request.source_trace.trim().is_empty() {
            return Err(AttachmentIngressError::EmptySourceTrace);
        }
        let limits = AttachmentContentLimits::from_bytes(&request.bytes)
            .ok_or(AttachmentIngressError::UnsupportedContent)?;
        if request.bytes.len() > limits.max_bytes(&self.policy) {
            return Err(AttachmentIngressError::PayloadTooLarge {
                limit: limits.max_bytes(&self.policy),
                actual: request.bytes.len(),
            });
        }
        Ok(())
    }
}

impl Default for AttachmentIngressRuntime {
    fn default() -> Self {
        Self::new(AttachmentIngressPolicy::default())
    }
}

fn build_preview(bytes: &[u8], kind: AttachmentKind, limit: usize) -> Option<String> {
    if kind != AttachmentKind::Text || bytes.len() > limit {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachmentContentLimits {
    Image,
    ToolOutput,
}

impl AttachmentContentLimits {
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if is_valid_image(bytes) {
            Some(Self::Image)
        } else if is_valid_pdf(bytes) || is_valid_text(bytes) {
            Some(Self::ToolOutput)
        } else {
            None
        }
    }

    const fn max_bytes(self, policy: &AttachmentIngressPolicy) -> usize {
        match self {
            Self::Image => policy.max_image_bytes,
            Self::ToolOutput => policy.max_tool_output_bytes,
        }
    }
}

fn is_valid_image(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_MAGIC)
        || bytes.starts_with(JPEG_MAGIC)
        || bytes.starts_with(GIF87A_MAGIC)
        || bytes.starts_with(GIF89A_MAGIC)
        || is_valid_webp(bytes)
}

fn is_valid_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12
        && bytes.starts_with(WEBP_RIFF_MAGIC)
        && bytes.get(8..12) == Some(WEBP_FORMAT_MAGIC)
}

fn is_valid_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(PDF_MAGIC)
}

fn is_valid_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
        && !bytes
            .iter()
            .any(|byte| *byte < 0x09 || (*byte > 0x0d && *byte < 0x20))
}

fn review_state_for_source(
    source: AttachmentSource,
    policy: &AttachmentIngressPolicy,
) -> AttachmentReviewState {
    match source {
        AttachmentSource::LocalUpload | AttachmentSource::RemoteUpload => {
            policy.upload_default_review_state
        }
        _ => AttachmentReviewState::Ready,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use axon_domain::{AttachmentKind, AttachmentSensitivity, AttachmentSource};

    use super::{
        AgentModelAttachmentProjection, AttachmentBlobStore, AttachmentIngressRequest,
        AttachmentIngressRuntime,
    };

    #[test]
    fn unsupported_content_is_rejected() {
        let mut runtime = AttachmentIngressRuntime::default();
        let result = runtime.ingest(AttachmentIngressRequest {
            source: AttachmentSource::LocalUpload,
            kind: AttachmentKind::Binary,
            mime_type: "text/plain".to_owned(),
            bytes: vec![0, 159, 146, 150],
            source_trace: "local".to_owned(),
            sensitivity: AttachmentSensitivity::Normal,
        });

        assert!(result.is_err());
    }

    #[test]
    fn archived_attachment_projects_to_provider_reference() {
        let mut runtime = AttachmentIngressRuntime::default();
        let attachment = runtime
            .ingest(AttachmentIngressRequest {
                source: AttachmentSource::ClipboardImage,
                kind: AttachmentKind::Image,
                mime_type: "image/png".to_owned(),
                bytes: png_1x1_bytes(),
                source_trace: "clipboard://1".to_owned(),
                sensitivity: AttachmentSensitivity::Normal,
            })
            .unwrap();
        let projection = AttachmentIngressRuntime::project_for_model(&attachment).unwrap();
        let AgentModelAttachmentProjection::Image(image) = projection else {
            panic!("图片必须投影为图片引用");
        };
        let archive_ref = format!("blobs/{}.blob", attachment.sha256);
        assert_eq!(image.archive_ref.as_deref(), Some(archive_ref.as_str()));
        assert_eq!(image.source_ref.as_deref(), Some("clipboard://1"));
    }

    #[test]
    fn file_without_preview_keeps_archive_reference() {
        let mut runtime = AttachmentIngressRuntime::default();
        let attachment = runtime
            .ingest(AttachmentIngressRequest {
                source: AttachmentSource::ToolOutputFile,
                kind: AttachmentKind::Binary,
                mime_type: "application/pdf".to_owned(),
                bytes: b"%PDF-1.7".to_vec(),
                source_trace: "tool://read".to_owned(),
                sensitivity: AttachmentSensitivity::Normal,
            })
            .unwrap();
        let projection = AttachmentIngressRuntime::project_for_model(&attachment).unwrap();
        let AgentModelAttachmentProjection::File(file) = projection else {
            panic!("普通文件必须投影为文件引用");
        };
        assert!(file.preview.is_empty());
        assert!(file.archive_ref.is_some());
        assert_eq!(file.source_ref.as_deref(), Some("tool://read"));
    }

    #[test]
    fn local_upload_writes_blob_record_preview_and_ttl() {
        let root = unique_test_blob_root("local_upload_writes_blob_record_preview_and_ttl");
        let mut store = AttachmentBlobStore::new(&root);
        let mut runtime = AttachmentIngressRuntime::default();
        let record = runtime
            .ingest_with_blob_store(
                AttachmentIngressRequest {
                    source: AttachmentSource::LocalUpload,
                    kind: AttachmentKind::Text,
                    mime_type: "text/plain".to_owned(),
                    bytes: "本机上传文本".as_bytes().to_vec(),
                    source_trace: "local://pane/1/upload/readme.txt".to_owned(),
                    sensitivity: AttachmentSensitivity::Normal,
                },
                &mut store,
                1_000,
            )
            .expect("本机上传应写入 BlobStore");

        assert!(record.blob_path.exists());
        assert_eq!(
            fs::read(&record.blob_path).unwrap(),
            "本机上传文本".as_bytes()
        );
        assert_eq!(record.attachment.byte_len, "本机上传文本".len());
        assert_eq!(record.attachment.mime_type, "text/plain");
        assert_eq!(
            record.attachment.source_trace,
            "local://pane/1/upload/readme.txt"
        );
        assert_eq!(record.attachment.preview.as_deref(), Some("本机上传文本"));
        assert!(
            record
                .preview_path
                .as_ref()
                .is_some_and(|path| path.exists())
        );
        assert_eq!(
            store.read(&record.attachment).unwrap(),
            "本机上传文本".as_bytes()
        );
        let mut tampered = record.attachment.clone();
        tampered.byte_len += 1;
        assert!(store.read(&tampered).is_err());
        assert_eq!(record.expires_at_ms, Some(604_801_000));
        assert_eq!(store.records().len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archived_attachment_cannot_be_read_from_another_session_root() {
        let source_root = unique_test_blob_root("archive-scope-source");
        let other_root = unique_test_blob_root("archive-scope-other");
        let mut store = AttachmentBlobStore::new(&source_root);
        let mut runtime = AttachmentIngressRuntime::default();
        let record = runtime
            .ingest_with_blob_store(
                AttachmentIngressRequest {
                    source: AttachmentSource::LocalUpload,
                    kind: AttachmentKind::Text,
                    mime_type: "text/plain".to_owned(),
                    bytes: b"session scoped".to_vec(),
                    source_trace: "upload://session-a/file".to_owned(),
                    sensitivity: AttachmentSensitivity::Normal,
                },
                &mut store,
                1,
            )
            .unwrap();
        let other_store = AttachmentBlobStore::new(&other_root);
        assert!(other_store.read(&record.attachment).is_err());
        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(other_root);
    }

    fn png_1x1_bytes() -> Vec<u8> {
        vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0,
            0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
        ]
    }

    fn unique_test_blob_root(name: &str) -> PathBuf {
        axon_test_support::test_temp_root().join(format!("axon-{name}-{}", std::process::id()))
    }
}
