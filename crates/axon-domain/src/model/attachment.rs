//! 附件领域对象。

use crate::AttachmentId;

/// 附件引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRef {
    /// 附件 ID。
    pub id: AttachmentId,
    /// 附件来源。
    pub source: AttachmentSource,
    /// 附件类型。
    pub kind: AttachmentKind,
    /// MIME 类型。
    pub mime_type: String,
    /// 原始字节数。
    pub byte_len: usize,
    /// 内容 hash。
    pub sha256: String,
    /// 可安全内联的预览文本。
    pub preview: Option<String>,
    /// 来源追踪。
    pub source_trace: String,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
    /// 审核状态。
    pub review_state: AttachmentReviewState,
    /// prompt 策略。
    pub prompt_policy: AttachmentPromptPolicy,
}

/// 附件来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentSource {
    /// 本机上传。
    LocalUpload,
    /// 拖拽文件。
    DragDrop,
    /// 剪贴板图片。
    ClipboardImage,
    /// 截图。
    Screenshot,
    /// 远控上传。
    RemoteUpload,
    /// MCP object。
    McpObject,
    /// 工具输出文件。
    ToolOutputFile,
}

/// 附件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// 图片。
    Image,
    /// 文本。
    Text,
    /// 普通文件。
    File,
    /// 二进制。
    Binary,
}

/// 附件敏感级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentSensitivity {
    /// 普通内容。
    Normal,
    /// 敏感内容。
    Sensitive,
}

/// 附件审核状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentReviewState {
    /// 可以作为引用进入后续上下文预算。
    Ready,
    /// 需要本机用户复核。
    NeedsUserReview,
}

/// 附件 prompt 策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentPromptPolicy {
    /// 只允许引用进入上下文。
    RefOnly,
    /// 允许安全预览进入上下文。
    AllowInlinePreview,
    /// 不允许进入 prompt。
    Block,
}
