//! Fresh window 附件边界回归测试。

use crate::{
    AgentModelLocalImageRef, AgentModelMessage, CanonicalCheckpoint, CompressionPolicy,
    PromptCacheProviderWireFormat, WindowRolloverPolicyRuntime,
};

fn checkpoint(messages: &[AgentModelMessage]) -> CanonicalCheckpoint {
    CanonicalCheckpoint::parse(
        "P0:\nTASK:\n附件预算回归\nCONSTRAINTS:\n保持附件引用\nVERIFIED:\n已完成预算验证\nOPEN:\n- none\nTOOLS:\n- none",
        messages,
    )
    .expect("附件预算测试 checkpoint 应通过门禁")
}

#[test]
fn fresh_window_drops_old_user_attachments_from_provider_history() {
    let messages = (0..12)
        .map(|index| {
            AgentModelMessage::text("user", format!("图片用户任务-{index:02}")).with_local_images(
                vec![AgentModelLocalImageRef::local("image.png", "image/png")],
            )
        })
        .collect::<Vec<_>>();
    let compacted = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: 30_000,
        context_window: 37_500,
    })
    .with_provider_wire_capabilities(
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        Vec::new(),
        false,
    )
    .rebuild_with_compression(&messages, &checkpoint(&messages));
    let retained_users = compacted
        .messages
        .iter()
        .filter(|message| {
            message.role == "user" && message.kind == crate::AgentModelMessageKind::Regular
        })
        .count();

    assert_eq!(retained_users, 0);
}
