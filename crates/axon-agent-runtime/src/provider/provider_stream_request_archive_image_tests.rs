use std::path::{Path, PathBuf};

use super::*;
use super::{refresh_prompt_cache_plan, request_input};
use crate::agent_model_tool_history::{
    IMAGE_CONTENT_OMITTED_PLACEHOLDER, PROVIDER_HISTORY_REASON_UNSUPPORTED_IMAGE_OMITTED,
};
use crate::{
    AgentModelLocalImageRef, AgentModelMessage, PromptCacheProviderWireFormat,
    ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime,
};

const TEST_PROVIDER_NAME: &str = "archive-image-test";
const TEST_MODEL_ID: &str = "archive-image-model";
const TEST_BASE_URL: &str = "https://provider.example/v1";
const IMAGE_MIME_TYPE: &str = "image/png";
const EXPECTED_IMAGE_BASE64: &str = "iVBORw0KGgo=";
const TEST_CONTEXT_SECTION_COUNT: usize = 1;

#[test]
fn read_file_image_receipt_round_trips_into_provider_bodies() {
    use crate::{
        ProviderToolCall, ReadFileImageReceipt, agent_run_read_file_image_receipt,
        agent_run_tool_result_messages,
    };

    let archive_root = temp_archive_root();
    let bytes = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let sha256 = axon_crypto::sha256_hex(bytes);
    let archive_ref = format!("blobs/{sha256}.blob");
    let blob_path = archive_root.join(&archive_ref);
    std::fs::create_dir_all(blob_path.parent().expect("归档 blob 必须有父目录")).unwrap();
    std::fs::write(&blob_path, bytes).unwrap();

    // 闭环起点：read_file 的图片分支产出的工具回执。
    let tool_call = ProviderToolCall::new(
        "call-read-file-image",
        "builtin.read_file",
        r#"{"path":"shot.png"}"#,
    );
    let receipt = agent_run_read_file_image_receipt(
        &tool_call,
        &ReadFileImageReceipt {
            path: "shot.png".to_owned(),
            absolute_path: "C:\\workspace\\shot.png".to_owned(),
            mime_type: IMAGE_MIME_TYPE.to_owned(),
            byte_len: bytes.len(),
            width: 1,
            height: 1,
            original_width: 1,
            original_height: 1,
            sha256: sha256.clone(),
            archive_ref: archive_ref.clone(),
            source_ref: "tool://builtin.read_file/shot.png".to_owned(),
        },
    );
    let messages = agent_run_tool_result_messages(
        std::slice::from_ref(&tool_call),
        &[crate::AgentRunToolResultReceipt {
            provider_call_id: receipt.provider_call_id.clone(),
            tool_name: receipt.tool_name.clone(),
            status: receipt.status.clone(),
            summary: receipt.summary.clone(),
            metadata: receipt.metadata.clone(),
            output_ref: receipt.output_ref.clone(),
            error_type: receipt.error_type.clone(),
            remote_trigger_allowed: receipt.remote_trigger_allowed,
            filesystem_write_performed: receipt.filesystem_write_performed,
        }],
        None,
        crate::AgentRunToolOutputPolicy::default(),
    )
    .expect("图片回执应能生成下一轮消息");
    assert!(
        messages
            .iter()
            .any(|message| !message.local_images.is_empty()),
        "回执必须带出图片消息"
    );

    // 闭环终点：三种 wire 都把该图片编码成可被模型读取的图片内容。
    for protocol in [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ] {
        let body = body_for_messages(protocol, &archive_root, messages.clone());
        let serialized = body.to_string();
        assert!(
            serialized.contains("data:image/png;base64,")
                || serialized.contains(EXPECTED_IMAGE_BASE64),
            "protocol={protocol:?} 未把图片编码进请求体"
        );
    }

    std::fs::remove_dir_all(archive_root).unwrap();
}

#[test]
fn archive_first_image_reaches_openai_responses_and_anthropic_bodies() {
    let archive_root = temp_archive_root();
    let bytes = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let sha256 = axon_crypto::sha256_hex(bytes);
    let archive_ref = format!("blobs/{sha256}.blob");
    let blob_path = archive_root.join(&archive_ref);
    std::fs::create_dir_all(blob_path.parent().expect("归档 blob 必须有父目录")).unwrap();
    std::fs::write(&blob_path, bytes).unwrap();

    let responses = request_body(
        ProviderStreamProtocol::OpenAiResponses,
        &archive_root,
        &archive_ref,
        &sha256,
        bytes.len() as u64,
    );
    assert_eq!(responses["input"][0]["content"][1]["type"], "input_image");
    assert!(
        responses["input"][0]["content"][1]["image_url"]
            .as_str()
            .is_some_and(|url| url.starts_with("data:image/png;base64,"))
    );

    let anthropic = request_body(
        ProviderStreamProtocol::AnthropicMessages,
        &archive_root,
        &archive_ref,
        &sha256,
        bytes.len() as u64,
    );
    assert_eq!(anthropic["messages"][0]["content"][1]["type"], "image");
    assert_eq!(
        anthropic["messages"][0]["content"][1]["source"]["media_type"],
        IMAGE_MIME_TYPE
    );
    assert_eq!(
        anthropic["messages"][0]["content"][1]["source"]["data"],
        EXPECTED_IMAGE_BASE64
    );
    assert!(!responses.to_string().contains("axon_media"));
    assert!(!anthropic.to_string().contains("axon_media"));

    let chat = request_body(
        ProviderStreamProtocol::OpenAiCompatible,
        &archive_root,
        &archive_ref,
        &sha256,
        bytes.len() as u64,
    );
    assert_eq!(chat["messages"][0]["content"][1]["type"], "image_url");
    assert!(
        chat["messages"][0]["content"][1]["image_url"]["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("data:image/png;base64,"))
    );
    assert!(!chat.to_string().contains("axon_media"));

    std::fs::remove_dir_all(archive_root).unwrap();
}

#[test]
fn text_only_model_omits_images_from_all_provider_bodies_without_file_io() {
    for protocol in [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ] {
        let plan = text_only_request_plan(protocol);

        assert!(plan.body_text.contains(IMAGE_CONTENT_OMITTED_PLACEHOLDER));
        assert!(!plan.body_text.contains("must-not-be-read.png"));
        assert!(!plan.body_text.contains("axon_media"));
        assert!(
            plan.reasons
                .iter()
                .any(|reason| reason == PROVIDER_HISTORY_REASON_UNSUPPORTED_IMAGE_OMITTED)
        );
    }
}

fn text_only_request_plan(protocol: ProviderStreamProtocol) -> ProviderStreamingHttpRequestPlan {
    let capabilities = provider_capabilities(protocol);
    let messages = vec![
        AgentModelMessage::text("user", "分析历史图片").with_local_images(vec![
            AgentModelLocalImageRef::local("must-not-be-read.png", IMAGE_MIME_TYPE),
        ]),
    ];
    let prompt_cache_plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            capabilities,
            format: wire_format(protocol),
            model_id: TEST_MODEL_ID.to_owned(),
            context_window: axon_config::DEFAULT_MODEL_CONTEXT_WINDOW,
            supports_tools: true,
            supports_vision: false,
            messages: messages.clone().into(),
            model_tools: Vec::new(),
            context_section_count: TEST_CONTEXT_SECTION_COUNT,
        })
        .unwrap();

    ProviderStreamingHttpRequestRuntime
        .plan(ProviderStreamingHttpRequestInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            capabilities,
            supports_vision: false,
            protocol,
            base_url: TEST_BASE_URL.to_owned(),
            model_id: TEST_MODEL_ID.to_owned(),
            messages: messages.into(),
            attachment_archive_root: None,
            model_tools: Vec::new(),
            prompt_cache_plan,
            api_key_env: None,
            api_key: None,
            service_tier: None,
            reasoning_effort: None,
            reasoning_summary: None,
            structured_output: None,
            server_conversation_state_ref: None,
            anthropic_max_tokens: axon_config::DEFAULT_MODEL_MAX_OUTPUT_TOKENS,
        })
        .unwrap()
}

fn request_body(
    protocol: ProviderStreamProtocol,
    archive_root: &Path,
    archive_ref: &str,
    sha256: &str,
    byte_len: u64,
) -> serde_json::Value {
    let messages = vec![
        AgentModelMessage::text("user", "看归档图片").with_local_images(vec![
            AgentModelLocalImageRef {
                path: String::new(),
                mime_type: IMAGE_MIME_TYPE.to_owned(),
                byte_len: Some(byte_len),
                sha256: Some(sha256.to_owned()),
                archive_ref: Some(archive_ref.to_owned()),
                source_ref: Some("attachment:test".to_owned()),
            },
        ]),
    ];
    body_for_messages(protocol, archive_root, messages)
}

fn body_for_messages(
    protocol: ProviderStreamProtocol,
    archive_root: &Path,
    messages: Vec<AgentModelMessage>,
) -> serde_json::Value {
    let capabilities = provider_capabilities(protocol);
    let prompt_cache_plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            capabilities,
            format: wire_format(protocol),
            model_id: TEST_MODEL_ID.to_owned(),
            context_window: axon_config::DEFAULT_MODEL_CONTEXT_WINDOW,
            supports_tools: true,
            supports_vision: true,
            messages: messages.clone().into(),
            model_tools: Vec::new(),
            context_section_count: TEST_CONTEXT_SECTION_COUNT,
        })
        .unwrap();
    let plan = ProviderStreamingHttpRequestRuntime
        .plan(ProviderStreamingHttpRequestInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            capabilities,
            supports_vision: true,
            protocol,
            base_url: TEST_BASE_URL.to_owned(),
            model_id: TEST_MODEL_ID.to_owned(),
            messages: messages.into(),
            attachment_archive_root: Some(archive_root.to_path_buf()),
            model_tools: Vec::new(),
            prompt_cache_plan,
            api_key_env: None,
            api_key: None,
            service_tier: None,
            reasoning_effort: None,
            reasoning_summary: None,
            structured_output: None,
            server_conversation_state_ref: None,
            anthropic_max_tokens: axon_config::DEFAULT_MODEL_MAX_OUTPUT_TOKENS,
        })
        .unwrap();

    serde_json::from_str(&plan.body_text).unwrap()
}

fn provider_capabilities(protocol: ProviderStreamProtocol) -> ProviderCapabilities {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => {
            ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI)
        }
        ProviderStreamProtocol::AnthropicMessages => {
            ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_ANTHROPIC)
        }
        ProviderStreamProtocol::OpenAiCompatible => ProviderCapabilities::stateless(),
    }
}

fn wire_format(protocol: ProviderStreamProtocol) -> PromptCacheProviderWireFormat {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => PromptCacheProviderWireFormat::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible => {
            PromptCacheProviderWireFormat::OpenAiCompatibleChat
        }
        ProviderStreamProtocol::AnthropicMessages => {
            PromptCacheProviderWireFormat::AnthropicMessages
        }
    }
}

fn temp_archive_root() -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-provider-archive-first-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

#[test]
fn openai_chat_request_embeds_local_image_data_url() {
    let image_path = temp_image_file("openai-chat-image");
    let mut input = vision_request_input();
    *input.messages = vec![
        AgentModelMessage::text("user", "看图").with_local_images(vec![
            AgentModelLocalImageRef::local(image_path.to_string_lossy(), IMAGE_MIME_TYPE),
        ]),
    ];
    refresh_prompt_cache_plan(&mut input);

    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
    assert!(
        body["messages"][0]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert!(!body.to_string().contains("axon_media"));
    let _ = std::fs::remove_file(image_path);
}

#[test]
fn openai_responses_request_maps_local_image_to_input_image() {
    let image_path = temp_image_file("responses-image");
    let mut input = vision_request_input();
    input.protocol = ProviderStreamProtocol::OpenAiResponses;
    *input.messages = vec![
        AgentModelMessage::text("user", "看图").with_local_images(vec![
            AgentModelLocalImageRef::local(image_path.to_string_lossy(), IMAGE_MIME_TYPE),
        ]),
    ];
    refresh_prompt_cache_plan(&mut input);

    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
    assert!(
        body["input"][0]["content"][1]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert!(!body.to_string().contains("axon_media"));
    let _ = std::fs::remove_file(image_path);
}

#[test]
fn provider_request_reads_image_from_controlled_archive_root() {
    let root = temp_archive_root();
    let blob_dir = root.join("blobs");
    std::fs::create_dir_all(&blob_dir).unwrap();
    let bytes = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let sha = axon_crypto::sha256_hex(bytes);
    std::fs::write(blob_dir.join(format!("{sha}.blob")), bytes).unwrap();

    let mut input = vision_request_input();
    input.attachment_archive_root = Some(root.clone());
    *input.messages = vec![
        AgentModelMessage::text("user", "看归档图片").with_local_images(vec![
            AgentModelLocalImageRef {
                path: "missing-local-path.png".to_owned(),
                mime_type: IMAGE_MIME_TYPE.to_owned(),
                byte_len: Some(bytes.len() as u64),
                sha256: Some(sha.clone()),
                archive_ref: Some(format!("blobs/{sha}.blob")),
                source_ref: Some("attachment:test".to_owned()),
            },
        ]),
    ];
    refresh_prompt_cache_plan(&mut input);

    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
    assert!(body.to_string().contains("data:image/png;base64"));
    assert!(!body.to_string().contains("archiveRef"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn provider_request_rejects_archive_path_traversal() {
    let mut input = vision_request_input();
    input.attachment_archive_root = Some(axon_test_support::test_temp_root());
    *input.messages = vec![
        AgentModelMessage::text("user", "越界").with_local_images(vec![AgentModelLocalImageRef {
            path: "unused.png".to_owned(),
            mime_type: IMAGE_MIME_TYPE.to_owned(),
            byte_len: None,
            sha256: None,
            archive_ref: Some("../outside.blob".to_owned()),
            source_ref: None,
        }]),
    ];
    refresh_prompt_cache_plan(&mut input);

    assert!(ProviderStreamingHttpRequestRuntime.plan(input).is_err());
}

#[test]
fn provider_request_rejects_tampered_local_image_metadata() {
    let image_path = temp_image_file("tampered-image");
    let mut input = vision_request_input();
    *input.messages = vec![
        AgentModelMessage::text("user", "看图").with_local_images(vec![AgentModelLocalImageRef {
            path: image_path.to_string_lossy().to_string(),
            mime_type: IMAGE_MIME_TYPE.to_owned(),
            byte_len: Some(1),
            sha256: Some("0".repeat(64)),
            archive_ref: None,
            source_ref: Some("attachment:1".to_owned()),
        }]),
    ];
    refresh_prompt_cache_plan(&mut input);

    let error = ProviderStreamingHttpRequestRuntime
        .plan(input)
        .expect_err("篡改后的媒体元数据必须阻断 Provider 请求");
    assert!(matches!(error, axon_domain::AxonError::Runtime(_)));
    let _ = std::fs::remove_file(image_path);
}

#[test]
fn provider_request_rejects_missing_local_image() {
    let mut input = vision_request_input();
    let missing_path = axon_test_support::test_temp_root().join(format!(
        "axon-provider-missing-image-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    *input.messages = vec![
        AgentModelMessage::text("user", "看图").with_local_images(vec![
            AgentModelLocalImageRef::local(missing_path.to_string_lossy(), IMAGE_MIME_TYPE),
        ]),
    ];
    refresh_prompt_cache_plan(&mut input);

    let error = ProviderStreamingHttpRequestRuntime.plan(input).unwrap_err();
    assert!(format!("{error:?}").contains("本地图片附件读取失败"));
}

#[test]
fn anthropic_request_embeds_local_image_base64_block() {
    let image_path = temp_image_file("anthropic-image");
    let mut input = vision_request_input();
    input.protocol = ProviderStreamProtocol::AnthropicMessages;
    input.model_id = "claude-test".to_owned();
    *input.messages = vec![
        AgentModelMessage::text("user", "看图").with_local_images(vec![
            AgentModelLocalImageRef::local(image_path.to_string_lossy(), IMAGE_MIME_TYPE),
        ]),
    ];
    refresh_prompt_cache_plan(&mut input);

    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    assert_eq!(body["messages"][0]["content"][1]["type"], "image");
    assert_eq!(
        body["messages"][0]["content"][1]["source"]["type"],
        "base64"
    );
    assert_eq!(
        body["messages"][0]["content"][1]["source"]["media_type"],
        IMAGE_MIME_TYPE
    );
    assert_eq!(
        body["messages"][0]["content"][1]["source"]["data"]
            .as_str()
            .unwrap(),
        EXPECTED_IMAGE_BASE64
    );
    let _ = std::fs::remove_file(image_path);
}

#[test]
fn provider_requests_detect_local_image_mime_from_bytes() {
    let image_path = axon_test_support::test_temp_root().join(format!(
        "axon-provider-mime-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&image_path, [0xff, 0xd8, 0xff, 0xd9]).expect("临时 JPEG 应可写入");

    for protocol in [
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::AnthropicMessages,
    ] {
        let mut input = vision_request_input();
        input.protocol = protocol;
        input.model_id = "vision-test".to_owned();
        *input.messages = vec![
            AgentModelMessage::text("user", "看图").with_local_images(vec![
                AgentModelLocalImageRef::local(image_path.to_string_lossy(), IMAGE_MIME_TYPE),
            ]),
        ];
        refresh_prompt_cache_plan(&mut input);

        let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
        let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
        let body_text = body.to_string();

        assert!(body_text.contains("image/jpeg"));
        assert!(!body_text.contains("data:image/png;base64"));
        assert!(!body_text.contains("axon_media"));
    }
    let _ = std::fs::remove_file(image_path);
}

fn vision_request_input() -> ProviderStreamingHttpRequestInput {
    let mut input = request_input(Vec::new());
    input.supports_vision = true;
    input
}

fn temp_image_file(label: &str) -> PathBuf {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-provider-{label}-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'])
        .expect("临时图片应可写入");
    path
}
