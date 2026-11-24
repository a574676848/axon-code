use std::borrow::Cow;

use super::*;
use crate::{AgentModelLocalImageRef, AgentModelMessage};

const IMAGE_MIME_TYPE: &str = "image/png";
const ARCHIVE_IMAGE_REF: &str = "blobs/archive-image.blob";

#[test]
fn provider_normalize_removes_invalid_image_projection_with_reason() {
    let message = AgentModelMessage::text("assistant", "回答").with_local_images(vec![
        AgentModelLocalImageRef {
            path: "local-image.png".to_owned(),
            mime_type: IMAGE_MIME_TYPE.to_owned(),
            byte_len: None,
            sha256: None,
            archive_ref: None,
            source_ref: None,
        },
    ]);

    let messages = [message];
    let normalized = normalize_provider_history(&messages, true);

    assert!(normalized.messages[0].local_images.is_empty());
    assert!(
        normalized
            .reasons
            .contains(&PROVIDER_HISTORY_REASON_IMAGE_PROJECTION_REMOVED.to_owned())
    );
}

#[test]
fn provider_normalize_borrows_archive_first_image_history() {
    let message = AgentModelMessage::text("user", "看归档图片").with_local_images(vec![
        AgentModelLocalImageRef {
            path: String::new(),
            mime_type: IMAGE_MIME_TYPE.to_owned(),
            byte_len: None,
            sha256: None,
            archive_ref: Some(ARCHIVE_IMAGE_REF.to_owned()),
            source_ref: None,
        },
    ]);
    let messages = [message];

    let normalized = normalize_provider_history(&messages, true);

    assert!(matches!(normalized.messages, Cow::Borrowed(_)));
    assert_eq!(
        normalized.messages[0].local_images[0]
            .archive_ref
            .as_deref(),
        Some(ARCHIVE_IMAGE_REF)
    );
    assert!(normalized.reasons.is_empty());
}

#[test]
fn provider_normalize_omits_images_for_text_only_model() {
    let message = AgentModelMessage::text("user", "分析历史图片").with_local_images(vec![
        AgentModelLocalImageRef::local("local-image.png", IMAGE_MIME_TYPE),
    ]);
    let messages = [message];

    let normalized = normalize_provider_history(&messages, false);

    assert!(matches!(normalized.messages, Cow::Owned(_)));
    assert!(normalized.messages[0].local_images.is_empty());
    assert_eq!(
        normalized.messages[0].content,
        format!("分析历史图片\n{IMAGE_CONTENT_OMITTED_PLACEHOLDER}")
    );
    assert!(
        normalized
            .reasons
            .contains(&PROVIDER_HISTORY_REASON_UNSUPPORTED_IMAGE_OMITTED.to_owned())
    );
}

#[test]
fn provider_normalize_omits_tool_result_images_for_text_only_model() {
    let call = AgentModelMessage::text("assistant", "").with_tool_calls(vec![
        crate::AgentModelToolCallMetadata {
            id: "call-image".to_owned(),
            name: "builtin.read".to_owned(),
            input: "{}".into(),
        },
    ]);
    let result =
        AgentModelMessage::tool_result("call-image", "工具输出", true).with_local_images(vec![
            AgentModelLocalImageRef::local("tool-image.png", IMAGE_MIME_TYPE),
        ]);
    let messages = [call, result];

    let normalized = normalize_provider_history(&messages, false);

    assert_eq!(normalized.messages.len(), 2);
    assert!(normalized.messages[1].local_images.is_empty());
    assert_eq!(
        tool_result(&normalized.messages[1])
            .expect("工具结果 metadata 必须保留")
            .output,
        format!("工具输出\n{IMAGE_CONTENT_OMITTED_PLACEHOLDER}")
    );
    assert!(
        normalized
            .reasons
            .contains(&PROVIDER_HISTORY_REASON_UNSUPPORTED_IMAGE_OMITTED.to_owned())
    );
}

#[test]
fn text_only_history_without_images_keeps_borrowed_fast_path() {
    let messages = [AgentModelMessage::text("user", "纯文本")];

    let normalized = normalize_provider_history(&messages, false);

    assert!(matches!(normalized.messages, Cow::Borrowed(_)));
    assert!(normalized.reasons.is_empty());
}
