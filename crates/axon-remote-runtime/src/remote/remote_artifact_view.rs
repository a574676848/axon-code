//! 远控受限产物查看。
//!
//! 该模块只把已经存在的安全投影转换为远控可见的 bounded artifact view。
//! 它不读取文件系统、BlobStore、原始命令输出、session transcript 或图片字节。

use axon_domain::{
    AttachmentRef, AttachmentSource, CellId, RemoteAction, RemoteCapabilityPolicy, SessionId,
};

use crate::RemoteOutputRefGrant;

/// 远控产物预览最大字节数。
pub const REMOTE_ARTIFACT_VIEW_MAX_BYTES: usize = 65_536;
/// 截图元数据预览最大字符数。
pub const REMOTE_ARTIFACT_SCREENSHOT_PREVIEW_MAX_CHARS: usize = 160;
/// 产物查看已投影原因。
pub const REMOTE_ARTIFACT_REASON_PROJECTED: &str = "remote_artifact_view:bounded_projected";
/// 原始内容关闭原因。
pub const REMOTE_ARTIFACT_REASON_RAW_CONTENT_CLOSED: &str =
    "remote_artifact_view:raw_content_closed";
/// 进程 IO 能力关闭原因。
pub const REMOTE_ARTIFACT_REASON_PROCESS_IO_CLOSED: &str = "remote_artifact_view:process_io_closed";
/// 直接文件读取关闭原因。
pub const REMOTE_ARTIFACT_REASON_DIRECT_FILE_READ_CLOSED: &str =
    "remote_artifact_view:direct_file_read_closed";

/// 远控可查看的产物类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteArtifactKind {
    /// 代码 diff。
    CodeDiff,
    /// 测试结果。
    TestResult,
    /// 命令输出。
    CommandOutput,
    /// 截图缩略信息。
    Screenshot,
}

/// 远控产物引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteArtifactReference {
    /// 已批准的单条 `OutputRef` grant。
    OutputGrant(RemoteOutputRefGrant),
    /// 截图附件引用。
    ScreenshotAttachment(AttachmentRef),
}

/// 远控产物查看请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteArtifactViewRequest {
    /// 所属 session。
    pub session_id: SessionId,
    /// 所属 RuntimeCell。
    pub cell_id: CellId,
    /// 请求查看的产物类型。
    pub kind: RemoteArtifactKind,
    /// 产物引用。
    pub reference: RemoteArtifactReference,
    /// 请求返回的最大字节数。
    pub requested_max_bytes: usize,
    /// 是否请求原始内容。
    pub request_raw_content: bool,
    /// 当前时间。
    pub now_ms: u64,
}

/// 远控产物查看决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteArtifactViewDecision {
    /// 允许返回 bounded view。
    Allow {
        /// 产物投影。
        view: RemoteArtifactView,
    },
    /// 阻断查看。
    Block {
        /// 阻断原因。
        reason: RemoteArtifactViewBlockReason,
    },
}

/// 远控产物查看阻断原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteArtifactViewBlockReason {
    /// 策略拒绝。
    PolicyDenied,
    /// 引用类型与请求类型不匹配。
    KindMismatch,
    /// session 不匹配。
    SessionMismatch,
    /// RuntimeCell 不匹配。
    CellMismatch,
    /// 请求字节数无效。
    InvalidMaxBytes,
    /// 请求字节数超过安全上限。
    MaxBytesExceeded,
    /// 请求原始内容。
    RawContentDenied,
    /// `OutputRef` grant 已过期。
    GrantExpired,
    /// 截图引用无效。
    InvalidScreenshotRef,
}

/// 远控产物查看权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteArtifactViewPermissions {
    /// 是否允许原始内容。
    pub raw_content_allowed: bool,
    /// 是否允许读取 session transcript。
    pub session_transcript_allowed: bool,
    /// 是否允许直接文件读取。
    pub direct_file_read_allowed: bool,
    /// 是否允许读取原始图片字节。
    pub raw_image_bytes_allowed: bool,
}

impl RemoteArtifactViewPermissions {
    const fn closed() -> Self {
        Self {
            raw_content_allowed: false,
            session_transcript_allowed: false,
            direct_file_read_allowed: false,
            raw_image_bytes_allowed: false,
        }
    }
}

/// 远控产物投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteArtifactView {
    /// 产物 ID。
    pub artifact_id: String,
    /// 产物类型。
    pub kind: RemoteArtifactKind,
    /// 所属 session。
    pub session_id: SessionId,
    /// 所属 RuntimeCell。
    pub cell_id: CellId,
    /// 内容 hash 或 diff hash。
    pub sha256: String,
    /// 来源追踪。
    pub source_trace: String,
    /// 有界预览。
    pub bounded_preview: String,
    /// 本次允许返回的最大字节数。
    pub max_bytes: usize,
    /// 远控权限。
    pub permissions: RemoteArtifactViewPermissions,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控受限产物查看 runtime。
#[derive(Debug, Clone)]
pub struct RemoteArtifactViewRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteArtifactViewRuntime {
    /// 创建 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 生成远控 bounded artifact view。
    #[must_use]
    pub fn view(&self, request: &RemoteArtifactViewRequest) -> RemoteArtifactViewDecision {
        if request.request_raw_content {
            return block(RemoteArtifactViewBlockReason::RawContentDenied);
        }
        if request.requested_max_bytes == 0 {
            return block(RemoteArtifactViewBlockReason::InvalidMaxBytes);
        }
        if request.requested_max_bytes > REMOTE_ARTIFACT_VIEW_MAX_BYTES {
            return block(RemoteArtifactViewBlockReason::MaxBytesExceeded);
        }

        match &request.reference {
            RemoteArtifactReference::OutputGrant(grant) => self.view_output(request, grant),
            RemoteArtifactReference::ScreenshotAttachment(attachment) => {
                self.view_screenshot(request, attachment)
            }
        }
    }

    fn view_output(
        &self,
        request: &RemoteArtifactViewRequest,
        grant: &RemoteOutputRefGrant,
    ) -> RemoteArtifactViewDecision {
        if self
            .policy
            .ensure_allowed(RemoteAction::ReadOutputRef)
            .is_err()
        {
            return block(RemoteArtifactViewBlockReason::PolicyDenied);
        }
        if !matches!(
            request.kind,
            RemoteArtifactKind::TestResult | RemoteArtifactKind::CommandOutput
        ) {
            return block(RemoteArtifactViewBlockReason::KindMismatch);
        }
        if grant.session_id != request.session_id {
            return block(RemoteArtifactViewBlockReason::SessionMismatch);
        }
        if grant.cell_id != request.cell_id {
            return block(RemoteArtifactViewBlockReason::CellMismatch);
        }
        if grant.expires_at_ms < request.now_ms {
            return block(RemoteArtifactViewBlockReason::GrantExpired);
        }
        if request.requested_max_bytes > grant.max_bytes {
            return block(RemoteArtifactViewBlockReason::MaxBytesExceeded);
        }

        allow(RemoteArtifactView {
            artifact_id: format!("output_ref:{}", grant.output_ref_id.0),
            kind: request.kind,
            session_id: request.session_id,
            cell_id: request.cell_id,
            sha256: grant.sha256.clone(),
            source_trace: grant.source_trace.clone(),
            bounded_preview: grant.bounded_preview.clone(),
            max_bytes: request.requested_max_bytes,
            ..closed_view_defaults()
        })
    }

    fn view_screenshot(
        &self,
        request: &RemoteArtifactViewRequest,
        attachment: &AttachmentRef,
    ) -> RemoteArtifactViewDecision {
        if self
            .policy
            .ensure_allowed(RemoteAction::ReadBoundedSummary)
            .is_err()
        {
            return block(RemoteArtifactViewBlockReason::PolicyDenied);
        }
        if request.kind != RemoteArtifactKind::Screenshot {
            return block(RemoteArtifactViewBlockReason::KindMismatch);
        }
        if attachment.source != AttachmentSource::Screenshot {
            return block(RemoteArtifactViewBlockReason::InvalidScreenshotRef);
        }

        allow(RemoteArtifactView {
            artifact_id: format!("attachment:{}", attachment.id.0),
            kind: RemoteArtifactKind::Screenshot,
            session_id: request.session_id,
            cell_id: request.cell_id,
            sha256: attachment.sha256.clone(),
            source_trace: attachment.source_trace.clone(),
            bounded_preview: bounded_chars(
                &screenshot_metadata(attachment),
                REMOTE_ARTIFACT_SCREENSHOT_PREVIEW_MAX_CHARS,
            ),
            max_bytes: request.requested_max_bytes,
            ..closed_view_defaults()
        })
    }
}

fn closed_view_defaults() -> RemoteArtifactView {
    RemoteArtifactView {
        artifact_id: String::new(),
        kind: RemoteArtifactKind::CommandOutput,
        session_id: SessionId(0),
        cell_id: CellId(0),
        sha256: String::new(),
        source_trace: String::new(),
        bounded_preview: String::new(),
        max_bytes: 0,
        permissions: RemoteArtifactViewPermissions::closed(),
        reasons: vec![
            REMOTE_ARTIFACT_REASON_PROJECTED.to_owned(),
            REMOTE_ARTIFACT_REASON_RAW_CONTENT_CLOSED.to_owned(),
            REMOTE_ARTIFACT_REASON_PROCESS_IO_CLOSED.to_owned(),
            REMOTE_ARTIFACT_REASON_DIRECT_FILE_READ_CLOSED.to_owned(),
        ],
    }
}

fn screenshot_metadata(attachment: &AttachmentRef) -> String {
    format!(
        "screenshot:{} mime:{} bytes:{} sha256:{}",
        attachment.id.0, attachment.mime_type, attachment.byte_len, attachment.sha256
    )
}

fn bounded_chars(value: &str, max_chars: usize) -> String {
    let mut output: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

fn allow(view: RemoteArtifactView) -> RemoteArtifactViewDecision {
    RemoteArtifactViewDecision::Allow { view }
}

fn block(reason: RemoteArtifactViewBlockReason) -> RemoteArtifactViewDecision {
    RemoteArtifactViewDecision::Block { reason }
}
