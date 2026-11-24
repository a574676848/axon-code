use crate::agent_model_tool_history::normalize_provider_history;
use crate::token_estimator::{
    estimated_message_tokens, estimated_message_tokens_with_capabilities,
};
use crate::{
    AgentModelLocalImageRef, AgentModelMessage, AgentModelToolCallMetadata,
    AgentModelToolResultMetadata, PromptCacheProviderWireFormat,
    estimated_history_tokens_for_provider, estimated_history_tokens_for_provider_with_capabilities,
};

const IMAGE_MIME_TYPE: &str = "image/png";
const ARCHIVE_REF: &str = "blobs/test.blob";
const IMAGE_PATH: &str = "fixtures/test.png";
const MIN_IMAGE_TOKEN_DELTA: u64 = 1_800;

fn archive_first_image() -> AgentModelLocalImageRef {
    AgentModelLocalImageRef {
        path: String::new(),
        mime_type: IMAGE_MIME_TYPE.to_owned(),
        byte_len: Some(8),
        sha256: Some("sha256".to_owned()),
        archive_ref: Some(ARCHIVE_REF.to_owned()),
        source_ref: Some("attachment:test".to_owned()),
    }
}

fn local_image() -> AgentModelLocalImageRef {
    AgentModelLocalImageRef::local(IMAGE_PATH, IMAGE_MIME_TYPE)
}

fn provider_formats() -> [PromptCacheProviderWireFormat; 3] {
    [
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        PromptCacheProviderWireFormat::OpenAiResponses,
        PromptCacheProviderWireFormat::AnthropicMessages,
    ]
}

#[test]
fn archive_first_image_adds_provider_media_budget_without_file_io() {
    let text = AgentModelMessage::text("user", "请分析图片");
    let archive = text.clone().with_local_images(vec![archive_first_image()]);

    for format in provider_formats() {
        let text_tokens = estimated_message_tokens(&text, format, &[]);
        let image_tokens = estimated_message_tokens(&archive, format, &[]);
        assert!(
            image_tokens >= text_tokens.saturating_add(MIN_IMAGE_TOKEN_DELTA),
            "format={format:?}, text={text_tokens}, image={image_tokens}"
        );
    }
}

#[test]
fn local_and_archive_images_share_the_same_media_budget_shape() {
    let text = AgentModelMessage::text("user", "请分析图片");
    let local = text.clone().with_local_images(vec![local_image()]);
    let archive = text.with_local_images(vec![archive_first_image()]);

    for format in provider_formats() {
        let local_delta = estimated_message_tokens(&local, format, &[]).saturating_sub(
            estimated_message_tokens(&AgentModelMessage::text("user", "请分析图片"), format, &[]),
        );
        let archive_delta = estimated_message_tokens(&archive, format, &[]).saturating_sub(
            estimated_message_tokens(&AgentModelMessage::text("user", "请分析图片"), format, &[]),
        );
        assert!(
            local_delta >= MIN_IMAGE_TOKEN_DELTA && archive_delta >= MIN_IMAGE_TOKEN_DELTA,
            "format={format:?}, local_delta={local_delta}, archive_delta={archive_delta}"
        );
        assert!(
            local_delta.abs_diff(archive_delta) < 32,
            "format={format:?}, local_delta={local_delta}, archive_delta={archive_delta}"
        );
    }
}

#[test]
fn provider_history_estimator_counts_each_media_message_once() {
    let messages = vec![
        AgentModelMessage::text("user", "前文").with_local_images(vec![archive_first_image()]),
        AgentModelMessage::text("assistant", "普通回答"),
    ];

    for format in provider_formats() {
        let history = estimated_history_tokens_for_provider(&messages, format, &[]);
        let expected = messages
            .iter()
            .map(|message| estimated_message_tokens(message, format, &[]))
            .sum::<u64>();
        assert_eq!(history, expected, "format={format:?}");
    }
}

#[test]
fn text_only_estimator_matches_provider_image_omission_projection() {
    let message = AgentModelMessage::text("user", "请分析图片")
        .with_local_images(vec![local_image(), archive_first_image()]);
    let source = [message.clone()];
    let normalized = normalize_provider_history(&source, false);

    for format in provider_formats() {
        let vision_tokens = estimated_message_tokens_with_capabilities(&message, format, &[], true);
        let text_only_tokens =
            estimated_message_tokens_with_capabilities(&message, format, &[], false);
        let normalized_tokens = estimated_message_tokens(&normalized.messages[0], format, &[]);
        let history_tokens = estimated_history_tokens_for_provider_with_capabilities(
            std::slice::from_ref(&message),
            format,
            &[],
            false,
        );

        assert_eq!(text_only_tokens, normalized_tokens, "format={format:?}");
        assert_eq!(history_tokens, normalized_tokens, "format={format:?}");
        assert!(
            text_only_tokens < vision_tokens,
            "format={format:?}, text_only={text_only_tokens}, vision={vision_tokens}"
        );
    }
}

#[test]
fn vision_estimator_matches_invalid_image_omission_projection() {
    let message =
        AgentModelMessage::text("assistant", "回答").with_local_images(vec![local_image()]);
    let normalized = normalize_provider_history(std::slice::from_ref(&message), true);

    assert!(normalized.messages[0].local_images.is_empty());
    for format in provider_formats() {
        let estimated = estimated_message_tokens_with_capabilities(&message, format, &[], true);
        let normalized_estimated = estimated_message_tokens(&normalized.messages[0], format, &[]);
        assert_eq!(estimated, normalized_estimated, "format={format:?}");
    }
}

#[test]
fn ordinary_text_estimation_remains_provider_framed() {
    let message = AgentModelMessage::text("user", "普通文本");
    for format in provider_formats() {
        assert!(estimated_message_tokens(&message, format, &[]) > 0);
    }
}

#[test]
fn combined_tool_call_and_result_history_matches_normalized_wire_budget() {
    let mut message = AgentModelMessage::text("assistant", "");
    message = message.with_tool_calls(vec![AgentModelToolCallMetadata {
        id: "call-1".to_owned(),
        name: "builtin.search".to_owned(),
        input: "{}".into(),
    }]);
    message
        .provider_metadata
        .as_mut()
        .expect("tool call metadata")
        .tool_result = Some(AgentModelToolResultMetadata {
        call_id: "call-1".to_owned(),
        output: "tool output with enough bytes to expose undercount".to_owned(),
        success: true,
    });
    let source = [message];

    for supports_vision in [true, false] {
        let normalized = normalize_provider_history(&source, supports_vision);
        assert_eq!(normalized.messages.len(), 2);
        for format in provider_formats() {
            let expected = normalized
                .messages
                .iter()
                .map(|item| {
                    estimated_message_tokens_with_capabilities(item, format, &[], supports_vision)
                })
                .sum::<u64>();
            let actual = estimated_history_tokens_for_provider_with_capabilities(
                &source,
                format,
                &[],
                supports_vision,
            );
            assert_eq!(
                actual, expected,
                "format={format:?}, supports_vision={supports_vision}"
            );
        }
    }
}
