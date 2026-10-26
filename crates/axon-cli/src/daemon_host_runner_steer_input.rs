//! 运行中追加输入的附件投影。
//!
//! Daemon 的 steer 通道只搬运一句话时代价很小，但附件必须与正文**同一条消息**进入
//! 轮次：图片走 `local_images`（Provider 请求阶段才读成 data URL，不支持视觉的模型
//! 由历史归一化显式丢弃），其余文件走 `local_files`（模型拿到绝对路径后仍可自行读取）。
//! 附件缓存可能已过期回收，因此缺失的路径只跳过，绝不因单个附件缺失去拒绝整条输入。

use std::path::Path;

use axon_agent_runtime::{AgentModelLocalFileRef, AgentModelLocalImageRef, AgentModelMessage};
use axon_domain::InteractionRecord;

/// 把一条追加输入投影成 Provider 可见的用户消息。
pub(super) fn steer_message(interaction: &InteractionRecord) -> AgentModelMessage {
    let mut images = Vec::new();
    let mut files = Vec::new();
    for attachment in &interaction.attachments {
        let trimmed = attachment.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = Path::new(trimmed);
        if !path.is_file() {
            continue;
        }
        match steer_image_mime_type(path) {
            Some(mime_type) => images.push(AgentModelLocalImageRef::local(trimmed, mime_type)),
            None => files.push(AgentModelLocalFileRef {
                name: path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("attachment")
                    .to_owned(),
                path: Some(trimmed.to_owned()),
                mime_type: "text/plain".to_owned(),
                preview: String::new(),
                byte_len: None,
                sha256: None,
                archive_ref: None,
                source_ref: Some(trimmed.to_owned()),
            }),
        }
    }
    let message = AgentModelMessage::text("user", interaction.content.clone());
    if images.is_empty() && files.is_empty() {
        return message;
    }
    message.with_local_images(images).with_local_files(files)
}

/// 追加输入的图片类型判定：只认 Provider 能内联的四种位图。
fn steer_image_mime_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}
