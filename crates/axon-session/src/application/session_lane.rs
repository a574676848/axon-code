//! Workspace 内的 Session 复合车道身份。
//!
//! Axon 对外只有持续存在的同一个 JARVIS，Session 只是内部运行窗口。Workspace
//! 决定项目事实范围，但不同 audience 和会话表面不得共享 Provider history。

use axon_crypto::sha256_hex;
use axon_domain::{AudienceContext, AudienceKind, DisclosurePolicy, InteractionRecord};

/// Session 车道 ID 前缀；与历史逐条 Interaction Session 明确区分。
const SESSION_LANE_ID_PREFIX: &str = "session-lane-";
/// 只允许当前私密上下文可见的 trust scope。
const TRUST_SCOPE_PRIVATE: &str = "private";
/// 允许披露 Workspace 内容的群聊 trust scope。
const TRUST_SCOPE_GROUP_WORKSPACE: &str = "group-workspace";
/// 未知受众组合的保守 trust scope。
const TRUST_SCOPE_RESTRICTED: &str = "restricted";

/// 返回复合车道对应的稳定内部 ID。
///
/// Workspace 负责项目事实范围；trust scope 防止私聊历史进入群聊；channel 和
/// destination 区分同一 Workspace 下的不同会话表面。ID 使用规范化四元组的
/// SHA-256，避免路由字段分隔符歧义和超长 destination 撑破 Session ID。
#[must_use]
pub fn session_lane_id(
    workspace_id: &str,
    audience: &AudienceContext,
    channel: &str,
    destination: &str,
) -> String {
    let lane_material = serde_json::json!([
        workspace_id.trim(),
        trust_scope(audience),
        channel.trim(),
        destination.trim(),
    ])
    .to_string();
    format!("{SESSION_LANE_ID_PREFIX}{}", sha256_hex(lane_material))
}

/// 从 Interaction 派生主 turn 车道；未绑定 Workspace 时返回 None。
#[must_use]
pub fn interaction_session_lane_id(interaction: &InteractionRecord) -> Option<String> {
    Some(session_lane_id(
        interaction.scope.workspace_id()?.as_str(),
        &interaction.audience,
        interaction.source.channel(),
        interaction.reply_route.destination(),
    ))
}

#[must_use]
const fn trust_scope(audience: &AudienceContext) -> &'static str {
    match (audience.kind(), audience.disclosure()) {
        (AudienceKind::Private, DisclosurePolicy::PrivateOnly) => TRUST_SCOPE_PRIVATE,
        (AudienceKind::Group, DisclosurePolicy::Workspace) => TRUST_SCOPE_GROUP_WORKSPACE,
        _ => TRUST_SCOPE_RESTRICTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_workspace_and_surface_resolves_to_same_session_lane() {
        let workspace = "workspace-0f1e2d3c4b5a6978";
        let audience = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
        assert_eq!(
            session_lane_id(workspace, &audience, "tui", "terminal-1"),
            session_lane_id(workspace, &audience, "tui", "terminal-1")
        );
    }

    #[test]
    fn different_workspaces_scopes_and_surfaces_resolve_to_different_lanes() {
        let private = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
        let group = AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace);
        let first = session_lane_id("workspace-a", &private, "tui", "terminal-1");
        let second = session_lane_id("workspace-b", &private, "tui", "terminal-1");
        let third = session_lane_id("workspace-a", &group, "feishu", "chat-a");
        let fourth = session_lane_id("workspace-a", &private, "feishu", "chat-b");

        assert_ne!(first, second);
        assert_ne!(first, third);
        assert_ne!(first, fourth);
    }

    #[test]
    fn session_lane_ignores_surrounding_whitespace() {
        assert_eq!(
            session_lane_id(
                "  workspace-0f1e2d3c4b5a6978  ",
                &AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
                " tui ",
                " terminal-1 ",
            ),
            session_lane_id(
                "workspace-0f1e2d3c4b5a6978",
                &AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
                "tui",
                "terminal-1",
            )
        );
    }

    #[test]
    fn interaction_lane_requires_workspace_and_uses_route_identity() {
        let interaction = InteractionRecord::new(
            axon_domain::InteractionId::new("interaction-1").expect("合法 ID"),
            axon_domain::InteractionSource::new("feishu", "chat-a:message-1").expect("合法来源"),
            axon_domain::ScopeRef::workspace("workspace-a").expect("合法作用域"),
            AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace),
            axon_domain::ReplyRoute::new("feishu", "chat-a").expect("合法路由"),
            "检查构建",
            42,
        )
        .expect("合法 Interaction");

        assert_eq!(
            interaction_session_lane_id(&interaction),
            Some(session_lane_id(
                "workspace-a",
                &interaction.audience,
                "feishu",
                "chat-a"
            ))
        );
    }
}
