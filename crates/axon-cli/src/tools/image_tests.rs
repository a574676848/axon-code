use std::{fs, io::Cursor, path::Path};

use axon_agent_runtime::{
    READ_FILE_IMAGE_NORMALIZED_MAX_BYTES, READ_FILE_IMAGE_SOURCE_MAX_BYTES,
    READ_FILE_IMAGE_SOURCE_PREFIX, agent_run_tool_result_messages,
    agent_run_tool_result_receipt_from_facts,
};
use axon_domain::SessionId;
use image::{ExtendedColorType, ImageEncoder, Rgb, RgbImage, codecs::png::PngEncoder};

use super::*;
use crate::tool_orchestrator::ToolOrchestratorContext;

#[test]
fn small_png_passes_through_and_is_delivered_as_local_image() {
    let root = temp_dir("png");
    fs::create_dir_all(root.join("assets")).unwrap();
    let bytes = png_fixture(64, 48);
    fs::write(root.join("assets").join("shot.png"), &bytes).unwrap();
    let context = context(&root);

    let receipt = execute(&context, "assets/shot.png");

    assert_eq!(receipt.status, "completed");
    let metadata = receipt
        .metadata
        .as_ref()
        .expect("回执必须带结构化 metadata");
    assert_eq!(metadata["kind"], "read_file");
    assert_eq!(metadata["contentKind"], "image");
    assert_eq!(metadata["path"], "assets/shot.png");
    assert_eq!(metadata["mimeType"], "image/png");
    assert_eq!(metadata["byteLen"], bytes.len());
    assert!(
        !metadata["absolutePath"].as_str().unwrap().contains("//?/"),
        "回执不得暴露 Windows verbatim 前缀：{}",
        metadata["absolutePath"]
    );
    assert_eq!(metadata["width"], 64);
    assert_eq!(metadata["height"], 48);
    assert_eq!(metadata["originalDimensions"]["width"], 64);

    let image = &metadata["localImages"][0];
    assert_eq!(image["mimeType"], "image/png");
    assert_eq!(image["byteLen"], bytes.len());
    assert_eq!(image["width"], 64);
    assert!(image["archiveRef"].as_str().unwrap().starts_with("blobs/"));
    assert!(
        image["sourceRef"]
            .as_str()
            .unwrap()
            .starts_with(READ_FILE_IMAGE_SOURCE_PREFIX)
    );
    // 图片本体走 `localImages`，摘要里只留可解析的信封。
    assert!(receipt.summary.contains("<type>image</type>"));
    assert!(
        receipt
            .summary
            .contains("read_file path=assets/shot.png content_kind=image")
    );
    assert!(!receipt.filesystem_write_performed);
    assert!(!receipt.remote_trigger_allowed);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn large_photo_is_downscaled_before_it_reaches_the_model() {
    let root = temp_dir("large");
    fs::create_dir_all(&root).unwrap();
    let bytes = png_fixture(3000, 2000);
    fs::write(root.join("photo.png"), &bytes).unwrap();
    let context = context(&root);

    let receipt = execute(&context, "photo.png");

    assert_eq!(receipt.status, "completed");
    let metadata = receipt.metadata.as_ref().unwrap();
    let width = metadata["width"].as_u64().unwrap();
    let height = metadata["height"].as_u64().unwrap();
    assert!(width * height <= 2048 * 2048, "归一化必须落在像素预算内");
    assert_eq!(metadata["originalDimensions"]["width"], 3000);
    assert_eq!(metadata["originalDimensions"]["height"], 2000);
    assert!(
        metadata["byteLen"].as_u64().unwrap()
            <= u64::try_from(READ_FILE_IMAGE_NORMALIZED_MAX_BYTES).unwrap(),
        "归一化必须落在字节预算内"
    );
    assert_eq!(metadata["mimeType"], "image/jpeg");
    assert!(
        receipt.summary.contains("降采样"),
        "模型必须知道坐标要按比例换算：{}",
        receipt.summary
    );

    let _ = fs::remove_dir_all(root);
}

/// 闭环：工具回执 → 下一轮 provider 消息，图片必须以归档引用形式被带过去。
#[test]
fn image_receipt_reaches_the_next_provider_message_with_its_archive_reference() {
    let root = temp_dir("loop");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("shot.png"), png_fixture(64, 48)).unwrap();
    let context = context(&root);
    let tool_call = call("shot.png");

    let receipt = execute(&context, "shot.png");
    let archive_ref = receipt.metadata.as_ref().unwrap()["localImages"][0]["archiveRef"]
        .as_str()
        .unwrap()
        .to_owned();

    let messages = agent_run_tool_result_messages(
        std::slice::from_ref(&tool_call),
        &[agent_run_tool_result_receipt_from_facts(&receipt)],
        None,
        axon_agent_runtime::AgentRunToolOutputPolicy::default(),
    )
    .expect("图片回执应能生成下一轮 provider 消息");

    let image_message = messages
        .iter()
        .find(|message| !message.local_images.is_empty())
        .expect("必须出现携带图片的消息");
    let image = &image_message.local_images[0];
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.archive_ref.as_deref(), Some(archive_ref.as_str()));
    assert!(image.byte_len.is_some());
    assert!(image_message.cache_excluded, "图片消息不得污染缓存前缀");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn text_files_still_take_the_text_branch() {
    let root = temp_dir("text-file");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("notes.txt"), "普通文本").unwrap();
    let context = context(&root);

    assert!(
        execute_read_file_image_with_archive_root(
            &call("notes.txt"),
            &context,
            &root.join("archive")
        )
        .is_none(),
        "文本文件必须继续走文本读取分支"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn extensionless_text_keeps_the_text_branch() {
    let root = temp_dir("plain");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("notes"), "这不是图片".as_bytes()).unwrap();
    let context = context(&root);

    assert!(
        execute_read_file_image_with_archive_root(&call("notes"), &context, &root.join("archive"))
            .is_none(),
        "无扩展名文本必须继续走文本分支，不得被图片分支截走"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn extension_less_image_is_detected_by_its_signature() {
    let root = temp_dir("no-extension");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("clipboard-dump"), png_fixture(32, 32)).unwrap();
    let context = context(&root);

    let receipt = execute(&context, "clipboard-dump");

    assert_eq!(receipt.status, "completed");
    assert_eq!(receipt.metadata.as_ref().unwrap()["mimeType"], "image/png");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn extension_mismatch_names_both_formats() {
    let root = temp_dir("mismatch");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("photo.jpg"), png_fixture(32, 32)).unwrap();

    let receipt = execute(&context(&root), "photo.jpg");

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("image/jpeg"));
    assert!(receipt.summary.contains("image/png"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn unsupported_extension_is_refused_with_format_contract() {
    let root = temp_dir("bmp");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("icon.bmp"), png_fixture(32, 32)).unwrap();

    let receipt = execute(&context(&root), "icon.bmp");

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains(".bmp"));
    assert!(receipt.summary.contains("PNG/JPEG/WebP/GIF"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn image_extension_with_non_image_content_is_refused() {
    let root = temp_dir("fake-png");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("shot.png"), "这不是图片".as_bytes()).unwrap();

    let receipt = execute(&context(&root), "shot.png");

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("不是受支持的图片格式"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_source_is_refused_before_reading_bytes() {
    let root = temp_dir("oversize");
    fs::create_dir_all(&root).unwrap();
    let oversized_len = usize::try_from(READ_FILE_IMAGE_SOURCE_MAX_BYTES).unwrap_or(usize::MAX) + 1;
    fs::write(root.join("huge.png"), vec![0_u8; oversized_len]).unwrap();

    let receipt = execute(&context(&root), "huge.png");

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("上限"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn model_without_image_input_is_refused_with_switch_hint() {
    let root = temp_dir("no-vision");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("shot.png"), png_fixture(32, 32)).unwrap();
    let mut context = context(&root);
    context.supports_vision = false;

    let receipt = execute(&context, "shot.png");

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("视觉"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_image_path_is_left_to_the_text_branch() {
    let root = temp_dir("missing");
    fs::create_dir_all(&root).unwrap();

    assert!(
        execute_read_file_image_with_archive_root(
            &call("ghost.png"),
            &context(&root),
            &root.join("archive")
        )
        .is_none(),
        "路径不存在时交给文本分支给出统一的定位提示"
    );

    let _ = fs::remove_dir_all(root);
}

fn execute(context: &ToolOrchestratorContext, path: &str) -> ToolReceipt {
    execute_read_file_image_with_archive_root(&call(path), context, &context.cwd.join("archive"))
        .expect("图片分支必须接手并给出回执")
}

fn call(path: &str) -> ProviderToolCall {
    ProviderToolCall::new(
        "call-read-file",
        "builtin.read_file",
        serde_json::json!({ "path": path }).to_string(),
    )
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    axon_test_support::test_temp_root().join(format!("axon-cli-image-{tag}-{}", std::process::id()))
}

/// 生成真实可解码的 PNG 夹具：归一化会真正解码，占位字节不再够用。
fn png_fixture(width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    });
    let mut buffer = Vec::new();
    PngEncoder::new(&mut Cursor::new(&mut buffer))
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgb8)
        .expect("测试 PNG 编码应成功");
    buffer
}

fn context(cwd: &Path) -> ToolOrchestratorContext {
    ToolOrchestratorContext {
        config_path: None,
        cwd: cwd.to_path_buf(),
        session_id: SessionId(1),
        thread_session_id: "thread-image".to_owned(),
        cell_id: axon_domain::CellId(1),
        model_messages: Vec::new(),
        execution_tools: std::sync::Arc::new(std::sync::RwLock::new(
            axon_agent_runtime::ToolRegistry::new(),
        )),
        memory_write_runtime: std::sync::Arc::new(
            crate::run_commands::AgentMemoryWriteRuntime::default(),
        ),
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        timeout: std::time::Duration::from_secs(10),
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        run_cancel_token: None,
        model_profile_id: axon_domain::ModelProfileId(0),
        supports_vision: true,
        permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: std::sync::Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: std::sync::Arc::new(
            crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
        ),
        subagent_cancel_tokens: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        user_question_port: None,
    }
}
