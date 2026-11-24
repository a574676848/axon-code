//! Interaction 测试夹具：会话标识是显式输入，不做任何派生。

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};

/// 测试夹具：会话标识是显式输入，这里只把可读串包成合法标识，不做任何派生。
pub(super) fn test_session(name: &str) -> DaemonSessionId {
    DaemonSessionId::new(name.to_owned()).expect("合法会话 ID")
}

pub(super) fn sample_interaction() -> InteractionRecord {
    interaction_record(
        &test_session("session-test"),
        "interaction-test",
        "message-test",
        10,
    )
}

pub(super) fn interaction_record(
    session_id: &DaemonSessionId,
    interaction_id: &str,
    message_id: &str,
    received_at_ms: u64,
) -> InteractionRecord {
    InteractionRecord::new(
        session_id.clone(),
        InteractionId::new(interaction_id).expect("交互 ID"),
        InteractionSource::new("tui", message_id).expect("来源"),
        ScopeRef::workspace("workspace-test").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-test").expect("路由"),
        "检查状态",
        received_at_ms,
    )
    .expect("Interaction")
}

/// 构造指定会话下的 Interaction：会话标识由调用方显式给出。
pub(super) fn interaction_in_session(
    session_id: &DaemonSessionId,
    interaction_id: &str,
    workspace: &str,
    destination: &str,
    received_at_ms: u64,
) -> InteractionRecord {
    InteractionRecord::new(
        session_id.clone(),
        InteractionId::new(interaction_id).expect("交互 ID"),
        InteractionSource::new("tui", format!("message-{interaction_id}")).expect("来源"),
        ScopeRef::workspace(workspace).expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", destination).expect("路由"),
        "检查状态",
        received_at_ms,
    )
    .expect("Interaction")
}
