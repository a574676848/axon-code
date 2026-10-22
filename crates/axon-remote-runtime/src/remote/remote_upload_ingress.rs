//! 远控上传入口计划。
//!
//! 该模块只处理远控侧车上传进入附件系统前的策略门禁和候选报告，
//! 不接收网络连接、不写 BlobStore、不把上传原文放入模型上下文。

use axon_domain::{
    AttachmentPromptPolicy, AttachmentReviewState, AttachmentSensitivity, AxonError, AxonResult,
    RemoteAction, RemoteCapabilityPolicy,
};

/// 远控上传 client id 最大字符数。
pub const REMOTE_UPLOAD_CLIENT_ID_MAX_CHARS: usize = 96;
/// 远控上传 id 最大字符数。
pub const REMOTE_UPLOAD_ID_MAX_CHARS: usize = 128;
/// 远控上传文件名最大字符数。
pub const REMOTE_UPLOAD_FILE_NAME_MAX_CHARS: usize = 180;
/// 远控上传 MIME hint 最大字符数。
pub const REMOTE_UPLOAD_MIME_HINT_MAX_CHARS: usize = 120;
/// 远控上传摘要最大字符数。
pub const REMOTE_UPLOAD_SUMMARY_MAX_CHARS: usize = 200;
/// 远控上传候选已创建原因。
pub const REMOTE_UPLOAD_REASON_CANDIDATE_CREATED: &str = "remote_upload:candidate_created";
/// 远控上传只允许 ref-only prompt 原因。
pub const REMOTE_UPLOAD_REASON_REF_ONLY: &str = "remote_upload:ref_only";
/// 远控上传关闭模型原文直入原因。
pub const REMOTE_UPLOAD_REASON_RAW_CONTEXT_CLOSED: &str = "remote_upload:raw_context_closed";

/// 远控上传入口请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUploadIngressRequest {
    /// 远控 client id。
    pub client_id: String,
    /// 上传 id。
    pub upload_id: String,
    /// 文件名。
    pub file_name: String,
    /// MIME hint。
    pub mime_hint: Option<String>,
    /// 上传字节数。
    pub byte_len: usize,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
}

/// 远控上传入口计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUploadIngressPlan {
    /// 远控 client id。
    pub client_id: String,
    /// 上传 id。
    pub upload_id: String,
    /// 文件名。
    pub file_name: String,
    /// MIME hint。
    pub mime_hint: Option<String>,
    /// 上传字节数。
    pub byte_len: usize,
    /// 敏感级别。
    pub sensitivity: AttachmentSensitivity,
    /// 规范 source trace。
    pub source_trace: String,
    /// 默认复核状态。
    pub review_state: AttachmentReviewState,
    /// 默认 prompt 策略。
    pub prompt_policy: AttachmentPromptPolicy,
    /// 是否允许远控直接进入模型上下文。
    pub remote_direct_model_context_allowed: bool,
    /// 是否允许上传原始 bytes 进入 prompt。
    pub raw_bytes_in_prompt_allowed: bool,
    /// 有界摘要。
    pub bounded_summary: String,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控上传入口 runtime。
#[derive(Debug, Clone)]
pub struct RemoteUploadIngressRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteUploadIngressRuntime {
    /// 创建 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 规划远控上传入口。
    ///
    /// # Errors
    ///
    /// 当远控策略不允许上传、元数据为空或超限时返回错误。
    pub fn plan(
        &self,
        request: &RemoteUploadIngressRequest,
    ) -> AxonResult<RemoteUploadIngressPlan> {
        self.policy.ensure_allowed(RemoteAction::UploadAttachment)?;
        validate_text(&request.client_id, REMOTE_UPLOAD_CLIENT_ID_MAX_CHARS)?;
        validate_text(&request.upload_id, REMOTE_UPLOAD_ID_MAX_CHARS)?;
        validate_text(&request.file_name, REMOTE_UPLOAD_FILE_NAME_MAX_CHARS)?;
        if let Some(mime_hint) = &request.mime_hint {
            validate_text(mime_hint, REMOTE_UPLOAD_MIME_HINT_MAX_CHARS)?;
        }
        if request.byte_len == 0 {
            return Err(AxonError::EmptyInput);
        }

        let source_trace = remote_upload_source_trace(&request.client_id, &request.upload_id);
        let bounded_summary = bounded_summary(request);
        Ok(RemoteUploadIngressPlan {
            client_id: request.client_id.clone(),
            upload_id: request.upload_id.clone(),
            file_name: request.file_name.clone(),
            mime_hint: request.mime_hint.clone(),
            byte_len: request.byte_len,
            sensitivity: request.sensitivity,
            source_trace,
            review_state: AttachmentReviewState::Ready,
            prompt_policy: AttachmentPromptPolicy::RefOnly,
            remote_direct_model_context_allowed: false,
            raw_bytes_in_prompt_allowed: false,
            bounded_summary,
            reasons: vec![
                REMOTE_UPLOAD_REASON_CANDIDATE_CREATED.to_owned(),
                REMOTE_UPLOAD_REASON_REF_ONLY.to_owned(),
                REMOTE_UPLOAD_REASON_RAW_CONTEXT_CLOSED.to_owned(),
            ],
        })
    }
}

fn validate_text(value: &str, limit: usize) -> AxonResult<()> {
    let actual = value.chars().count();
    if value.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    if actual > limit {
        return Err(AxonError::InputTooLong { limit, actual });
    }
    Ok(())
}

fn remote_upload_source_trace(client_id: &str, upload_id: &str) -> String {
    format!("remote://client/{client_id}/upload/{upload_id}")
}

fn bounded_summary(request: &RemoteUploadIngressRequest) -> String {
    let mime = request.mime_hint.as_deref().unwrap_or("unknown");
    let summary = format!(
        "remote upload client={} upload={} file={} mime={} bytes={} sensitivity={:?}",
        request.client_id,
        request.upload_id,
        request.file_name,
        mime,
        request.byte_len,
        request.sensitivity
    );
    summary
        .chars()
        .take(REMOTE_UPLOAD_SUMMARY_MAX_CHARS)
        .collect()
}
