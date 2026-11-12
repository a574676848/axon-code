//! 模型 payload 的附件装配。
//!
//! 从 `run_request_payload` 下沉：图片与本地文件的摄取、归档写入与模型投影自成一段，
//! 载荷装配主流程只保留编排与并行预热。

use std::path::{Path, PathBuf};

use axon_agent_runtime::{
    AgentModelAttachmentProjection, AgentModelMessage, AgentModelPayload, AttachmentBlobStore,
    AttachmentIngressRuntime, default_attachment_archive_root,
};

use crate::memory_image_ref::image_mime_type;

use super::LOCAL_UPLOAD_SOURCE_PREFIX;

pub(super) fn attach_vision_files_to_payload(
    payload: AgentModelPayload,
    files: &[PathBuf],
    session_id: &str,
    supports_vision: bool,
) -> Result<AgentModelPayload, String> {
    let archive_root =
        default_attachment_archive_root().ok_or_else(|| "无法解析附件归档根目录".to_owned())?;
    attach_vision_files_to_payload_with_archive_root(
        payload,
        files,
        session_id,
        supports_vision,
        &archive_root,
    )
}

pub(super) fn attach_vision_files_to_payload_with_archive_root(
    mut payload: AgentModelPayload,
    files: &[PathBuf],
    session_id: &str,
    supports_vision: bool,
    archive_root: &Path,
) -> Result<AgentModelPayload, String> {
    let mut ingress = AttachmentIngressRuntime::default();
    let mut store = AttachmentBlobStore::new(archive_root);
    let mut images = Vec::new();
    let mut local_files = Vec::new();
    for (index, path) in files.iter().enumerate() {
        let mime_hint = image_mime_type(path);
        if mime_hint.is_some() && !supports_vision {
            continue;
        }
        let bytes = std::fs::read(path).map_err(|error| format!("读取附件失败：{error}"))?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("附件文件名无效：{}", path.display()))?
            .to_owned();
        let record = ingress
            .ingest_local_upload_with_blob_store(
                axon_agent_runtime::AttachmentLocalUploadRequest {
                    file_name: file_name.clone(),
                    mime_hint: mime_hint.map(ToOwned::to_owned),
                    bytes,
                    source_trace: local_upload_source_trace(session_id, index),
                    sensitivity: axon_domain::AttachmentSensitivity::Normal,
                },
                &mut store,
                0,
            )
            .map_err(|error| error.to_string())?;
        match AttachmentIngressRuntime::project_for_model(&record.attachment)
            .map_err(|error| error.to_string())?
        {
            AgentModelAttachmentProjection::Image(image) if supports_vision => images.push(image),
            AgentModelAttachmentProjection::Image(_) => {}
            AgentModelAttachmentProjection::File(mut file) => {
                file.name = file_name;
                // 原始绝对路径随引用一起交给模型：预览被截断时它才是读取原文件的正路。
                file.path = Some(path.to_string_lossy().to_string());
                local_files.push(file);
            }
        }
    }
    if images.is_empty() && local_files.is_empty() {
        return Ok(payload);
    }
    if let Some(message) = payload
        .messages
        .iter_mut()
        .rev()
        .find(|message| message.role == "user")
    {
        *message = message
            .clone()
            .with_local_images(images)
            .with_local_files(local_files);
    } else {
        payload.messages.push(
            AgentModelMessage::text("user", "")
                .with_local_images(images)
                .with_local_files(local_files),
        );
    }
    Ok(payload)
}

fn local_upload_source_trace(session_id: &str, index: usize) -> String {
    format!("{LOCAL_UPLOAD_SOURCE_PREFIX}{session_id}/{index}")
}
