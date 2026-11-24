use axon_agent_runtime::{AgentModelMessageKind, AgentRuntimeNoteKind};
use axon_config::{ModelConfig, ProviderConfig};
use axon_domain::QuotedMessageContext;

use super::quoted_message_input::provider_streaming_transport_input;
use super::tests::{fixture_request, temp_workspace};

#[test]
fn normal_provider_input_projects_quoted_sidecar_without_mutating_canonical_history() {
    let root = temp_workspace("quoted-provider-input");
    let mut request = fixture_request(root.clone());
    request.quoted_message = QuotedMessageContext::new("om_parent", "父消息正文").ok();
    let provider = ProviderConfig::new(
        "quoted-test-provider",
        axon_config::PROVIDER_PROTOCOL_OPENAI,
        "https://provider.invalid/v1",
    );
    let model = ModelConfig {
        name: "quoted-test-model".to_owned(),
        id: "quoted-test-model".to_owned(),
        provider: provider.name.clone(),
        context_window: 32_000,
        max_output_tokens: Some(4_096),
        supports_vision: false,
        ..ModelConfig::default()
    };

    let input = provider_streaming_transport_input(&request, &model, &provider).unwrap();
    let messages = input.messages.as_slice();

    assert_eq!(request.model_payload.messages.len(), 1);
    assert!(
        !request.model_payload.messages[0]
            .content
            .contains("父消息正文")
    );
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].kind,
        AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::QuotedMessageContext)
    );
    assert!(messages[0].cache_excluded);
    assert!(messages[0].content.contains("om_parent"));
    assert!(messages[0].content.contains("父消息正文"));
    assert_eq!(messages[1].content, "修复构建失败");
    let _ = std::fs::remove_dir_all(root);
}
