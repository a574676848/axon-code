//! 远控 bounded 投影入口。

use axon_domain::{
    AxonResult, RemoteAction, RemoteCapabilityPolicy, RemoteSessionProjection, RemoteVisibility,
    SessionId,
};

/// 远控投影 runtime。
#[derive(Debug, Clone)]
pub struct RemoteProjectionRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteProjectionRuntime {
    /// 创建远控投影 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 生成 bounded 投影。
    pub fn project_summary(&self, session_id: SessionId) -> AxonResult<RemoteSessionProjection> {
        self.policy
            .ensure_allowed(RemoteAction::ReadBoundedSummary)?;
        Ok(RemoteSessionProjection {
            session_id,
            visibility: RemoteVisibility::BoundedSummary,
            truncated: false,
        })
    }

    /// 校验远控动作。
    pub fn ensure_action_allowed(&self, action: RemoteAction) -> AxonResult<()> {
        self.policy.ensure_allowed(action)
    }
}
