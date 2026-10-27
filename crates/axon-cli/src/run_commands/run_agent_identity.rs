//! Provider turn 的执行身份与受众信任域判定。
//!
//! 执行身份与受众信任域都由运行时权威写入 `transportContext`；Provider 层只消费该合同，
//! 不再自行推断。本模块是唯一判定入口，主 turn 与后台执行共用同一套规则。

use axon_daemon::PRIMARY_TURN_ACTION_KIND;
use axon_domain::{AudienceContext, AudienceKind, DisclosurePolicy};
use axon_persona::Channel;
use axon_session_runtime::ProviderTurnRequest;

/// Execution payload 中携带原始受众信任域的字段。
const TRANSPORT_AUDIENCE_KEY: &str = "audience";

/// 本次 Provider turn 的执行身份。
///
/// 执行身份和受众信任域都由运行时权威写入 transportContext；Provider 层只消费
/// 该合同，不再自行推断。主代理承担用户可见的人格与决策，后台 Goal Execution
/// 是隔离的快速执行子代理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunAgentIdentity {
    /// 私聊或本地主代理 turn：可见 Personal Memory。
    PrimaryPrivate,
    /// 群聊或受限主代理 turn：只可见 Workspace Memory。
    PrimaryWorkspace,
    /// 后台 Goal Execution 的快速执行子代理：不装配 Soul、不继承主 turn 思考强度。
    GoalExecutionSubagent,
}

impl RunAgentIdentity {
    /// 是否装配完整用户 Soul；主代理即使进入群聊也仍是同一个主智能体。
    pub(crate) const fn includes_user_soul(self) -> bool {
        !matches!(self, Self::GoalExecutionSubagent)
    }

    /// 是否装配 owner 私有的 Personal Memory。
    pub(crate) const fn includes_personal_memory(self) -> bool {
        matches!(self, Self::PrimaryPrivate)
    }

    /// 是否保持主 turn 的模型槽位、推理强度、工具策略与稳定前缀。
    pub(crate) const fn is_primary(self) -> bool {
        !matches!(self, Self::GoalExecutionSubagent)
    }

    /// 本次请求的 Memory 披露边界。
    ///
    /// 群聊与子代理只可读取当前 Workspace Memory，避免跨边界泄露 Global Memory。
    pub(crate) const fn memory_disclosure_policy(self) -> DisclosurePolicy {
        match self {
            Self::PrimaryPrivate => DisclosurePolicy::PrivateOnly,
            Self::PrimaryWorkspace | Self::GoalExecutionSubagent => DisclosurePolicy::Workspace,
        }
    }
}

/// 把客户端渠道映射为人格场景帧的渠道记号。
pub(crate) fn persona_channel(channel: axon_client_identity::ClientChannel) -> Channel {
    match channel {
        axon_client_identity::ClientChannel::Tui => Channel::Tui,
        axon_client_identity::ClientChannel::Desktop => Channel::Desktop,
        axon_client_identity::ClientChannel::Feishu => Channel::Feishu,
        axon_client_identity::ClientChannel::Cli | axon_client_identity::ClientChannel::Memory => {
            Channel::Cli
        }
    }
}

/// 后台 Goal Execution 是隔离的快速执行子代理；其余 Provider turn 都是用户可见的主代理。
pub(crate) fn run_agent_identity(request: &ProviderTurnRequest) -> RunAgentIdentity {
    if request.action_kind != PRIMARY_TURN_ACTION_KIND {
        return RunAgentIdentity::GoalExecutionSubagent;
    }
    let Some(context) = request.transport_context.as_ref() else {
        return RunAgentIdentity::PrimaryPrivate;
    };
    let Some(audience_value) = context.get(TRANSPORT_AUDIENCE_KEY) else {
        return RunAgentIdentity::PrimaryWorkspace;
    };
    let Ok(audience) = serde_json::from_value::<AudienceContext>(audience_value.clone()) else {
        return RunAgentIdentity::PrimaryWorkspace;
    };
    if matches!(
        (audience.kind(), audience.disclosure()),
        (AudienceKind::Private, DisclosurePolicy::PrivateOnly)
    ) {
        RunAgentIdentity::PrimaryPrivate
    } else {
        RunAgentIdentity::PrimaryWorkspace
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_session_runtime::JarvisRuntimeProfile;

    fn request_with(action_kind: &str, context: Option<serde_json::Value>) -> ProviderTurnRequest {
        ProviderTurnRequest {
            session_id: "s-1".to_owned(),
            action_kind: action_kind.to_owned(),
            prompt: "hello".to_owned(),
            quoted_message: None,
            cwd: ".".to_owned(),
            run_session_id: "r-1".to_owned(),
            runtime_profile: JarvisRuntimeProfile::Jarvis,
            transport_context: context,
        }
    }

    #[test]
    fn private_audience_maps_to_primary_private() {
        let audience = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
        let request = request_with(
            PRIMARY_TURN_ACTION_KIND,
            Some(serde_json::json!({ "audience": audience })),
        );

        assert_eq!(
            run_agent_identity(&request),
            RunAgentIdentity::PrimaryPrivate
        );
        assert!(run_agent_identity(&request).includes_personal_memory());
    }

    #[test]
    fn group_audience_maps_to_primary_workspace_without_personal_memory() {
        let audience = AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace);
        let request = request_with(
            PRIMARY_TURN_ACTION_KIND,
            Some(serde_json::json!({ "audience": audience })),
        );

        let identity = run_agent_identity(&request);
        assert_eq!(identity, RunAgentIdentity::PrimaryWorkspace);
        assert!(identity.includes_user_soul());
        assert!(!identity.includes_personal_memory());
    }

    #[test]
    fn missing_or_invalid_audience_fails_closed_to_workspace() {
        assert_eq!(
            run_agent_identity(&request_with(PRIMARY_TURN_ACTION_KIND, None)),
            RunAgentIdentity::PrimaryPrivate
        );
        assert_eq!(
            run_agent_identity(&request_with(
                PRIMARY_TURN_ACTION_KIND,
                Some(serde_json::json!({ "audience": "not-an-audience" }))
            )),
            RunAgentIdentity::PrimaryWorkspace
        );
    }

    #[test]
    fn non_primary_action_kind_is_goal_execution_subagent() {
        let identity = run_agent_identity(&request_with("other_action", None));

        assert_eq!(identity, RunAgentIdentity::GoalExecutionSubagent);
        assert!(!identity.includes_user_soul());
        assert!(!identity.is_primary());
    }
}
