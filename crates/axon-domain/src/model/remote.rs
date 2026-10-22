//! 远控领域边界。

use crate::{AxonError, AxonResult, SessionId};

/// 远控入口可能请求的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAction {
    /// 查看 bounded summary。
    ReadBoundedSummary,
    /// 上传附件候选。
    UploadAttachment,
    /// 读取单个已授权 `OutputRef`。
    ReadOutputRef,
    /// 控制远控线程生命周期。
    ControlThread,
    /// 接受 `GhostText` 或命令补全。
    AcceptCompletion,
    /// 切换执行后端。
    SwitchBackend,
}

/// 远控拒绝原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteDenialReason {
    /// 远控永远不能拥有该能力。
    ForbiddenByInvariant,
    /// 当前策略未授权。
    NotGrantedByPolicy,
}

/// 远控投影可见性等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteVisibility {
    /// 只允许 bounded summary。
    BoundedSummary,
    /// 允许附件候选元数据。
    AttachmentCandidateMetadata,
}

/// 远控能力策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCapabilityPolicy {
    /// 是否允许读取 bounded summary。
    pub allow_bounded_summary: bool,
    /// 是否允许上传附件候选。
    pub allow_attachment_upload: bool,
    /// 是否允许读取已批准的单个 `OutputRef` grant。
    pub allow_output_ref_grant: bool,
    /// 是否允许提交结构化线程控制命令。
    pub allow_thread_control: bool,
}

impl RemoteCapabilityPolicy {
    /// 创建 M0/M1 默认远控策略。
    #[must_use]
    pub const fn default_sidecar() -> Self {
        Self {
            allow_bounded_summary: true,
            allow_attachment_upload: true,
            allow_output_ref_grant: true,
            allow_thread_control: true,
        }
    }

    /// 判断远控动作是否允许。
    ///
    /// # Errors
    ///
    /// 当远控动作违反安全不变量或未被策略授权时返回错误。
    pub fn ensure_allowed(&self, action: RemoteAction) -> AxonResult<()> {
        if matches!(
            action,
            RemoteAction::AcceptCompletion | RemoteAction::SwitchBackend
        ) {
            return Err(AxonError::RemoteDenied {
                action,
                reason: RemoteDenialReason::ForbiddenByInvariant,
            });
        }

        let allowed = match action {
            RemoteAction::ReadBoundedSummary => self.allow_bounded_summary,
            RemoteAction::UploadAttachment => self.allow_attachment_upload,
            RemoteAction::ReadOutputRef => self.allow_output_ref_grant,
            RemoteAction::ControlThread => self.allow_thread_control,
            RemoteAction::AcceptCompletion | RemoteAction::SwitchBackend => false,
        };

        if allowed {
            Ok(())
        } else {
            Err(AxonError::RemoteDenied {
                action,
                reason: RemoteDenialReason::NotGrantedByPolicy,
            })
        }
    }
}

/// 远控会话投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSessionProjection {
    /// 会话 ID。
    pub session_id: SessionId,
    /// 可见性。
    pub visibility: RemoteVisibility,
    /// 是否已被截断。
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use crate::{AxonError, RemoteAction, RemoteCapabilityPolicy, RemoteDenialReason};

    #[test]
    fn default_remote_sidecar_allows_only_safe_summary_and_upload() {
        let policy = RemoteCapabilityPolicy::default_sidecar();

        assert!(
            policy
                .ensure_allowed(RemoteAction::ReadBoundedSummary)
                .is_ok()
        );
        assert!(
            policy
                .ensure_allowed(RemoteAction::UploadAttachment)
                .is_ok()
        );
        assert!(policy.ensure_allowed(RemoteAction::ReadOutputRef).is_ok());
    }

    #[test]
    fn remote_sidecar_forbidden_actions_are_invariant_denied() {
        let policy = RemoteCapabilityPolicy::default_sidecar();

        for action in [RemoteAction::AcceptCompletion, RemoteAction::SwitchBackend] {
            assert_eq!(
                policy.ensure_allowed(action),
                Err(AxonError::RemoteDenied {
                    action,
                    reason: RemoteDenialReason::ForbiddenByInvariant,
                })
            );
        }
    }

    #[test]
    fn remote_sidecar_policy_denial_is_not_invariant_denial() {
        let policy = RemoteCapabilityPolicy {
            allow_bounded_summary: false,
            allow_attachment_upload: false,
            allow_output_ref_grant: false,
            allow_thread_control: false,
        };

        assert_eq!(
            policy.ensure_allowed(RemoteAction::ReadBoundedSummary),
            Err(AxonError::RemoteDenied {
                action: RemoteAction::ReadBoundedSummary,
                reason: RemoteDenialReason::NotGrantedByPolicy,
            })
        );
    }
}
