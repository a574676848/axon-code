//! 远控 `OutputRef` 单引用授权。
//!
//! 该模块只生成可审计 grant，不读取 `OutputRef` 原文，也不把授权扩展成
//! session transcript 或终端能力。

use axon_domain::{
    CellId, ExecutionRequestId, OutputRef, OutputRefId, RemoteAction, RemoteCapabilityPolicy,
    SessionId,
};

/// 远控 `OutputRef` grant 默认最大字节数。
pub const REMOTE_OUTPUT_REF_GRANT_MAX_BYTES: usize = 65_536;
/// 远控 `OutputRef` grant 默认最大有效期。
pub const REMOTE_OUTPUT_REF_GRANT_MAX_TTL_MS: u64 = 300_000;
/// 远控 `OutputRef` grant 已发放原因。
pub const REMOTE_OUTPUT_REF_REASON_GRANTED: &str = "remote_output_ref:single_ref_granted";
/// 远控 `OutputRef` grant 不开放 session 原文原因。
pub const REMOTE_OUTPUT_REF_REASON_SESSION_CLOSED: &str = "remote_output_ref:session_closed";
/// 远控 `OutputRef` grant 不开放终端能力原因。
pub const REMOTE_OUTPUT_REF_REASON_TERMINAL_CLOSED: &str = "remote_output_ref:terminal_closed";

/// 远控 `OutputRef` grant 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOutputRefGrantRequest {
    /// 执行关联请求 ID。
    pub execution_request_id: ExecutionRequestId,
    /// 所属 session。
    pub session_id: SessionId,
    /// 所属 RuntimeCell。
    pub cell_id: CellId,
    /// 待授权的单个输出引用。
    pub output_ref: OutputRef,
    /// 请求开放的最大字节数。
    pub requested_max_bytes: usize,
    /// 请求开放的有效期。
    pub requested_ttl_ms: u64,
    /// 当前时间。
    pub now_ms: u64,
}

/// 远控 `OutputRef` grant 决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteOutputRefGrantDecision {
    /// 授权已发放。
    Allow {
        /// 授权内容。
        grant: RemoteOutputRefGrant,
    },
    /// 授权被阻断。
    Block {
        /// 阻断原因。
        reason: RemoteOutputRefGrantBlockReason,
    },
}

/// 远控 `OutputRef` grant 阻断原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOutputRefGrantBlockReason {
    /// 策略未允许远控读取单个 `OutputRef` grant。
    PolicyDenied,
    /// 请求字节数无效。
    InvalidMaxBytes,
    /// 请求字节数超过上限。
    MaxBytesExceeded,
    /// 请求有效期无效。
    InvalidTtl,
    /// 请求有效期超过上限。
    TtlExceeded,
}

/// 远控 `OutputRef` grant。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOutputRefGrant {
    /// 被授权的输出引用 ID。
    pub output_ref_id: OutputRefId,
    /// 所属 session。
    pub session_id: SessionId,
    /// 所属 RuntimeCell。
    pub cell_id: CellId,
    /// 执行关联请求 ID。
    pub execution_request_id: ExecutionRequestId,
    /// 输出 hash。
    pub sha256: String,
    /// 来源追踪。
    pub source_trace: String,
    /// 允许读取的最大字节数。
    pub max_bytes: usize,
    /// 授权过期时间。
    pub expires_at_ms: u64,
    /// 有界预览。
    pub bounded_preview: String,
    /// 是否允许读取 session transcript。
    pub session_transcript_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控 `OutputRef` grant runtime。
#[derive(Debug, Clone)]
pub struct RemoteOutputRefGrantRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteOutputRefGrantRuntime {
    /// 创建 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 评估并发放单引用授权；真实原文读取由外层 BlobStore/OutputRef store 执行。
    #[must_use]
    pub fn evaluate(&self, request: &RemoteOutputRefGrantRequest) -> RemoteOutputRefGrantDecision {
        if self
            .policy
            .ensure_allowed(RemoteAction::ReadOutputRef)
            .is_err()
        {
            return block(RemoteOutputRefGrantBlockReason::PolicyDenied);
        }
        if request.requested_max_bytes == 0 {
            return block(RemoteOutputRefGrantBlockReason::InvalidMaxBytes);
        }
        if request.requested_max_bytes > REMOTE_OUTPUT_REF_GRANT_MAX_BYTES
            || request.requested_max_bytes > request.output_ref.byte_len
        {
            return block(RemoteOutputRefGrantBlockReason::MaxBytesExceeded);
        }
        if request.requested_ttl_ms == 0 {
            return block(RemoteOutputRefGrantBlockReason::InvalidTtl);
        }
        if request.requested_ttl_ms > REMOTE_OUTPUT_REF_GRANT_MAX_TTL_MS {
            return block(RemoteOutputRefGrantBlockReason::TtlExceeded);
        }

        let expires_at_ms = request.now_ms.saturating_add(request.requested_ttl_ms);

        RemoteOutputRefGrantDecision::Allow {
            grant: RemoteOutputRefGrant {
                output_ref_id: request.output_ref.id,
                session_id: request.session_id,
                cell_id: request.cell_id,
                execution_request_id: request.execution_request_id,
                sha256: request.output_ref.sha256.clone(),
                source_trace: request.output_ref.source_trace.clone(),
                max_bytes: request.requested_max_bytes,
                expires_at_ms,
                bounded_preview: request.output_ref.preview.clone(),
                session_transcript_allowed: false,
                reasons: vec![
                    REMOTE_OUTPUT_REF_REASON_GRANTED.to_owned(),
                    REMOTE_OUTPUT_REF_REASON_SESSION_CLOSED.to_owned(),
                    REMOTE_OUTPUT_REF_REASON_TERMINAL_CLOSED.to_owned(),
                ],
            },
        }
    }
}

fn block(reason: RemoteOutputRefGrantBlockReason) -> RemoteOutputRefGrantDecision {
    RemoteOutputRefGrantDecision::Block { reason }
}
