//! 视觉理解输入门禁。

use axon_domain::{
    AttachmentKind, AttachmentPromptPolicy, AttachmentRef, AttachmentReviewState,
    AttachmentSensitivity, AxonResult, ModelProfile, ModelProfileId, TokenBudget,
};

use crate::{
    ContextBudgetPlan, ContextBudgetRequest, ContextBudgetRuntime, ContextSectionDecision,
    ContextSectionInput, ContextSectionKind, OverflowAction,
};

/// 默认单个图片 ref 进入上下文预算的 token 估算。
pub const DEFAULT_VISION_ATTACHMENT_REF_TOKENS: u64 = 64;
/// 视觉输入不需要工具预算，但领域预算必须为正数。
pub const DEFAULT_VISION_TOOL_RESERVED_TOKENS: u64 = 1;

/// 视觉理解请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionUnderstandingRequest {
    /// 当前模型画像。
    pub model: ModelProfile,
    /// 待理解图片附件。
    pub attachments: Vec<AttachmentRef>,
    /// 上下文预算上限。
    pub total_context_budget: TokenBudget,
    /// 是否已完成本机用户确认。
    pub local_user_confirmed: bool,
}

/// 已准备好的视觉输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedVisionInput {
    /// 模型 ID。
    pub model_id: axon_domain::ModelProfileId,
    /// 只包含引用，不包含图片原文。
    pub attachment_refs: Vec<AttachmentRef>,
    /// 上下文预算计划。
    pub context_budget_plan: ContextBudgetPlan,
    /// 审计摘要。
    pub audit_summary: String,
}

/// 视觉输入阻断原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisionBlockedReason {
    /// 模型不支持视觉。
    ModelCapabilityMissing {
        /// 当前模型声明的视觉能力。
        supports_vision: bool,
    },
    /// 缺少本机确认。
    LocalConfirmationRequired,
    /// 没有附件。
    NoAttachment,
    /// 附件不是图片。
    AttachmentKindUnsupported {
        /// 附件 MIME。
        mime_type: String,
    },
    /// 附件尚未通过本机复核。
    AttachmentNeedsReview {
        /// 附件来源追踪。
        source_trace: String,
    },
    /// 附件策略禁止进入 prompt。
    AttachmentPromptBlocked {
        /// 附件来源追踪。
        source_trace: String,
    },
    /// 附件敏感，需要复核。
    SensitiveAttachmentNeedsReview {
        /// 附件来源追踪。
        source_trace: String,
    },
    /// MIME 不在模型 allowlist。
    MimeUnsupported {
        /// 被拒绝的 MIME。
        mime_type: String,
    },
    /// 单张图片超过模型限制。
    ImageBytesExceeded {
        /// 单图字节上限。
        limit: u64,
        /// 实际字节数。
        actual: usize,
    },
    /// 单次请求总图片字节超过模型限制。
    TotalBytesExceeded {
        /// 总字节上限。
        limit: u64,
        /// 实际总字节数。
        actual: u64,
    },
    /// 上下文预算无法容纳安全策略与附件引用。
    ContextBudgetExceeded,
}

/// 视觉理解 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct VisionUnderstandingRuntime;

/// 视觉 provider 协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionProviderProtocol {
    /// OpenAI-compatible image content part。
    OpenAiCompatible,
    /// Anthropic Messages image content block。
    AnthropicMessages,
}

/// 视觉 provider 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionProviderRequestPlan {
    /// 目标协议。
    pub protocol: VisionProviderProtocol,
    /// 模型 ID。
    pub model_id: ModelProfileId,
    /// 图片引用。
    pub image_refs: Vec<VisionProviderImageRef>,
    /// 是否允许读取图片原始 bytes。
    pub raw_image_bytes_allowed: bool,
    /// 是否允许远控读取 provider 原始 payload。
    pub remote_raw_payload_allowed: bool,
    /// timeline 是否持久化原始图片。
    pub timeline_persists_raw_image: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 视觉 provider 图片引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionProviderImageRef {
    /// 附件 ID。
    pub attachment_id: axon_domain::AttachmentId,
    /// MIME 类型。
    pub mime_type: String,
    /// 字节数。
    pub byte_len: usize,
    /// 来源引用。
    pub source_ref: String,
}

/// 视觉 provider adapter 计划完成原因。
pub const VISION_PROVIDER_REASON_PREPARED_INPUT_ONLY: &str = "vision_provider:prepared_input_only";
/// 视觉 provider adapter 原始图片关闭原因。
pub const VISION_PROVIDER_REASON_RAW_IMAGE_CLOSED: &str = "vision_provider:raw_image_closed";
/// 视觉 provider adapter 原始 payload 关闭原因。
pub const VISION_PROVIDER_REASON_RAW_PAYLOAD_CLOSED: &str = "vision_provider:raw_payload_closed";

/// 视觉 provider adapter runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct VisionProviderAdapterRuntime;

impl VisionUnderstandingRuntime {
    /// 准备视觉输入，输出只包含附件引用和审计信息，不读取图片原文。
    pub fn prepare(
        &self,
        request: VisionUnderstandingRequest,
    ) -> Result<PreparedVisionInput, VisionBlockedReason> {
        if !request.model.supports_vision {
            return Err(VisionBlockedReason::ModelCapabilityMissing {
                supports_vision: false,
            });
        }
        if !request.local_user_confirmed {
            return Err(VisionBlockedReason::LocalConfirmationRequired);
        }
        if request.attachments.is_empty() {
            return Err(VisionBlockedReason::NoAttachment);
        }

        Self::ensure_attachments_allowed(&request.model, &request.attachments)?;
        let plan = Self::plan_context_budget(&request)?;
        if matches!(plan.overflow_action, OverflowAction::BlockUntilUserChooses)
            || plan.sections.iter().any(|section| {
                section.kind == ContextSectionKind::Attachment
                    && section.decision != ContextSectionDecision::KeepRef
            })
        {
            return Err(VisionBlockedReason::ContextBudgetExceeded);
        }

        Ok(PreparedVisionInput {
            model_id: request.model.id,
            attachment_refs: request.attachments,
            context_budget_plan: plan,
            audit_summary: "视觉输入已通过模型能力、附件策略、MIME、预算与本机确认门禁".to_owned(),
        })
    }

    fn ensure_attachments_allowed(
        model: &ModelProfile,
        attachments: &[AttachmentRef],
    ) -> Result<(), VisionBlockedReason> {
        let limits = model.vision_input_limits;
        let mut total_bytes = 0_u64;
        for attachment in attachments {
            if attachment.kind != AttachmentKind::Image {
                return Err(VisionBlockedReason::AttachmentKindUnsupported {
                    mime_type: attachment.mime_type.clone(),
                });
            }
            if attachment.review_state != AttachmentReviewState::Ready {
                return Err(VisionBlockedReason::AttachmentNeedsReview {
                    source_trace: attachment.source_trace.clone(),
                });
            }
            if attachment.prompt_policy == AttachmentPromptPolicy::Block {
                return Err(VisionBlockedReason::AttachmentPromptBlocked {
                    source_trace: attachment.source_trace.clone(),
                });
            }
            if attachment.sensitivity == AttachmentSensitivity::Sensitive {
                return Err(VisionBlockedReason::SensitiveAttachmentNeedsReview {
                    source_trace: attachment.source_trace.clone(),
                });
            }
            if !model.supports_vision_mime(&attachment.mime_type) {
                return Err(VisionBlockedReason::MimeUnsupported {
                    mime_type: attachment.mime_type.clone(),
                });
            }
            if attachment.byte_len as u64 > limits.max_bytes_per_image {
                return Err(VisionBlockedReason::ImageBytesExceeded {
                    limit: limits.max_bytes_per_image,
                    actual: attachment.byte_len,
                });
            }
            total_bytes = total_bytes.saturating_add(attachment.byte_len as u64);
        }

        if total_bytes > limits.max_total_bytes {
            return Err(VisionBlockedReason::TotalBytesExceeded {
                limit: limits.max_total_bytes,
                actual: total_bytes,
            });
        }
        Ok(())
    }

    fn plan_context_budget(
        request: &VisionUnderstandingRequest,
    ) -> Result<ContextBudgetPlan, VisionBlockedReason> {
        let tool_budget = TokenBudget::new(DEFAULT_VISION_TOOL_RESERVED_TOKENS)
            .map_err(|_| VisionBlockedReason::ContextBudgetExceeded)?;
        let sections = request
            .attachments
            .iter()
            .map(|attachment| ContextSectionInput {
                kind: ContextSectionKind::Attachment,
                label: format!("vision:{}", attachment.id.0),
                token_estimate: DEFAULT_VISION_ATTACHMENT_REF_TOKENS,
                summary_token_estimate: DEFAULT_VISION_ATTACHMENT_REF_TOKENS,
                compressible: false,
                ref_only: true,
                source_ref: Some(attachment.source_trace.clone()),
            })
            .collect();

        Ok(ContextBudgetRuntime.plan(ContextBudgetRequest {
            total_limit: request.total_context_budget,
            reserved_for_tools: tool_budget,
            sections,
        }))
    }
}

impl VisionProviderAdapterRuntime {
    /// 从已批准视觉输入构造 provider 请求计划。
    ///
    /// # Errors
    ///
    /// 当视觉输入为空，或调用方传入的输入不再满足 ref-only、本机复核、
    /// 图片类型和 prompt policy 边界时返回错误。
    pub fn build_request_plan(
        &self,
        input: &PreparedVisionInput,
        protocol: VisionProviderProtocol,
    ) -> Result<VisionProviderRequestPlan, VisionBlockedReason> {
        if input.attachment_refs.is_empty() {
            return Err(VisionBlockedReason::NoAttachment);
        }
        let image_refs = input
            .attachment_refs
            .iter()
            .map(provider_image_ref)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(VisionProviderRequestPlan {
            protocol,
            model_id: input.model_id,
            image_refs,
            raw_image_bytes_allowed: false,
            remote_raw_payload_allowed: false,
            timeline_persists_raw_image: false,
            reasons: vec![
                VISION_PROVIDER_REASON_PREPARED_INPUT_ONLY.to_owned(),
                VISION_PROVIDER_REASON_RAW_IMAGE_CLOSED.to_owned(),
                VISION_PROVIDER_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
            ],
        })
    }
}

fn provider_image_ref(
    attachment: &AttachmentRef,
) -> Result<VisionProviderImageRef, VisionBlockedReason> {
    if attachment.kind != AttachmentKind::Image {
        return Err(VisionBlockedReason::AttachmentKindUnsupported {
            mime_type: attachment.mime_type.clone(),
        });
    }
    if attachment.review_state != AttachmentReviewState::Ready {
        return Err(VisionBlockedReason::AttachmentNeedsReview {
            source_trace: attachment.source_trace.clone(),
        });
    }
    if attachment.prompt_policy != AttachmentPromptPolicy::RefOnly {
        return Err(VisionBlockedReason::AttachmentPromptBlocked {
            source_trace: attachment.source_trace.clone(),
        });
    }
    Ok(VisionProviderImageRef {
        attachment_id: attachment.id,
        mime_type: attachment.mime_type.clone(),
        byte_len: attachment.byte_len,
        source_ref: attachment.source_trace.clone(),
    })
}

/// 将视觉阻断原因转为领域错误。
pub fn vision_blocked_error(reason: &VisionBlockedReason) -> AxonResult<()> {
    match reason {
        VisionBlockedReason::ModelCapabilityMissing { .. } => Err(
            axon_domain::AxonError::provider_no_model_route("当前模型不支持视觉输入"),
        ),
        _ => Err(axon_domain::AxonError::runtime_failure(
            "视觉输入未通过本机门禁",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{AttachmentId, AttachmentSource, CostClass, VisionInputLimits};

    #[test]
    fn vision_input_accepts_more_than_four_images_within_byte_budget() {
        let model = ModelProfile {
            id: ModelProfileId(1),
            provider: "fixture".to_owned(),
            model: "vision".to_owned(),
            context_window: 16_384,
            supports_tools: true,
            supports_vision: true,
            vision_input_limits: VisionInputLimits::standard_images(),
            supported_input_mime_types: vec!["image/png".to_owned()],
            cost_class: CostClass::Low,
        };
        let attachments = (0..5)
            .map(|index| AttachmentRef {
                id: AttachmentId(index + 1),
                source: AttachmentSource::LocalUpload,
                kind: AttachmentKind::Image,
                mime_type: "image/png".to_owned(),
                byte_len: 1_024,
                sha256: format!("sha-{index}"),
                preview: None,
                source_trace: format!("fixture://{index}"),
                sensitivity: AttachmentSensitivity::Normal,
                review_state: AttachmentReviewState::Ready,
                prompt_policy: AttachmentPromptPolicy::RefOnly,
            })
            .collect();

        let prepared = VisionUnderstandingRuntime
            .prepare(VisionUnderstandingRequest {
                model,
                attachments,
                total_context_budget: TokenBudget::new(4_096).unwrap(),
                local_user_confirmed: true,
            })
            .unwrap();

        assert_eq!(prepared.attachment_refs.len(), 5);
    }
}
